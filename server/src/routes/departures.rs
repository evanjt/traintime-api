use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;

use crate::ojp::fetch_departures;
use crate::state::AppState;
use traintime_core::favourites::{parse_favourites, partition_favourites};
use traintime_core::ojp::FlatDeparture;

#[derive(Deserialize)]
pub struct DeparturesQuery {
    id: Option<String>,
    limit: Option<u32>,
    favourites: Option<String>,
}

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
    let fav_pairs = params
        .favourites
        .as_deref()
        .map(parse_favourites)
        .unwrap_or_default();
    let has_favourites = !fav_pairs.is_empty();

    // Always fetch 50 to populate cache; partition down to `limit` on return
    let fetch_limit = 50u32;
    let cache_key = format!("departures:{station_id}:{fetch_limit}");

    if let Some(cached) = state.cache.get(&cache_key) {
        println!("CACHE HIT {cache_key}");
        if let Ok(departures) = serde_json::from_str::<Vec<FlatDeparture>>(&cached) {
            if has_favourites {
                let (favs, deps) = partition_favourites(&departures, &fav_pairs, limit as usize);
                return (
                    StatusCode::OK,
                    Json(serde_json::json!({ "favourites": favs, "departures": deps })),
                );
            }
            let mut departures = departures;
            departures.truncate(limit as usize);
            return (
                StatusCode::OK,
                Json(serde_json::json!({ "departures": departures })),
            );
        }
    }

    println!("CACHE MISS {cache_key}");
    match fetch_departures(&state.http, &state.ojp_api_key, &station_id, fetch_limit).await {
        Ok(departures) => {
            if let Ok(json_str) = serde_json::to_string(&departures) {
                state.cache.put(&cache_key, &json_str, state.cache_ttl);
            }

            if has_favourites {
                let (favs, deps) = partition_favourites(&departures, &fav_pairs, limit as usize);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "favourites": favs, "departures": deps })),
                )
            } else {
                let mut departures = departures;
                departures.truncate(limit as usize);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "departures": departures })),
                )
            }
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        ),
    }
}
