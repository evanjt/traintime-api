use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use traintime_core::CacheStatus;
use worker::console_log;

use crate::cache::{self, Cached};
use crate::ojp::unix_now;
use traintime_core::ojp::{drop_departed, FlatDeparture};
use crate::source::fetch_departures_for;
use crate::routes::{respond, respond_error};
use crate::AppState;
use traintime_core::region::region_for_station_id;
use traintime_core::favourites::parse_favourites;
use traintime_core::{departures_body, departures_on_failure, stale_opt_in};

#[derive(Deserialize)]
pub struct DeparturesQuery {
    id: Option<String>,
    limit: Option<u32>,
    favourites: Option<String>,
    /// `1` opts in to a list older than the short window, marked with its age.
    stale: Option<String>,
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
    let opt_in = stale_opt_in(params.stale.as_deref());
    let fav_pairs = params
        .favourites
        .as_deref()
        .map(parse_favourites)
        .unwrap_or_default();

    // Always fetch 50 to populate cache; partition down to `limit` on return
    let fetch_limit = 50u32;
    let region = region_for_station_id(&station_id);
    let cache_key = format!("departures:{station_id}:{fetch_limit}");
    let mut cache;
    let (departures, fetched_at) = match cache::get(&cache_key, state.cache_ttl).await {
        Cached::Fresh(v) => {
            console_log!("CACHE HIT {}", cache_key);
            cache = CacheStatus::Hit;
            (parse(&v, cache), None)
        }
        cached => {
            console_log!("CACHE MISS {}", cache_key);
            cache = CacheStatus::Miss;
            match fetch_departures_for(&state, region, &station_id, fetch_limit).await {
                Ok(departures) => {
                    let json = serde_json::to_string(&departures).unwrap_or_default();
                    cache::put(&cache_key, &json, state.cache_ttl).await;
                    (Ok(departures), None)
                }
                Err(e) => {
                    let now_secs = (cache::now_ms() / 1000) as i64;
                    let stale = match &cached {
                        Cached::Stale(v, age) => departures_on_failure(v, *age, now_secs, opt_in).map(|s| (s, *age)),
                        _ => None,
                    };
                    match stale {
                        Some((s, age)) => {
                            console_log!("STALE {} {}s: {}", cache_key, age, e);
                            cache = CacheStatus::Hit;
                            (Ok(s.departures), Some((s.fetched_at, age)))
                        }
                        None => {
                            console_log!("Departures error: {}", e);
                            return respond_error(e.response(), cache);
                        }
                    }
                }
            }
        }
    };
    let departures = match departures {
        Ok(d) => drop_departed(d, unix_now()),
        Err(resp) => return resp,
    };
    let body = departures_body(departures, &fav_pairs, limit as usize, fetched_at.and_then(|(at, _)| at));
    respond(StatusCode::OK, body, fetched_at.map(|(_, age)| age), cache)
}

fn parse(json: &str, cache: CacheStatus) -> Result<Vec<FlatDeparture>, Response> {
    serde_json::from_str(json).map_err(|e| {
        respond(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }), None, cache)
    })
}
