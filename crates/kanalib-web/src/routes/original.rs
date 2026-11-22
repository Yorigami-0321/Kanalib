//! 私网原图回源：校验条目后 302 到 box 的短期签名 URL。

use axum::{
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use kanalib_wall_model::valid_item_id;

use crate::{
    AppState, crypto, db,
    response::{internal, not_found},
};

pub async fn original(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if !valid_item_id(&id) {
        return not_found();
    }
    let (Some(origin), Some(key)) = (
        state.writes.box_origin.as_deref(),
        state.writes.media_signing_key.as_deref(),
    ) else {
        return not_found();
    };

    let db_path = state.db_path.as_ref().clone();
    let lookup = id.clone();
    let exists = tokio::task::spawn_blocking(move || {
        let conn = db::open_read(&db_path)?;
        db::item_is_live(&conn, &lookup)
    })
    .await;
    match exists {
        Ok(Ok(true)) => {}
        Ok(Ok(false)) => return not_found(),
        Ok(Err(error)) => {
            tracing::error!(%error, "original lookup failed");
            return internal();
        }
        Err(error) => {
            tracing::error!(%error, "original lookup task failed");
            return internal();
        }
    }

    let path = format!("/original/{id}");
    let ttl = i64::try_from(state.writes.signed_url_ttl.as_secs()).unwrap_or(300);
    let expires = crypto::unix_now().saturating_add(ttl.max(1));
    let canonical = format!("GET\n{path}\n{expires}");
    let Some(signature) = crypto::hmac_sha256_hex(key.as_bytes(), canonical.as_bytes()) else {
        return internal();
    };
    let location = format!("{origin}{path}?expires={expires}&sig={signature}");
    let Ok(location) = HeaderValue::from_str(&location) else {
        tracing::error!("box origin produced an invalid header value");
        return internal();
    };

    let mut response = StatusCode::FOUND.into_response();
    let headers = response.headers_mut();
    headers.insert(header::LOCATION, location);
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    response
}
