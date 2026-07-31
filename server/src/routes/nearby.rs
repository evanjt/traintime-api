use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;

use crate::ojp::fetch_departures;
use crate::state::AppState;
use traintime_core::stations::{default_station_id, group_nearby};

#[derive(Deserialize)]
pub struct NearbyQuery {
    lat: Option<f64>,
    lon: Option<f64>,
    query: Option<String>,
    mode: Option<String>,
}

pub async fn handle_nearby(
    State(state): State<AppState>,
    Query(params): Query<NearbyQuery>,
) -> impl IntoResponse {
    let (lat, lon) = match (params.lat, params.lon) {
        (Some(lat), Some(lon)) if lat.is_finite() && lon.is_finite() => (lat, lon),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Missing or invalid lat/lon parameters" })),
            );
        }
    };

    let query_lower = params.query.map(|q| q.to_lowercase());
    let requested_mode = params.mode;

    let rows = state.db.candidates(lat, lon, query_lower.as_deref());
    let mut groups = group_nearby(rows.into_iter(), lat, lon);

    // Fetch departures for the default station — prioritize requested mode, then fall back
    // Fetch 50 to share cache key with /v1/departures, but only embed first 20 in response
    let fetch_limit = 50u32;
    let embed_limit = 20usize;
    let default_id = default_station_id(&groups, requested_mode.as_deref());

    if let Some(id) = default_id {
        let cache_key = format!("departures:{id}:{fetch_limit}");
        let mut deps = None;

        if let Some(cached) = state.cache.get(&cache_key) {
            println!("CACHE HIT {cache_key}");
            deps = serde_json::from_str(&cached).ok();
        }

        if deps.is_none() {
            println!("CACHE MISS {cache_key}");
            if let Ok(fetched) =
                fetch_departures(&state.http, &state.ojp_api_key, &id, fetch_limit).await
            {
                if let Ok(json_str) = serde_json::to_string(&fetched) {
                    state.cache.put(&cache_key, &json_str, state.cache_ttl);
                }
                deps = Some(fetched);
            }
        }

        if let Some(mut d) = deps {
            d.truncate(embed_limit);
            groups.attach_departures(&id, d);
        }
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "train": groups.train,
            "bus": groups.bus,
            "tram": groups.tram,
            "special": groups.special,
        })),
    )
}
