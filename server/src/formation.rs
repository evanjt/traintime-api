use traintime_core::formation::{
    parse_formation_response, FormationError, FormationResult,
};
use traintime_core::UpstreamError;

use crate::ojp::request_error;

pub async fn fetch_formation(
    http: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    evu: &str,
    date: &str,
    train_number: &str,
    stop_uic: Option<&str>,
) -> Result<FormationResult, FormationError> {
    let url =
        format!("{endpoint}?evu={evu}&operationDate={date}&trainNumber={train_number}");

    let start = std::time::Instant::now();

    let resp = http
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(request_error)?;

    let status = resp.status().as_u16();
    println!(
        "FORMATION {} {} evu={} {}ms",
        train_number,
        status,
        evu,
        start.elapsed().as_millis()
    );

    FormationError::check_status(status)?;

    let text = resp.text().await.map_err(request_error)?;
    let body: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| UpstreamError::Unparseable(e.to_string()))?;

    Ok(parse_formation_response(&body, stop_uic)?)
}
