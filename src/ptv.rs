use futures::future::join_all;
use wasm_bindgen::JsValue;
use worker::console_log;

use crate::upstream::{request_failed, timed};
use traintime_core::ojp::FlatDeparture;
use traintime_core::ptv::{credentials, departures_url, merge, parse_departures};
use traintime_core::UpstreamError;

/// Same parser as the OJP path, for the same reason.
fn iso_to_ms(iso: &str) -> f64 {
    js_sys::Date::new(&JsValue::from_str(iso)).get_time()
}

/// One call per (route type, stop) the station needs, asked at once and merged.
pub async fn fetch_departures(
    endpoint: &str,
    devid: Option<&str>,
    key: Option<&str>,
    asked: &[(u8, String)],
    limit: u32,
) -> Result<Vec<FlatDeparture>, UpstreamError> {
    let (devid, key) = credentials(devid.map(str::to_string), key.map(str::to_string))?;
    let calls = asked.iter().map(|(rt, stop_id)| {
        let url = departures_url(endpoint, *rt, stop_id, limit, &devid, &key);
        async move { fetch_one(&url, *rt, stop_id).await }
    });
    merge(join_all(calls).await, limit)
}

async fn fetch_one(url: &str, route_type: u8, stop_id: &str) -> Result<Vec<FlatDeparture>, UpstreamError> {
    let start = js_sys::Date::new_0().get_time();
    let headers = worker::Headers::new();
    headers.set("User-Agent", "traintime/1.0").map_err(request_failed)?;
    let mut init = worker::RequestInit::new();
    init.with_method(worker::Method::Get).with_headers(headers);
    let req = worker::Request::new_with_init(url, &init).map_err(request_failed)?;

    let mut resp = timed(worker::Fetch::Request(req).send()).await?;
    let status = resp.status_code();
    // The URL carries the devid and signature, so only the stop is logged.
    console_log!("PTV {}/{} {} {}ms", route_type, stop_id, status, (js_sys::Date::new_0().get_time() - start) as u64);
    UpstreamError::check_status(status)?;

    let body = resp.text().await.map_err(request_failed)?;
    parse_departures(&body, route_type, iso_to_ms)
}
