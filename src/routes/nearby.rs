use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use traintime_core::CacheStatus;
use wasm_bindgen::JsValue;
use worker::console_log;

use crate::cache::{self, Cached};
use crate::ojp::{fetch_departures, FlatDeparture};
use crate::routes::respond;
use crate::AppState;
use traintime_core::geo::bounding_box;
use traintime_core::stations::{default_station_id, group_nearby, Station};

#[derive(Deserialize)]
pub struct NearbyQuery {
    lat: Option<f64>,
    lon: Option<f64>,
    query: Option<String>,
    mode: Option<String>,
}

#[worker::send]
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

    let (lat_min, lat_max, lon_min, lon_max) = bounding_box(lat, lon);

    // SQLite's LOWER and LIKE are ASCII only, so a query like "lö" would miss
    // "Kirchhausen (Kr LÖ)". Fetch by bounding box and filter in Rust, as the
    // native server does.
    let bound = state.db
        .prepare("SELECT id, name, lat, lon, mode FROM stations WHERE lat BETWEEN ?1 AND ?2 AND lon BETWEEN ?3 AND ?4")
        .bind(&[JsValue::from_f64(lat_min), JsValue::from_f64(lat_max), JsValue::from_f64(lon_min), JsValue::from_f64(lon_max)]);

    let stmt = match bound {
        Ok(s) => s,
        Err(e) => {
            return respond(
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({ "error": e.to_string() }), None, CacheStatus::None);
        }
    };

    let rows: Vec<Station> = match stmt.all().await {
        Ok(r) => match r.results::<Station>() {
            Ok(rows) => match query_lower {
                Some(q) => rows
                    .into_iter()
                    .filter(|s| s.name.to_lowercase().contains(&q))
                    .collect(),
                None => rows,
            },
            Err(e) => {
                return respond(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({ "error": e.to_string() }), None, CacheStatus::None);
            }
        },
        Err(e) => {
            return respond(
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({ "error": e.to_string() }), None, CacheStatus::None);
        }
    };

    let mut groups = group_nearby(rows.iter(), lat, lon);

    // Fetch departures for the default station — prioritize requested mode, then fall back
    // Fetch 50 to share cache key with /v1/departures, but only embed first 20 in response
    let fetch_limit = 50u32;
    let embed_limit = 20usize;
    let default_id = default_station_id(&groups, requested_mode.as_deref());

    let mut stale = None;
    let mut cache = CacheStatus::None;
    if let Some(id) = default_id {
        let cache_key = format!("departures:{id}:{fetch_limit}");
        let json = match cache::get(&state.cache, &cache_key, state.cache_ttl).await {
            Cached::Fresh(v) => {
                console_log!("CACHE HIT {}", cache_key);
                cache = CacheStatus::Hit;
                Some(v)
            }
            cached => {
                console_log!("CACHE MISS {}", cache_key);
                cache = CacheStatus::Miss;
                match fetch_departures(&state.ojp_api_key, &id, fetch_limit).await {
                    Ok(fetched) => {
                        let json = serde_json::to_string(&fetched).unwrap_or_default();
                        cache::put(&state.cache, &cache_key, &json, state.cache_ttl).await;
                        Some(json)
                    }
                    Err(e) => match cached {
                        Cached::Stale(v, age) => {
                            console_log!("STALE {} {}s: {}", cache_key, age, e);
                            cache = CacheStatus::Hit;
                            stale = Some(age);
                            Some(v)
                        }
                        _ => None,
                    },
                }
            }
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
