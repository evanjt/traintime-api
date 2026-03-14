use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Station {
    pub id: String,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub mode: String,
}

pub static STATIONS: LazyLock<Vec<Station>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("data/stations.json")).expect("failed to parse stations.json")
});
