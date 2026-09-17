use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;

use crate::fetch::Fetched;
use crate::ojp::fetch_departures;
use crate::routes::respond;
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
) -> Response {
    let station_id = match params.id {
        Some(id) => id,
        None => {
            return respond(
                StatusCode::BAD_REQUEST,
                serde_json::json!({ "error": "Missing id parameter" }),
                None,
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
    let fetched = state
        .inflight
        .cached(&state.cache, &cache_key, state.cache_ttl, || async {
            let deps = fetch_departures(&state.http, &state.ojp_api_key, &station_id, fetch_limit).await?;
            serde_json::to_string(&deps).map_err(|e| e.to_string())
        })
        .await;
    let (json, stale) = match fetched {
        Fetched::Fresh(v) => (v, None),
        Fetched::Stale(v, age) => (v, Some(age)),
        Fetched::Failed(e) => {
            return respond(StatusCode::INTERNAL_SERVER_ERROR, serde_json::json!({ "error": e }), None);
        }
    };
    let departures: Vec<FlatDeparture> = match serde_json::from_str(&json) {
        Ok(d) => d,
        Err(e) => {
            return respond(StatusCode::INTERNAL_SERVER_ERROR, serde_json::json!({ "error": e.to_string() }), None);
        }
    };
    if has_favourites {
        let (favs, deps) = partition_favourites(&departures, &fav_pairs, limit as usize);
        return respond(
            StatusCode::OK,
            serde_json::json!({ "favourites": favs, "departures": deps }),
            stale,
        );
    }
    let mut departures = departures;
    departures.truncate(limit as usize);
    respond(StatusCode::OK, serde_json::json!({ "departures": departures }), stale)
}
