//! 私网原图回源：302 到本服务相对路径，再由私网 listener 反代 box。
//!
//! `Location` 不再携带 `box_origin`，客户端只能看到私网 listener 的地址；
//! box 的真实地址与签名 URL 都留在服务端。

use std::{sync::OnceLock, time::Duration};

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use kanalib_wall_model::valid_item_id;
use serde::Deserialize;

use crate::{
    AppState, crypto, db,
    response::{internal, not_found},
};

#[derive(Debug, Deserialize)]
pub(crate) struct SignedQuery {
    expires: Option<i64>,
    sig: Option<String>,
}

/// 只透传表示媒体本身的响应头；连接级/传输级头由各段连接各自管理。
const FORWARD_HEADERS: [header::HeaderName; 9] = [
    header::CONTENT_TYPE,
    header::CONTENT_LENGTH,
    header::CONTENT_RANGE,
    header::CONTENT_ENCODING,
    header::ACCEPT_RANGES,
    header::CACHE_CONTROL,
    header::LAST_MODIFIED,
    header::ETAG,
    header::CONTENT_DISPOSITION,
];

pub async fn original(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if !valid_item_id(&id) {
        return not_found();
    }
    let (Some(key), true) = (
        state.writes.media_signing_key.as_deref(),
        state.writes.box_origin.is_some(),
    ) else {
        return not_found();
    };

    let db_path = state.db_path.as_ref().clone();
    let lookup = id.clone();
    let exists = tokio::task::spawn_blocking(move || {
        let conn = db::open_read(&db_path)?;
        db::item_is_public(&conn, &lookup)
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
    let Ok(location) = HeaderValue::from_str(&format!("{path}?expires={expires}&sig={signature}"))
    else {
        tracing::error!("signed original location is not a valid header value");
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

/// 私网 listener 上的原图反代：流式转发 GET/HEAD 与单区间 Range。
pub(crate) async fn serve(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<SignedQuery>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    if !valid_item_id(&id) {
        return not_found();
    }
    let Some(origin) = state.writes.box_origin.as_deref() else {
        return not_found();
    };
    let (Some(expires), Some(signature)) = (query.expires, query.sig.as_deref()) else {
        return not_found();
    };
    if expires <= 0 || !valid_signature(signature) {
        return not_found();
    }

    let url = format!("{origin}/original/{id}?expires={expires}&sig={signature}");
    let mut request = client().request(method.clone(), url);
    if let Some(range) = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
    {
        request = request.header(header::RANGE, range);
    }

    let upstream = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%error, %id, "box original request failed");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    let status = upstream.status();
    let upstream_headers = upstream.headers().clone();
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from_stream(upstream.bytes_stream())
    };

    let mut response = Response::builder()
        .status(status)
        .body(body)
        .unwrap_or_else(|_| internal());
    let out = response.headers_mut();
    for name in FORWARD_HEADERS {
        if let Some(value) = upstream_headers.get(&name) {
            out.insert(name, value.clone());
        }
    }
    response
}

fn valid_signature(signature: &str) -> bool {
    signature.len() == 64 && signature.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// 连接池与回源配置全局复用；原图可能很大，不设整体超时。
fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .no_proxy()
            .build()
            .expect("build original proxy client")
    })
}
