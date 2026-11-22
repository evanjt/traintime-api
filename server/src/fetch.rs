use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};

use traintime_core::{CacheStatus, UpstreamError};

use crate::cache::{Cache, STALE_MAX_AGE};

/// What a route got back for a cache key.
pub enum Fetched<E> {
    /// Served from the cache, fetched by an earlier request.
    Cached(String),
    /// Fetched upstream by this request.
    Fresh(String),
    /// Upstream failed, this is the last good payload and its age in seconds.
    Stale(String, u64),
    Failed(E),
}

impl<E> Fetched<E> {
    pub fn cache_status(&self) -> CacheStatus {
        match self {
            Fetched::Cached(_) | Fetched::Stale(_, _) => CacheStatus::Hit,
            Fetched::Fresh(_) | Fetched::Failed(_) => CacheStatus::Miss,
        }
    }
}

/// A fetch in flight. Waiters read the leader's error from here, so a
/// concurrent request fails the same way the one that fetched did.
struct Slot<E> {
    done: Notify,
    failure: std::sync::Mutex<Option<E>>,
}

/// One upstream call per cache key per pod. Across pods the shared cache
/// catches the second miss once the first fetch has landed. Concurrent misses wait for the
/// first fetch and then read what it cached.
#[derive(Clone)]
pub struct Inflight<E> {
    keys: Arc<Mutex<HashMap<String, Arc<Slot<E>>>>>,
}

impl<E> Default for Inflight<E> {
    fn default() -> Self {
        Inflight { keys: Default::default() }
    }
}

impl<E: Clone + From<UpstreamError>> Inflight<E> {
    pub async fn cached<F, Fut>(&self, cache: &Cache, key: &str, ttl: u64, fetch: F) -> Fetched<E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<String, E>>,
    {
        if let Some(v) = cache.lookup(key).await {
            return Fetched::Cached(v);
        }
        let mut keys = self.keys.lock().await;
        if let Some(slot) = keys.get(key).cloned() {
            let notified = slot.done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            drop(keys);
            notified.await;
            let failure = slot.failure.lock().unwrap().clone().unwrap_or_else(|| {
                UpstreamError::Unreachable("concurrent fetch left nothing cached".to_string()).into()
            });
            return from_cache(cache, key, failure);
        }
        let slot = Arc::new(Slot { done: Notify::new(), failure: Default::default() });
        keys.insert(key.to_string(), slot.clone());
        drop(keys);
        println!("FETCH {key}");
        let result = fetch().await;
        match &result {
            Ok(v) => cache.store(key, v, ttl).await,
            Err(e) => *slot.failure.lock().unwrap() = Some(e.clone()),
        }
        self.keys.lock().await.remove(key);
        slot.done.notify_waiters();
        match result {
            Ok(v) => Fetched::Fresh(v),
            Err(e) => from_cache(cache, key, e),
        }
    }
}

fn from_cache<E>(cache: &Cache, key: &str, error: E) -> Fetched<E> {
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
    use crate::memcached::Memcached;

    type Err = UpstreamError;

    #[tokio::test]
    async fn concurrent_misses_fetch_once() {
        let cache = Cache::default();
        let inflight = Inflight::<Err>::default();
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

    // Scenario: two replicas with their own in-pod caches share a Memcached.
    // Expected behaviour: the second pod serves the first pod's fetch and
    // never calls upstream itself.
    #[tokio::test]
    async fn a_miss_on_one_pod_is_a_hit_on_the_other() {
        let server = crate::memcached::fake::start(false).await;
        let pod_a = Cache::with_shared(Memcached::new(&server.addr));
        let pod_b = Cache::with_shared(Memcached::new(&server.addr));
        let r = Inflight::<Err>::default()
            .cached(&pod_a, "k", 60, || async { Ok("v".to_string()) })
            .await;
        assert!(matches!(r, Fetched::Fresh(v) if v == "v"));
        let r = Inflight::<Err>::default()
            .cached(&pod_b, "k", 60, || async { panic!("upstream called") })
            .await;
        assert!(matches!(r, Fetched::Cached(v) if v == "v"));
        assert_eq!(pod_b.get("k"), None);
    }

    // Scenario: MEMCACHED_URL points at nothing that answers.
    // Expected behaviour: the fetch path works as without a shared cache.
    #[tokio::test]
    async fn unreachable_shared_cache_changes_nothing() {
        let cache = Cache::with_shared(Memcached::new("127.0.0.1:1"));
        let inflight = Inflight::<Err>::default();
        let r = inflight
            .cached(&cache, "k", 60, || async { Ok("v".to_string()) })
            .await;
        assert!(matches!(r, Fetched::Fresh(v) if v == "v"));
        let r = inflight
            .cached(&cache, "k", 60, || async { panic!("upstream called") })
            .await;
        assert!(matches!(r, Fetched::Cached(v) if v == "v"));
    }

    #[tokio::test]
    async fn failed_fetch_serves_stale_then_fails() {
        let cache = Cache::default();
        let inflight = Inflight::<Err>::default();
        cache.put("k", "old", 0);
        let r = inflight
            .cached(&cache, "k", 60, || async { Err(UpstreamError::Status(502)) })
            .await;
        assert!(matches!(r, Fetched::Stale(v, 0) if v == "old"));
        let r = inflight
            .cached(&cache, "none", 60, || async { Err(UpstreamError::Status(502)) })
            .await;
        assert!(matches!(r, Fetched::Failed(UpstreamError::Status(502))));
    }

    // Scenario: a second request for the same key arrives while the first
    // fetch is still waiting on a failing upstream, with nothing cached.
    // Expected behaviour: both fail with the leader's error, not a generic one.
    #[tokio::test]
    async fn concurrent_waiters_fail_with_the_leaders_error() {
        let cache = Cache::default();
        let inflight = Inflight::<Err>::default();
        let tasks: Vec<_> = (0..5)
            .map(|_| {
                let (cache, inflight) = (cache.clone(), inflight.clone());
                tokio::spawn(async move {
                    inflight
                        .cached(&cache, "k", 60, || async {
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                            Err(UpstreamError::Timeout)
                        })
                        .await
                })
            })
            .collect();
        for t in tasks {
            assert!(matches!(t.await.unwrap(), Fetched::Failed(UpstreamError::Timeout)));
        }
    }
}
