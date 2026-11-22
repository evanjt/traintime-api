use wasm_bindgen::JsValue;
use worker::console_log;

use crate::upstream::{timed, request_failed};
use traintime_core::UpstreamError;

pub use traintime_core::ojp::FlatDeparture;
use traintime_core::ojp::{build_stop_event_request_xml, parse_stop_events};

/// Parse ISO timestamp string to unix milliseconds using js_sys::Date.
///
/// Deliberately not shared with the native server: the JS engine's parser is
/// more lenient than chrono's RFC3339, and swapping it would silently turn a
/// loosely-formatted OJP timestamp into a 1970 departure.
fn iso_to_ms(iso: &str) -> f64 {
    let date = js_sys::Date::new(&JsValue::from_str(iso));
    date.get_time()
}

pub async fn fetch_departures(
    endpoint: &str,
    api_key: &str,
    stop_ref: &str,
    limit: u32,
) -> Result<Vec<FlatDeparture>, UpstreamError> {
    let now = js_sys::Date::new_0();
    let body = build_stop_event_request_xml(
        stop_ref,
        limit,
        &String::from(now.to_iso_string()),
        now.get_time() as u64,
    );

    let start = js_sys::Date::new_0().get_time();

    let headers = worker::Headers::new();
    headers.set("Content-Type", "application/xml").map_err(request_failed)?;
    headers.set("Authorization", &format!("Bearer {api_key}")).map_err(request_failed)?;
    headers.set("User-Agent", "traintime/1.0").map_err(request_failed)?;

    let mut init = worker::RequestInit::new();
    init.with_method(worker::Method::Post)
        .with_headers(headers)
        .with_body(Some(JsValue::from_str(&body)));

    let req = worker::Request::new_with_init(endpoint, &init).map_err(request_failed)?;

    let mut resp = timed(worker::Fetch::Request(req).send()).await?;
    let elapsed = js_sys::Date::new_0().get_time() - start;
    let status = resp.status_code();
    console_log!("OJP {} {} {}ms", stop_ref, status, elapsed as u64);

    UpstreamError::check_status(status)?;

    let xml = resp.text().await.map_err(request_failed)?;
    Ok(parse_stop_events(&xml, iso_to_ms))
}
