//! Runtime-agnostic types and parsers shared by the Cloudflare Worker and the
//! native server.
//!
//! Nothing here may reach for a clock, the network, or the filesystem. That is
//! enforced by the dependency list in Cargo.toml (serde, serde_json, regex),
//! which is what keeps this crate compiling for wasm32-unknown-unknown.

pub mod error;
pub mod favourites;
pub mod formation;
pub mod geo;
pub mod ojp;
pub mod stations;

pub use error::CoreError;
pub use favourites::{parse_favourites, partition_favourites};
pub use formation::{
    extract_train_number, operator_ref_to_evu, parse_formation_response,
    parse_formation_short_string, FormationResult, Wagon, FORMATION_ENDPOINT,
};
pub use geo::{bounding_box, haversine_distance, MAX_DISTANCE, MAX_PER_MODE};
pub use ojp::{
    build_stop_event_request_xml, parse_stop_events, FlatDeparture, OJP_ENDPOINT,
};
pub use stations::{default_station_id, group_nearby, ModeGroups, NearbyStation, Station};
