use std::fmt;

/// The error code clients read to tell an upstream outage from a fault of
/// ours. Load balancers also answer 502 and 503 when our own hosts are down,
/// so the status alone cannot say which side failed.
pub const UPSTREAM_UNAVAILABLE: &str = "upstream_unavailable";

/// Seconds a client should wait before asking again. Upstream outages last
/// minutes, and every retry spends the shared per-key quota.
pub const RETRY_AFTER_SECS: u64 = 30;

/// Why OJP or the formation service gave no usable answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpstreamError {
    Status(u16),
    Timeout,
    Unreachable(String),
    Unparseable(String),
}

/// What a route answers for a failed request, in terms both builds map onto
/// their own HTTP types.
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorResponse {
    pub status: u16,
    pub retry_after: Option<u64>,
    pub body: serde_json::Value,
}

impl UpstreamError {
    /// Anything outside 2xx is an upstream failure.
    pub fn check_status(status: u16) -> Result<(), UpstreamError> {
        if (200..300).contains(&status) {
            Ok(())
        } else {
            Err(UpstreamError::Status(status))
        }
    }

    /// Every upstream failure is a 503. A 500 is kept for our own faults.
    pub fn response(&self) -> ErrorResponse {
        ErrorResponse {
            status: 503,
            retry_after: Some(RETRY_AFTER_SECS),
            body: serde_json::json!({ "error": UPSTREAM_UNAVAILABLE }),
        }
    }
}

impl fmt::Display for UpstreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpstreamError::Status(status) => write!(f, "upstream answered {status}"),
            UpstreamError::Timeout => write!(f, "upstream timed out"),
            UpstreamError::Unreachable(e) => write!(f, "upstream unreachable: {e}"),
            UpstreamError::Unparseable(e) => write!(f, "upstream body unparseable: {e}"),
        }
    }
}

impl std::error::Error for UpstreamError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_failure() -> Vec<UpstreamError> {
        vec![
            UpstreamError::Status(502),
            UpstreamError::Status(429),
            UpstreamError::Status(404),
            UpstreamError::Timeout,
            UpstreamError::Unreachable("connection refused".into()),
            UpstreamError::Unparseable("expected value at line 1".into()),
        ]
    }

    #[test]
    fn every_upstream_failure_is_503_upstream_unavailable() {
        for e in every_failure() {
            let r = e.response();
            assert_eq!(r.status, 503, "{e}");
            assert_eq!(r.retry_after, Some(30), "{e}");
            assert_eq!(r.body, serde_json::json!({ "error": "upstream_unavailable" }), "{e}");
        }
    }

    #[test]
    fn only_2xx_passes_the_status_check() {
        assert_eq!(UpstreamError::check_status(200), Ok(()));
        assert_eq!(UpstreamError::check_status(204), Ok(()));
        assert_eq!(UpstreamError::check_status(199), Err(UpstreamError::Status(199)));
        assert_eq!(UpstreamError::check_status(301), Err(UpstreamError::Status(301)));
        assert_eq!(UpstreamError::check_status(502), Err(UpstreamError::Status(502)));
    }
}
