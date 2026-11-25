//! box 原图回源：只接受网页签发的短期 URL，校验签名、过期与条目存活后流式返回。
//!
//! 路径从不进入响应；URL 里的 `id` 只用于数据库反查与签名规范串。

use std::{
    io::SeekFrom,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
use axum::{
    Router,
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use kanalib_wall_model::valid_item_id;
use rusqlite::OptionalExtension;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::{db::Db, push};

const MULTIPART_UNSUPPORTED: &str = "single range only";
const MAX_CACHE_CONTROL: &str = "no-store";

#[derive(Clone, Debug)]
pub struct MediaState {
    pub db_path: Arc<PathBuf>,
    pub signing_key: Arc<str>,
}

impl MediaState {
    pub fn new(db_path: PathBuf, signing_key: impl Into<Arc<str>>) -> Self {
        Self {
            db_path: Arc::new(db_path),
            signing_key: signing_key.into(),
        }
    }
}

pub fn router(state: MediaState) -> Router {
    Router::new()
        .route("/original/{id}", get(serve_original))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct SignedQuery {
    expires: Option<i64>,
    sig: Option<String>,
}

async fn serve_original(
    State(state): State<MediaState>,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<SignedQuery>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    if !valid_item_id(&id) {
        return not_found();
    }
    let (Some(expires), Some(signature)) = (query.expires, query.sig.as_deref()) else {
        return not_found();
    };
    if !push::verify_original(
        &state.signing_key,
        &id,
        expires,
        signature,
        crate::db::now(),
    ) {
        return not_found();
    }

    let db_path = state.db_path.as_ref().clone();
    let lookup_id = id.clone();
    let record = match tokio::task::spawn_blocking(move || {
        let db = Db::open_read(&db_path)?;
        live_original(&db, &lookup_id)
    })
    .await
    {
        Ok(Ok(Some(record))) => record,
        Ok(Ok(None)) => return not_found(),
        Ok(Err(error)) => {
            tracing::error!(%error, "original lookup failed");
            return unavailable();
        }
        Err(error) => {
            tracing::error!(%error, "original lookup task failed");
            return unavailable();
        }
    };

    serve_file_response(&record.path, &record.ext, method, &headers).await
}

/// 流式返回单个文件，支持单区间 Range；签名原图与管理端原图共用。
pub(crate) async fn serve_file_response(
    path: &Path,
    extension: &str,
    method: Method,
    headers: &HeaderMap,
) -> Response {
    let mut file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "media file missing");
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

    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .map(|value| parse_range(value, size))
        .unwrap_or(RangeSpec::Full);
    let content_type = content_type(extension);

    match range {
        RangeSpec::Unsatisfiable => {
            let mut response = Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .body(Body::empty())
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            let response_headers = response.headers_mut();
            base_headers(response_headers, &content_type);
            response_headers.insert(
                header::CONTENT_RANGE,
                header_value(&format!("bytes */{size}")),
            );
            response
        }
        RangeSpec::Partial { start, end } => {
            let length = end - start + 1;
            if let Err(error) = file.seek(SeekFrom::Start(start)).await {
                tracing::warn!(%error, "media seek failed");
                return unavailable();
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
            base_headers(response_headers, &content_type);
            response_headers.insert(
                header::CONTENT_RANGE,
                header_value(&format!("bytes {start}-{end}/{size}")),
            );
            response_headers.insert(header::CONTENT_LENGTH, header_value(&length.to_string()));
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
            base_headers(response_headers, &content_type);
            response_headers.insert(header::CONTENT_LENGTH, header_value(&size.to_string()));
            response
        }
    }
}

struct OriginalRecord {
    path: PathBuf,
    ext: String,
}

fn live_original(db: &Db, item_id: &str) -> Result<Option<OriginalRecord>> {
    let Some(row) = db.item_row(item_id)? else {
        return Ok(None);
    };
    if row.decision != "allow" {
        return Ok(None);
    }
    let Some(path) = row.original_path else {
        return Ok(None);
    };
    let live: Option<i64> = db
        .connection()
        .query_row(
            "SELECT status != 'deleted' FROM works WHERE id = ?1",
            [&row.work_id],
            |row| row.get(0),
        )
        .optional()?;
    if live != Some(1) {
        return Ok(None);
    }
    let ext = row
        .ext
        .or_else(|| {
            Path::new(&path)
                .extension()
                .and_then(|ext| ext.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "jpg".to_string());
    Ok(Some(OriginalRecord {
        path: PathBuf::from(path),
        ext,
    }))
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
        tracing::debug!(reason = MULTIPART_UNSUPPORTED, "rejecting range");
        return RangeSpec::Unsatisfiable;
    }
    let Some((start_text, end_text)) = spec.split_once('-') else {
        return RangeSpec::Unsatisfiable;
    };
    if start_text.is_empty() {
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

fn content_type(ext: &str) -> String {
    match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "image/jpeg",
    }
    .to_string()
}

fn base_headers(headers: &mut HeaderMap, content_type: &str) {
    headers.insert(header::CONTENT_TYPE, header_value(content_type));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(MAX_CACHE_CONTROL),
    );
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
}

fn header_value(value: &str) -> HeaderValue {
    HeaderValue::from_str(value).unwrap_or_else(|_| HeaderValue::from_static(""))
}

fn not_found() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

fn unavailable() -> Response {
    StatusCode::SERVICE_UNAVAILABLE.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request;
    use tower::ServiceExt;

    const ITEM_ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SIGNING_KEY: &str = "media-test-key";

    fn seeded(directory: &Path) -> (MediaState, PathBuf) {
        let db_path = directory.join("ingest.db");
        let original = directory.join("original.jpg");
        std::fs::write(&original, b"0123456789").expect("original");
        let db = Db::open(&db_path).expect("db");
        db.connection()
            .execute(
                "INSERT INTO works (id, source, external_id, title, page_count)
                 VALUES ('work-media', 'pixiv', '1', 'Media', 1)",
                [],
            )
            .expect("work");
        db.connection()
            .execute(
                "INSERT INTO items
                    (id, work_id, source, page, original_path, ext, width, height, bytes, decision)
                 VALUES (?1, 'work-media', 'pixiv', 0, ?2, 'jpg', 1, 1, 10, 'allow')",
                rusqlite::params![ITEM_ID, original.to_string_lossy()],
            )
            .expect("item");
        (MediaState::new(db_path, SIGNING_KEY), original)
    }

    fn uri(expires: i64, signature: &str) -> String {
        format!("/original/{ITEM_ID}?expires={expires}&sig={signature}")
    }

    fn signed_uri(now: i64) -> String {
        let expires = now + 300;
        uri(expires, &push::sign_original(SIGNING_KEY, ITEM_ID, expires))
    }

    async fn request(state: MediaState, uri: &str, range: Option<&str>) -> Response {
        let mut builder = Request::builder().uri(uri);
        if let Some(range) = range {
            builder = builder.header(header::RANGE, range);
        }
        router(state)
            .oneshot(builder.body(Body::empty()).expect("request"))
            .await
            .expect("response")
    }

    #[tokio::test]
    async fn signed_request_streams_original() {
        let directory = tempfile::tempdir().expect("tempdir");
        let (state, _) = seeded(directory.path());
        let response = request(state, &signed_uri(crate::db::now()), None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/jpeg"
        );
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert_eq!(&body[..], b"0123456789");
    }

    #[tokio::test]
    async fn expired_or_forged_signature_is_not_found() {
        let directory = tempfile::tempdir().expect("tempdir");
        let (state, _) = seeded(directory.path());
        let now = crate::db::now();
        let expired = now - 1;
        let expired_uri = uri(expired, &push::sign_original(SIGNING_KEY, ITEM_ID, expired));
        assert_eq!(
            request(state.clone(), &expired_uri, None).await.status(),
            StatusCode::NOT_FOUND
        );
        let valid = signed_uri(now);
        let forged = valid.replace("sig=", "sig=0");
        assert_eq!(
            request(state, &forged, None).await.status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn missing_item_is_not_found_even_with_valid_signature() {
        let directory = tempfile::tempdir().expect("tempdir");
        let (state, _) = seeded(directory.path());
        let missing = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let expires = crate::db::now() + 300;
        let signature = push::sign_original(SIGNING_KEY, missing, expires);
        let uri = format!("/original/{missing}?expires={expires}&sig={signature}");
        assert_eq!(
            request(state, &uri, None).await.status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn range_request_returns_partial_content() {
        let directory = tempfile::tempdir().expect("tempdir");
        let (state, _) = seeded(directory.path());
        let response = request(state, &signed_uri(crate::db::now()), Some("bytes=2-5")).await;
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response.headers().get(header::CONTENT_RANGE).unwrap(),
            "bytes 2-5/10"
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert_eq!(&body[..], b"2345");
    }
}
