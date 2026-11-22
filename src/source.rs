use traintime_core::ojp::FlatDeparture;
use traintime_core::region::{Region, Source};
use traintime_core::UpstreamError;

use crate::ojp::fetch_departures;
use crate::AppState;

/// Departures for a station from its region's source.
pub async fn fetch_departures_for(
    state: &AppState,
    region: &Region,
    stop_ref: &str,
    limit: u32,
) -> Result<Vec<FlatDeparture>, UpstreamError> {
    match region.source {
        Source::Ojp => fetch_departures(&state.ojp_endpoint, &state.ojp_api_key, stop_ref, limit).await,
    }
}
