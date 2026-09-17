use std::future::Future;
use std::time::Duration;

use futures::future::{select, Either};
use worker::Delay;

const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(10);

/// The runtime never abandons a hung upstream call on its own. Give up after
/// ten seconds so the caller can fall back to the stale cache.
pub async fn timed<T>(name: &str, fut: impl Future<Output = worker::Result<T>>) -> worker::Result<T> {
    futures::pin_mut!(fut);
    match select(fut, Delay::from(UPSTREAM_TIMEOUT)).await {
        Either::Left((result, _)) => result,
        Either::Right(_) => Err(worker::Error::RustError(format!(
            "{name} timed out after {}s",
            UPSTREAM_TIMEOUT.as_secs()
        ))),
    }
}
