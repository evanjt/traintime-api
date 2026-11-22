use std::sync::Arc;

use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use tower_service::Service;
use worker::*;

use traintime_core::{auth, format_line, ApiKeys, CacheStatus};

mod cache;
mod formation;
mod ojp;
mod ptv;
mod replica;
mod routes;
mod source;
mod upstream;

type AxumResponse = axum::http::Response<axum::body::Body>;

#[derive(Clone)]
pub struct AppState {
    /// D1 reads, served by the nearest replica.
    pub db: Arc<replica::ReadSession>,
    pub ojp_api_key: String,
    /// OJP_ENDPOINT override. Local burst tests point it at a stub so a run
    /// never reaches SBB.
    pub ojp_endpoint: String,
    pub formation_api_key: String,
    /// PTV_ENDPOINT override, for the same reason as OJP_ENDPOINT.
    pub ptv_endpoint: String,
    /// PTV_DEVID and PTV_KEY. Unset, Victorian stations answer 503.
    pub ptv_devid: Option<String>,
    pub ptv_key: Option<String>,
    pub cache_ttl: u64,
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/nearby", get(routes::nearby::handle_nearby))
        .route("/v1/departures", get(routes::departures::handle_departures))
        .route("/v1/formation", get(routes::formation::handle_formation))
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

// The one request line. Never the URI with its query, see core::reqlog.
fn log_request(method: &str, path: &str, resp: &AxumResponse, started: u64, key: Option<&str>) {
    let cache = resp
        .extensions()
        .get::<CacheStatus>()
        .copied()
        .unwrap_or_default();
    console_log!(
        "{}",
        format_line(
            method,
            path,
            resp.status().as_u16(),
            Date::now().as_millis().saturating_sub(started),
            cache,
            key,
        )
    );
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

/// A secret, or a plain var for local runs, or nothing.
fn optional_secret(env: &Env, name: &str) -> Option<String> {
    env.secret(name)
        .map(|s| s.to_string())
        .or_else(|_| env.var(name).map(|v| v.to_string()))
        .ok()
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

    let started = Date::now().as_millis();
    let mut key_label: Option<String> = None;

    // Auth check (skip /health)
    if path != "/health" {
        let provided_key = req
            .headers()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok());
        let api_keys = ApiKeys::parse(
            env.secret("API_KEYS")
                .map(|s| s.to_string())
                .or_else(|_| env.var("API_KEYS").map(|v| v.to_string()))
                .ok()
                .as_deref(),
            env.secret("API_KEY")
                .map(|s| s.to_string())
                .or_else(|_| env.var("API_KEY").map(|v| v.to_string()))
                .ok()
                .as_deref(),
        );

        if let (Ok(keys), Some(provided)) = (&api_keys, provided_key) {
            key_label = keys.matched(provided).map(|k| auth::label(k).to_string());
        }

        if key_label.is_none() {
            let mut resp = axum::http::Response::builder()
                .status(401)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::to_string(&serde_json::json!({ "error": "Unauthorized" }))
                        .unwrap(),
                ))
                .unwrap();
            add_cors_headers(&mut resp);
            log_request(method.as_str(), &path, &resp, started, None);
            return Ok(resp);
        }
    }

    // Build state
    let db = replica::ReadSession::open(env.d1("DB")?)?;
    let ojp_api_key = env
        .secret("OJP_API_KEY")
        .map(|s| s.to_string())
        .or_else(|_| env.var("OJP_API_KEY").map(|v| v.to_string()))?;
    let ojp_endpoint = env
        .var("OJP_ENDPOINT")
        .map(|v| v.to_string())
        .unwrap_or_else(|_| traintime_core::ojp::OJP_ENDPOINT.to_string());
    let formation_api_key = env
        .secret("FORMATION_API_KEY")
        .map(|s| s.to_string())
        .or_else(|_| env.var("FORMATION_API_KEY").map(|v| v.to_string()))?;
    let ptv_endpoint = env
        .var("PTV_ENDPOINT")
        .map(|v| v.to_string())
        .unwrap_or_else(|_| traintime_core::ptv::PTV_ENDPOINT.to_string());
    let ptv_devid = optional_secret(&env, "PTV_DEVID");
    let ptv_key = optional_secret(&env, "PTV_KEY");
    let cache_ttl = env
        .var("DEPARTURE_CACHE_TTL")
        .ok()
        .and_then(|v| v.to_string().parse::<u64>().ok())
        .unwrap_or(60);

    let state = AppState {
        db,
        ojp_api_key,
        ojp_endpoint,
        formation_api_key,
        ptv_endpoint,
        ptv_devid,
        ptv_key,
        cache_ttl,
    };

    // Route the request
    match router(state).call(req).await {
        Ok(mut resp) => {
            add_cors_headers(&mut resp);
            log_request(method.as_str(), &path, &resp, started, key_label.as_deref());
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

#[cfg(test)]
mod tests {
    // Every file of the Worker. A new one is added here so the scan below sees it.
    const SOURCES: &[(&str, &str)] = &[
        ("lib.rs", include_str!("lib.rs")),
        ("cache.rs", include_str!("cache.rs")),
        ("formation.rs", include_str!("formation.rs")),
        ("ojp.rs", include_str!("ojp.rs")),
        ("ptv.rs", include_str!("ptv.rs")),
        ("source.rs", include_str!("source.rs")),
        ("upstream.rs", include_str!("upstream.rs")),
        ("routes/mod.rs", include_str!("routes/mod.rs")),
        ("routes/nearby.rs", include_str!("routes/nearby.rs")),
        ("routes/departures.rs", include_str!("routes/departures.rs")),
        ("routes/formation.rs", include_str!("routes/formation.rs")),
    ];

    // The request state holds only the replica session, so a read site cannot
    // reach the primary by accident.
    #[allow(dead_code)]
    fn state_reads_through_session(state: &super::AppState) -> &super::replica::ReadSession {
        &state.db
    }

    #[test]
    fn only_the_replica_module_names_the_bare_database() {
        let bare = concat!("D1", "Database");
        for (name, src) in SOURCES {
            assert!(!src.contains(bare), "{name} names the bare database handle");
        }
    }
}
