//! Characterisation tests. These pin the parser output as it behaved before the
//! Worker and the native server were split onto a shared crate. They exist to
//! catch drift, so update them only alongside a deliberate behaviour change.

use traintime_core::{
    operator_ref_to_evu, parse_favourites, parse_formation_short_string, parse_stop_events,
    partition_favourites, FlatDeparture,
};

fn wagon_tuple(w: &traintime_core::Wagon) -> (usize, u32, u8, &str, Vec<&str>, bool) {
    (
        w.position,
        w.number,
        w.class,
        w.sector.as_str(),
        w.features.iter().map(|f| f.as_str()).collect(),
        w.closed,
    )
}

#[test]
fn formation_sectors_and_features() {
    let (sectors, wagons) = parse_formation_short_string("@A2:1#VH,2:2,@B2:3,1:4#VR,@C1:5");

    assert_eq!(sectors, ["A", "B", "C"]);
    assert_eq!(
        wagons.iter().map(wagon_tuple).collect::<Vec<_>>(),
        [
            (1, 1, 2, "A", vec![], false),
            (2, 2, 2, "A", vec![], false),
            (3, 3, 2, "B", vec![], false),
            (4, 4, 1, "B", vec!["restaurant"], false),
            (5, 5, 1, "C", vec![], false),
        ]
    );
}

#[test]
fn formation_unit_groups_and_closed_marker() {
    // Vehicles before the first @SECTOR carry an empty sector, and `%` marks closed.
    let (sectors, wagons) =
        parse_formation_short_string("[(1:11#BHP;NF,2:12#VH):3],@D2:9#VH;FZ,%2:8");

    assert_eq!(sectors, ["D"]);
    assert_eq!(
        wagons.iter().map(wagon_tuple).collect::<Vec<_>>(),
        [
            (1, 11, 1, "", vec!["wheelchair", "low_floor"], false),
            (2, 12, 2, "", vec![], false),
            (3, 9, 2, "D", vec!["family"], false),
            (4, 8, 2, "D", vec![], true),
        ]
    );
}

#[test]
fn formation_class_only_numbers_fall_back_to_position() {
    // No car numbers given, so the sentinel-resolution pass produces duplicates,
    // which then collapse to sequential positions.
    let (sectors, wagons) = parse_formation_short_string("@A1,1,2,2,@B2");

    assert_eq!(sectors, ["A", "B"]);
    assert_eq!(
        wagons.iter().map(|w| (w.position, w.number, w.class)).collect::<Vec<_>>(),
        [(1, 1, 1), (2, 2, 1), (3, 3, 2), (4, 4, 2), (5, 5, 2)]
    );
}

#[test]
fn formation_skips_non_passenger_vehicles() {
    let (sectors, wagons) = parse_formation_short_string("F,LK,@A2:1,2:2#FS;BZ@B");

    assert_eq!(sectors, ["A", "B"]);
    assert_eq!(
        wagons.iter().map(wagon_tuple).collect::<Vec<_>>(),
        [
            (1, 1, 2, "A", vec![], false),
            (2, 2, 2, "A", vec!["business"], false),
        ]
    );
}

#[test]
fn favourites_are_cloned_not_removed_and_sorted_by_departure() {
    let departures = vec![
        departure("IR90", "Brig", 300),
        departure("IC8", "Romanshorn", 100),
        departure("S3", "Aigle", 200),
    ];
    let pairs = parse_favourites("IR90:Brig,IC8:Romanshorn");
    assert_eq!(
        pairs,
        [
            ("IR90".to_string(), "Brig".to_string()),
            ("IC8".to_string(), "Romanshorn".to_string()),
        ]
    );

    let (favs, remaining) = partition_favourites(&departures, &pairs, 2);

    // Sorted by departure time, not by the order given in the query string.
    assert_eq!(
        favs.iter().map(|d| d.number.as_str()).collect::<Vec<_>>(),
        ["IC8", "IR90"]
    );
    // Favourites still appear in the truncated regular list.
    assert_eq!(
        remaining.iter().map(|d| d.number.as_str()).collect::<Vec<_>>(),
        ["IR90", "IC8"]
    );
}

#[test]
fn favourites_ignores_malformed_pairs() {
    assert_eq!(parse_favourites("IC8:,:Brig,,IR90:Visp").len(), 1);
}

#[test]
fn stop_events_parse_delay_and_platform_change() {
    let xml = r#"
    <StopEventResult><StopEvent>
      <Service>
        <Mode><ShortName><Text>IR</Text></ShortName></Mode>
        <PublishedServiceName><Text>IR90</Text></PublishedServiceName>
        <DestinationText><Text>Brig</Text></DestinationText>
        <siri:TrainNumber>1790</siri:TrainNumber>
        <siri:OperatorRef>11</siri:OperatorRef>
      </Service>
      <ThisCall><CallAtStop>
        <ServiceDeparture>
          <TimetabledTime>60000</TimetabledTime>
          <EstimatedTime>240000</EstimatedTime>
        </ServiceDeparture>
        <PlannedQuay><Text>3</Text></PlannedQuay>
        <EstimatedQuay><Text>5</Text></EstimatedQuay>
      </CallAtStop></ThisCall>
    </StopEvent></StopEventResult>"#;

    // Stub clock: the ISO string is already the millisecond value.
    let events = parse_stop_events(xml, |s| s.parse::<f64>().unwrap_or(0.0));

    assert_eq!(events.len(), 1);
    let e = &events[0];
    assert_eq!(e.to, "Brig");
    assert_eq!(e.category, "IR");
    assert_eq!(e.number, "IR90");
    assert_eq!(e.departure, 60);
    assert_eq!(e.delay, Some(3)); // 180_000ms rounds to 3 minutes
    assert_eq!(e.platform, "5");
    assert!(e.platform_changed);
    assert_eq!(e.train_number.as_deref(), Some("1790"));
    assert_eq!(e.operator_ref.as_deref(), Some("11"));
}

#[test]
fn stop_events_without_estimate_have_no_delay() {
    let xml = r#"
    <StopEventResult><StopEvent>
      <Service>
        <PublishedServiceName><Text>S3</Text></PublishedServiceName>
        <DestinationText><Text>Aigle</Text></DestinationText>
      </Service>
      <ThisCall><CallAtStop>
        <ServiceDeparture><TimetabledTime>60000</TimetabledTime></ServiceDeparture>
        <PlannedQuay><Text>1</Text></PlannedQuay>
      </CallAtStop></ThisCall>
    </StopEvent></StopEventResult>"#;

    let events = parse_stop_events(xml, |s| s.parse::<f64>().unwrap_or(0.0));

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].delay, None);
    assert_eq!(events[0].platform, "1");
    assert!(!events[0].platform_changed);
    assert_eq!(events[0].departure, 60);
}

#[test]
fn missing_destination_falls_back_to_question_mark() {
    let xml = r#"<StopEventResult><StopEvent><Service>
        <PublishedServiceName><Text>B1</Text></PublishedServiceName>
      </Service></StopEvent></StopEventResult>"#;

    let events = parse_stop_events(xml, |_| 0.0);
    assert_eq!(events[0].to, "?");
}

fn departure(number: &str, to: &str, departure: i64) -> FlatDeparture {
    FlatDeparture {
        to: to.to_string(),
        category: "IR".to_string(),
        number: number.to_string(),
        departure,
        delay: None,
        platform: "1".to_string(),
        platform_changed: false,
        train_number: None,
        operator_ref: None,
    }
}

#[test]
fn operator_ref_accepts_namespaced_form() {
    assert_eq!(operator_ref_to_evu("11"), Some("SBBP"));
    assert_eq!(operator_ref_to_evu("ojp:11"), Some("SBBP"));
    assert_eq!(operator_ref_to_evu("ojp:33"), Some("BLSP"));
    assert_eq!(operator_ref_to_evu("ojp:74"), None);
}
