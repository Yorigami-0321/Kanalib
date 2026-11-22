use axum::{
    Json,
    http::{HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use serde_json::json;

const NOSNIFF: HeaderName = HeaderName::from_static("x-content-type-options");

pub fn json_response<T: Serialize>(status: StatusCode, value: T) -> Response {
    let mut response = (status, Json(value)).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(NOSNIFF, HeaderValue::from_static("nosniff"));
    response
}

pub fn api_error(status: StatusCode, code: &'static str, message: &'static str) -> Response {
    json_response(
        status,
        json!({ "error": { "code": code, "message": message } }),
    )
}

pub fn not_found() -> Response {
    api_error(StatusCode::NOT_FOUND, "not_found", "not found")
}

pub fn db_unavailable() -> Response {
    api_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "db_unavailable",
        "database unavailable",
    )
}

pub fn internal() -> Response {
    api_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "internal error",
    )
}
