use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use regex::Regex;
use serde::{Deserialize, Serialize};

pub const OJP_ENDPOINT: &str = "https://api.opentransportdata.swiss/ojp20";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlatDeparture {
    pub to: String,
    pub category: String,
    pub number: String,
    pub departure: i64,
    pub delay: Option<i64>,
    pub platform: String,
    #[serde(rename = "platformChanged")]
    pub platform_changed: bool,
    #[serde(rename = "trainNumber", skip_serializing_if = "Option::is_none")]
    pub train_number: Option<String>,
    #[serde(rename = "operatorRef", skip_serializing_if = "Option::is_none")]
    pub operator_ref: Option<String>,
}

/// `now_iso` and `message_id` are passed in rather than read from a clock, so this
/// crate stays free of any runtime-specific time source.
pub fn build_stop_event_request_xml(
    stop_ref: &str,
    limit: u32,
    now_iso: &str,
    message_id: u64,
) -> String {
    let now = now_iso;
    let msg_id = message_id;
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<OJP xmlns="http://www.vdv.de/ojp" xmlns:siri="http://www.siri.org.uk/siri" version="2.0">
  <OJPRequest>
    <siri:ServiceRequest>
      <siri:RequestTimestamp>{now}</siri:RequestTimestamp>
      <siri:RequestorRef>traintime_prod</siri:RequestorRef>
      <OJPStopEventRequest>
        <siri:RequestTimestamp>{now}</siri:RequestTimestamp>
        <siri:MessageIdentifier>SER-{msg_id}</siri:MessageIdentifier>
        <Location>
          <PlaceRef>
            <siri:StopPointRef>{stop_ref}</siri:StopPointRef>
          </PlaceRef>
        </Location>
        <Params>
          <NumberOfResults>{limit}</NumberOfResults>
          <StopEventType>departure</StopEventType>
        </Params>
      </OJPStopEventRequest>
    </siri:ServiceRequest>
  </OJPRequest>
</OJP>"#
    )
}

thread_local! {
    static PATTERNS: RefCell<HashMap<String, Rc<Regex>>> = RefCell::new(HashMap::new());
    #[cfg(test)]
    static COMPILES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Compiling a pattern costs milliseconds in wasm and a response needs over a
/// thousand lookups, which ran the Worker past its CPU limit. Each pattern is
/// compiled once per thread. The `Rc` matters: a cloned `Regex` starts with an
/// empty match cache and pays to rebuild it on its first search.
pub fn compile(pattern: &str) -> Option<Rc<Regex>> {
    PATTERNS.with(|patterns| {
        if let Some(re) = patterns.borrow().get(pattern) {
            return Some(re.clone());
        }
        #[cfg(test)]
        COMPILES.with(|c| c.set(c.get() + 1));
        let re = Rc::new(Regex::new(pattern).ok()?);
        patterns.borrow_mut().insert(pattern.to_string(), re.clone());
        Some(re)
    })
}

/// Extract text content from a leaf tag (no children), matching optional namespace prefix
pub fn xml_text(xml: &str, tag: &str) -> Option<String> {
    let pattern = format!(r"<(?:[a-z]+:)?{tag}[^>]*>([^<]*)</(?:[a-z]+:)?{tag}>");
    let re = compile(&pattern)?;
    re.captures(xml).map(|cap| cap[1].trim().to_string())
}

/// Extract all blocks matching a tag (outermost match — finds first closing tag)
pub fn xml_blocks(xml: &str, tag: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let open_pattern = format!(r"(?i)<(?:[a-z]+:)?{tag}[^>]*>");
    let close_pattern = format!(r"(?i)</(?:[a-z]+:)?{tag}>");
    let open_re = compile(&open_pattern).unwrap();
    let close_re = compile(&close_pattern).unwrap();

    for open_match in open_re.find_iter(xml) {
        let search_start = open_match.end();
        if let Some(close_match) = close_re.find_at(xml, search_start) {
            blocks.push(xml[open_match.start()..close_match.end()].to_string());
        }
    }
    blocks
}

/// Extract <Text xml:lang="...">value</Text> from within a parent block
pub fn xml_lang_text(xml: &str) -> Option<String> {
    let re = compile(r"(?i)<Text[^>]*>([^<]*)</Text>").unwrap();
    re.captures(xml).map(|cap| cap[1].trim().to_string())
}

/// `iso_to_ms` is supplied by the caller so each runtime keeps its own timestamp
/// parser. The Worker passes a `js_sys::Date` closure, the native server a chrono
/// one — swapping either for the other would change how loose timestamps parse.
pub fn parse_stop_events<F>(xml: &str, iso_to_ms: F) -> Vec<FlatDeparture>
where
    F: Fn(&str) -> f64,
{
    let mut events: Vec<(Option<String>, FlatDeparture)> = Vec::new();
    let result_blocks = xml_blocks(xml, "StopEventResult");

    for result in &result_blocks {
        let stop_event_blocks = xml_blocks(result, "StopEvent");
        let stop_event_block = match stop_event_blocks.first() {
            Some(b) => b,
            None => continue,
        };

        // --- Service info (destination) ---
        let service_blocks = xml_blocks(stop_event_block, "Service");
        let service_block = match service_blocks.first() {
            Some(b) => b,
            None => continue,
        };

        let destination = xml_blocks(service_block, "DestinationText")
            .first()
            .and_then(|b| xml_lang_text(b))
            .unwrap_or_else(|| "?".to_string());

        // --- Mode (category) and line number ---
        let category = xml_blocks(service_block, "Mode")
            .first()
            .and_then(|mode_block| {
                xml_blocks(mode_block, "ShortName")
                    .first()
                    .and_then(|b| xml_lang_text(b))
            })
            .unwrap_or_default();

        let number = xml_blocks(service_block, "PublishedServiceName")
            .first()
            .and_then(|b| xml_lang_text(b))
            .unwrap_or_default();

        let train_number = xml_text(service_block, "TrainNumber");
        let operator_ref = xml_text(service_block, "OperatorRef");

        // The operating day is part of the key so a board spanning midnight keeps
        // both runs of a journey that repeats the next day.
        let journey_key = xml_text(service_block, "JourneyRef").map(|journey| {
            let day = xml_text(service_block, "OperatingDayRef").unwrap_or_default();
            format!("{day}|{journey}")
        });

        // --- ThisCall > CallAtStop (departure time, platform) ---
        let this_call_blocks = xml_blocks(stop_event_block, "ThisCall");
        let call_block = this_call_blocks
            .first()
            .and_then(|tc| xml_blocks(tc, "CallAtStop").into_iter().next())
            .or_else(|| this_call_blocks.first().cloned())
            .unwrap_or_else(|| stop_event_block.clone());

        // ServiceDeparture > TimetabledTime / EstimatedTime
        let dep_block = xml_blocks(&call_block, "ServiceDeparture")
            .into_iter()
            .next()
            .unwrap_or_else(|| call_block.clone());

        let timetabled_time = xml_text(&dep_block, "TimetabledTime");
        let estimated_time = xml_text(&dep_block, "EstimatedTime");

        // Platform: PlannedQuay / EstimatedQuay
        let planned_quay = xml_blocks(&call_block, "PlannedQuay")
            .first()
            .and_then(|b| xml_lang_text(b))
            .unwrap_or_default();

        let estimated_quay = xml_blocks(&call_block, "EstimatedQuay")
            .first()
            .and_then(|b| xml_lang_text(b));

        // Compute timestamp and delay
        let mut departure_timestamp: i64 = 0;
        let mut delay: Option<i64> = None;

        if let Some(ref tt) = timetabled_time {
            departure_timestamp = (iso_to_ms(tt) / 1000.0) as i64;
        }
        if let (Some(ref et), Some(ref tt)) = (&estimated_time, &timetabled_time) {
            let delay_ms = iso_to_ms(et) - iso_to_ms(tt);
            let delay_min = (delay_ms / 60000.0).round() as i64;
            delay = if delay_min > 0 { Some(delay_min) } else { None };
        }

        let platform_changed = estimated_quay
            .as_ref()
            .map(|eq| eq != &planned_quay)
            .unwrap_or(false);

        events.push((
            journey_key,
            FlatDeparture {
                to: destination,
                category,
                number,
                departure: departure_timestamp,
                delay,
                platform: estimated_quay.unwrap_or(planned_quay),
                platform_changed,
                train_number,
                operator_ref,
            },
        ));
    }

    keep_latest_per_journey(events)
}

/// OJP can publish one journey twice at a stop: two StopEventResults sharing a
/// JourneyRef but carrying different timetabled times, the earlier of which no
/// operator board shows. Seen along the Valais corridor, where the Visp-Brig
/// works make the IR90/IR95 additional (`Unplanned`) journeys.
///
/// Keying on the journey is what makes this safe. Two genuinely different trains
/// a minute apart carry different JourneyRefs, so only real twins collapse -
/// which a (time, line, destination) key could not tell apart. The published row
/// is the later of the pair, checked against the SBB board at Sion, Visp, Bex
/// and Brig.
fn keep_latest_per_journey(events: Vec<(Option<String>, FlatDeparture)>) -> Vec<FlatDeparture> {
    let mut kept: Vec<FlatDeparture> = Vec::with_capacity(events.len());
    let mut seen: HashMap<String, usize> = HashMap::new();

    for (key, event) in events {
        let Some(key) = key else {
            kept.push(event);
            continue;
        };
        match seen.get(&key) {
            Some(&at) => {
                if event.departure > kept[at].departure {
                    kept[at] = event;
                }
            }
            None => {
                seen.insert(key, kept.len());
                kept.push(event);
            }
        }
    }

    kept.sort_by_key(|event| event.departure);
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    const STOP_EVENT: &str = r#"<StopEventResult><StopEvent>
      <ThisCall><CallAtStop>
        <ServiceDeparture><TimetabledTime>2026-09-17T10:00:00Z</TimetabledTime></ServiceDeparture>
        <PlannedQuay><Text>7</Text></PlannedQuay>
      </CallAtStop></ThisCall>
      <Service>
        <Mode><ShortName><Text>IC</Text></ShortName></Mode>
        <PublishedServiceName><Text>IC1</Text></PublishedServiceName>
        <DestinationText><Text>Bern</Text></DestinationText>
      </Service>
    </StopEvent></StopEventResult>"#;

    fn compiles_for(events: usize) -> usize {
        let xml = format!("<OJP>{}</OJP>", STOP_EVENT.repeat(events));
        let before = COMPILES.with(|c| c.get());
        assert_eq!(parse_stop_events(&xml, |_| 0.0).len(), events);
        COMPILES.with(|c| c.get()) - before
    }

    fn event(journey: &str, day: &str, time: &str, quay: &str) -> String {
        format!(
            r#"<StopEventResult><StopEvent>
              <ThisCall><CallAtStop>
                <ServiceDeparture><TimetabledTime>{time}</TimetabledTime></ServiceDeparture>
                <PlannedQuay><Text>{quay}</Text></PlannedQuay>
              </CallAtStop></ThisCall>
              <Service>
                <OperatingDayRef>{day}</OperatingDayRef>
                <JourneyRef>{journey}</JourneyRef>
                <Mode><ShortName><Text>IR</Text></ShortName></Mode>
                <PublishedServiceName><Text>IR90</Text></PublishedServiceName>
                <DestinationText><Text>Visp</Text></DestinationText>
              </Service>
            </StopEvent></StopEventResult>"#
        )
    }

    fn parse(events: &[String]) -> Vec<FlatDeparture> {
        let xml = format!("<OJP>{}</OJP>", events.concat());
        parse_stop_events(&xml, |iso| {
            let clock = &iso[11..19];
            let parts: Vec<f64> = clock.split(':').map(|v| v.parse().unwrap()).collect();
            (parts[0] * 3600.0 + parts[1] * 60.0 + parts[2]) * 1000.0
        })
    }

    const DAY: &str = "2026-09-19";
    const IR90: &str = "ch:1:sjyid:100001:30617-006";
    const IR95: &str = "ch:1:sjyid:100001:30919-005";
    const R91: &str = "ch:1:sjyid:100055:6218-001";

    // Scenario: OJP publishes an additional journey twice at Sion, the twins
    // differing only in timetabled time and quay.
    // Expected behaviour: one row survives, the later one, which is what the
    // SBB board shows.
    #[test]
    fn twin_results_for_one_journey_collapse_to_the_later_row() {
        let rows = parse(&[
            event(IR90, DAY, "2026-09-19T10:56:18Z", ""),
            event(IR90, DAY, "2026-09-19T10:57:00Z", "2"),
        ]);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].departure, 10 * 3600 + 57 * 60);
        assert_eq!(rows[0].platform, "2");
    }

    // The twins arrive in either order, so the rule cannot rely on the later row
    // coming second.
    #[test]
    fn the_later_row_wins_whichever_order_it_arrives_in() {
        let rows = parse(&[
            event(IR90, DAY, "2026-09-19T10:57:00Z", "2"),
            event(IR90, DAY, "2026-09-19T10:56:18Z", ""),
        ]);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].departure, 10 * 3600 + 57 * 60);
    }

    // Scenario: two different trains really do leave a minute apart.
    // Expected behaviour: both stay, because the key is the journey and not the
    // departure minute.
    #[test]
    fn different_journeys_a_minute_apart_both_survive() {
        let rows = parse(&[
            event(IR95, DAY, "2026-09-19T11:19:00Z", "2"),
            event(R91, DAY, "2026-09-19T11:20:00Z", "3"),
        ]);

        assert_eq!(rows.len(), 2);
    }

    // A board that spans midnight sees the same journey on two operating days.
    #[test]
    fn one_journey_on_two_operating_days_stays_twice() {
        let rows = parse(&[
            event(IR90, DAY, "2026-09-19T23:57:00Z", "2"),
            event(IR90, "2026-09-20", "2026-09-20T00:57:00Z", "2"),
        ]);

        assert_eq!(rows.len(), 2);
    }

    // A response without JourneyRef must not collapse into a single row.
    #[test]
    fn results_without_a_journey_ref_are_all_kept() {
        let xml = format!("<OJP>{}</OJP>", STOP_EVENT.repeat(3));
        assert_eq!(parse_stop_events(&xml, |_| 0.0).len(), 3);
    }

    // Scenario: a cache miss parses 50 stop events inside the Worker's CPU
    // limit, where compiling one pattern costs milliseconds.
    // Expected behaviour: each pattern compiles once, however many events
    // the response holds.
    #[test]
    fn pattern_compiles_do_not_grow_with_the_event_count() {
        let first = compiles_for(1);
        assert!(first <= 32, "{first} compiles for one event");
        assert_eq!(compiles_for(50), 0);
    }
}
