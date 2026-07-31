use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;

use crate::formation::fetch_formation;
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
) -> impl IntoResponse {
    let train_raw = match params.train {
        Some(t) if !t.is_empty() => t,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Missing train parameter" })),
            );
        }
    };

    let date = match params.date {
        Some(d) if !d.is_empty() => d,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Missing date parameter" })),
            );
        }
    };

    let train_number = extract_train_number(&train_raw);
    if train_number.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid train number" })),
        );
    }

    let evu = if let Some(ref e) = params.evu {
        e.clone()
    } else if let Some(ref op) = params.operator_ref {
        match operator_ref_to_evu(op) {
            Some(mapped) => mapped.to_string(),
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({ "error": "No formation data" })),
                );
            }
        }
    } else {
        "SBBP".to_string()
    };
    let stop_key = params.stop.as_deref().unwrap_or("all");
    let cache_key = format!("formation:{evu}:{date}:{train_number}:{stop_key}");

    if let Some(cached) = state.cache.get(&cache_key) {
        println!("CACHE HIT {cache_key}");
        if let Ok(result) = serde_json::from_str::<serde_json::Value>(&cached) {
            return (StatusCode::OK, Json(result));
        }
    }

    println!("CACHE MISS {cache_key}");
    match fetch_formation(
        &state.http,
        &state.formation_api_key,
        &evu,
        &date,
        &train_number,
        params.stop.as_deref(),
    )
    .await
    {
        Ok(result) => {
            let json_val = serde_json::to_value(&result).unwrap_or_default();
            if let Ok(json_str) = serde_json::to_string(&json_val) {
                state.cache.put(&cache_key, &json_str, FORMATION_CACHE_TTL);
            }
            (StatusCode::OK, Json(json_val))
        }
        Err(e) => {
            println!("Formation error: {e}");
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "No formation data" })),
            )
        }
    }
}
