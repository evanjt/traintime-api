use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

mod cache;
mod fetch;
mod formation;
mod memcached;
mod ojp;
mod routes;
mod state;
mod stations;

use traintime_core::{auth, format_line, ApiKeys, CacheStatus};

use cache::Cache;
use state::AppState;
use stations::Stations;

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn fallback() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": "Not found" })),
    )
}

/// Fail-closed x-api-key check, skipped for /health so it stays probe-usable.
/// Also writes the one request line. Never the query string: see core::reqlog.
async fn auth(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    if req.method() == axum::http::Method::OPTIONS {
        return with_cors(StatusCode::NO_CONTENT.into_response());
    }

    let started = std::time::Instant::now();
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let mut key_label: Option<String> = None;

    if path != "/health" {
        let provided = req
            .headers()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();

        match state.api_keys.matched(provided) {
            Some(key) => key_label = Some(auth::label(key).to_string()),
            None => {
                let resp = with_cors(
                    (
                        StatusCode::UNAUTHORIZED,
                        Json(serde_json::json!({ "error": "Unauthorized" })),
                    )
                        .into_response(),
                );
                log_request(&method, &path, &resp, started, None);
                return resp;
            }
        }
    }

    let resp = with_cors(next.run(req).await);
    log_request(&method, &path, &resp, started, key_label.as_deref());
    resp
}

fn log_request(
    method: &str,
    path: &str,
    resp: &Response,
    started: std::time::Instant,
    key: Option<&str>,
) {
    let cache = resp
        .extensions()
        .get::<CacheStatus>()
        .copied()
        .unwrap_or_default();
    println!(
        "{}",
        format_line(
            method,
            path,
            resp.status().as_u16(),
            started.elapsed().as_millis() as u64,
            cache,
            key,
        )
    );
}

fn with_cors(mut resp: Response) -> Response {
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
    resp
}

fn required_env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("missing required env var {name}"))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.ok();
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }

    println!("shutdown signal received, draining");
}

#[tokio::main]
async fn main() -> ExitCode {
    if let Err(e) = run().await {
        eprintln!("fatal: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

async fn run() -> Result<(), String> {
    let stations_path = PathBuf::from(
        std::env::var("STATIONS_PATH").unwrap_or_else(|_| "data/stations.json".to_string()),
    );

    // Fail fast: a missing or malformed dataset should crash-loop and leave the
    // previous ReplicaSet serving, not quietly return empty /v1/nearby results.
    let db = Stations::load(&stations_path)?;
    println!(
        "loaded {} stations from {}",
        db.len(),
        stations_path.display()
    );

    // Unset means each pod caches for itself, exactly as before.
    let cache = match std::env::var("MEMCACHED_URL") {
        Ok(url) if !url.trim().is_empty() => {
            let shared = memcached::Memcached::new(&url);
            println!("sharing the cache through memcached at {}", shared.addr());
            Cache::with_shared(shared)
        }
        _ => Cache::default(),
    };

    let state = AppState {
        cache,
        inflight: fetch::Inflight::default(),
        db: Arc::new(db),
        http: reqwest::Client::builder()
            .user_agent("traintime/1.0")
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .map_err(|e| e.to_string())?,
        ojp_endpoint: std::env::var("OJP_ENDPOINT")
            .unwrap_or_else(|_| traintime_core::ojp::OJP_ENDPOINT.to_string()),
        ojp_api_key: required_env("OJP_API_KEY")?,
        formation_api_key: required_env("FORMATION_API_KEY")?,
        cache_ttl: std::env::var("DEPARTURE_CACHE_TTL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60),
        api_keys: Arc::new(ApiKeys::parse(
            std::env::var("API_KEYS").ok().as_deref(),
            std::env::var("API_KEY").ok().as_deref(),
        )?),
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/nearby", get(routes::nearby::handle_nearby))
        .route("/v1/departures", get(routes::departures::handle_departures))
        .route("/v1/formation", get(routes::formation::handle_formation))
        .fallback(fallback)
        .layer(middleware::from_fn_with_state(state.clone(), auth))
        .with_state(state);

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|e| format!("bind 0.0.0.0:{port}: {e}"))?;

    println!("listening on http://0.0.0.0:{port}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| e.to_string())
}
