//! 缩略图媒体服务：档位白名单、DB 反查、路径前缀校验、ETag 与单区间 Range。

use std::{
    io::SeekFrom,
    path::{Path, PathBuf},
};

use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::{
    AppState, db,
    response::{api_error, not_found},
};

pub const TIERS: [&str; 4] = ["t256", "t512", "t1024", "t2048"];
pub const MEDIA_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbFormat {
    Jpeg,
    Avif,
}

impl ThumbFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Avif => "avif",
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Avif => "image/avif",
        }
    }
}

pub(crate) fn valid_tier(tier: &str) -> bool {
    TIERS.contains(&tier)
}

pub(crate) fn format_for_file(file: &str) -> Option<ThumbFormat> {
    if file.strip_suffix(".jpg").is_some() {
        Some(ThumbFormat::Jpeg)
    } else if file.strip_suffix(".avif").is_some() {
        Some(ThumbFormat::Avif)
    } else {
        None
    }
}

/// 文件名必须是 40–64 位小写 hex 加 `.jpg` 或 `.avif`。
pub(crate) fn valid_file_name(file: &str) -> bool {
    let Some(stem) = file
        .strip_suffix(".jpg")
        .or_else(|| file.strip_suffix(".avif"))
    else {
        return false;
    };
    (40..=64).contains(&stem.len())
        && stem
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn detect_thumb_format(body: &[u8]) -> Option<ThumbFormat> {
    if body.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(ThumbFormat::Jpeg);
    }
    if body.len() < 12 || &body[4..8] != b"ftyp" {
        return None;
    }
    let box_size = u32::from_be_bytes(body[0..4].try_into().ok()?) as usize;
    if box_size < 12 || box_size > body.len() {
        return None;
    }
    body[8..box_size]
        .as_chunks::<4>()
        .0
        .iter()
        .any(|brand| *brand == *b"avif" || *brand == *b"avis")
        .then_some(ThumbFormat::Avif)
}

fn fallback_tiers(tier: &str) -> &'static [&'static str] {
    match tier {
        "t256" => &["t512"],
        "t1024" => &["t512"],
        "t2048" => &["t1024", "t512"],
        _ => &[],
    }
}

fn etag(id: &str, updated_at: i64, tier: &str, format: ThumbFormat) -> String {
    format!("W/\"{id}-{updated_at}-{tier}-{}\"", format.extension())
}

fn accepts_avif(headers: &HeaderMap) -> bool {
    let Some(value) = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let mut selected: Option<(u8, f32)> = None;
    for entry in value.split(',') {
        let mut parts = entry.split(';').map(str::trim);
        let media_type = parts.next().unwrap_or_default().to_ascii_lowercase();
        let specificity = match media_type.as_str() {
            "image/avif" => 3,
            "image/*" => 2,
            "*/*" => 1,
            _ => continue,
        };
        let quality = parts
            .find_map(|parameter| {
                let (name, value) = parameter.split_once('=')?;
                name.eq_ignore_ascii_case("q").then_some(value)
            })
            .map(|value| value.trim().trim_matches(['"', '\'']))
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| value.is_finite())
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        if selected.is_none_or(|(best_specificity, best_quality)| {
            specificity > best_specificity
                || (specificity == best_specificity && quality > best_quality)
        }) {
            selected = Some((specificity, quality));
        }
    }
    selected.is_some_and(|(_, quality)| quality > 0.0)
}

fn if_none_match(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|candidate| {
            candidate == etag
                || candidate == "*"
                || candidate.trim_start_matches("W/") == etag.trim_start_matches("W/")
        })
}

fn base_media_headers(etag_value: &str, format: ThumbFormat) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(etag_value).unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(MEDIA_CACHE_CONTROL),
    );
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(format.content_type()),
    );
    headers.insert(header::VARY, HeaderValue::from_static("Accept"));
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers
}

enum RangeSpec {
    Full,
    Partial { start: u64, end: u64 },
    Unsatisfiable,
}

fn parse_range(value: &str, size: u64) -> RangeSpec {
    let Some(spec) = value.strip_prefix("bytes=") else {
        return RangeSpec::Full;
    };
    if spec.contains(',') || size == 0 {
        return RangeSpec::Unsatisfiable;
    }
    let Some((start_text, end_text)) = spec.split_once('-') else {
        return RangeSpec::Unsatisfiable;
    };
    if start_text.is_empty() {
        // 后缀区间：bytes=-N
        let Ok(suffix) = end_text.parse::<u64>() else {
            return RangeSpec::Unsatisfiable;
        };
        if suffix == 0 {
            return RangeSpec::Unsatisfiable;
        }
        let start = size.saturating_sub(suffix);
        return RangeSpec::Partial {
            start,
            end: size - 1,
        };
    }
    let Ok(start) = start_text.parse::<u64>() else {
        return RangeSpec::Unsatisfiable;
    };
    if start >= size {
        return RangeSpec::Unsatisfiable;
    }
    let end = if end_text.is_empty() {
        size - 1
    } else {
        let Ok(end) = end_text.parse::<u64>() else {
            return RangeSpec::Unsatisfiable;
        };
        if end < start {
            return RangeSpec::Unsatisfiable;
        }
        end.min(size - 1)
    };
    RangeSpec::Partial { start, end }
}

fn canonical_media_root(root: &Path) -> Option<PathBuf> {
    root.canonicalize().ok()
}

fn resolve_media_path(
    root: &Path,
    tier: &str,
    stem: &str,
    formats: &[ThumbFormat],
) -> Option<(PathBuf, ThumbFormat)> {
    let canonical_root = canonical_media_root(root)?;
    let candidates = std::iter::once(tier).chain(fallback_tiers(tier).iter().copied());
    for candidate in candidates {
        for &format in formats {
            let path = canonical_root
                .join(candidate)
                .join(format!("{stem}.{}", format.extension()));
            let Ok(canonical) = path.canonicalize() else {
                continue;
            };
            if !canonical.starts_with(&canonical_root) {
                continue;
            }
            if canonical.is_file() {
                return Some((canonical, format));
            }
        }
    }
    None
}

pub async fn serve_media(
    State(state): State<AppState>,
    AxumPath((tier, file)): AxumPath<(String, String)>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    if !valid_tier(&tier) || !valid_file_name(&file) {
        return not_found();
    }
    let Some(requested_format) = format_for_file(&file) else {
        return not_found();
    };
    let suffix = if requested_format == ThumbFormat::Jpeg {
        ".jpg"
    } else {
        ".avif"
    };
    let Some(stem) = file.strip_suffix(suffix) else {
        return not_found();
    };
    let id = stem.to_string();
    let formats: &[ThumbFormat] = if requested_format == ThumbFormat::Avif {
        &[ThumbFormat::Avif]
    } else if accepts_avif(&headers) {
        &[ThumbFormat::Avif, ThumbFormat::Jpeg]
    } else {
        &[ThumbFormat::Jpeg]
    };

    let updated_at = {
        let db_path = state.db_path.as_ref().clone();
        let lookup_id = id.clone();
        match tokio::task::spawn_blocking(move || {
            let conn = db::open_read(&db_path)?;
            db::item_updated_at(&conn, &lookup_id)
        })
        .await
        {
            Ok(Ok(Some(updated_at))) => updated_at,
            Ok(Ok(None)) => return not_found(),
            Ok(Err(error)) => {
                tracing::warn!(%error, "media db lookup failed");
                return api_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "db_unavailable",
                    "database unavailable",
                );
            }
            Err(error) => {
                tracing::error!(%error, "media db task failed");
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal error",
                );
            }
        }
    };

    let media_root = state.media_root.as_ref().clone();
    let requested_tier = tier.clone();
    let requested_stem = stem.to_string();
    let resolved = match tokio::task::spawn_blocking(move || {
        resolve_media_path(&media_root, &requested_tier, &requested_stem, formats)
    })
    .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, "media path task failed");
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal error",
            );
        }
    };
    let Some((path, format)) = resolved else {
        return not_found();
    };

    let etag_value = etag(&id, updated_at, &tier, format);
    if if_none_match(&headers, &etag_value) {
        return (
            StatusCode::NOT_MODIFIED,
            base_media_headers(&etag_value, format),
        )
            .into_response();
    }

    let mut file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "media open failed");
            return not_found();
        }
    };
    let size = match file.metadata().await {
        Ok(metadata) => metadata.len(),
        Err(error) => {
            tracing::warn!(%error, "media metadata failed");
            return not_found();
        }
    };

    let range_spec = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .map(|value| parse_range(value, size))
        .unwrap_or(RangeSpec::Full);

    match range_spec {
        RangeSpec::Unsatisfiable => {
            let mut response = Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .body(Body::empty())
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            let mut media_headers = base_media_headers(&etag_value, format);
            media_headers.insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{size}"))
                    .unwrap_or_else(|_| HeaderValue::from_static("bytes */*")),
            );
            response.headers_mut().extend(media_headers);
            response
        }
        RangeSpec::Partial { start, end } => {
            let length = end - start + 1;
            if let Err(error) = file.seek(SeekFrom::Start(start)).await {
                tracing::warn!(%error, "media seek failed");
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal error",
                );
            }
            let body = if method == Method::HEAD {
                Body::empty()
            } else {
                Body::from_stream(ReaderStream::new(file.take(length)))
            };
            let mut response = Response::builder()
                .status(StatusCode::PARTIAL_CONTENT)
                .body(body)
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            let response_headers = response.headers_mut();
            response_headers.extend(base_media_headers(&etag_value, format));
            response_headers.insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes {start}-{end}/{size}"))
                    .unwrap_or_else(|_| HeaderValue::from_static("bytes */*")),
            );
            response_headers.insert(
                header::CONTENT_LENGTH,
                HeaderValue::from_str(&length.to_string())
                    .unwrap_or_else(|_| HeaderValue::from_static("0")),
            );
            response
        }
        RangeSpec::Full => {
            let body = if method == Method::HEAD {
                Body::empty()
            } else {
                Body::from_stream(ReaderStream::new(file))
            };
            let mut response = Response::builder()
                .status(StatusCode::OK)
                .body(body)
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            let response_headers = response.headers_mut();
            response_headers.extend(base_media_headers(&etag_value, format));
            response_headers.insert(
                header::CONTENT_LENGTH,
                HeaderValue::from_str(&size.to_string())
                    .unwrap_or_else(|_| HeaderValue::from_static("0")),
            );
            response
        }
    }
}
