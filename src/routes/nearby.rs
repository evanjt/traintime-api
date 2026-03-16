use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsValue;
use worker::console_log;

use crate::ojp::{fetch_departures, FlatDeparture};
use crate::AppState;

const MAX_PER_MODE: usize = 5;
const MAX_DISTANCE: f64 = 5000.0;

#[derive(Deserialize)]
pub struct NearbyQuery {
    lat: Option<f64>,
    lon: Option<f64>,
    query: Option<String>,
}

#[derive(Serialize)]
struct NearbyStation {
    id: String,
    name: String,
    dist: i64,
    lat: f64,
    lon: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    departures: Option<Vec<FlatDeparture>>,
}

#[derive(Deserialize)]
struct StationRow {
    id: String,
    name: String,
    lat: f64,
    lon: f64,
    mode: String,
}

fn haversine_distance(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R: f64 = 6371000.0;
    let to_rad = |deg: f64| deg * std::f64::consts::PI / 180.0;
    let d_lat = to_rad(lat2 - lat1);
    let d_lon = to_rad(lon2 - lon1);
    let a = (d_lat / 2.0).sin().powi(2)
        + to_rad(lat1).cos() * to_rad(lat2).cos() * (d_lon / 2.0).sin().powi(2);
    R * 2.0 * a.sqrt().atan2((1.0 - a).sqrt())
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

    // ~5km bounding box in degrees
    let dlat = MAX_DISTANCE / 111000.0;
    let dlon = MAX_DISTANCE / 75700.0;

    let lat_min = lat - dlat;
    let lat_max = lat + dlat;
    let lon_min = lon - dlon;
    let lon_max = lon + dlon;

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

    let rows: Vec<StationRow> = match stmt.all().await {
        Ok(r) => match r.results::<StationRow>() {
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

    let mut train: Vec<(String, String, i64, f64, f64)> = Vec::new();
    let mut bus: Vec<(String, String, i64, f64, f64)> = Vec::new();
    let mut tram: Vec<(String, String, i64, f64, f64)> = Vec::new();
    let mut special: Vec<(String, String, i64, f64, f64)> = Vec::new();

    for row in rows {
        let dist = haversine_distance(lat, lon, row.lat, row.lon);
        if dist > MAX_DISTANCE {
            continue;
        }

        let entry = (row.id, row.name, dist.round() as i64, row.lat, row.lon);

        match row.mode.as_str() {
            "bus" => bus.push(entry),
            "tram" => tram.push(entry),
            "special" => special.push(entry),
            _ => train.push(entry),
        }
    }

    // Sort by distance and limit
    for group in [&mut train, &mut bus, &mut tram, &mut special] {
        group.sort_by_key(|e| e.2);
        group.truncate(MAX_PER_MODE);
    }

    // Fetch departures for only the default station (first available mode's closest)
    let dep_limit = 10u32;
    let default_id = [&train, &bus, &tram, &special]
        .iter()
        .find_map(|g| g.first().map(|s| s.0.clone()));

    let mut departure_map: std::collections::HashMap<String, Vec<FlatDeparture>> =
        std::collections::HashMap::new();

    if let Some(id) = default_id {
        let cache_key = format!("departures:{id}:{dep_limit}");
        let mut deps: Option<Vec<FlatDeparture>> = None;

        if let Ok(Some(cached)) = state.cache.get(&cache_key).text().await {
            console_log!("CACHE HIT departures:{}:{}", id, dep_limit);
            deps = serde_json::from_str(&cached).ok();
        }

        if deps.is_none() {
            console_log!("CACHE MISS departures:{}:{}", id, dep_limit);
            if let Ok(fetched) = fetch_departures(&state.ojp_api_key, &id, dep_limit).await {
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

        if let Some(d) = deps {
            departure_map.insert(id, d);
        }
    }

    let to_response = |list: &[(String, String, i64, f64, f64)]| -> Vec<NearbyStation> {
        list.iter()
            .map(|(id, name, dist, lat, lon)| NearbyStation {
                id: id.clone(),
                name: name.clone(),
                dist: *dist,
                lat: *lat,
                lon: *lon,
                departures: departure_map.get(id).cloned(),
            })
            .collect()
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "train": to_response(&train),
            "bus": to_response(&bus),
            "tram": to_response(&tram),
            "special": to_response(&special),
        })),
    )
}
