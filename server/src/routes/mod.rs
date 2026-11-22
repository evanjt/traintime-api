pub mod departures;
pub mod formation;
pub mod nearby;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use traintime_core::{CacheStatus, ErrorResponse};

/// A JSON response, marked when it came from the stale cache. The cache
/// status rides on the extensions for the request log line.
pub fn respond(
    status: StatusCode,
    body: serde_json::Value,
    stale_age: Option<u64>,
    cache: CacheStatus,
) -> Response {
    let mut resp = (status, Json(body)).into_response();
    resp.extensions_mut().insert(cache);
    if let Some(age) = stale_age {
        resp.headers_mut()
            .insert("x-traintime-stale", age.to_string().parse().unwrap());
    }
    resp
}

/// A failed request as core decided it should be answered.
pub fn respond_error(error: ErrorResponse, cache: CacheStatus) -> Response {
    let status = StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut resp = respond(status, error.body, None, cache);
    if let Some(secs) = error.retry_after {
        resp.headers_mut()
            .insert("retry-after", secs.to_string().parse().unwrap());
    }
    resp
}
