use worker::kv::KvStore;

use traintime_core::{classify, CachedPayload, Freshness, STALE_MAX_AGE_SECS};

pub enum Cached {
    Fresh(String),
    /// Past its TTL, with the age in seconds.
    Stale(String, u64),
    Missing,
}

fn now_ms() -> u64 {
    js_sys::Date::new_0().get_time() as u64
}

/// A value written before the envelope existed is served as fresh; it expires
/// from KV on its own within the old TTL.
pub async fn get(kv: &KvStore, key: &str, ttl_secs: u64) -> Cached {
    let Ok(Some(raw)) = kv.get(key).text().await else {
        return Cached::Missing;
    };
    let Ok(entry) = serde_json::from_str::<CachedPayload>(&raw) else {
        return Cached::Fresh(raw);
    };
    match classify(entry.fetched_at, now_ms(), ttl_secs) {
        Freshness::Fresh => Cached::Fresh(entry.payload),
        Freshness::Stale(age) => Cached::Stale(entry.payload, age),
        Freshness::Expired => Cached::Missing,
    }
}

/// KV keeps the entry for the stale window, freshness is judged on read. A
/// failed write is ignored: on the free plan KV writes run out and caching
/// simply stops.
pub async fn put(kv: &KvStore, key: &str, payload: &str, ttl_secs: u64) {
    let entry = CachedPayload {
        fetched_at: now_ms(),
        payload: payload.to_string(),
    };
    let Ok(raw) = serde_json::to_string(&entry) else {
        return;
    };
    let Ok(put) = kv.put(key, &raw) else {
        return;
    };
    let _ = put
        .expiration_ttl(ttl_secs.max(STALE_MAX_AGE_SECS))
        .execute()
        .await;
}
