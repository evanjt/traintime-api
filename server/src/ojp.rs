use traintime_core::ojp::{build_stop_event_request_xml, parse_stop_events, FlatDeparture};

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
    endpoint: &str,
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
        .post(endpoint)
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::post;
    use axum::Router;

    const STOP_EVENT_XML: &str = r#"<OJP><StopEventResult><StopEvent>
      <ThisCall><CallAtStop>
        <ServiceDeparture><TimetabledTime>2026-09-17T10:00:00Z</TimetabledTime></ServiceDeparture>
        <PlannedQuay><Text>7</Text></PlannedQuay>
      </CallAtStop></ThisCall>
      <Service>
        <Mode><ShortName><Text>IC</Text></ShortName></Mode>
        <PublishedServiceName><Text>IC1</Text></PublishedServiceName>
        <DestinationText><Text>Bern</Text></DestinationText>
      </Service>
    </StopEvent></StopEventResult></OJP>"#;

    // Scenario: a load test points OJP_ENDPOINT at a stub so SBB's per-key
    // limit is never touched. Expected behaviour: the server posts to the
    // override, not the built-in endpoint, and parses what it gets back.
    #[tokio::test]
    async fn fetch_departures_posts_to_the_endpoint_override() {
        let app = Router::new().route("/ojp20", post(|| async { STOP_EVENT_XML }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let http = reqwest::Client::new();
        let endpoint = format!("http://{addr}/ojp20");
        let deps = fetch_departures(&http, &endpoint, "key", "8507000", 5)
            .await
            .unwrap();

        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].to, "Bern");
        assert_eq!(deps[0].number, "IC1");
    }
}
