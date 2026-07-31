use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Entries above this count trigger an expiry sweep on the next insert.
/// KV dropped expired keys for us; a HashMap does not, and formation keys
/// (`formation:{evu}:{date}:{train}:{stop}`) are high cardinality.
const SWEEP_THRESHOLD: usize = 4096;

/// Per-pod stand-in for the Workers KV binding. Same get/put-with-TTL surface.
#[derive(Clone, Default)]
pub struct Cache {
    inner: Arc<Mutex<HashMap<String, (String, Instant)>>>,
}

impl Cache {
    pub fn get(&self, key: &str) -> Option<String> {
        let mut map = self.inner.lock().unwrap();
        match map.get(key) {
            Some((value, expiry)) if *expiry > Instant::now() => Some(value.clone()),
            Some(_) => {
                map.remove(key);
                None
            }
            None => None,
        }
    }

    pub fn put(&self, key: &str, value: &str, ttl_secs: u64) {
        let mut map = self.inner.lock().unwrap();

        if map.len() > SWEEP_THRESHOLD {
            let now = Instant::now();
            map.retain(|_, (_, expiry)| *expiry > now);
        }

        map.insert(
            key.to_string(),
            (
                value.to_string(),
                Instant::now() + Duration::from_secs(ttl_secs),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_entries_are_not_returned() {
        let cache = Cache::default();
        cache.put("k", "v", 0);
        assert_eq!(cache.get("k"), None);
    }

    #[test]
    fn live_entries_round_trip() {
        let cache = Cache::default();
        cache.put("k", "v", 60);
        assert_eq!(cache.get("k").as_deref(), Some("v"));
    }

    #[test]
    fn sweep_drops_expired_entries_once_over_threshold() {
        let cache = Cache::default();
        for i in 0..=SWEEP_THRESHOLD {
            cache.put(&format!("stale{i}"), "v", 0);
        }
        cache.put("fresh", "v", 60);

        let len = cache.inner.lock().unwrap().len();
        assert_eq!(len, 1, "sweep should leave only the live entry");
    }
}
