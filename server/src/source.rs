use traintime_core::ojp::FlatDeparture;
use traintime_core::ptv::requests;
use traintime_core::region::{Region, Source};
use traintime_core::UpstreamError;

use crate::state::AppState;

/// Departures for a station from its region's source.
pub async fn fetch_departures_for(
    state: &AppState,
    region: &Region,
    stop_ref: &str,
    limit: u32,
) -> Result<Vec<FlatDeparture>, UpstreamError> {
    match region.source {
        Source::Ojp => {
            crate::ojp::fetch_departures(&state.http, &state.ojp_endpoint, &state.ojp_api_key, stop_ref, limit)
                .await
        }
        Source::Ptv => {
            let local_id = region.local_id(stop_ref);
            let asked = match state.db.station(stop_ref) {
                Some(s) => requests(local_id, &s.mode, s.route_types.as_deref(), s.stop_ids.as_deref()),
                None => requests(local_id, "train", None, None),
            };
            crate::ptv::fetch_departures(
                &state.http,
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
