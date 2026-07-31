use std::sync::Arc;

use crate::cache::Cache;
use crate::stations::Stations;

#[derive(Clone)]
pub struct AppState {
    pub cache: Cache,
    pub db: Arc<Stations>,
    /// Shared so connections to the OJP gateway are pooled. Rebuilding this per
    /// request would add a TLS handshake to every cold departure fetch.
    pub http: reqwest::Client,
    pub ojp_api_key: String,
    pub formation_api_key: String,
    pub cache_ttl: u64,
    pub api_key: String,
}
