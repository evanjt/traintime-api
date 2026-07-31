use traintime_core::ojp::{
    build_stop_event_request_xml, parse_stop_events, FlatDeparture, OJP_ENDPOINT,
};

/// Parse ISO timestamp string to unix milliseconds.
///
/// The Worker uses `js_sys::Date` here. The two are kept separate on purpose:
/// the JS parser is the more lenient of the pair, so they must not be merged
/// without checking real OJP payloads first.
fn iso_to_ms(iso: &str) -> f64 {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(iso) {
        return dt.timestamp_millis() as f64;
    }
    // OJP occasionally omits the zone designator; treat as UTC, matching what
    // the Workers runtime did.
    chrono::NaiveDateTime::parse_from_str(iso, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(iso, "%Y-%m-%dT%H:%M:%S"))
        .map(|n| n.and_utc().timestamp_millis() as f64)
        .unwrap_or(0.0)
}

pub async fn fetch_departures(
    http: &reqwest::Client,
    api_key: &str,
    stop_ref: &str,
    limit: u32,
) -> Result<Vec<FlatDeparture>, String> {
    let now = chrono::Utc::now();
    let body = build_stop_event_request_xml(
        stop_ref,
        limit,
        &now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        now.timestamp_millis() as u64,
    );

    let start = std::time::Instant::now();

    let resp = http
        .post(OJP_ENDPOINT)
        .header("Content-Type", "application/xml")
        .header("Authorization", format!("Bearer {api_key}"))
        .body(body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status = resp.status().as_u16();
    println!(
        "OJP {} {} {}ms",
        stop_ref,
        status,
        start.elapsed().as_millis()
    );

    if status != 200 {
        return Err(format!("OJP API error: {status}"));
    }

    let xml = resp.text().await.map_err(|e| e.to_string())?;
    Ok(parse_stop_events(&xml, iso_to_ms))
}
