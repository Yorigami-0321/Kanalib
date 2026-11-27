//! 私网桌面桥接：pull / push / upload，全部走设备令牌。

use axum::{
    Json,
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, Method, Request, StatusCode, header},
    response::Response,
};
use http_body_util::BodyExt;
use kanalib_wall_model::{IngestTag, WallItemDto, valid_item_id};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::AsyncWriteExt;

use crate::{
    AppState, crypto, db,
    query::{base64url_decode, base64url_encode},
    response::{api_error, db_unavailable, internal, json_response, not_found},
};

const DEFAULT_PULL_LIMIT: u32 = 100;
const MAX_PULL_LIMIT: u32 = 200;
const MAX_PUSH_ITEMS: usize = 1_000;
const MAX_PUSH_TAGS: usize = 256;
const MAX_TAG_NAME_LEN: usize = 256;
const MAX_TAG_CATEGORY_LEN: usize = 64;
const MAX_PUSH_COLLECTIONS: usize = 256;
const MAX_EXT_LEN: usize = 8;
const MAX_LOCAL_ITEMS: usize = 200;
const MAX_UPLOAD_FILES: usize = 5_000;
const MAX_UPLOAD_BYTES: u64 = 8 * 1024 * 1024 * 1024;
static UPLOAD_PART_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
enum Capability {
    Pull,
    Push,
}

impl Capability {
    fn allowed(self, device: &db::DeviceAuth) -> bool {
        match self {
            Self::Pull => device.can_pull,
            Self::Push => device.can_push,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct PullQuery {
    pub since: Option<String>,
    pub limit: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PullCursor {
    v: u8,
    u: i64,
    id: String,
}

#[derive(Serialize)]
struct MediaLinks {
    t256: String,
    t512: String,
    t1024: String,
    t2048: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    original: Option<String>,
}

#[derive(Serialize)]
struct PullItem {
    #[serde(flatten)]
    item: WallItemDto,
    revision: i64,
    updated_at: i64,
    media: MediaLinks,
}

#[derive(Serialize)]
struct PullResponse {
    items: Vec<PullItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next: Option<String>,
    has_more: bool,
}

#[derive(Debug, Deserialize)]
pub struct PushBody {
    #[serde(default)]
    pub items: Vec<PushWire>,
}

#[derive(Debug, Deserialize)]
pub struct PushWire {
    pub id: String,
    pub base_revision: Option<i64>,
    #[serde(default)]
    pub star: Option<u8>,
    #[serde(default)]
    pub favorite: Option<bool>,
    #[serde(default)]
    pub rating: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<IngestTag>>,
    #[serde(default)]
    pub collections: Option<Vec<i64>>,
}

#[derive(Serialize)]
struct PushResponse {
    ok: bool,
    applied: u32,
    conflicts: Vec<db::PushConflict>,
    missing: Vec<String>,
}

#[derive(Serialize)]
struct UploadResponse {
    ok: bool,
    file: String,
    bytes: u64,
}

#[derive(Debug, Default, Deserialize)]
pub struct LocalItemsBody {
    #[serde(default)]
    pub items: Vec<LocalItemWire>,
}

/// 本地发布的第一阶段：只写元数据，原图上传成功后再放上墙。
#[derive(Debug, Deserialize)]
pub struct LocalItemWire {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub title: String,
    pub width: u32,
    pub height: u32,
    #[serde(default = "default_rating")]
    pub rating: String,
    #[serde(default)]
    pub star: u8,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub mtime: i64,
    #[serde(default)]
    pub posted_at: Option<i64>,
    #[serde(default)]
    pub orig: Option<String>,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub tags: Vec<IngestTag>,
    #[serde(default)]
    pub work_id: Option<String>,
    #[serde(default)]
    pub work_title: String,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub page_count: Option<u32>,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Serialize)]
struct LocalItemResult {
    id: String,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct LocalItemsResponse {
    ok: bool,
    results: Vec<LocalItemResult>,
}

fn default_rating() -> String {
    "G".to_string()
}

pub async fn pull(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<PullQuery>,
) -> Response {
    if let Err(response) = authorize(&state, &headers, Capability::Pull).await {
        return *response;
    }

    let limit = match parse_limit(query.limit.as_deref()) {
        Ok(limit) => limit,
        Err(response) => return *response,
    };
    let (since_updated, since_id) = match decode_since(query.since.as_deref()) {
        Ok(cursor) => cursor,
        Err(response) => return *response,
    };
    let original = original_available(&state);
    let db_path = state.db_path.as_ref().clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db::open_read(&db_path)?;
        db::pull_items(&conn, since_updated, &since_id, limit)
    })
    .await;

    let (rows, has_more) = match result {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            tracing::error!(%error, "desktop pull failed");
            return internal();
        }
        Err(error) => {
            tracing::error!(%error, "desktop pull task failed");
            return internal();
        }
    };

    let next = has_more.then(|| rows.last()).flatten().and_then(|row| {
        serde_json::to_vec(&PullCursor {
            v: 1,
            u: row.updated_at,
            id: row.item.id.clone(),
        })
        .ok()
        .map(|bytes| base64url_encode(&bytes))
    });
    let items = rows
        .into_iter()
        .map(|row| PullItem {
            media: media_links(&row.item.id, original),
            item: row.item,
            revision: row.revision,
            updated_at: row.updated_at,
        })
        .collect();
    json_response(
        StatusCode::OK,
        PullResponse {
            items,
            next,
            has_more,
        },
    )
}

pub async fn push(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PushBody>,
) -> Response {
    if let Err(response) = authorize(&state, &headers, Capability::Push).await {
        return *response;
    }
    if body.items.len() > MAX_PUSH_ITEMS {
        return api_error(StatusCode::BAD_REQUEST, "invalid_push", "too many items");
    }
    let mut updates = Vec::with_capacity(body.items.len());
    for entry in body.items {
        if !valid_item_id(&entry.id) {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_item",
                "item id must be 40 or 64 lowercase hex",
            );
        }
        let Some(base_revision) = entry.base_revision else {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_push",
                "base_revision is required",
            );
        };
        if entry.star.is_some_and(|star| star > 5) {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_push",
                "star must be between 0 and 5",
            );
        }
        let rating = match entry.rating {
            Some(rating) => {
                let rating = rating.trim().to_ascii_uppercase();
                if !matches!(rating.as_str(), "G" | "S" | "Q" | "E") {
                    return api_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_rating",
                        "rating must be one of G/S/Q/E",
                    );
                }
                Some(rating)
            }
            None => None,
        };
        if let Some(tags) = &entry.tags {
            if tags.len() > MAX_PUSH_TAGS {
                return api_error(StatusCode::BAD_REQUEST, "invalid_push", "too many tags");
            }
            for tag in tags {
                if tag.name.trim().is_empty()
                    || tag.name.len() > MAX_TAG_NAME_LEN
                    || tag.category.len() > MAX_TAG_CATEGORY_LEN
                {
                    return api_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_push",
                        "tag name/category is invalid",
                    );
                }
            }
        }
        if entry
            .collections
            .as_ref()
            .is_some_and(|collections| collections.len() > MAX_PUSH_COLLECTIONS)
        {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_push",
                "too many collections",
            );
        }
        updates.push(db::PushUpdate {
            id: entry.id,
            base_revision,
            star: entry.star,
            favorite: entry.favorite,
            rating,
            tags: entry.tags,
            collections: entry.collections,
        });
    }

    let db_path = state.db_path.as_ref().clone();
    let result = tokio::task::spawn_blocking(move || {
        let mut conn = db::open(&db_path)?;
        db::apply_push(&mut conn, &updates, crypto::unix_now())
    })
    .await;
    match result {
        Ok(Ok(apply)) => json_response(
            StatusCode::OK,
            PushResponse {
                ok: true,
                applied: apply.applied,
                conflicts: apply.conflicts,
                missing: apply.missing,
            },
        ),
        Ok(Err(error)) => {
            tracing::error!(%error, "desktop push failed");
            internal()
        }
        Err(error) => {
            tracing::error!(%error, "desktop push task failed");
            internal()
        }
    }
}

pub async fn upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    if let Err(response) = authorize(&state, &headers, Capability::Push).await {
        return *response;
    }
    let Some(id) = headers
        .get("x-kanalib-item-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| valid_item_id(value))
    else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_item",
            "X-Kanalib-Item-Id must be 40 or 64 lowercase hex",
        );
    };
    let id = id.to_string();

    let db_path = state.db_path.as_ref().clone();
    let lookup = id.clone();
    let found = tokio::task::spawn_blocking(move || {
        let conn = db::open_read(&db_path)?;
        db::item_origin_and_ext(&conn, &lookup)
    })
    .await;
    let info = match found {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            tracing::error!(%error, "desktop upload lookup failed");
            return internal();
        }
        Err(error) => {
            tracing::error!(%error, "desktop upload lookup task failed");
            return internal();
        }
    };
    // `item_origin_and_ext` 已经过滤 deleted=1，所以不存在与已删除都按 404 处理。
    let Some((origin, item_ext)) = info else {
        return not_found();
    };
    let local = origin == "local";

    let ext = match upload_ext(&headers, item_ext.as_deref()) {
        Ok(ext) => ext,
        Err(response) => return *response,
    };
    // 本地条目的扩展名以元数据为准，避免 header 与 DB 不一致导致原图路径找不到。
    let ext = if local {
        item_ext.clone().unwrap_or(ext)
    } else {
        ext
    };
    let limit = state.writes.upload_limit_bytes;
    if let Some(length) = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        && length > limit
    {
        return payload_too_large();
    }

    let root = state
        .media_root
        .as_ref()
        .join(if local { "originals" } else { "uploads" });
    if let Err(error) = tokio::fs::create_dir_all(&root).await {
        tracing::error!(%error, "create upload root failed");
        return internal();
    }
    let file_name = format!("{id}.{ext}");
    let final_path = root.join(&file_name);
    let sequence = UPLOAD_PART_SEQ.fetch_add(1, Ordering::Relaxed);
    let part_path = root.join(format!(
        "{file_name}.{}-{sequence}.part",
        std::process::id()
    ));
    let mut file = match tokio::fs::File::create(&part_path).await {
        Ok(file) => file,
        Err(error) => {
            tracing::error!(%error, "create upload part failed");
            return internal();
        }
    };

    let mut body = request.into_body();
    let mut written = 0u64;
    while let Some(frame) = body.frame().await {
        let frame = match frame {
            Ok(frame) => frame,
            Err(error) => {
                tracing::debug!(%error, "upload body stream failed");
                cleanup_part(&part_path).await;
                return api_error(StatusCode::BAD_REQUEST, "invalid_upload", "incomplete body");
            }
        };
        let Ok(data) = frame.into_data() else {
            continue;
        };
        written += data.len() as u64;
        if written > limit {
            drop(file);
            cleanup_part(&part_path).await;
            return payload_too_large();
        }
        if let Err(error) = file.write_all(&data).await {
            tracing::error!(%error, "write upload failed");
            drop(file);
            cleanup_part(&part_path).await;
            return internal();
        }
    }
    if let Err(error) = file.flush().await {
        tracing::error!(%error, "flush upload failed");
        drop(file);
        cleanup_part(&part_path).await;
        return internal();
    }
    drop(file);
    if tokio::fs::try_exists(&final_path).await.unwrap_or(false) {
        let _ = tokio::fs::remove_file(&final_path).await;
    }
    if let Err(error) = tokio::fs::rename(&part_path, &final_path).await {
        tracing::error!(%error, "finalize upload failed");
        cleanup_part(&part_path).await;
        return internal();
    }

    if local {
        let media_root = state.media_root.as_ref().clone();
        let outcome =
            match crate::thumbs::generate(media_root, id.clone(), ext.clone(), final_path.clone())
                .await
            {
                Ok(outcome) => outcome,
                Err(error) => {
                    tracing::warn!(%error, %id, "local thumbnail generation failed");
                    return api_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_image",
                        "image cannot be decoded",
                    );
                }
            };
        let long_edge = outcome.width.max(outcome.height);
        let sharp = if long_edge >= 2048 {
            2048
        } else if long_edge >= 1024 {
            1024
        } else if long_edge >= 512 {
            512
        } else {
            long_edge
        };
        let db_path = state.db_path.as_ref().clone();
        let finalize_id = id.clone();
        let finalized = tokio::task::spawn_blocking(move || {
            let mut conn = db::open(&db_path)?;
            db::finalize_local_item(
                &mut conn,
                &finalize_id,
                outcome.width,
                outcome.height,
                sharp,
                crypto::unix_now(),
            )
        })
        .await;
        match finalized {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => {
                tracing::warn!(%id, "local item disappeared before finalize");
                return not_found();
            }
            Ok(Err(error)) => {
                tracing::error!(%error, "finalize local item failed");
                return internal();
            }
            Err(error) => {
                tracing::error!(%error, "finalize local item task failed");
                return internal();
            }
        }
    } else {
        enforce_upload_quota(&root).await;
    }

    json_response(
        StatusCode::OK,
        UploadResponse {
            ok: true,
            file: format!(
                "{}/{file_name}",
                if local { "originals" } else { "uploads" }
            ),
            bytes: written,
        },
    )
}

/// 写入一批本地条目元数据；条目在原图与缩略图就绪前保持 hidden。
pub async fn items(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<LocalItemsBody>,
) -> Response {
    if let Err(response) = authorize(&state, &headers, Capability::Push).await {
        return *response;
    }
    if body.items.len() > MAX_LOCAL_ITEMS {
        return api_error(
            StatusCode::BAD_REQUEST,
            "too_many_items",
            "at most 200 items per request",
        );
    }

    let mut prepared = Vec::new();
    let mut results = Vec::new();
    for wire in body.items {
        let id = wire.id.clone();
        match local_item_from_wire(wire) {
            Ok(item) => prepared.push(item),
            Err(error) => results.push(LocalItemResult {
                id,
                status: "rejected",
                error: Some(error),
            }),
        }
    }

    let db_path = state.db_path.as_ref().clone();
    let applied = tokio::task::spawn_blocking(move || {
        let mut conn = db::open(&db_path)?;
        let now = crypto::unix_now();
        let mut out = Vec::with_capacity(prepared.len());
        for item in &prepared {
            match db::upsert_local_item(&mut conn, item, now) {
                Ok(write) => out.push((item.id.clone(), Some(write), None)),
                Err(error) => out.push((item.id.clone(), None, Some(error.to_string()))),
            }
        }
        Ok::<_, anyhow::Error>(out)
    })
    .await;

    let applied = match applied {
        Ok(Ok(applied)) => applied,
        Ok(Err(error)) => {
            tracing::error!(%error, "local items write failed");
            return internal();
        }
        Err(error) => {
            tracing::error!(%error, "local items task failed");
            return internal();
        }
    };
    for (id, write, error) in applied {
        let (status, error) = match (write, error) {
            (Some(db::LocalWrite::Created), _) => ("created", None),
            (Some(db::LocalWrite::Updated), _) => ("updated", None),
            (Some(db::LocalWrite::Exists), _) => ("exists", None),
            (None, Some(error)) => ("error", Some(error)),
            (None, None) => ("error", Some("unknown error".to_string())),
        };
        results.push(LocalItemResult { id, status, error });
    }

    json_response(StatusCode::OK, LocalItemsResponse { ok: true, results })
}

fn local_item_from_wire(wire: LocalItemWire) -> Result<db::LocalItem, String> {
    if !valid_item_id(&wire.id) {
        return Err("id must be 40 or 64 lowercase hex".to_string());
    }
    let name = wire.name.trim();
    if name.is_empty() || name.len() > 512 {
        return Err("name must be 1-512 bytes".to_string());
    }
    if wire.title.len() > 512 {
        return Err("title is too long".to_string());
    }
    if wire.width == 0 || wire.height == 0 || wire.width > 50_000 || wire.height > 50_000 {
        return Err("width/height is out of range".to_string());
    }
    if wire.star > 5 {
        return Err("star must be 0-5".to_string());
    }
    if wire.group.len() > 128 {
        return Err("group is too long".to_string());
    }
    let rating = wire.rating.trim().to_ascii_uppercase();
    if !matches!(rating.as_str(), "G" | "S" | "Q" | "E") {
        return Err("rating must be G, S, Q or E".to_string());
    }
    let ext = wire
        .orig
        .as_deref()
        .map(|value| value.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|value| {
            !value.is_empty()
                && value.len() <= MAX_EXT_LEN
                && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .ok_or_else(|| "orig extension is invalid".to_string())?;
    if wire.tags.len() > MAX_PUSH_TAGS {
        return Err("too many tags".to_string());
    }
    for tag in &wire.tags {
        if tag.name.trim().is_empty()
            || tag.name.len() > MAX_TAG_NAME_LEN
            || tag.category.len() > MAX_TAG_CATEGORY_LEN
        {
            return Err("tag name/category is invalid".to_string());
        }
    }
    let source = wire.source.as_deref().unwrap_or("local").trim().to_string();
    if source.is_empty()
        || source.len() > 32
        || !source
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err("source is invalid".to_string());
    }
    let work_id = match wire.work_id.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(value) => {
            if value.len() > 128
                || !value.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
                })
            {
                return Err("work_id is invalid".to_string());
            }
            Some(value.to_string())
        }
    };
    if let (Some(page), Some(count)) = (wire.page, wire.page_count)
        && (count == 0 || page > count)
    {
        return Err("page is outside page_count".to_string());
    }
    Ok(db::LocalItem {
        id: wire.id,
        source,
        name: name.to_string(),
        title: wire.title.trim().to_string(),
        width: wire.width,
        height: wire.height,
        rating,
        star: wire.star,
        favorite: wire.favorite,
        mtime: if wire.mtime > 0 {
            wire.mtime
        } else {
            crypto::unix_now()
        },
        posted_at: wire.posted_at,
        orig_ext: ext,
        group_name: wire.group.trim().to_string(),
        work_id,
        work_title: wire.work_title.trim().to_string(),
        page: wire.page,
        page_count: wire.page_count,
        tags: wire.tags,
    })
}

/// 上传文件目前不入库，但要能被授权客户端读回，否则只是磁盘死区。
pub async fn serve_upload(
    State(state): State<AppState>,
    AxumPath(file): AxumPath<String>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = authorize(&state, &headers, Capability::Pull).await {
        return *response;
    }
    let Some((stem, ext)) = file.rsplit_once('.') else {
        return not_found();
    };
    if !valid_item_id(stem) {
        return not_found();
    }
    let content_type = match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        _ => return not_found(),
    };
    let root = state.media_root.as_ref().join("uploads");
    let path = root.join(&file);
    let Ok(canonical_root) = root.canonicalize() else {
        return not_found();
    };
    let Ok(canonical) = path.canonicalize() else {
        return not_found();
    };
    if !canonical.starts_with(&canonical_root) {
        return not_found();
    }
    crate::media::serve_file_range(&canonical, content_type, method, &headers, "no-store").await
}

/// 超出文件数或总字节上限时按 mtime 从旧到新清理。
async fn enforce_upload_quota(root: &std::path::Path) {
    let mut files = Vec::new();
    let mut total = 0u64;
    let Ok(mut dir) = tokio::fs::read_dir(root).await else {
        return;
    };
    while let Ok(Some(entry)) = dir.next_entry().await {
        let name = entry.file_name();
        if name.to_string_lossy().ends_with(".part") {
            // 正在写入的分片不参与配额淘汰；成功上传后会 rename 成正式文件。
            continue;
        }
        let Ok(metadata) = entry.metadata().await else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        total = total.saturating_add(metadata.len());
        files.push((metadata.modified().ok(), entry.path(), metadata.len()));
    }
    if files.len() <= MAX_UPLOAD_FILES && total <= MAX_UPLOAD_BYTES {
        return;
    }
    files.sort_by_key(|(modified, _, _)| *modified);
    let mut count = files.len();
    for (_, path, size) in files {
        if count <= MAX_UPLOAD_FILES && total <= MAX_UPLOAD_BYTES {
            break;
        }
        if tokio::fs::remove_file(&path).await.is_ok() {
            count -= 1;
            total = total.saturating_sub(size);
        }
    }
}

async fn authorize(
    state: &AppState,
    headers: &HeaderMap,
    need: Capability,
) -> Result<db::DeviceAuth, Box<Response>> {
    let Some(token) = bearer_token(headers) else {
        return Err(Box::new(bad_token()));
    };
    let token_hash = crypto::sha256_hex(token.as_bytes());
    let db_path = state.db_path.as_ref().clone();
    let now = crypto::unix_now();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db::open(&db_path)?;
        db::authenticate_device(&conn, &token_hash, now)
    })
    .await;
    match result {
        Ok(Ok(Some(device))) if need.allowed(&device) => Ok(device),
        Ok(Ok(Some(_))) => Err(Box::new(api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "device token lacks this capability",
        ))),
        Ok(Ok(None)) => Err(Box::new(bad_token())),
        Ok(Err(error)) => {
            tracing::error!(%error, "device authentication failed");
            Err(Box::new(db_unavailable()))
        }
        Err(error) => {
            tracing::error!(%error, "device authentication task failed");
            Err(Box::new(internal()))
        }
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

fn bad_token() -> Response {
    api_error(
        StatusCode::UNAUTHORIZED,
        "bad_device_token",
        "missing or invalid device token",
    )
}

fn parse_limit(raw: Option<&str>) -> Result<u32, Box<Response>> {
    match raw {
        None => Ok(DEFAULT_PULL_LIMIT),
        Some(raw) => raw
            .trim()
            .parse::<u32>()
            .map(|limit| limit.clamp(1, MAX_PULL_LIMIT))
            .map_err(|_| {
                Box::new(api_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_limit",
                    "invalid limit",
                ))
            }),
    }
}

fn decode_since(raw: Option<&str>) -> Result<(i64, String), Box<Response>> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok((0, String::new()));
    };
    if let Ok(timestamp) = raw.parse::<i64>() {
        return Ok((timestamp.max(0), String::new()));
    }
    let bytes = base64url_decode(raw).ok_or_else(|| Box::new(invalid_cursor()))?;
    let cursor: PullCursor =
        serde_json::from_slice(&bytes).map_err(|_| Box::new(invalid_cursor()))?;
    if cursor.v != 1 || !valid_item_id(&cursor.id) {
        return Err(Box::new(invalid_cursor()));
    }
    Ok((cursor.u.max(0), cursor.id))
}

fn invalid_cursor() -> Response {
    api_error(
        StatusCode::BAD_REQUEST,
        "invalid_cursor",
        "invalid pull cursor",
    )
}

fn upload_ext(headers: &HeaderMap, item_ext: Option<&str>) -> Result<String, Box<Response>> {
    let raw = headers
        .get("x-kanalib-ext")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or(item_ext)
        .unwrap_or("jpg");
    let ext = raw.trim_start_matches('.').to_ascii_lowercase();
    if ext.is_empty()
        || ext.len() > MAX_EXT_LEN
        || !ext.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(Box::new(api_error(
            StatusCode::BAD_REQUEST,
            "invalid_upload",
            "invalid extension",
        )));
    }
    Ok(ext)
}

fn payload_too_large() -> Response {
    api_error(
        StatusCode::PAYLOAD_TOO_LARGE,
        "upload_too_large",
        "upload exceeds configured limit",
    )
}

async fn cleanup_part(path: &std::path::Path) {
    let _ = tokio::fs::remove_file(path).await;
}

fn original_available(state: &AppState) -> bool {
    state.writes.media_signing_key.is_some()
}

fn media_links(id: &str, original: bool) -> MediaLinks {
    MediaLinks {
        t256: format!("/media/t256/{id}.jpg"),
        t512: format!("/media/t512/{id}.jpg"),
        t1024: format!("/media/t1024/{id}.jpg"),
        t2048: format!("/media/t2048/{id}.jpg"),
        original: original.then(|| format!("/api/items/{id}/original")),
    }
}
