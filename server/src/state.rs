use std::sync::Arc;

use traintime_core::ApiKeys;

use crate::cache::Cache;
use crate::fetch::Inflight;
use crate::stations::Stations;

#[derive(Clone)]
pub struct AppState {
    pub cache: Cache,
    pub inflight: Inflight,
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
