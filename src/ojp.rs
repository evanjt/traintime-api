use wasm_bindgen::JsValue;
use worker::console_log;

pub use traintime_core::ojp::FlatDeparture;
use traintime_core::ojp::{build_stop_event_request_xml, parse_stop_events, OJP_ENDPOINT};

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
    api_key: &str,
    stop_ref: &str,
    limit: u32,
) -> worker::Result<Vec<FlatDeparture>> {
    let now = js_sys::Date::new_0();
    let body = build_stop_event_request_xml(
        stop_ref,
        limit,
        &String::from(now.to_iso_string()),
        now.get_time() as u64,
    );

    let start = js_sys::Date::new_0().get_time();

    let headers = worker::Headers::new();
    headers.set("Content-Type", "application/xml")?;
    headers.set("Authorization", &format!("Bearer {api_key}"))?;
    headers.set("User-Agent", "traintime/1.0")?;

    let mut init = worker::RequestInit::new();
    init.with_method(worker::Method::Post)
        .with_headers(headers)
        .with_body(Some(JsValue::from_str(&body)));

    let req = worker::Request::new_with_init(OJP_ENDPOINT, &init)?;

    let mut resp = worker::Fetch::Request(req).send().await?;
    let elapsed = js_sys::Date::new_0().get_time() - start;
    let status = resp.status_code();
    console_log!("OJP {} {} {}ms", stop_ref, status, elapsed as u64);

    if status != 200 {
        return Err(worker::Error::RustError(format!(
            "OJP API error: {status}"
        )));
    }

    let xml = resp.text().await?;
    Ok(parse_stop_events(&xml, iso_to_ms))
}
