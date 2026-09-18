use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::memcached::Memcached;

/// Entries above this count trigger an expiry sweep on the next insert.
/// KV dropped expired keys for us; a HashMap does not, and formation keys
/// (`formation:{evu}:{date}:{train}:{stop}`) are high cardinality.
const SWEEP_THRESHOLD: usize = 4096;

/// How long an expired entry is kept so it can be served when upstream fails.
pub const STALE_MAX_AGE: Duration = Duration::from_secs(300);

struct Entry {
    value: String,
    expiry: Instant,
    fetched_at: Instant,
}

/// Per-pod stand-in for the Workers KV binding. Same get/put-with-TTL surface,
/// plus expired entries linger for `STALE_MAX_AGE` behind `get_stale`.
/// With a shared Memcached the replicas see each other's fetches; without
/// one this is exactly the per-pod cache.
#[derive(Clone, Default)]
pub struct Cache {
    inner: Arc<Mutex<HashMap<String, Entry>>>,
    shared: Option<Memcached>,
}

impl Cache {
    pub fn with_shared(shared: Memcached) -> Self {
        Self {
            inner: Default::default(),
            shared: Some(shared),
        }
    }

    /// The shared cache first, then this pod's own. Memcached answers inside
    /// its timeout or not at all, so this never fails, only misses.
    pub async fn lookup(&self, key: &str) -> Option<String> {
        if let Some(shared) = &self.shared {
            if let Some(v) = shared.get(key).await {
                return Some(v);
            }
        }
        self.get(key)
    }

    /// Writes this pod's cache and, when configured, the shared one with the
    /// same TTL.
    pub async fn store(&self, key: &str, value: &str, ttl_secs: u64) {
        self.put(key, value, ttl_secs);
        if let Some(shared) = &self.shared {
            shared.set(key, value, ttl_secs).await;
        }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let map = self.inner.lock().unwrap();
        match map.get(key) {
            Some(e) if e.expiry > Instant::now() => Some(e.value.clone()),
            _ => None,
        }
    }

    /// An entry past its TTL but fetched within `max_age`, with its age in
    /// seconds. Fresh entries qualify too, so callers check `get` first.
    pub fn get_stale(&self, key: &str, max_age: Duration) -> Option<(String, u64)> {
        let map = self.inner.lock().unwrap();
        let e = map.get(key)?;
        let age = e.fetched_at.elapsed();
        if age > max_age {
            return None;
        }
        Some((e.value.clone(), age.as_secs()))
    }

    pub fn put(&self, key: &str, value: &str, ttl_secs: u64) {
        self.put_at(key, value, ttl_secs, Instant::now());
    }

    fn put_at(&self, key: &str, value: &str, ttl_secs: u64, fetched_at: Instant) {
        let mut map = self.inner.lock().unwrap();
        if map.len() > SWEEP_THRESHOLD {
            let now = Instant::now();
            map.retain(|_, e| e.fetched_at + STALE_MAX_AGE > now);
        }
        map.insert(
            key.to_string(),
            Entry {
                value: value.to_string(),
                expiry: fetched_at + Duration::from_secs(ttl_secs),
                fetched_at,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_aged(cache: &Cache, key: &str, age: Duration) {
        cache.put_at(key, "v", 0, Instant::now() - age);
    }

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
    fn stale_just_under_max_age_is_served_with_its_age() {
        let cache = Cache::default();
        put_aged(&cache, "k", Duration::from_secs(299));
        assert_eq!(cache.get("k"), None);
        let (value, age) = cache.get_stale("k", STALE_MAX_AGE).unwrap();
        assert_eq!(value, "v");
        assert_eq!(age, 299);
    }

    #[test]
    fn stale_just_over_max_age_is_gone() {
        let cache = Cache::default();
        put_aged(&cache, "k", STALE_MAX_AGE + Duration::from_millis(10));
        assert_eq!(cache.get_stale("k", STALE_MAX_AGE), None);
    }

    #[test]
    fn sweep_keeps_stale_entries_inside_the_window() {
        let cache = Cache::default();
        for i in 0..=SWEEP_THRESHOLD {
            put_aged(
                &cache,
                &format!("old{i}"),
                STALE_MAX_AGE + Duration::from_secs(1),
            );
        }
        put_aged(&cache, "stale", Duration::from_secs(120));
        cache.put("fresh", "v", 60);
        let len = cache.inner.lock().unwrap().len();
        // "stale" and "fresh" survive, the expired batch does not.
        assert_eq!(len, 2);
    }
}
