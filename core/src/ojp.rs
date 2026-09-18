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
    let mut events = Vec::new();
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

        events.push(FlatDeparture {
            to: destination,
            category,
            number,
            departure: departure_timestamp,
            delay,
            platform: estimated_quay.unwrap_or(planned_quay),
            platform_changed,
            train_number,
            operator_ref,
        });
    }

    events
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
