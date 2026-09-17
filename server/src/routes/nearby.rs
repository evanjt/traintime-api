use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use traintime_core::CacheStatus;

use crate::fetch::Fetched;
use crate::ojp::fetch_departures;
use crate::routes::respond;
use crate::state::AppState;
use traintime_core::ojp::FlatDeparture;
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
) -> Response {
    let (lat, lon) = match (params.lat, params.lon) {
        (Some(lat), Some(lon)) if lat.is_finite() && lon.is_finite() => (lat, lon),
        _ => {
            return respond(
                StatusCode::BAD_REQUEST,
                serde_json::json!({ "error": "Missing or invalid lat/lon parameters" }), None, CacheStatus::None);
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

    let mut stale = None;
    let mut cache = CacheStatus::None;
    if let Some(id) = default_id {
        let cache_key = format!("departures:{id}:{fetch_limit}");
        let fetched = state
            .inflight
            .cached(&state.cache, &cache_key, state.cache_ttl, || async {
                let deps = fetch_departures(&state.http, &state.ojp_api_key, &id, fetch_limit).await?;
                serde_json::to_string(&deps).map_err(|e| e.to_string())
            })
            .await;
        cache = fetched.cache_status();
        let json = match fetched {
            Fetched::Fresh(v) | Fetched::Cached(v) => Some(v),
            Fetched::Stale(v, age) => {
                stale = Some(age);
                Some(v)
            }
            Fetched::Failed(_) => None,
        };
        if let Some(mut d) = json.and_then(|j| serde_json::from_str::<Vec<FlatDeparture>>(&j).ok()) {
            d.truncate(embed_limit);
            groups.attach_departures(&id, d);
        }
    }

    respond(
        StatusCode::OK,
        serde_json::json!({
            "train": groups.train,
            "bus": groups.bus,
            "tram": groups.tram,
            "special": groups.special,
        }), stale, cache)
}
