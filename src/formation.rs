use worker::console_log;

pub use traintime_core::formation::FormationResult;
use traintime_core::formation::{parse_formation_response, FORMATION_ENDPOINT};

pub async fn fetch_formation(
    api_key: &str,
    evu: &str,
    date: &str,
    train_number: &str,
    stop_uic: Option<&str>,
) -> worker::Result<FormationResult> {
    let url = format!(
        "{FORMATION_ENDPOINT}?evu={evu}&operationDate={date}&trainNumber={train_number}"
    );

    let start = js_sys::Date::new_0().get_time();

    let headers = worker::Headers::new();
    headers.set("Authorization", &format!("Bearer {api_key}"))?;
    headers.set("Accept", "application/json")?;
    headers.set("User-Agent", "traintime/1.0")?;

    let mut init = worker::RequestInit::new();
    init.with_method(worker::Method::Get).with_headers(headers);

    let req = worker::Request::new_with_init(&url, &init)?;
    let mut resp = worker::Fetch::Request(req).send().await?;

    let elapsed = js_sys::Date::new_0().get_time() - start;
    let status = resp.status_code();
    console_log!(
        "FORMATION {} {} evu={} {}ms",
        train_number,
        status,
        evu,
        elapsed as u64
    );

    if status != 200 {
        return Err(worker::Error::RustError(format!(
            "Formation API error: {status}"
        )));
    }

    let body: serde_json::Value = resp.json().await?;

    parse_formation_response(&body, stop_uic).map_err(|e| worker::Error::RustError(e.to_string()))
}
