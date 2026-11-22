use std::future::Future;
use std::time::Duration;

use futures::future::{select, Either};
use traintime_core::UpstreamError;
use worker::Delay;

const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(10);

/// The runtime never abandons a hung upstream call on its own. Give up after
/// ten seconds so the caller can fall back to the stale cache.
pub async fn timed<T>(fut: impl Future<Output = worker::Result<T>>) -> Result<T, UpstreamError> {
    futures::pin_mut!(fut);
    match select(fut, Delay::from(UPSTREAM_TIMEOUT)).await {
        Either::Left((result, _)) => result.map_err(request_failed),
        Either::Right(_) => Err(UpstreamError::Timeout),
    }
}

/// A request that could not be built or sent, or whose body could not be read.
pub fn request_failed(e: worker::Error) -> UpstreamError {
    UpstreamError::Unreachable(e.to_string())
}
