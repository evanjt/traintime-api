use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};

use traintime_core::CacheStatus;

use crate::cache::{Cache, STALE_MAX_AGE};

/// What a route got back for a cache key.
pub enum Fetched {
    /// Served from the cache, fetched by an earlier request.
    Cached(String),
    /// Fetched upstream by this request.
    Fresh(String),
    /// Upstream failed, this is the last good payload and its age in seconds.
    Stale(String, u64),
    Failed(String),
}

impl Fetched {
    pub fn cache_status(&self) -> CacheStatus {
        match self {
            Fetched::Cached(_) | Fetched::Stale(_, _) => CacheStatus::Hit,
            Fetched::Fresh(_) | Fetched::Failed(_) => CacheStatus::Miss,
        }
    }
}

/// One upstream call per cache key per pod. Concurrent misses wait for the
/// first fetch and then read what it cached.
#[derive(Clone, Default)]
pub struct Inflight {
    keys: Arc<Mutex<HashMap<String, Arc<Notify>>>>,
}

impl Inflight {
    pub async fn cached<F, Fut>(&self, cache: &Cache, key: &str, ttl: u64, fetch: F) -> Fetched
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<String, String>>,
    {
        if let Some(v) = cache.get(key) {
            return Fetched::Cached(v);
        }
        let mut keys = self.keys.lock().await;
        if let Some(notify) = keys.get(key).cloned() {
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            drop(keys);
            notified.await;
            return from_cache(cache, key, "upstream unavailable".to_string());
        }
        let notify = Arc::new(Notify::new());
        keys.insert(key.to_string(), notify.clone());
        drop(keys);
        println!("FETCH {key}");
        let result = fetch().await;
        if let Ok(v) = &result {
            cache.put(key, v, ttl);
        }
        self.keys.lock().await.remove(key);
        notify.notify_waiters();
        match result {
            Ok(v) => Fetched::Fresh(v),
            Err(e) => from_cache(cache, key, e),
        }
    }
}

fn from_cache(cache: &Cache, key: &str, error: String) -> Fetched {
    if let Some(v) = cache.get(key) {
        return Fetched::Cached(v);
    }
    match cache.get_stale(key, STALE_MAX_AGE) {
        Some((v, age)) => Fetched::Stale(v, age),
        None => Fetched::Failed(error),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn concurrent_misses_fetch_once() {
        let cache = Cache::default();
        let inflight = Inflight::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let tasks: Vec<_> = (0..20)
            .map(|_| {
                let (cache, inflight, calls) = (cache.clone(), inflight.clone(), calls.clone());
                tokio::spawn(async move {
                    inflight
                        .cached(&cache, "k", 60, || async {
                            calls.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                            Ok("v".to_string())
                        })
                        .await
                })
            })
            .collect();
        for t in tasks {
            assert!(matches!(t.await.unwrap(), Fetched::Fresh(v) | Fetched::Cached(v) if v == "v"));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_fetch_serves_stale_then_fails() {
        let cache = Cache::default();
        let inflight = Inflight::default();
        cache.put("k", "old", 0);
        let r = inflight.cached(&cache, "k", 60, || async { Err("boom".to_string()) }).await;
        assert!(matches!(r, Fetched::Stale(v, 0) if v == "old"));
        let r = inflight.cached(&cache, "none", 60, || async { Err("boom".to_string()) }).await;
        assert!(matches!(r, Fetched::Failed(e) if e == "boom"));
    }
}
