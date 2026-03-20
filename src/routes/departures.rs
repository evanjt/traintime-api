use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use worker::console_log;

use crate::ojp::{fetch_departures, FlatDeparture};
use crate::AppState;

#[derive(Deserialize)]
pub struct DeparturesQuery {
    id: Option<String>,
    limit: Option<u32>,
    favourites: Option<String>,
}

/// Parse "IC8:Brig,IR90:Visp" into vec of (lineNumber, destination)
pub fn parse_favourites(raw: &str) -> Vec<(String, String)> {
    raw.split(',')
        .filter_map(|pair| {
            let pair = pair.trim();
            let colon = pair.find(':')?;
            let line = pair[..colon].to_string();
            let dest = pair[colon + 1..].to_string();
            if line.is_empty() || dest.is_empty() {
                None
            } else {
                Some((line, dest))
            }
        })
        .collect()
}

/// Partition departures: clone first match per favourite pair, return (favourites, remaining)
/// Favourites are cloned (not removed) so they can still appear in the regular list.
/// Favourites are sorted by departure time.
pub fn partition_favourites(
    departures: &[FlatDeparture],
    fav_pairs: &[(String, String)],
    limit: usize,
) -> (Vec<FlatDeparture>, Vec<FlatDeparture>) {
    let mut favourites = Vec::new();

    for (line, dest) in fav_pairs {
        if let Some(d) = departures.iter().find(|d| d.number == *line && d.to == *dest) {
            favourites.push(d.clone());
        }
    }

    favourites.sort_by_key(|d| d.departure);

    let remaining = departures.iter().take(limit).cloned().collect();
    (favourites, remaining)
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
    let fav_pairs = params
        .favourites
        .as_deref()
        .map(parse_favourites)
        .unwrap_or_default();
    let has_favourites = !fav_pairs.is_empty();

    // Always fetch 50 to populate cache; partition down to `limit` on return
    let fetch_limit = 50u32;
    let cache_key = format!("departures:{station_id}:{fetch_limit}");

    // Check cache
    if let Ok(Some(cached)) = state.cache.get(&cache_key).text().await {
        console_log!("CACHE HIT {}", cache_key);
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

    console_log!("CACHE MISS {}", cache_key);
    match fetch_departures(&state.ojp_api_key, &station_id, fetch_limit).await {
        Ok(departures) => {
            // Cache the full result
            if let Ok(json_str) = serde_json::to_string(&departures) {
                let _ = state
                    .cache
                    .put(&cache_key, &json_str)
                    .unwrap()
                    .expiration_ttl(state.cache_ttl)
                    .execute()
                    .await;
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
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}
