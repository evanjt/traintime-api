use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use worker::console_log;

use crate::ojp::fetch_departures;
use crate::AppState;

#[derive(Deserialize)]
pub struct DeparturesQuery {
    id: Option<String>,
    limit: Option<u32>,
}

#[worker::send]
pub async fn handle_departures(
    State(state): State<AppState>,
    Query(params): Query<DeparturesQuery>,
) -> impl IntoResponse {
    let station_id = match params.id {
        Some(id) => id,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Missing id parameter" })),
            );
        }
    };

    let limit = params.limit.unwrap_or(5);
    let cache_key = format!("departures:{station_id}:{limit}");

    // Check cache
    if let Ok(Some(cached)) = state.cache.get(&cache_key).text().await {
        console_log!("CACHE HIT {}", cache_key);
        if let Ok(departures) = serde_json::from_str::<serde_json::Value>(&cached) {
            return (
                StatusCode::OK,
                Json(serde_json::json!({ "departures": departures })),
            );
        }
    }

    console_log!("CACHE MISS {}", cache_key);
    match fetch_departures(&state.ojp_api_key, &station_id, limit).await {
        Ok(departures) => {
            // Cache the result
            if let Ok(json_str) = serde_json::to_string(&departures) {
                let _ = state
                    .cache
                    .put(&cache_key, &json_str)
                    .unwrap()
                    .expiration_ttl(state.cache_ttl)
                    .execute()
                    .await;
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({ "departures": departures })),
            )
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}
