//! The formationShortString grammar, checked against strings taken from the live feed.
//! Cars run front first; `(` and `)` close a unit to its neighbour; the type gives the
//! class and the restaurant; `-` is closed, `%` a restaurant open but not serving.

use traintime_core::{parse_formation_short_string, Wagon};

const IC708_BERN: &str =
    "@H,F,[(FA:18#KW;NF,2:17#VR;KW;NF@G,2:16#VR;KW;NF,2:15#BHP;NF@F,%W2:14#BHP;NF,1:13#VR;NF@E,1:12#VR;KW;NF,1):11#VR;BZ;NF@D,(FA:8#KW;NF,2:7#VR;KW;NF@C,2:6#VR;KW;NF@B,2:5#BHP;NF,W2:4#BHP;NF,1:3#VR;NF@A,1:2#VR;KW;NF,1):1#VR;BZ;NF],F";
const IC779_ZURICH: &str =
    "@D,F,F,[(2:17#VR;KW;FZ,2:16@C,W1:15,1:14#BHP,1:13#BZ,2:12,2):11#VR@B,(2:7#VR;KW;FZ,2:6,W1:5@A,1:4#BHP,1:3#BZ,2:2,2):1#VR]";
const IR2127_ZURICH: &str =
    "@D,F,F,F,F@C,F,F,[(FA:9#VH;NF,2:8#NF@B,2:7#NF,2:6#NF,2:5#NF,2:4#NF@A,1:3#NF,1:2#NF,1:1#BHP;BZ;NF,LK)]";
const IC977_BERN: &str =
    "@A,F,F@B,[(2:20#VR;FZ,2:19#VR@C,1:18,2:17#VR@D,2):16#VR,(FA:7#VR;NF@E,2:6#NF,2:5#NF@F,2:4#NF,W2:3#NF@G,1:2#NF,1:1#BHP;BZ;NF@H,LK)],F,F,F";

const KNOWN_FEATURES: [&str; 6] = ["wheelchair", "restaurant", "family", "business", "low_floor", "bike"];

fn wagons(short: &str) -> Vec<Wagon> {
    parse_formation_short_string(short).1
}

fn numbers(ws: &[Wagon]) -> Vec<u32> {
    ws.iter().map(|w| w.number).collect()
}

fn numbers_where(ws: &[Wagon], pick: impl Fn(&Wagon) -> bool) -> Vec<u32> {
    ws.iter().filter(|w| pick(w)).map(|w| w.number).collect()
}

#[test]
fn every_passenger_car_keeps_its_own_number() {
    assert_eq!(numbers(&wagons(IC708_BERN)), [18, 17, 16, 15, 14, 13, 12, 11, 8, 7, 6, 5, 4, 3, 2, 1]);
    assert_eq!(numbers(&wagons(IC779_ZURICH)), [17, 16, 15, 14, 13, 12, 11, 7, 6, 5, 4, 3, 2, 1]);
    assert_eq!(numbers(&wagons(IR2127_ZURICH)), [9, 8, 7, 6, 5, 4, 3, 2, 1]);
    assert_eq!(numbers(&wagons(IC977_BERN)), [20, 19, 18, 17, 16, 7, 6, 5, 4, 3, 2, 1]);
}

#[test]
fn unit_boundaries_mark_no_passage_after_the_last_car_of_a_unit() {
    let after = |s| numbers_where(&wagons(s), |w| w.no_passage_after);
    assert_eq!(after(IC708_BERN), [11]);
    assert_eq!(after(IC779_ZURICH), [11]);
    assert_eq!(after(IR2127_ZURICH), Vec::<u32>::new());
    assert_eq!(after(IC977_BERN), [16]);
}

#[test]
fn restaurant_comes_from_the_car_type_only() {
    let restaurant = |s| numbers_where(&wagons(s), |w| w.features.iter().any(|f| f == "restaurant"));
    // Car 14 of IC 708 is a restaurant open but not serving; VR is a bike offer.
    assert_eq!(restaurant(IC708_BERN), [4]);
    assert_eq!(restaurant(IC779_ZURICH), [15, 5]);
    assert_eq!(restaurant(IR2127_ZURICH), Vec::<u32>::new());
    assert_eq!(restaurant(IC977_BERN), [3]);
}

#[test]
fn class_stays_one_or_two_for_every_car_type() {
    let ws = wagons(IC779_ZURICH);
    let class_of = |n| ws.iter().find(|w| w.number == n).unwrap().class;
    assert_eq!(class_of(15), 1, "W1 is a first-class restaurant car");
    assert_eq!(class_of(17), 2);
    let ws = wagons(IC708_BERN);
    assert_eq!(ws.iter().find(|w| w.number == 18).unwrap().class, 2, "FA is a second-class family car");
    assert!(ws.iter().all(|w| w.class == 1 || w.class == 2));

    let mixed = wagons("@A12#NF,2#NF");
    assert_eq!((mixed[0].class, mixed[0].mixed_class), (1, true));
    assert_eq!((mixed[1].class, mixed[1].mixed_class), (2, false));
}

#[test]
fn features_use_only_known_strings_with_the_old_ones_first() {
    for short in [IC708_BERN, IC779_ZURICH, IR2127_ZURICH, IC977_BERN] {
        for w in wagons(short) {
            for f in &w.features {
                assert!(KNOWN_FEATURES.contains(&f.as_str()), "unexpected feature {f} on car {}", w.number);
            }
            if let Some(bike) = w.features.iter().position(|f| f == "bike") {
                assert_eq!(bike, w.features.len() - 1, "bike comes after the older features on car {}", w.number);
            }
        }
    }
    let ws = wagons(IC708_BERN);
    let car = |n| ws.iter().find(|w| w.number == n).unwrap();
    assert_eq!(car(18).features, ["family", "low_floor"]);
    assert_eq!(car(11).features, ["business", "low_floor", "bike"]);
    assert!(car(11).bike_reservation);
    assert_eq!(car(4).features, ["wheelchair", "restaurant", "low_floor"]);
    let ws = wagons(IR2127_ZURICH);
    let nine = ws.iter().find(|w| w.number == 9).unwrap();
    assert_eq!(nine.features, ["family", "low_floor", "bike"]);
    assert!(!nine.bike_reservation, "VH needs no reservation");
}

#[test]
fn closed_comes_from_the_dash_and_not_from_the_restaurant_marker() {
    let ws = wagons("@A[(2:3,-2:2,2):1]");
    assert_eq!(numbers_where(&ws, |w| w.closed), [2]);
    let ws = wagons(IC708_BERN);
    let fourteen = ws.iter().find(|w| w.number == 14).unwrap();
    assert!(!fourteen.closed);
    assert!(!fourteen.features.iter().any(|f| f == "restaurant"));
}

#[test]
fn cars_without_a_number_fall_back_to_position_and_say_so() {
    let ws = wagons("@A1,1,2,2,@B2");
    assert_eq!(numbers(&ws), [1, 2, 3, 4, 5]);
    assert!(ws.iter().all(|w| w.number_unknown));
    assert!(wagons(IC708_BERN).iter().all(|w| !w.number_unknown));
}

#[test]
fn a_plain_car_serialises_with_no_new_keys() {
    let ws = wagons("@A2:1");
    let json = serde_json::to_value(&ws[0]).unwrap();
    let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["class", "features", "number", "position", "sector"]);

    let ws = wagons(IC708_BERN);
    let json = serde_json::to_value(ws.iter().find(|w| w.number == 11).unwrap()).unwrap();
    assert_eq!(json["noPassageAfter"], true);
    assert_eq!(json["bikeReservation"], true);
}

#[test]
fn a_narrow_gauge_unit_keeps_its_own_numbers() {
    let ws = wagons("@D,F,F,F,F,F,F@C,[(1:711#VH;NF,2:712#VH;NF,2:713#BHP;VH;NF,2):714]@B,F,F,F@A,F,F,F,F");
    assert_eq!(numbers(&ws), [711, 712, 713, 714]);
    assert!(ws.iter().all(|w| !w.no_passage_after));
    assert_eq!(ws[0].class, 1);
}
