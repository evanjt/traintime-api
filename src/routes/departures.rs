use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use traintime_core::CacheStatus;
use worker::console_log;

use crate::cache::{self, Cached};
use crate::ojp::{fetch_departures, FlatDeparture};
use crate::routes::respond;
use crate::AppState;
use traintime_core::favourites::{parse_favourites, partition_favourites};

#[derive(Deserialize)]
pub struct DeparturesQuery {
    id: Option<String>,
    limit: Option<u32>,
    favourites: Option<String>,
}

#[worker::send]
pub async fn handle_departures(
    State(state): State<AppState>,
    Query(params): Query<DeparturesQuery>,
) -> Response {
    let station_id = match params.id {
        Some(id) => id,
        None => {
            return respond(
                StatusCode::BAD_REQUEST,
                serde_json::json!({ "error": "Missing id parameter" }), None, CacheStatus::None);
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
    let mut cache;
    let (json, stale) = match cache::get(&state.cache, &cache_key, state.cache_ttl).await {
        Cached::Fresh(v) => {
            console_log!("CACHE HIT {}", cache_key);
            cache = CacheStatus::Hit;
            (v, None)
        }
        cached => {
            console_log!("CACHE MISS {}", cache_key);
            cache = CacheStatus::Miss;
            match fetch_departures(&state.ojp_endpoint, &state.ojp_api_key, &station_id, fetch_limit).await {
                Ok(departures) => {
                    let json = serde_json::to_string(&departures).unwrap_or_default();
                    cache::put(&state.cache, &cache_key, &json, state.cache_ttl).await;
                    (json, None)
                }
                Err(e) => match cached {
                    Cached::Stale(v, age) => {
                        console_log!("STALE {} {}s: {}", cache_key, age, e);
                        cache = CacheStatus::Hit;
                        (v, Some(age))
                    }
                    _ => {
                        return respond(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            serde_json::json!({ "error": e.to_string() }), None, cache);
                    }
                },
            }
        }
    };
    let departures: Vec<FlatDeparture> = match serde_json::from_str(&json) {
        Ok(d) => d,
        Err(e) => {
            return respond(
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({ "error": e.to_string() }), None, cache);
        }
    };
    if has_favourites {
        let (favs, deps) = partition_favourites(&departures, &fav_pairs, limit as usize);
        return respond(
            StatusCode::OK,
            serde_json::json!({ "favourites": favs, "departures": deps }), stale, cache);
    }
    let mut departures = departures;
    departures.truncate(limit as usize);
    respond(StatusCode::OK, serde_json::json!({ "departures": departures }), stale, cache)
}
