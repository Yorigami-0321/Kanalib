//! 双 listener 路由：公开只读 + 私网写入。

pub mod desktop;
pub mod ingest;
pub mod original;
pub mod read;

use axum::{
    Router,
    body::Body,
    extract::{DefaultBodyLimit, Request, State},
    response::Response,
    routing::{get, post},
};
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

use crate::{
    AppState,
    response::{internal, not_found},
};

pub fn public_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(read::health))
        .route("/api/index", get(read::index))
        .route("/api/items", get(read::items))
        .route("/api/items/{id}", get(read::item))
        .route("/api/items/{id}/original", get(original_not_available))
        .route("/api/works", get(read::works))
        .route("/api/works/{id}", get(read::work))
        .route("/api/authors", get(read::authors))
        .route("/api/authors/{id}", get(read::author))
        .route("/api/authors/{id}/works", get(read::author_works))
        .route("/api/collections", get(read::collections))
        .route("/api/collections/{id}", get(read::collection))
        .route("/api/collections/{id}/works", get(read::collection_works))
        .route("/api/tags", get(read::tags))
        .route("/media/{tier}/{file}", get(crate::media::serve_media))
        .fallback(public_fallback)
        .with_state(state)
}

pub fn private_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(read::health))
        .route("/internal/ingest/batch", post(ingest::batch))
        .route("/internal/ingest/delete", post(ingest::delete_batch))
        .route("/internal/ingest/thumb/{tier}/{file}", post(ingest::thumb))
        .route("/api/desktop/pull", get(desktop::pull))
        .route("/api/desktop/push", post(desktop::push))
        .route("/api/desktop/upload", post(desktop::upload))
        .route("/api/items/{id}/original", get(original::original))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .fallback(private_fallback)
        .with_state(state)
}

async fn private_fallback() -> Response {
    not_found()
}

/// 公网 listener 永远不暴露原图（包括 302 的目标私网地址）。
async fn original_not_available() -> Response {
    not_found()
}

async fn public_fallback(State(state): State<AppState>, request: Request<Body>) -> Response {
    let path = request.uri().path();
    if path == "/api"
        || path.starts_with("/api/")
        || path == "/internal"
        || path.starts_with("/internal/")
    {
        return not_found();
    }

    let Some(web_root) = state.web_root.as_ref().map(|root| root.as_ref().clone()) else {
        return not_found();
    };
    let index = web_root.join("index.html");
    if !index.is_file() {
        return not_found();
    }

    let service = ServeDir::new(&web_root).fallback(ServeFile::new(index));
    service
        .oneshot(request)
        .await
        .map(|response| response.map(Body::new))
        .unwrap_or_else(|_| internal())
}
