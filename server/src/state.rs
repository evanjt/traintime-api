use std::sync::Arc;

use traintime_core::{ApiKeys, FormationError, UpstreamError};

use crate::cache::Cache;
use crate::fetch::Inflight;
use crate::stations::Stations;

#[derive(Clone)]
pub struct AppState {
    pub cache: Cache,
    pub inflight: Inflight<UpstreamError>,
    pub formation_inflight: Inflight<FormationError>,
    pub db: Arc<Stations>,
    /// Shared so connections to the OJP gateway are pooled. Rebuilding this per
    /// request would add a TLS handshake to every cold departure fetch.
    pub http: reqwest::Client,
    /// OJP_ENDPOINT override. Load tests point it at a stub so a run never
    /// counts against SBB's per-key quota.
    pub ojp_endpoint: String,
    pub ojp_api_key: String,
    pub formation_api_key: String,
    pub cache_ttl: u64,
    pub api_keys: Arc<ApiKeys>,
}

#[cfg(test)]
impl AppState {
    /// No stations and one API key, with OJP at `ojp_endpoint`.
    pub fn for_test(ojp_endpoint: &str) -> Self {
        AppState {
            cache: Cache::default(),
            inflight: Inflight::default(),
            formation_inflight: Inflight::default(),
            db: Arc::new(Stations::from_rows(Vec::new())),
            http: reqwest::Client::new(),
            ojp_endpoint: ojp_endpoint.to_string(),
            ojp_api_key: "key".to_string(),
            formation_api_key: "key".to_string(),
            cache_ttl: 60,
            api_keys: Arc::new(ApiKeys::parse(Some("test-key"), None).unwrap()),
        }
    }
}
