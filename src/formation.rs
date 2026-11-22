use worker::console_log;

use crate::upstream::{timed, request_failed};
use traintime_core::UpstreamError;

pub use traintime_core::formation::FormationResult;
use traintime_core::formation::{parse_formation_response, FormationError, FORMATION_ENDPOINT};

pub async fn fetch_formation(
    api_key: &str,
    evu: &str,
    date: &str,
    train_number: &str,
    stop_uic: Option<&str>,
) -> Result<FormationResult, FormationError> {
    let url = format!(
        "{FORMATION_ENDPOINT}?evu={evu}&operationDate={date}&trainNumber={train_number}"
    );

    let start = js_sys::Date::new_0().get_time();

    let headers = worker::Headers::new();
    headers.set("Authorization", &format!("Bearer {api_key}")).map_err(request_failed)?;
    headers.set("Accept", "application/json").map_err(request_failed)?;
    headers.set("User-Agent", "traintime/1.0").map_err(request_failed)?;

    let mut init = worker::RequestInit::new();
    init.with_method(worker::Method::Get).with_headers(headers);

    let req = worker::Request::new_with_init(&url, &init).map_err(request_failed)?;
    let mut resp = timed(worker::Fetch::Request(req).send()).await?;

    let elapsed = js_sys::Date::new_0().get_time() - start;
    let status = resp.status_code();
    console_log!(
        "FORMATION {} {} evu={} {}ms",
        train_number,
        status,
        evu,
        elapsed as u64
    );

    FormationError::check_status(status)?;

    let text = resp.text().await.map_err(request_failed)?;
    let body: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| UpstreamError::Unparseable(e.to_string()))?;

    Ok(parse_formation_response(&body, stop_uic)?)
}
