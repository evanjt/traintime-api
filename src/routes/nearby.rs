use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use wasm_bindgen::JsValue;
use worker::console_log;

use crate::ojp::fetch_departures;
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

    let (lat_min, lat_max, lon_min, lon_max) = bounding_box(lat, lon);

    let bound = if let Some(ref q) = query_lower {
        let pattern = format!("%{q}%");
        state.db
            .prepare("SELECT id, name, lat, lon, mode FROM stations WHERE lat BETWEEN ?1 AND ?2 AND lon BETWEEN ?3 AND ?4 AND LOWER(name) LIKE LOWER(?5)")
            .bind(&[JsValue::from_f64(lat_min), JsValue::from_f64(lat_max), JsValue::from_f64(lon_min), JsValue::from_f64(lon_max), JsValue::from_str(&pattern)])
    } else {
        state.db
            .prepare("SELECT id, name, lat, lon, mode FROM stations WHERE lat BETWEEN ?1 AND ?2 AND lon BETWEEN ?3 AND ?4")
            .bind(&[JsValue::from_f64(lat_min), JsValue::from_f64(lat_max), JsValue::from_f64(lon_min), JsValue::from_f64(lon_max)])
    };

    let stmt = match bound {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            );
        }
    };

    let rows: Vec<Station> = match stmt.all().await {
        Ok(r) => match r.results::<Station>() {
            Ok(rows) => rows,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                );
            }
        },
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            );
        }
    };

    let mut groups = group_nearby(rows.iter(), lat, lon);

    // Fetch departures for the default station — prioritize requested mode, then fall back
    // Fetch 50 to share cache key with /v1/departures, but only embed first 20 in response
    let fetch_limit = 50u32;
    let embed_limit = 20usize;
    let default_id = default_station_id(&groups, requested_mode.as_deref());

    if let Some(id) = default_id {
        let cache_key = format!("departures:{id}:{fetch_limit}");
        let mut deps = None;

        if let Ok(Some(cached)) = state.cache.get(&cache_key).text().await {
            console_log!("CACHE HIT departures:{}:{}", id, fetch_limit);
            deps = serde_json::from_str(&cached).ok();
        }

        if deps.is_none() {
            console_log!("CACHE MISS departures:{}:{}", id, fetch_limit);
            if let Ok(fetched) = fetch_departures(&state.ojp_api_key, &id, fetch_limit).await {
                if let Ok(json_str) = serde_json::to_string(&fetched) {
                    let _ = state
                        .cache
                        .put(&cache_key, &json_str)
                        .unwrap()
                        .expiration_ttl(state.cache_ttl)
                        .execute()
                        .await;
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
