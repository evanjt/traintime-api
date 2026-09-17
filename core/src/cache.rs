//! Freshness of a cached payload judged from when it was fetched, so an entry
//! can outlive its TTL and still be served when upstream is down.

use serde::{Deserialize, Serialize};

/// How long past its TTL an entry may still be served on upstream failure.
pub const STALE_MAX_AGE_SECS: u64 = 300;

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

pub fn classify(fetched_at_ms: u64, now_ms: u64, ttl_secs: u64) -> Freshness {
    let age_secs = now_ms.saturating_sub(fetched_at_ms) / 1000;
    if age_secs < ttl_secs {
        Freshness::Fresh
    } else if age_secs <= STALE_MAX_AGE_SECS {
        Freshness::Stale(age_secs)
    } else {
        Freshness::Expired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_fresh_inside_ttl() {
        assert_eq!(classify(0, 59_999, 60), Freshness::Fresh);
        assert_eq!(classify(0, 0, 60), Freshness::Fresh);
    }

    #[test]
    fn test_classify_stale_between_ttl_and_max_age() {
        assert_eq!(classify(0, 60_000, 60), Freshness::Stale(60));
        assert_eq!(classify(0, 300_999, 60), Freshness::Stale(300));
    }

    #[test]
    fn test_classify_expired_past_max_age() {
        assert_eq!(classify(0, 301_000, 60), Freshness::Expired);
    }

    #[test]
    fn test_classify_clock_skew_is_fresh() {
        // A fetched_at in the future counts as age zero.
        assert_eq!(classify(10_000, 5_000, 60), Freshness::Fresh);
    }
}
