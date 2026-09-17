pub mod departures;
pub mod formation;
pub mod nearby;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use traintime_core::CacheStatus;

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
