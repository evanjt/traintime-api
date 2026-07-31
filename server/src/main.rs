use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

mod cache;
mod formation;
mod ojp;
mod routes;
mod state;
mod stations;

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
async fn auth(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    if req.method() == axum::http::Method::OPTIONS {
        return with_cors(StatusCode::NO_CONTENT.into_response());
    }

    if req.uri().path() != "/health" {
        let provided = req
            .headers()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();

        if provided != state.api_key {
            return with_cors(
                (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({ "error": "Unauthorized" })),
                )
                    .into_response(),
            );
        }
    }

    with_cors(next.run(req).await)
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
    println!("loaded {} stations from {}", db.len(), stations_path.display());

    let state = AppState {
        cache: Cache::default(),
        db: Arc::new(db),
        http: reqwest::Client::builder()
            .user_agent("traintime/1.0")
            .build()
            .map_err(|e| e.to_string())?,
        ojp_api_key: required_env("OJP_API_KEY")?,
        formation_api_key: required_env("FORMATION_API_KEY")?,
        cache_ttl: std::env::var("DEPARTURE_CACHE_TTL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60),
        api_key: required_env("API_KEY")?,
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
