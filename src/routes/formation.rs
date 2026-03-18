use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use worker::console_log;

use crate::formation::fetch_formation;
use crate::AppState;

const FORMATION_CACHE_TTL: u64 = 300;

#[derive(Deserialize)]
pub struct FormationQuery {
    train: Option<String>,
    date: Option<String>,
    stop: Option<String>,
    evu: Option<String>,
}

/// Strip non-numeric prefix from train number (e.g. "IR95" -> "95", "S3" -> "3")
fn extract_train_number(raw: &str) -> String {
    raw.trim_start_matches(|c: char| !c.is_ascii_digit())
        .to_string()
}

#[worker::send]
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

    let evu = params.evu.unwrap_or_else(|| "SBBP".to_string());
    let stop_key = params.stop.as_deref().unwrap_or("all");
    let cache_key = format!("formation:{evu}:{date}:{train_number}:{stop_key}");

    // Check cache
    if let Ok(Some(cached)) = state.cache.get(&cache_key).text().await {
        console_log!("CACHE HIT {}", cache_key);
        if let Ok(result) = serde_json::from_str::<serde_json::Value>(&cached) {
            return (StatusCode::OK, Json(result));
        }
    }

    console_log!("CACHE MISS {}", cache_key);
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
            let json_val = serde_json::to_value(&result).unwrap_or_default();
            // Cache the result
            if let Ok(json_str) = serde_json::to_string(&json_val) {
                let _ = state
                    .cache
                    .put(&cache_key, &json_str)
                    .unwrap()
                    .expiration_ttl(FORMATION_CACHE_TTL)
                    .execute()
                    .await;
            }
            (StatusCode::OK, Json(json_val))
        }
        Err(e) => {
            console_log!("Formation error: {}", e);
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "No formation data" })),
            )
        }
    }
}
