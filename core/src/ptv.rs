//! PTV Timetable API v3, the departures source for Victoria. Departures are
//! asked per route type and stop, so a station served by metro and V/Line takes
//! two calls, and a tram stop merged from both directions takes two.

use std::collections::HashMap;

use serde::Deserialize;

use crate::ojp::FlatDeparture;
use crate::upstream::UpstreamError;

pub const PTV_ENDPOINT: &str = "https://timetableapi.ptv.vic.gov.au";

pub const METRO: u8 = 0;
pub const TRAM: u8 = 1;
pub const V_LINE: u8 = 3;

/// PTV signs with HMAC-SHA1. Written here because this crate takes no
/// dependency beyond serde and regex, which keeps it building for wasm.
fn sha1(message: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&((message.len() as u64) * 8).to_be_bytes());

    for block in padded.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (hi, v) in h.iter_mut().zip([a, b, c, d, e]) {
            *hi = hi.wrapping_add(v);
        }
    }

    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

pub fn hmac_sha1(key: &[u8], message: &[u8]) -> [u8; 20] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..20].copy_from_slice(&sha1(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner: Vec<u8> = block.iter().map(|b| b ^ 0x36).collect();
    inner.extend_from_slice(message);
    let mut outer: Vec<u8> = block.iter().map(|b| b ^ 0x5c).collect();
    outer.extend_from_slice(&sha1(&inner));
    sha1(&outer)
}

/// Uppercase hex HMAC-SHA1 of the path and query, keyed with the API key.
pub fn signature(path_and_query: &str, key: &str) -> String {
    hmac_sha1(key.as_bytes(), path_and_query.as_bytes())
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect()
}

/// The signed part of a departures request: everything after the host, with
/// `devid` last because the signature covers it.
pub fn departures_path(route_type: u8, stop_id: &str, limit: u32, devid: &str) -> String {
    format!(
        "/v3/departures/route_type/{route_type}/stop/{stop_id}?max_results={limit}\
         &expand=Route&expand=Direction&expand=Run&include_cancelled=true&devid={devid}"
    )
}

pub fn departures_url(endpoint: &str, route_type: u8, stop_id: &str, limit: u32, devid: &str, key: &str) -> String {
    let path = departures_path(route_type, stop_id, limit, devid);
    let sig = signature(&path, key);
    format!("{}{path}&signature={sig}", endpoint.trim_end_matches('/'))
}

/// The route types to ask for a station of the given mode. A train station may
/// be served by metro, V/Line or both, and a type the stop lacks answers empty.
pub fn route_types_for_mode(mode: &str) -> &'static [u8] {
    match mode {
        "tram" => &[TRAM],
        _ => &[METRO, V_LINE],
    }
}

/// Comma-separated list from a station row, with blanks dropped.
fn list(value: Option<&str>) -> Vec<&str> {
    value.unwrap_or_default().split(',').map(str::trim).filter(|v| !v.is_empty()).collect()
}

/// Every (route type, stop id) a station's board is asked from. A merged tram
/// stop keeps one PTV stop per direction in `stop_ids`, so each is asked. The
/// row's `route_types` decide the types, and the mode only stands in for a row
/// imported without them.
pub fn requests(local_id: &str, mode: &str, route_types: Option<&str>, stop_ids: Option<&str>) -> Vec<(u8, String)> {
    let mut types: Vec<u8> = list(route_types).iter().filter_map(|t| t.parse().ok()).collect();
    if types.is_empty() {
        types = route_types_for_mode(mode).to_vec();
    }
    let mut stops = list(stop_ids);
    if stops.is_empty() {
        stops.push(local_id);
    }
    types
        .iter()
        .flat_map(|&rt| stops.iter().map(move |&stop| (rt, stop.to_string())))
        .collect()
}

/// Labels no client mistakes for a Swiss category such as IC or S, so nothing
/// asks for a formation or reads a train class from them.
fn category(route_type: u8) -> &'static str {
    match route_type {
        METRO => "Metro",
        TRAM => "Tram",
        V_LINE => "V/Line",
        _ => "",
    }
}

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    departures: Vec<Departure>,
    #[serde(default)]
    routes: HashMap<String, Route>,
    #[serde(default)]
    directions: HashMap<String, Direction>,
    #[serde(default)]
    runs: HashMap<String, Run>,
}

#[derive(Deserialize)]
struct Departure {
    route_id: Option<i64>,
    direction_id: Option<i64>,
    run_ref: Option<String>,
    scheduled_departure_utc: Option<String>,
    estimated_departure_utc: Option<String>,
    platform_number: Option<String>,
}

#[derive(Deserialize)]
struct Route {
    route_type: Option<u8>,
    route_name: Option<String>,
    route_number: Option<String>,
    route_gtfs_id: Option<String>,
}

impl Route {
    /// Metro and V/Line routes carry no number, and their names clip in a
    /// line chip, so the code after the dash in the GTFS id (`2-BEG`) stands in.
    fn line(&self) -> Option<String> {
        if let Some(number) = non_empty(&self.route_number) {
            return Some(number.to_string());
        }
        let code = non_empty(&self.route_gtfs_id)
            .and_then(|id| id.split_once('-'))
            .map(|(_, code)| code.trim())
            .filter(|code| !code.is_empty());
        code.map(str::to_uppercase).or_else(|| non_empty(&self.route_name).map(str::to_string))
    }
}

#[derive(Deserialize)]
struct Direction {
    direction_name: Option<String>,
}

#[derive(Deserialize)]
struct Run {
    destination_name: Option<String>,
    /// `scheduled`, `updated` or `cancelled` for a metro run, `scheduled` for
    /// every other mode.
    status: Option<String>,
}

fn non_empty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

/// `iso_to_ms` comes from the caller, as for OJP, so each build keeps its own
/// timestamp parser. A run whose status reads cancelled is flagged cancelled.
pub fn parse_departures<F>(json: &str, route_type: u8, iso_to_ms: F) -> Result<Vec<FlatDeparture>, UpstreamError>
where
    F: Fn(&str) -> f64,
{
    let body: Response = serde_json::from_str(json).map_err(|e| UpstreamError::Unparseable(e.to_string()))?;

    let mut rows: Vec<FlatDeparture> = body
        .departures
        .iter()
        .filter_map(|d| {
            let scheduled = d.scheduled_departure_utc.as_deref()?;
            let scheduled_ms = iso_to_ms(scheduled);
            let route = d.route_id.and_then(|id| body.routes.get(&id.to_string()));
            let run = d.run_ref.as_ref().and_then(|r| body.runs.get(r));
            let cancelled = run
                .and_then(|run| non_empty(&run.status))
                .is_some_and(|status| status.eq_ignore_ascii_case("cancelled"));
            let number = route.and_then(Route::line).unwrap_or_default();
            // The run's destination is a stop name with a stop number, so the
            // direction name is shown and the run is only the fallback.
            let to = d
                .direction_id
                .and_then(|id| body.directions.get(&id.to_string()))
                .and_then(|dir| non_empty(&dir.direction_name))
                .or_else(|| run.and_then(|run| non_empty(&run.destination_name)))
                .unwrap_or("?")
                .to_string();
            let delay = d.estimated_departure_utc.as_deref().and_then(|est| {
                let minutes = ((iso_to_ms(est) - scheduled_ms) / 60000.0).round() as i64;
                (minutes > 0).then_some(minutes)
            });
            Some(FlatDeparture {
                to,
                category: category(route.and_then(|r| r.route_type).unwrap_or(route_type)).to_string(),
                number,
                departure: (scheduled_ms / 1000.0) as i64,
                delay,
                platform: non_empty(&d.platform_number).unwrap_or_default().to_string(),
                platform_changed: false,
                train_number: None,
                operator_ref: None,
                cancelled,
            })
        })
        .collect();

    rows.sort_by_key(|r| r.departure);
    Ok(rows)
}

/// One board from a station's answers, one per route type and stop. A type the
/// stop does not serve may fail while another answers, so only all failing is
/// an outage. A run two of the stops both report is kept once.
pub fn merge(answers: Vec<Result<Vec<FlatDeparture>, UpstreamError>>, limit: u32) -> Result<Vec<FlatDeparture>, UpstreamError> {
    let mut rows = Vec::new();
    let mut first_error = None;
    let mut answered = false;
    for answer in answers {
        match answer {
            Ok(mut part) => {
                answered = true;
                rows.append(&mut part);
            }
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    if !answered {
        return Err(first_error.unwrap_or_else(|| UpstreamError::Unparseable("no route types to ask".into())));
    }
    rows.sort_by(|a, b| (a.departure, &a.category, &a.number, &a.to).cmp(&(b.departure, &b.category, &b.number, &b.to)));
    rows.dedup_by(|a, b| a.departure == b.departure && a.number == b.number && a.to == b.to && a.category == b.category);
    rows.truncate(limit as usize);
    Ok(rows)
}

/// `PTV_DEVID` and `PTV_KEY`. Without them a Victorian station answers 503 like
/// any upstream outage, and the Swiss routes are untouched.
pub fn credentials(devid: Option<String>, key: Option<String>) -> Result<(String, String), UpstreamError> {
    match (devid.filter(|v| !v.is_empty()), key.filter(|v| !v.is_empty())) {
        (Some(devid), Some(key)) => Ok((devid, key)),
        _ => Err(UpstreamError::Unreachable("PTV_DEVID or PTV_KEY not set".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRAINS: &str = include_str!("../fixtures/ptv_train_departures.json");
    const TRAMS: &str = include_str!("../fixtures/ptv_tram_departures.json");
    const DEVID: &str = "3000123";
    const KEY: &str = "0a1b2c3d-4e5f-6789-abcd-ef0123456789";

    fn iso_to_ms(iso: &str) -> f64 {
        let clock = &iso[11..19];
        let parts: Vec<f64> = clock.split(':').map(|v| v.parse().unwrap()).collect();
        (parts[0] * 3600.0 + parts[1] * 60.0 + parts[2]) * 1000.0
    }

    fn at(h: i64, m: i64, s: i64) -> i64 {
        h * 3600 + m * 60 + s
    }

    #[test]
    fn hmac_sha1_matches_the_published_vector() {
        let mac = hmac_sha1(b"key", b"The quick brown fox jumps over the lazy dog");
        let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "de7c9b85b8b78aa6bc8a7a36f70a90701c9db4d9");
    }

    #[test]
    fn hmac_sha1_handles_keys_and_messages_past_one_block() {
        let key = [0xaau8; 80];
        let mac = hmac_sha1(&key, b"Test Using Larger Than Block-Size Key - Hash Key First");
        let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "aa4ae5e15272d00e95705637ce8a3b55ed402112");
    }

    #[test]
    fn signature_for_a_fixed_path_devid_and_key_is_known() {
        let path = departures_path(0, "1105", 5, DEVID);
        assert_eq!(
            path,
            "/v3/departures/route_type/0/stop/1105?max_results=5&expand=Route&expand=Direction&expand=Run&include_cancelled=true&devid=3000123"
        );
        assert_eq!(signature(&path, KEY), "AFA923F72A3C4149F3F9B4312544BC758B55F3A6");
    }

    #[test]
    fn url_appends_the_signature_to_the_signed_path() {
        let url = departures_url("http://stub", 0, "1105", 5, DEVID, KEY);
        assert_eq!(
            url,
            format!("http://stub{}&signature=AFA923F72A3C4149F3F9B4312544BC758B55F3A6", departures_path(0, "1105", 5, DEVID))
        );
    }

    #[test]
    fn trains_parse_with_direction_line_platform_and_delay() {
        let rows = parse_departures(TRAINS, 0, iso_to_ms).unwrap();

        assert_eq!(rows.len(), 3);
        // Sorted by scheduled time. Estimate 3 min 20 s late rounds to 3.
        assert_eq!(rows[0].departure, at(9, 5, 0));
        assert_eq!(rows[0].to, "City (Flinders Street)");
        assert_eq!(rows[0].number, "BEG");
        assert_eq!(rows[0].category, "Metro");
        assert_eq!(rows[0].platform, "1");
        assert_eq!(rows[0].delay, Some(3));
        assert!(!rows[0].platform_changed);
        assert_eq!(rows[0].train_number, None);
        assert_eq!(rows[0].operator_ref, None);

        // On time, then no estimate at all: both read as no delay.
        assert_eq!(rows[1].number, "GLW");
        assert_eq!(rows[1].to, "Glen Waverley");
        assert_eq!(rows[1].delay, None);
        assert_eq!(rows[2].departure, at(9, 20, 0));
        assert_eq!(rows[2].delay, None);
        assert_eq!(rows[2].platform, "2");
    }

    #[test]
    fn trams_use_the_route_number_the_clean_direction_and_no_platform() {
        let rows = parse_departures(TRAMS, 1, iso_to_ms).unwrap();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].number, "67");
        assert_eq!(rows[0].to, "Carnegie");
        assert_eq!(rows[0].category, "Tram");
        assert_eq!(rows[0].platform, "");
        assert_eq!(rows[0].delay, Some(5));
        // Running early is not a delay.
        assert_eq!(rows[1].number, "16");
        assert_eq!(rows[1].to, "St Kilda Beach");
        assert_eq!(rows[1].delay, None);
    }

    fn one_route(route: &str) -> String {
        format!(
            r#"{{"departures": [{{"route_id": 1, "direction_id": 1, "scheduled_departure_utc": "2026-11-22T09:00:00Z"}}],
            "routes": {{"1": {route}}}, "directions": {{"1": {{"direction_name": "Somewhere"}}}}}}"#
        )
    }

    fn line_of(route: &str) -> String {
        parse_departures(&one_route(route), 0, iso_to_ms).unwrap().remove(0).number
    }

    #[test]
    fn lines_without_a_number_take_the_code_from_the_gtfs_route_id() {
        assert_eq!(line_of(r#"{"route_type": 0, "route_name": "Belgrave", "route_number": "", "route_gtfs_id": "2-BEG"}"#), "BEG");
        assert_eq!(line_of(r#"{"route_type": 3, "route_name": "Geelong", "route_number": null, "route_gtfs_id": "1-gel"}"#), "GEL");
        assert_eq!(line_of(r#"{"route_type": 3, "route_name": "Coach", "route_number": " ", "route_gtfs_id": "1-V43"}"#), "V43");
    }

    #[test]
    fn a_route_number_wins_over_the_gtfs_code() {
        assert_eq!(line_of(r#"{"route_type": 1, "route_name": "Melbourne University - St Kilda Beach", "route_number": "16", "route_gtfs_id": "3-16"}"#), "16");
    }

    #[test]
    fn the_route_name_is_the_line_only_when_there_is_no_number_or_code() {
        assert_eq!(line_of(r#"{"route_type": 0, "route_name": "Belgrave", "route_number": ""}"#), "Belgrave");
        assert_eq!(line_of(r#"{"route_type": 0, "route_name": "Belgrave", "route_number": "", "route_gtfs_id": "2-"}"#), "Belgrave");
        assert_eq!(line_of(r#"{"route_type": 0, "route_name": "Belgrave", "route_number": "", "route_gtfs_id": "BEG"}"#), "Belgrave");
    }

    fn with_run_status(status: &str) -> FlatDeparture {
        let json = format!(
            r#"{{"departures": [{{"route_id": 1, "direction_id": 1, "run_ref": "7001", "scheduled_departure_utc": "2026-11-22T09:00:00Z"}}],
            "routes": {{"1": {{"route_type": 0, "route_number": "", "route_gtfs_id": "2-ABC"}}}},
            "directions": {{"1": {{"direction_name": "Somewhere"}}}},
            "runs": {{"7001": {{"destination_name": "Somewhere", "status": "{status}"}}}}}}"#
        );
        parse_departures(&json, 0, iso_to_ms).unwrap().remove(0)
    }

    // Scenario: a cancelled metro run, which include_cancelled asks PTV to return.
    // Expected behaviour: the row carries cancelled, a scheduled run does not.
    #[test]
    fn a_cancelled_run_is_flagged() {
        assert!(with_run_status("cancelled").cancelled);
        assert!(with_run_status("Cancelled").cancelled);
        assert!(!with_run_status("scheduled").cancelled);
        assert!(!parse_departures(TRAINS, 0, iso_to_ms).unwrap().iter().any(|r| r.cancelled));
    }

    #[test]
    fn categories_are_never_a_swiss_formation_label() {
        let v_line = TRAINS.replace("\"route_type\": 0", "\"route_type\": 3");
        let rows = parse_departures(&v_line, 3, iso_to_ms).unwrap();
        assert!(rows.iter().all(|r| r.category == "V/Line"));
    }

    #[test]
    fn a_body_that_is_not_json_is_unparseable() {
        assert!(matches!(parse_departures("<html>", 0, iso_to_ms), Err(UpstreamError::Unparseable(_))));
    }

    #[test]
    fn train_stations_ask_metro_and_v_line_and_tram_stops_ask_tram() {
        assert_eq!(route_types_for_mode("train"), &[0, 3]);
        assert_eq!(route_types_for_mode("tram"), &[1]);
    }

    #[test]
    fn merge_sorts_both_route_types_and_trims_to_the_limit() {
        let metro = parse_departures(TRAINS, 0, iso_to_ms).unwrap();
        let trams = parse_departures(TRAMS, 1, iso_to_ms).unwrap();

        let rows = merge(vec![Ok(metro), Ok(trams)], 4).unwrap();

        let times: Vec<i64> = rows.iter().map(|r| r.departure).collect();
        assert_eq!(times, vec![at(9, 3, 0), at(9, 5, 0), at(9, 10, 0), at(9, 12, 0)]);
    }

    #[test]
    fn merge_keeps_what_answered_when_one_route_type_fails() {
        let metro = parse_departures(TRAINS, 0, iso_to_ms).unwrap();
        let rows = merge(vec![Ok(metro), Err(UpstreamError::Status(400))], 50).unwrap();
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn merge_fails_only_when_every_route_type_fails() {
        let answer = merge(vec![Err(UpstreamError::Status(502)), Err(UpstreamError::Timeout)], 50);
        assert_eq!(answer.unwrap_err(), UpstreamError::Status(502));
    }

    // The other platform of a merged tram stop: the same routes towards the city, at other times.
    fn citybound_trams() -> String {
        TRAMS
            .replace("\"direction_name\": \"St Kilda Beach\"", "\"direction_name\": \"Melbourne University\"")
            .replace("\"direction_name\": \"Carnegie\"", "\"direction_name\": \"Melbourne University\"")
            .replace("09:10:00Z", "09:06:00Z")
            .replace("09:03:00Z", "09:12:00Z")
            .replace("09:09:00Z", "09:06:00Z")
            .replace("09:07:40Z", "09:12:00Z")
    }

    #[test]
    fn a_merged_tram_stop_asks_each_of_its_stops() {
        let asked = requests("2003", "tram", Some("1"), Some("2003,2005"));
        assert_eq!(asked, vec![(TRAM, "2003".to_string()), (TRAM, "2005".to_string())]);
    }

    #[test]
    fn requests_cross_every_route_type_with_every_stop() {
        let asked = requests("9001", "train", Some("0,3"), Some("9001"));
        assert_eq!(asked, vec![(METRO, "9001".to_string()), (V_LINE, "9001".to_string())]);
    }

    #[test]
    fn route_types_come_from_the_row_before_the_mode() {
        assert_eq!(requests("9001", "train", Some("3"), None), vec![(V_LINE, "9001".to_string())]);
    }

    #[test]
    fn a_row_without_lists_asks_its_own_id_for_its_mode() {
        assert_eq!(requests("1105", "train", None, None), vec![(METRO, "1105".to_string()), (V_LINE, "1105".to_string())]);
        assert_eq!(requests("2496", "tram", Some(""), Some(" ")), vec![(TRAM, "2496".to_string())]);
    }

    #[test]
    fn merge_of_both_stops_is_one_sorted_board_with_both_directions() {
        let outbound = parse_departures(TRAMS, TRAM, iso_to_ms).unwrap();
        let citybound = parse_departures(&citybound_trams(), TRAM, iso_to_ms).unwrap();

        let rows = merge(vec![Ok(outbound), Ok(citybound)], 50).unwrap();

        let board: Vec<(i64, &str)> = rows.iter().map(|r| (r.departure, r.to.as_str())).collect();
        assert_eq!(
            board,
            vec![
                (at(9, 3, 0), "Carnegie"),
                (at(9, 6, 0), "Melbourne University"),
                (at(9, 10, 0), "St Kilda Beach"),
                (at(9, 12, 0), "Melbourne University"),
            ]
        );
    }

    #[test]
    fn merge_drops_a_departure_two_stops_both_report() {
        let a = parse_departures(TRAMS, TRAM, iso_to_ms).unwrap();
        let b = parse_departures(TRAMS, TRAM, iso_to_ms).unwrap();
        assert_eq!(merge(vec![Ok(a), Ok(b)], 50).unwrap().len(), 2);
    }

    #[test]
    fn missing_credentials_are_an_upstream_failure() {
        assert!(credentials(None, Some("k".into())).is_err());
        assert!(credentials(Some("1".into()), Some(String::new())).is_err());
        assert_eq!(credentials(Some("1".into()), Some("k".into())).unwrap(), ("1".into(), "k".into()));
    }
}
