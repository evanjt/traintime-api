//! Freshness of a cached payload judged from when it was fetched, so an entry
//! can outlive its TTL and still be served when upstream is down.

use serde::{Deserialize, Serialize};

use crate::ojp::FlatDeparture;

/// How long after its fetch an entry may still be served on upstream failure.
pub const STALE_MAX_AGE_SECS: u64 = 300;

/// The same for a station's departures list, for a request that opts in with
/// `stale=1`. Through a longer outage most of its rows are still right, and the
/// response carries its age. A client that does not read the age would show the
/// list as live, so without the opt-in the list keeps the short window.
pub const DEPARTURES_STALE_MAX_AGE_SECS: u64 = 1800;

/// Whether the `stale` query value opts in to the long departures window.
pub fn stale_opt_in(value: Option<&str>) -> bool {
    value == Some("1")
}

/// How long after its fetch a departures list may be served on upstream
/// failure, given whether the request opted in.
pub fn departures_stale_window_secs(opt_in: bool) -> u64 {
    if opt_in {
        DEPARTURES_STALE_MAX_AGE_SECS
    } else {
        STALE_MAX_AGE_SECS
    }
}

/// The key prefix `/v1/departures` and `/v1/nearby` share for a station's list.
pub const DEPARTURES_KEY_PREFIX: &str = "departures:";

/// How long after its fetch the entry under `key` is kept, so it can be served
/// stale. A departures list is kept for the long window; which part of it a
/// request may be served is `departures_on_failure`'s decision.
pub fn stale_max_age_secs(key: &str) -> u64 {
    if key.starts_with(DEPARTURES_KEY_PREFIX) {
        DEPARTURES_STALE_MAX_AGE_SECS
    } else {
        STALE_MAX_AGE_SECS
    }
}

/// What a store keeps per key: the payload and the fetch time in unix ms.
#[derive(Serialize, Deserialize)]
pub struct CachedPayload {
    pub fetched_at: u64,
    pub payload: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    /// Past its TTL, with the age in seconds.
    Stale(u64),
    Expired,
}

pub fn classify(fetched_at_ms: u64, now_ms: u64, ttl_secs: u64, stale_max_secs: u64) -> Freshness {
    let age_secs = now_ms.saturating_sub(fetched_at_ms) / 1000;
    if age_secs < ttl_secs {
        Freshness::Fresh
    } else if age_secs <= stale_max_secs {
        Freshness::Stale(age_secs)
    } else {
        Freshness::Expired
    }
}

/// A station's last list served in place of an upstream that failed.
pub struct StaleDepartures {
    pub departures: Vec<FlatDeparture>,
    /// When the list was fetched, in unix seconds, set only when it is past the
    /// short window and so must be shown with its age. Inside the short window
    /// it is served as it was before the long window existed, unmarked.
    pub fetched_at: Option<i64>,
}

/// What a departures request serves when upstream failed and the cache holds
/// `payload`, fetched `age_secs` ago: its rows not yet gone, while it is inside
/// the window the request is allowed (`opt_in` for the long one) and any are
/// left. `None` is the upstream error.
pub fn departures_on_failure(payload: &str, age_secs: u64, now_secs: i64, opt_in: bool) -> Option<StaleDepartures> {
    if age_secs > departures_stale_window_secs(opt_in) {
        return None;
    }
    let departures: Vec<FlatDeparture> = serde_json::from_str(payload).ok()?;
    let departures = crate::ojp::drop_departed(departures, now_secs);
    if departures.is_empty() {
        return None;
    }
    let fetched_at = (age_secs > STALE_MAX_AGE_SECS).then(|| now_secs - age_secs as i64);
    Some(StaleDepartures { departures, fetched_at })
}

/// The `/v1/departures` body: the first `limit` rows, the favourites split
/// out first when any are asked for, and when served stale, `stale` and the
/// list's `fetchedAt` in unix seconds.
pub fn departures_body(
    departures: Vec<FlatDeparture>,
    favourites: &[(String, String)],
    limit: usize,
    fetched_at: Option<i64>,
) -> serde_json::Value {
    let mut body = if favourites.is_empty() {
        let mut departures = departures;
        departures.truncate(limit);
        serde_json::json!({ "departures": departures })
    } else {
        let (favs, deps) = crate::favourites::partition_favourites(&departures, favourites, limit);
        serde_json::json!({ "favourites": favs, "departures": deps })
    };
    mark_stale(&mut body, fetched_at);
    body
}

/// Marks a response body that carries a list past the short window with
/// `stale` and the list's `fetchedAt` in unix seconds. `None` leaves it as is.
pub fn mark_stale(body: &mut serde_json::Value, fetched_at: Option<i64>) {
    if let Some(fetched_at) = fetched_at {
        body["stale"] = serde_json::json!(true);
        body["fetchedAt"] = serde_json::json!(fetched_at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_fresh_inside_ttl() {
        assert_eq!(classify(0, 59_999, 60, STALE_MAX_AGE_SECS), Freshness::Fresh);
        assert_eq!(classify(0, 0, 60, STALE_MAX_AGE_SECS), Freshness::Fresh);
    }

    #[test]
    fn test_classify_stale_between_ttl_and_max_age() {
        assert_eq!(classify(0, 60_000, 60, STALE_MAX_AGE_SECS), Freshness::Stale(60));
        assert_eq!(classify(0, 300_999, 60, STALE_MAX_AGE_SECS), Freshness::Stale(300));
    }

    #[test]
    fn test_classify_expired_past_max_age() {
        assert_eq!(classify(0, 301_000, 60, STALE_MAX_AGE_SECS), Freshness::Expired);
    }

    fn row(to: &str, departure: i64, delay: Option<i64>) -> FlatDeparture {
        FlatDeparture {
            to: to.into(),
            category: "IR".into(),
            number: "15".into(),
            departure,
            delay,
            platform: "3".into(),
            platform_changed: false,
            train_number: None,
            operator_ref: None,
            cancelled: false,
        }
    }

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn test_departures_window_is_longer_than_other_keys() {
        assert_eq!(stale_max_age_secs("departures:8507000:50"), DEPARTURES_STALE_MAX_AGE_SECS);
        assert_eq!(stale_max_age_secs("formation:11:2026-11-25:15:8507000"), STALE_MAX_AGE_SECS);
        assert_eq!(classify(0, 1_200_000, 60, stale_max_age_secs("departures:1:50")), Freshness::Stale(1200));
        assert_eq!(classify(0, 2_400_000, 60, stale_max_age_secs("departures:1:50")), Freshness::Expired);
    }

    #[test]
    fn test_twenty_minute_old_list_is_served_stale_without_departed_rows() {
        let list = serde_json::to_string(&vec![
            row("Gone", NOW - 600, None),
            row("Late but due", NOW - 120, Some(5)),
            row("Next", NOW + 300, None),
        ])
        .unwrap();

        let stale = departures_on_failure(&list, 20 * 60, NOW, true).expect("servable stale");

        assert_eq!(stale.fetched_at, Some(NOW - 20 * 60));
        let to: Vec<&str> = stale.departures.iter().map(|d| d.to.as_str()).collect();
        assert_eq!(to, vec!["Late but due", "Next"]);
    }

    #[test]
    fn test_twenty_minute_old_list_is_the_upstream_error_without_the_opt_in() {
        let list = serde_json::to_string(&vec![row("Next", NOW + 300, None)]).unwrap();
        assert!(departures_on_failure(&list, 20 * 60, NOW, false).is_none());
        let stale = departures_on_failure(&list, 20 * 60, NOW, true).expect("servable with the opt-in");
        assert_eq!(stale.fetched_at, Some(NOW - 20 * 60));
    }

    #[test]
    fn test_four_minute_old_list_is_served_without_stale_fields_either_way() {
        let list = serde_json::to_string(&vec![row("Next", NOW + 300, None)]).unwrap();
        for opt_in in [false, true] {
            let stale = departures_on_failure(&list, 4 * 60, NOW, opt_in).expect("inside the short window");
            assert_eq!(stale.fetched_at, None, "opt_in {opt_in}");
            let body = departures_body(stale.departures, &[], 5, stale.fetched_at);
            assert!(body.get("stale").is_none() && body.get("fetchedAt").is_none());
        }
    }

    #[test]
    fn test_only_the_value_one_opts_in() {
        assert!(stale_opt_in(Some("1")));
        assert!(!stale_opt_in(None));
        assert!(!stale_opt_in(Some("0")));
        assert!(!stale_opt_in(Some("true")));
    }

    #[test]
    fn test_forty_minute_old_list_is_not_served() {
        let list = serde_json::to_string(&vec![row("Next", NOW + 300, None)]).unwrap();
        assert!(departures_on_failure(&list, 40 * 60, NOW, true).is_none());
    }

    #[test]
    fn test_list_with_every_row_departed_is_not_served() {
        let list = serde_json::to_string(&vec![row("Gone", NOW - 600, None)]).unwrap();
        assert!(departures_on_failure(&list, 60, NOW, true).is_none());
    }

    #[test]
    fn test_stale_body_carries_its_fetch_time() {
        let body = departures_body(vec![row("Next", NOW + 300, None)], &[], 5, Some(NOW - 1200));
        assert_eq!(body["stale"], true);
        assert_eq!(body["fetchedAt"], NOW - 1200);
        assert_eq!(body["departures"][0]["to"], "Next");
        let fresh = departures_body(vec![row("Next", NOW + 300, None)], &[], 5, None);
        assert!(fresh.get("stale").is_none() && fresh.get("fetchedAt").is_none());
    }

    #[test]
    fn test_classify_clock_skew_is_fresh() {
        // A fetched_at in the future counts as age zero.
        assert_eq!(classify(10_000, 5_000, 60, STALE_MAX_AGE_SECS), Freshness::Fresh);
    }
}
