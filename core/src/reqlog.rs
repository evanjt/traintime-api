//! The one request line both builds log. It carries nothing that identifies
//! a person: no IP, no user agent, no header, and never the query string,
//! because `lat` and `lon` live there and the privacy page promises they are
//! not logged.

/// Whether the handler served the cached upstream answer. Set by the handler
/// on its response extensions, read by the logging middleware.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CacheStatus {
    #[default]
    None,
    Hit,
    Miss,
}

impl CacheStatus {
    fn as_str(self) -> &'static str {
        match self {
            CacheStatus::None => "-",
            CacheStatus::Hit => "hit",
            CacheStatus::Miss => "miss",
        }
    }
}

/// The route template for a path. Anything not served is "other", so an
/// unknown path a scanner tries never lands in the log verbatim.
pub fn route_template(path: &str) -> &'static str {
    match path {
        "/health" => "/health",
        "/v1/nearby" => "/v1/nearby",
        "/v1/departures" => "/v1/departures",
        "/v1/formation" => "/v1/formation",
        _ => "other",
    }
}

/// `key` is the four-character label from `auth::label`, or None when the
/// request carried no valid key.
pub fn format_line(
    method: &str,
    path: &str,
    status: u16,
    ms: u64,
    cache: CacheStatus,
    key: Option<&str>,
) -> String {
    format!(
        "req method={method} route={} status={status} ms={ms} cache={} key={}",
        route_template(path),
        cache.as_str(),
        key.unwrap_or("-"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_paths_map_to_their_template() {
        assert_eq!(route_template("/v1/nearby"), "/v1/nearby");
        assert_eq!(route_template("/health"), "/health");
    }

    #[test]
    fn unknown_paths_are_other() {
        assert_eq!(route_template("/v1/nearby/../etc/passwd"), "other");
        assert_eq!(route_template("/admin"), "other");
    }

    #[test]
    fn line_has_every_field_and_no_query() {
        let line = format_line("GET", "/v1/nearby", 200, 412, CacheStatus::Hit, Some("and-"));
        assert_eq!(
            line,
            "req method=GET route=/v1/nearby status=200 ms=412 cache=hit key=and-"
        );
        assert!(!line.contains("lat="));
    }

    #[test]
    fn missing_key_and_cache_print_as_dash() {
        let line = format_line("GET", "/v1/x", 401, 1, CacheStatus::None, None);
        assert_eq!(line, "req method=GET route=other status=401 ms=1 cache=- key=-");
    }
}
