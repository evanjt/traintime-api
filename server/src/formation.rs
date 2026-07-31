use traintime_core::formation::{parse_formation_response, FormationResult, FORMATION_ENDPOINT};

pub async fn fetch_formation(
    http: &reqwest::Client,
    api_key: &str,
    evu: &str,
    date: &str,
    train_number: &str,
    stop_uic: Option<&str>,
) -> Result<FormationResult, String> {
    let url =
        format!("{FORMATION_ENDPOINT}?evu={evu}&operationDate={date}&trainNumber={train_number}");

    let start = std::time::Instant::now();

    let resp = http
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status = resp.status().as_u16();
    println!(
        "FORMATION {} {} evu={} {}ms",
        train_number,
        status,
        evu,
        start.elapsed().as_millis()
    );

    if status != 200 {
        return Err(format!("Formation API error: {status}"));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    parse_formation_response(&body, stop_uic).map_err(|e| e.to_string())
}
