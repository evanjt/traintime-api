use serde::Serialize;

use crate::error::CoreError;

pub const FORMATION_ENDPOINT: &str =
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
    #[serde(rename = "noPassageAfter", skip_serializing_if = "std::ops::Not::not")]
    pub no_passage_after: bool,
    #[serde(rename = "mixedClass", skip_serializing_if = "std::ops::Not::not")]
    pub mixed_class: bool,
    #[serde(rename = "bikeReservation", skip_serializing_if = "std::ops::Not::not")]
    pub bike_reservation: bool,
    #[serde(rename = "numberUnknown", skip_serializing_if = "std::ops::Not::not")]
    pub number_unknown: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FormationResult {
    pub track: String,
    pub sectors: Vec<String>,
    pub wagons: Vec<Wagon>,
}

/// Strip non-numeric prefix from train number (e.g. "IR95" -> "95", "S3" -> "3")
pub fn extract_train_number(raw: &str) -> String {
    raw.trim_start_matches(|c: char| !c.is_ascii_digit())
        .to_string()
}

/// Map OJP OperatorRef numeric code to formation API EVU code.
/// See https://api.opentransportdata.swiss/formation/v1 for supported EVUs.
pub fn operator_ref_to_evu(op_ref: &str) -> Option<&'static str> {
    // OJP started prefixing refs with a namespace ("ojp:11") in 2026.
    let code = op_ref.rsplit(':').next().unwrap_or(op_ref);
    match code {
        "11" => Some("SBBP"),   // SBB
        "33" => Some("BLSP"),   // BLS
        "65" => Some("THURBO"), // Thurbo
        "82" => Some("SOB"),    // Südostbahn
        "86" => Some("ZB"),     // Zentralbahn
        "53" => Some("TPF"),    // Transports publics fribourgeois
        "73" => Some("TRN"),    // TransN
        "72" => Some("RhB"),    // Rhätische Bahn
        _ => None,              // Unknown or unsupported (RegionAlps=74, MBC, etc.)
    }
}

/// One vehicle token of a formationShortString, before passenger filtering.
struct Vehicle {
    kind: String,
    sector: String,
    number: Option<u32>,
    opens: bool,
    closes: bool,
    closed: bool,
    not_serving: bool,
    offers: Vec<String>,
}

/// Parse a formationShortString into sectors and passenger wagons, front first.
///
/// A vehicle is `[(][status][(]TYPE[)][:NUMBER][#OFFER;OFFER]`, comma separated, with
/// `@SECTOR` leading or trailing a token and `[` `]` around the train's own vehicles.
/// `(` and `)` mark a vehicle end with no access to its neighbour. Status: `-` closed,
/// `%` restaurant open but not serving, `>` and `=` group boarding and reservation.
/// Types: 1, 2, 12 (mixed), CC, FA, WL, WR, W1, W2, K; LK (traction), D (baggage),
/// F (fictitious, pads the platform) and X (parked) are not passenger cars.
///
/// The output keeps the shape older apps read: class is 1 or 2, number is always set
/// (position when the feed gives none, flagged numberUnknown), and features list the
/// original five strings before `bike`.
pub fn parse_formation_short_string(short: &str) -> (Vec<String>, Vec<Wagon>) {
    let token_re = crate::ojp::compile(
        r"^(\()?([-%>=]*)(\()?(LK|CC|FA|WL|WR|W1|W2|12|1|2|D|F|K|X)(\))?(?::(\d+))?(?:#([A-Za-z0-9;]+))?$",
    )
    .unwrap();
    let mut sectors: Vec<String> = Vec::new();
    let mut vehicles: Vec<Vehicle> = Vec::new();
    let mut sector = String::new();

    let enter = |s: &str, sector: &mut String, sectors: &mut Vec<String>| {
        *sector = s.to_string();
        if !sectors.iter().any(|x| x == s) {
            sectors.push(s.to_string());
        }
    };

    for raw in short.split(',') {
        let mut token = raw.trim().replace(['[', ']'], "");
        if let Some(rest) = token.strip_prefix('@') {
            let letter: String = rest.chars().take_while(|c| c.is_ascii_alphabetic()).take(1).collect();
            if !letter.is_empty() {
                enter(&letter.to_uppercase(), &mut sector, &mut sectors);
                token = rest[letter.len()..].to_string();
            }
        }
        let mut trailing = None;
        if let Some(at) = token.rfind('@') {
            let tail = &token[at + 1..];
            if tail.len() == 1 && tail.chars().all(|c| c.is_ascii_alphabetic()) {
                trailing = Some(tail.to_uppercase());
                token.truncate(at);
            }
        }
        if !token.is_empty() {
            if let Some(c) = token_re.captures(&token) {
                let status = c.get(2).map_or("", |m| m.as_str());
                vehicles.push(Vehicle {
                    kind: c[4].to_string(),
                    sector: sector.clone(),
                    number: c.get(6).and_then(|m| m.as_str().parse().ok()),
                    opens: c.get(1).is_some() || c.get(3).is_some(),
                    closes: c.get(5).is_some(),
                    closed: status.contains('-'),
                    not_serving: status.contains('%'),
                    offers: c
                        .get(7)
                        .map(|m| m.as_str().split(';').map(|o| o.trim().to_uppercase()).collect())
                        .unwrap_or_default(),
                });
            }
        }
        if let Some(s) = trailing {
            enter(&s, &mut sector, &mut sectors);
        }
    }

    let mut wagons: Vec<Wagon> = Vec::new();
    let mut boundary_pending = false;
    for (i, v) in vehicles.iter().enumerate() {
        let next_opens = vehicles.get(i + 1).is_some_and(|n| n.opens);
        if matches!(v.kind.as_str(), "LK" | "D" | "F" | "X") {
            // A boundary on either side of a non-passenger vehicle still separates the cars around it
            boundary_pending |= v.closes || next_opens;
            continue;
        }
        if boundary_pending || v.opens {
            if let Some(prev) = wagons.last_mut() {
                prev.no_passage_after = true;
            }
        }
        boundary_pending = v.closes || next_opens;

        let has = |o: &str| v.offers.iter().any(|x| x == o);
        let mut features = Vec::new();
        if has("BHP") {
            features.push("wheelchair");
        }
        if matches!(v.kind.as_str(), "WR" | "W1" | "W2") && !v.not_serving {
            features.push("restaurant");
        }
        if v.kind == "FA" || has("FZ") {
            features.push("family");
        }
        if has("BZ") {
            features.push("business");
        }
        if has("NF") {
            features.push("low_floor");
        }
        if has("VH") || has("VR") {
            features.push("bike");
        }

        let position = wagons.len() + 1;
        wagons.push(Wagon {
            position,
            number: v.number.unwrap_or(position as u32),
            class: if matches!(v.kind.as_str(), "1" | "W1" | "12") { 1 } else { 2 },
            sector: v.sector.clone(),
            features: features.into_iter().map(String::from).collect(),
            closed: v.closed,
            no_passage_after: false,
            mixed_class: v.kind == "12",
            bike_reservation: has("VR"),
            number_unknown: v.number.is_none(),
        });
    }
    // Only a boundary between two passenger cars counts; the train's own ends are open platform
    if let Some(last) = wagons.last_mut() {
        last.no_passage_after = false;
    }

    (sectors, wagons)
}

/// Navigate the formation API response down to a single stop and parse its
/// formationShortString. Pure JSON walking, shared by both runtimes.
pub fn parse_formation_response(
    body: &serde_json::Value,
    stop_uic: Option<&str>,
) -> Result<FormationResult, CoreError> {
    let stops = body
        .get("formationsAtScheduledStops")
        .and_then(|s| s.as_array())
        .ok_or(CoreError::MissingField("formationsAtScheduledStops"))?;

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
            .ok_or_else(|| CoreError::StopNotFound(uic.to_string()))?
    } else {
        stops
            .first()
            .ok_or(CoreError::MissingField("stops in formation"))?
    };

    let track = entry
        .pointer("/scheduledStop/track")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();

    let short_string = entry
        .pointer("/formationShort/formationShortString")
        .and_then(|s| s.as_str())
        .ok_or(CoreError::MissingField("formationShortString"))?;

    let (sectors, wagons) = parse_formation_short_string(short_string);

    Ok(FormationResult {
        track,
        sectors,
        wagons,
    })
}
