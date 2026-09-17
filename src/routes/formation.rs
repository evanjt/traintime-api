use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use traintime_core::CacheStatus;
use worker::console_log;

use crate::cache::{self, Cached};
use crate::formation::fetch_formation;
use crate::routes::respond;
use crate::AppState;
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

#[worker::send]
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

    let mut cache;
    let (json, stale) = match cache::get(&state.cache, &cache_key, FORMATION_CACHE_TTL).await {
        Cached::Fresh(v) => {
            console_log!("CACHE HIT {}", cache_key);
            cache = CacheStatus::Hit;
            (v, None)
        }
        cached => {
            console_log!("CACHE MISS {}", cache_key);
            cache = CacheStatus::Miss;
            match fetch_formation(
                &state.formation_api_key,
                &evu,
                &date,
                &train_number,
                params.stop.as_deref(),
            )
            .await
            {
                Ok(result) => {
                    let json = serde_json::to_string(&result).unwrap_or_default();
                    cache::put(&state.cache, &cache_key, &json, FORMATION_CACHE_TTL).await;
                    (json, None)
                }
                Err(e) => match cached {
                    Cached::Stale(v, age) => {
                        console_log!("STALE {} {}s: {}", cache_key, age, e);
                        cache = CacheStatus::Hit;
                        (v, Some(age))
                    }
                    _ => {
                        console_log!("Formation error: {}", e);
                        return respond(
                            StatusCode::NOT_FOUND,
                            serde_json::json!({ "error": "No formation data" }), None, cache);
                    }
                },
            }
        }
    };
    match serde_json::from_str::<serde_json::Value>(&json) {
        Ok(v) => respond(StatusCode::OK, v, stale, cache),
        Err(e) => respond(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }), None, cache),
    }
}
