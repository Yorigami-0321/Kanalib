//! 私网内部写入：box allow 批次与删除事件。
//!
//! 只挂在 private listener；公开 listener 不注册本模块。

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use kanalib_wall_model::{IngestBatch, IngestDelete, valid_item_id};
use serde::Serialize;

use crate::{
    AppState, crypto, db,
    response::{api_error, internal, json_response},
};

const MAX_BATCH_ITEMS: usize = 5_000;
const MAX_TAGS_PER_ITEM: usize = 256;
const MAX_TEXT_LEN: usize = 1_024;
const MAX_THUMB_BYTES: usize = 8 * 1024 * 1024;

const SIGNATURE: &str = "x-kanalib-signature";
const TIMESTAMP: &str = "x-kanalib-timestamp";
const BATCH_ID: &str = "x-kanalib-batch-id";

pub const ALLOW_PATH: &str = "/internal/ingest/batch";
pub const DELETE_PATH: &str = "/internal/ingest/delete";
pub const THUMB_PREFIX: &str = "/internal/ingest/thumb";

#[derive(Serialize)]
struct BatchResponse {
    ok: bool,
    duplicate: bool,
    inserted: u32,
    updated: u32,
    accepted: u32,
    stale: u32,
}

#[derive(Serialize)]
struct DeleteResponse {
    ok: bool,
    duplicate: bool,
    deleted: u32,
    stale: u32,
    missing: u32,
}

pub async fn batch(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(secret) = state.writes.ingest_secret.clone() else {
        return not_configured();
    };
    let batch_id = match batch_id(&headers) {
        Ok(id) => id,
        Err(response) => return *response,
    };
    if let Err(response) = verify_signature(&headers, ALLOW_PATH, &batch_id, &secret, &body) {
        return *response;
    }
    let batch: IngestBatch = match serde_json::from_slice(&body) {
        Ok(batch) => batch,
        Err(error) => {
            tracing::debug!(%error, "invalid ingest batch body");
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_batch",
                "invalid batch body",
            );
        }
    };
    if let Err(response) = validate_batch(&batch) {
        return *response;
    }

    let db_path = state.db_path.as_ref().clone();
    let result = tokio::task::spawn_blocking(move || {
        let mut conn = db::open(&db_path)?;
        db::apply_ingest_batch(&mut conn, &batch_id, &batch, crypto::unix_now())
    })
    .await;

    match result {
        Ok(Ok(apply)) => json_response(
            StatusCode::OK,
            BatchResponse {
                ok: true,
                duplicate: apply.duplicate,
                inserted: apply.inserted,
                updated: apply.updated,
                accepted: apply.accepted(),
                stale: apply.stale,
            },
        ),
        Ok(Err(error)) => {
            tracing::error!(%error, "ingest batch failed");
            internal()
        }
        Err(error) => {
            tracing::error!(%error, "ingest batch task failed");
            internal()
        }
    }
}

pub async fn delete_batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(secret) = state.writes.ingest_secret.clone() else {
        return not_configured();
    };
    let batch_id = match batch_id(&headers) {
        Ok(id) => id,
        Err(response) => return *response,
    };
    if let Err(response) = verify_signature(&headers, DELETE_PATH, &batch_id, &secret, &body) {
        return *response;
    }
    let delete: IngestDelete = match serde_json::from_slice(&body) {
        Ok(delete) => delete,
        Err(error) => {
            tracing::debug!(%error, "invalid ingest delete body");
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_batch",
                "invalid delete body",
            );
        }
    };
    if let Err(response) = validate_delete(&delete) {
        return *response;
    }

    let db_path = state.db_path.as_ref().clone();
    let result = tokio::task::spawn_blocking(move || {
        let mut conn = db::open(&db_path)?;
        db::apply_ingest_delete(&mut conn, &batch_id, &delete, crypto::unix_now())
    })
    .await;

    match result {
        Ok(Ok(apply)) => json_response(
            StatusCode::OK,
            DeleteResponse {
                ok: true,
                duplicate: apply.duplicate,
                deleted: apply.deleted,
                stale: apply.stale,
                missing: apply.missing,
            },
        ),
        Ok(Err(error)) => {
            tracing::error!(%error, "ingest delete failed");
            internal()
        }
        Err(error) => {
            tracing::error!(%error, "ingest delete task failed");
            internal()
        }
    }
}

/// box → 网页缩略图上传。只走 private listener，body 为 JPEG 原字节。
pub async fn thumb(
    State(state): State<AppState>,
    Path((tier, file)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(secret) = state.writes.ingest_secret.clone() else {
        return not_configured();
    };
    if !crate::media::valid_tier(&tier) || !crate::media::valid_file_name(&file) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_thumb",
            "tier or file name is invalid",
        );
    }
    if body.is_empty() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_thumb",
            "thumbnail is empty",
        );
    }
    if body.len() > MAX_THUMB_BYTES {
        return api_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "invalid_thumb",
            "thumbnail is too large",
        );
    }
    if !body.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_thumb",
            "thumbnail must be a JPEG",
        );
    }
    let path = format!("{THUMB_PREFIX}/{tier}/{file}");
    if let Err(response) = verify_payload(&headers, &path, "", &secret, &body) {
        return *response;
    }

    let media_root = state.media_root.as_ref().clone();
    let result = tokio::task::spawn_blocking(move || {
        let dir = media_root.join(&tier);
        std::fs::create_dir_all(&dir)?;
        let target = dir.join(&file);
        let temp = dir.join(format!(".{file}.upload.tmp"));
        std::fs::write(&temp, &body)?;
        std::fs::rename(&temp, &target)?;
        Ok::<_, std::io::Error>(())
    })
    .await;

    match result {
        Ok(Ok(())) => json_response(StatusCode::OK, serde_json::json!({ "ok": true })),
        Ok(Err(error)) => {
            tracing::error!(%error, "thumbnail write failed");
            internal()
        }
        Err(error) => {
            tracing::error!(%error, "thumbnail task failed");
            internal()
        }
    }
}

fn batch_id(headers: &HeaderMap) -> Result<String, Box<Response>> {
    let value = headers
        .get(BATCH_ID)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(value) = value else {
        return Err(Box::new(api_error(
            StatusCode::BAD_REQUEST,
            "missing_batch_id",
            "X-Kanalib-Batch-Id is required",
        )));
    };
    if value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(Box::new(api_error(
            StatusCode::BAD_REQUEST,
            "missing_batch_id",
            "invalid X-Kanalib-Batch-Id",
        )));
    }
    Ok(value.to_string())
}

/// 批次签名规范串：方法、路径、批次 id、时间戳与 body 哈希全部纳入。
fn signature_payload(path: &str, batch_id: &str, timestamp: i64, body: &[u8]) -> String {
    format!(
        "POST\n{path}\n{batch_id}\n{timestamp}\n{}",
        crypto::sha256_hex(body)
    )
}

fn verify_payload(
    headers: &HeaderMap,
    path: &str,
    batch_id: &str,
    secrets: &str,
    body: &[u8],
) -> Result<(), Box<Response>> {
    let timestamp = headers
        .get(TIMESTAMP)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<i64>().ok());
    let Some(timestamp) = timestamp else {
        return Err(Box::new(bad_signature("missing or invalid timestamp")));
    };
    if (crypto::unix_now() - timestamp).abs() > crypto::MAX_CLOCK_SKEW_SECS {
        return Err(Box::new(bad_signature("timestamp outside allowed window")));
    }
    let signature = headers
        .get(SIGNATURE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let payload = signature_payload(path, batch_id, timestamp, body);
    let valid = secrets
        .split(',')
        .map(str::trim)
        .filter(|secret| !secret.is_empty())
        .any(|secret| crypto::hmac_sha256_verify(secret.as_bytes(), payload.as_bytes(), signature));
    if !valid {
        return Err(Box::new(bad_signature("signature mismatch")));
    }
    Ok(())
}

fn verify_signature(
    headers: &HeaderMap,
    path: &str,
    batch_id: &str,
    secrets: &str,
    body: &[u8],
) -> Result<(), Box<Response>> {
    verify_payload(headers, path, batch_id, secrets, body)
}

fn bad_signature(message: &'static str) -> Response {
    api_error(StatusCode::UNAUTHORIZED, "bad_signature", message)
}

fn not_configured() -> Response {
    api_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "not_configured",
        "ingest endpoint is not configured",
    )
}

fn validate_batch(batch: &IngestBatch) -> Result<(), Box<Response>> {
    if batch.source.trim().is_empty() || batch.source.len() > 64 {
        return Err(Box::new(invalid_batch("invalid source")));
    }
    if batch.items.len() > MAX_BATCH_ITEMS {
        return Err(Box::new(invalid_batch("too many items in batch")));
    }
    if batch
        .rules_version
        .as_deref()
        .is_some_and(|value| value.len() > 128)
    {
        return Err(Box::new(invalid_batch("rules_version too long")));
    }
    for entry in &batch.items {
        if !entry.decision.eq_ignore_ascii_case("allow") {
            return Err(Box::new(api_error(
                StatusCode::BAD_REQUEST,
                "non_allow_batch",
                "batch contains non-allow items",
            )));
        }
        if !valid_item_id(&entry.item.id) {
            return Err(Box::new(api_error(
                StatusCode::BAD_REQUEST,
                "invalid_item",
                "item id must be 40 or 64 lowercase hex",
            )));
        }
        // fail-closed：未知分级不能被 normalize_rating 折叠成 G。
        if !matches!(
            entry.item.rating.trim().to_ascii_uppercase().as_str(),
            "G" | "S" | "Q" | "E"
        ) {
            return Err(Box::new(api_error(
                StatusCode::BAD_REQUEST,
                "invalid_rating",
                "rating must be one of G/S/Q/E",
            )));
        }
        if entry.revision < 1
            || entry.item.tags.len() > MAX_TAGS_PER_ITEM
            || entry.item.name.len() > MAX_TEXT_LEN
            || entry
                .item
                .title
                .as_deref()
                .is_some_and(|v| v.len() > MAX_TEXT_LEN)
            || entry.item.group.as_deref().is_some_and(|v| v.len() > 256)
            || entry
                .item
                .author
                .as_deref()
                .is_some_and(|v| v.len() > MAX_TEXT_LEN)
        {
            return Err(Box::new(invalid_batch("item fields out of range")));
        }
    }
    Ok(())
}

fn validate_delete(delete: &IngestDelete) -> Result<(), Box<Response>> {
    if delete.source.trim().is_empty() || delete.source.len() > 64 {
        return Err(Box::new(invalid_batch("invalid source")));
    }
    if delete.items.len() > MAX_BATCH_ITEMS {
        return Err(Box::new(invalid_batch("too many items in batch")));
    }
    for entry in &delete.items {
        if !valid_item_id(&entry.id) {
            return Err(Box::new(api_error(
                StatusCode::BAD_REQUEST,
                "invalid_item",
                "item id must be 40 or 64 lowercase hex",
            )));
        }
        if entry.revision < 1 {
            return Err(Box::new(invalid_batch("revision must be >= 1")));
        }
    }
    Ok(())
}

fn invalid_batch(message: &'static str) -> Response {
    api_error(StatusCode::BAD_REQUEST, "invalid_batch", message)
}
