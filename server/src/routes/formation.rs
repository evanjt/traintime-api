use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use traintime_core::CacheStatus;

use crate::fetch::Fetched;
use crate::formation::fetch_formation;
use crate::routes::respond;
use crate::state::AppState;
use traintime_core::formation::{extract_train_number, operator_ref_to_evu};

const FORMATION_CACHE_TTL: u64 = 300;

#[derive(Deserialize)]
pub struct FormationQuery {
    train: Option<String>,
    date: Option<String>,
    stop: Option<String>,
    evu: Option<String>,
    #[serde(rename = "operatorRef")]
    operator_ref: Option<String>,
}

pub async fn handle_formation(
    State(state): State<AppState>,
    Query(params): Query<FormationQuery>,
) -> Response {
    let train_raw = match params.train {
        Some(t) if !t.is_empty() => t,
        _ => {
            return respond(
                StatusCode::BAD_REQUEST,
                serde_json::json!({ "error": "Missing train parameter" }), None, CacheStatus::None);
        }
    };

    let date = match params.date {
        Some(d) if !d.is_empty() => d,
        _ => {
            return respond(
                StatusCode::BAD_REQUEST,
                serde_json::json!({ "error": "Missing date parameter" }), None, CacheStatus::None);
        }
    };

    let train_number = extract_train_number(&train_raw);
    if train_number.is_empty() {
        return respond(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "error": "Invalid train number" }), None, CacheStatus::None);
    }

    let evu = if let Some(ref e) = params.evu {
        e.clone()
    } else if let Some(ref op) = params.operator_ref {
        match operator_ref_to_evu(op) {
            Some(mapped) => mapped.to_string(),
            None => {
                return respond(
                    StatusCode::NOT_FOUND,
                    serde_json::json!({ "error": "No formation data" }), None, CacheStatus::None);
            }
        }
    } else {
        "SBBP".to_string()
    };
    let stop_key = params.stop.as_deref().unwrap_or("all");
    let cache_key = format!("formation:{evu}:{date}:{train_number}:{stop_key}");

    let fetched = state
        .inflight
        .cached(&state.cache, &cache_key, FORMATION_CACHE_TTL, || async {
            let result = fetch_formation(
                &state.http,
                &state.formation_api_key,
                &evu,
                &date,
                &train_number,
                params.stop.as_deref(),
            )
            .await?;
            serde_json::to_string(&result).map_err(|e| e.to_string())
        })
        .await;
    let cache = fetched.cache_status();
    let (json, stale) = match fetched {
        Fetched::Fresh(v) | Fetched::Cached(v) => (v, None),
        Fetched::Stale(v, age) => (v, Some(age)),
        Fetched::Failed(e) => {
            println!("Formation error: {e}");
            return respond(
                StatusCode::NOT_FOUND,
                serde_json::json!({ "error": "No formation data" }), None, cache);
        }
    };
    match serde_json::from_str::<serde_json::Value>(&json) {
        Ok(v) => respond(StatusCode::OK, v, stale, cache),
        Err(e) => respond(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }), None, cache),
    }
}
