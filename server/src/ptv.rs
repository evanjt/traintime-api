use traintime_core::ojp::FlatDeparture;
use traintime_core::ptv::{credentials, departures_url, merge, parse_departures};
use traintime_core::UpstreamError;

use crate::ojp::{iso_to_ms, request_error};

/// One call per (route type, stop) the station needs, merged into one board.
pub async fn fetch_departures(
    http: &reqwest::Client,
    endpoint: &str,
    devid: Option<&str>,
    key: Option<&str>,
    asked: &[(u8, String)],
    limit: u32,
) -> Result<Vec<FlatDeparture>, UpstreamError> {
    let (devid, key) = credentials(devid.map(str::to_string), key.map(str::to_string))?;
    // Every call is made at once so a station costs one round trip.
    let mut calls = tokio::task::JoinSet::new();
    for (i, (rt, stop_id)) in asked.iter().cloned().enumerate() {
        let url = departures_url(endpoint, rt, &stop_id, limit, &devid, &key);
        let http = http.clone();
        calls.spawn(async move { (i, fetch_one(&http, &url, rt, &stop_id).await) });
    }
    let mut answers = Vec::with_capacity(asked.len());
    while let Some(joined) = calls.join_next().await {
        match joined {
            Ok(answer) => answers.push(answer),
            Err(e) => answers.push((usize::MAX, Err(UpstreamError::Unreachable(e.to_string())))),
        }
    }
    answers.sort_by_key(|(i, _)| *i);
    merge(answers.into_iter().map(|(_, a)| a).collect(), limit)
}

async fn fetch_one(
    http: &reqwest::Client,
    url: &str,
    route_type: u8,
    stop_id: &str,
) -> Result<Vec<FlatDeparture>, UpstreamError> {
    let start = std::time::Instant::now();
    let resp = http.get(url).send().await.map_err(request_error)?;
    let status = resp.status().as_u16();
    // The URL carries the devid and signature, so only the stop is logged.
    println!("PTV {route_type}/{stop_id} {status} {}ms", start.elapsed().as_millis());
    UpstreamError::check_status(status)?;
    let body = resp.text().await.map_err(request_error)?;
    parse_departures(&body, route_type, iso_to_ms)
}
