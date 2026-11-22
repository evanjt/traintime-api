use serde::Deserialize;
use wasm_bindgen::JsValue;

use traintime_core::ojp::FlatDeparture;
use traintime_core::ptv::requests;
use traintime_core::region::{Region, Source};
use traintime_core::UpstreamError;

use crate::AppState;

/// Departures for a station from its region's source.
pub async fn fetch_departures_for(
    state: &AppState,
    region: &Region,
    stop_ref: &str,
    limit: u32,
) -> Result<Vec<FlatDeparture>, UpstreamError> {
    match region.source {
        Source::Ojp => crate::ojp::fetch_departures(&state.ojp_endpoint, &state.ojp_api_key, stop_ref, limit).await,
        Source::Ptv => {
            let local_id = region.local_id(stop_ref);
            let asked = match station_row(state, stop_ref).await {
                Some(row) => requests(local_id, &row.mode, row.route_types.as_deref(), row.stop_ids.as_deref()),
                None => requests(local_id, "train", None, None),
            };
            crate::ptv::fetch_departures(
                &state.ptv_endpoint,
                state.ptv_devid.as_deref(),
                state.ptv_key.as_deref(),
                &asked,
                limit,
            )
            .await
        }
    }
}

#[derive(Deserialize)]
struct StationRow {
    mode: String,
    route_types: Option<String>,
    stop_ids: Option<String>,
}

/// The columns of a station's row that decide what its source is asked.
async fn station_row(state: &AppState, id: &str) -> Option<StationRow> {
    let stmt = state
        .db
        .prepare("SELECT mode, route_types, stop_ids FROM stations WHERE id = ?1 LIMIT 1")
        .ok()?
        .bind(&[JsValue::from_str(id)])
        .ok()?;
    stmt.first::<StationRow>(None).await.ok().flatten()
}
