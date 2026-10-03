use worker::{Cache, Headers, Response};

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

/// The Cache API keys on a URL. A host on our own zone keeps the entry ours,
/// and one fixed host lets api and api2 share it.
fn cache_url(key: &str) -> String {
    let key = String::from(js_sys::encode_uri_component(key));
    format!("https://api.traintime.ch/__cache/{key}")
}

/// The edge cache of the data centre serving the request. KV cost one write
/// per miss against a free-plan allowance of 1,000 a day shared with every
/// Worker on the account; the Cache API has no operation limits. It is per data
/// centre and may evict early, which for a Swiss audience means Zürich and
/// Geneva each fetching a station at most once a TTL.
pub async fn get(key: &str, ttl_secs: u64) -> Cached {
    let Ok(Some(mut resp)) = Cache::default().get(cache_url(key), false).await else {
        return Cached::Missing;
    };
    let Ok(raw) = resp.text().await else {
        return Cached::Missing;
    };
    let Ok(entry) = serde_json::from_str::<CachedPayload>(&raw) else {
        return Cached::Missing;
    };
    match classify(entry.fetched_at, now_ms(), ttl_secs) {
        Freshness::Fresh => Cached::Fresh(entry.payload),
        Freshness::Stale(age) => Cached::Stale(entry.payload, age),
        Freshness::Expired => Cached::Missing,
    }
}

/// The entry is kept for the stale window, freshness is judged on read. A
/// failed put is ignored and the next request simply fetches again. The Cache
/// API is a no-op outside a custom domain, so this also holds under
/// `wrangler dev` without a zone.
pub async fn put(key: &str, payload: &str, ttl_secs: u64) {
    let entry = CachedPayload {
        fetched_at: now_ms(),
        payload: payload.to_string(),
    };
    let Ok(raw) = serde_json::to_string(&entry) else {
        return;
    };
    let headers = Headers::new();
    let max_age = ttl_secs.max(STALE_MAX_AGE_SECS);
    if headers.set("Cache-Control", &format!("max-age={max_age}")).is_err() {
        return;
    }
    let Ok(resp) = Response::ok(raw) else {
        return;
    };
    let _ = Cache::default()
        .put(cache_url(key), resp.with_headers(headers))
        .await;
}
