use serde::Serialize;
use worker::console_log;

const FORMATION_ENDPOINT: &str =
    "https://api.opentransportdata.swiss/formation/v1/formations_stop_based";

#[derive(Debug, Clone, Serialize)]
pub struct Wagon {
    pub position: usize,
    pub number: u32,
    pub class: u8,
    pub sector: String,
    pub features: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub closed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FormationResult {
    pub track: String,
    pub sectors: Vec<String>,
    pub wagons: Vec<Wagon>,
}

/// Parse a formationShortString into sectors and wagons.
///
/// Format: `@SECTOR` marks sector boundaries, comma-separated vehicles.
/// Each vehicle is `CLASS:NUMBER` optionally followed by `#FEATURE` flags.
/// Features: VR = restaurant, BHP = wheelchair accessible, W = wheelchair space.
/// Vehicles in `[(...):N]` are a train unit group.
/// `F` means a non-passenger vehicle (skip it).
/// `%` prefix means closed/unavailable (include but mark).
pub fn parse_formation_short_string(short: &str) -> (Vec<String>, Vec<Wagon>) {
    let mut sectors: Vec<String> = Vec::new();
    let mut wagons: Vec<Wagon> = Vec::new();
    let mut current_sector = String::new();
    let mut position: usize = 0;

    // Remove train-unit group brackets and trailing group IDs like ):3
    let cleaned = short
        .replace('[', "")
        .replace('(', "");
    // Remove "):N" patterns (train unit group IDs)
    let re = regex::Regex::new(r"\):\d+").unwrap();
    let cleaned = re.replace_all(&cleaned, "").to_string();
    let cleaned = cleaned.replace(')', "");

    for token in cleaned.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }

        // A token may start with @SECTOR — extract it first
        // e.g. "@D" or just "2:9#VH;FZ" or "2:5#VH@B" (sector change after features)
        let token = if let Some(rest) = token.strip_prefix('@') {
            // Pure sector marker like "@D" or sector+vehicle like "@D2:9#VH"
            // Split: sector letter is just the first char if followed by digit or end
            if rest.len() == 1 || (rest.len() > 1 && !rest.as_bytes()[1].is_ascii_alphabetic()) {
                current_sector = rest[..1].to_uppercase();
                if !sectors.contains(&current_sector) {
                    sectors.push(current_sector.clone());
                }
                &rest[1..]
            } else {
                current_sector = rest.to_uppercase();
                if !sectors.contains(&current_sector) {
                    sectors.push(current_sector.clone());
                }
                ""
            }
        } else {
            token
        };

        if token.is_empty() {
            continue;
        }

        // Check for sector change embedded after features: "2:5#VH@B"
        // Split on @ to get vehicle part and trailing sector
        let (vehicle_part, trailing_sector) = if let Some(at_pos) = token.rfind('@') {
            let sector = &token[at_pos + 1..];
            if sector.len() == 1 && sector.as_bytes()[0].is_ascii_alphabetic() {
                (&token[..at_pos], Some(sector.to_uppercase()))
            } else {
                (token, None)
            }
        } else {
            (token, None)
        };

        // Check for closed marker
        let (vehicle_part, closed) = if let Some(rest) = vehicle_part.strip_prefix('%') {
            (rest, true)
        } else {
            (vehicle_part, false)
        };

        // Skip non-passenger vehicles (F, LK = locomotive)
        if vehicle_part == "F" || vehicle_part == "LK" || vehicle_part.starts_with("F#") {
            // Still update sector if trailing
            if let Some(s) = trailing_sector {
                current_sector = s;
                if !sectors.contains(&current_sector) {
                    sectors.push(current_sector.clone());
                }
            }
            continue;
        }

        // Split off features (marked with #, multiple separated by ;)
        // e.g. "2:9#VH;FZ" or "1:2#BHP;NF"
        let parts: Vec<&str> = vehicle_part.splitn(2, '#').collect();
        let base = parts[0];
        let mut features: Vec<String> = Vec::new();

        if parts.len() > 1 {
            for feat in parts[1].split(';') {
                let feat = feat.trim().to_uppercase();
                match feat.as_str() {
                    "VR" => features.push("restaurant".to_string()),
                    "BHP" => features.push("wheelchair".to_string()),
                    "NF" => features.push("low_floor".to_string()),
                    "VH" => {} // VH = Velohaken (bike hooks) — skip for now
                    "FZ" => features.push("family".to_string()),
                    "BZ" => features.push("business".to_string()),
                    "FS" => {} // FreeSurf wifi — skip
                    other if !other.is_empty() => features.push(other.to_lowercase()),
                    _ => {}
                }
            }
        }

        // Parse CLASS:NUMBER
        let class_number: Vec<&str> = base.split(':').collect();
        if class_number.len() < 2 {
            // Update trailing sector even if vehicle doesn't parse
            if let Some(s) = trailing_sector {
                current_sector = s;
                if !sectors.contains(&current_sector) {
                    sectors.push(current_sector.clone());
                }
            }
            continue;
        }

        let class_val: u8 = match class_number[0].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let number_val: u32 = match class_number[1].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };

        position += 1;
        wagons.push(Wagon {
            position,
            number: number_val,
            class: class_val,
            sector: current_sector.clone(),
            features,
            closed,
        });

        // Update sector after this wagon if there was a trailing @SECTOR
        if let Some(s) = trailing_sector {
            current_sector = s;
            if !sectors.contains(&current_sector) {
                sectors.push(current_sector.clone());
            }
        }
    }

    (sectors, wagons)
}

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

    // Navigate: { "formationsAtScheduledStops": [ { "scheduledStop": { "stopPoint": { "uic": N }, "track": "X" }, "formationShort": { "formationShortString": "..." } } ] }
    let stops = body
        .get("formationsAtScheduledStops")
        .and_then(|s| s.as_array())
        .ok_or_else(|| worker::Error::RustError("No formationsAtScheduledStops".into()))?;

    // Find the matching stop (by UIC) or use the first one
    let entry = if let Some(uic) = stop_uic {
        let uic_num: i64 = uic.parse().unwrap_or(0);
        stops
            .iter()
            .find(|s| {
                s.pointer("/scheduledStop/stopPoint/uic")
                    .and_then(|u| u.as_i64())
                    .map(|id| id == uic_num)
                    .unwrap_or(false)
            })
            .ok_or_else(|| {
                worker::Error::RustError(format!("Stop {uic} not found in formation"))
            })?
    } else {
        stops
            .first()
            .ok_or_else(|| worker::Error::RustError("No stops in formation".into()))?
    };

    let track = entry
        .pointer("/scheduledStop/track")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();

    let short_string = entry
        .pointer("/formationShort/formationShortString")
        .and_then(|s| s.as_str())
        .ok_or_else(|| worker::Error::RustError("No formationShortString".into()))?;

    console_log!("Formation short string: {}", short_string);

    let (sectors, wagons) = parse_formation_short_string(short_string);

    Ok(FormationResult {
        track,
        sectors,
        wagons,
    })
}
