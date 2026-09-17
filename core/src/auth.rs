//! Client API keys: a comma-separated list so one platform's key can be
//! rotated without touching the others, compared in constant time.

pub struct ApiKeys(Vec<String>);

impl ApiKeys {
    /// `list` is the `API_KEYS` value, `single` the legacy `API_KEY`. Blank
    /// entries are dropped, and zero usable keys is an error so a server never
    /// boots accepting the empty string.
    pub fn parse(list: Option<&str>, single: Option<&str>) -> Result<Self, &'static str> {
        let mut keys: Vec<String> = list
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(str::to_string)
            .collect();
        if keys.is_empty() {
            if let Some(k) = single.map(str::trim).filter(|k| !k.is_empty()) {
                keys.push(k.to_string());
            }
        }
        if keys.is_empty() {
            return Err("no API keys configured (API_KEYS or API_KEY)");
        }
        Ok(Self(keys))
    }

    /// Every key is checked so the response time does not depend on which key
    /// matched, or on how far into a key the first wrong byte sits.
    pub fn matched(&self, provided: &str) -> Option<&str> {
        let mut found: Option<&str> = None;
        for key in &self.0 {
            if constant_time_eq(key.as_bytes(), provided.as_bytes()) {
                found = Some(key.as_str());
            }
        }
        found
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// The first four characters of a key, the platform prefix on `ios-`/`and-`/
/// `gar-` keys, which is safe to log.
pub fn label(key: &str) -> &str {
    let end = key
        .char_indices()
        .nth(4)
        .map_or(key.len(), |(i, _)| i);
    &key[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_rejected() {
        assert!(ApiKeys::parse(None, None).is_err());
        assert!(ApiKeys::parse(Some(""), Some("")).is_err());
        assert!(ApiKeys::parse(Some(" , ,"), Some("  ")).is_err());
    }

    #[test]
    fn test_parse_drops_blank_entries() {
        let keys = ApiKeys::parse(Some(" a , ,b,, c "), None).unwrap();
        assert_eq!(keys.len(), 3);
        assert_eq!(keys.matched("a"), Some("a"));
        assert_eq!(keys.matched("c"), Some("c"));
        assert_eq!(keys.matched(" a "), None);
    }

    #[test]
    fn test_parse_falls_back_to_single() {
        let keys = ApiKeys::parse(None, Some(" legacy ")).unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys.matched("legacy"), Some("legacy"));
        // A non-empty list wins over the single key.
        let keys = ApiKeys::parse(Some("new"), Some("legacy")).unwrap();
        assert_eq!(keys.matched("legacy"), None);
    }

    #[test]
    fn test_matched_wrong_key() {
        let keys = ApiKeys::parse(Some("ios-1,and-2,gar-3"), None).unwrap();
        assert_eq!(keys.matched("ios-"), None);
        assert_eq!(keys.matched("ios-11"), None);
        assert_eq!(keys.matched(""), None);
    }

    #[test]
    fn test_matched_returns_the_right_key() {
        let keys = ApiKeys::parse(Some("ios-1,and-2,gar-3"), None).unwrap();
        assert_eq!(keys.matched("and-2"), Some("and-2"));
        assert_eq!(keys.matched("gar-3"), Some("gar-3"));
    }

    #[test]
    fn test_label() {
        assert_eq!(label("ios-abcdef"), "ios-");
        assert_eq!(label("ab"), "ab");
        assert_eq!(label(""), "");
        // Char-boundary safe: three two-byte characters stay whole.
        assert_eq!(label("ééé"), "ééé");
        assert_eq!(label("ééééé"), "éééé");
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }
}
