use std::sync::Arc;

use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use tower_service::Service;
use worker::kv::KvStore;
use worker::*;

mod ojp;
mod routes;

type AxumResponse = axum::http::Response<axum::body::Body>;

#[derive(Clone)]
pub struct AppState {
    pub cache: KvStore,
    pub db: Arc<worker::d1::D1Database>,
    pub ojp_api_key: String,
    pub cache_ttl: u64,
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/nearby", get(routes::nearby::handle_nearby))
        .route("/v1/departures", get(routes::departures::handle_departures))
        .fallback(fallback)
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn fallback() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": "Not found" })),
    )
}

fn add_cors_headers(resp: &mut AxumResponse) {
    let h = resp.headers_mut();
    h.insert("access-control-allow-origin", "*".parse().unwrap());
    h.insert(
        "access-control-allow-methods",
        "GET, OPTIONS".parse().unwrap(),
    );
    h.insert(
        "access-control-allow-headers",
        "Content-Type, X-API-Key".parse().unwrap(),
    );
    h.insert("x-api-version", "1".parse().unwrap());
}

#[event(fetch)]
async fn fetch(req: HttpRequest, env: Env, _ctx: Context) -> Result<AxumResponse> {
    let method = req.method().clone();
    let path = req.uri().path().to_string();

    // OPTIONS preflight
    if method == axum::http::Method::OPTIONS {
        let mut resp = axum::http::Response::builder()
            .status(204)
            .body(axum::body::Body::empty())
            .unwrap();
        add_cors_headers(&mut resp);
        return Ok(resp);
    }

    // Auth check (skip /health)
    if path != "/health" {
        let provided_key = req
            .headers()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok());
        let api_key = env
            .secret("API_KEY")
            .map(|s| s.to_string())
            .or_else(|_| env.var("API_KEY").map(|v| v.to_string()))
            .ok();

        let authorized = match (&api_key, provided_key) {
            (Some(expected), Some(provided)) => provided == expected.as_str(),
            _ => false,
        };

        if !authorized {
            let mut resp = axum::http::Response::builder()
                .status(401)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::to_string(&serde_json::json!({ "error": "Unauthorized" }))
                        .unwrap(),
                ))
                .unwrap();
            add_cors_headers(&mut resp);
            return Ok(resp);
        }
    }

    // Build state
    let cache = env.kv("CACHE")?;
    let db = env.d1("DB")?;
    let ojp_api_key = env
        .secret("OJP_API_KEY")
        .map(|s| s.to_string())
        .or_else(|_| env.var("OJP_API_KEY").map(|v| v.to_string()))?;
    let cache_ttl = env
        .var("DEPARTURE_CACHE_TTL")
        .ok()
        .and_then(|v| v.to_string().parse::<u64>().ok())
        .unwrap_or(60);

    let state = AppState {
        cache,
        db: Arc::new(db),
        ojp_api_key,
        cache_ttl,
    };

    // Route the request
    match router(state).call(req).await {
        Ok(mut resp) => {
            add_cors_headers(&mut resp);
            Ok(resp)
        }
        Err(e) => {
            let body =
                serde_json::to_string(&serde_json::json!({ "error": e.to_string() })).unwrap();
            let mut resp = axum::http::Response::builder()
                .status(500)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body))
                .unwrap();
            add_cors_headers(&mut resp);
            Ok(resp)
        }
    }
}
