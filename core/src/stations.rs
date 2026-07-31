use serde::{Deserialize, Serialize};

use crate::geo::{haversine_distance, MAX_DISTANCE, MAX_PER_MODE};
use crate::ojp::FlatDeparture;

/// A candidate station. The Worker builds these from D1 rows, the native server
/// from the bundled stations.json. Both then feed the same grouping code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Station {
    pub id: String,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub mode: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NearbyStation {
    pub id: String,
    pub name: String,
    pub dist: i64,
    pub lat: f64,
    pub lon: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub departures: Option<Vec<FlatDeparture>>,
}

#[derive(Debug, Clone, Default)]
pub struct ModeGroups {
    pub train: Vec<NearbyStation>,
    pub bus: Vec<NearbyStation>,
    pub tram: Vec<NearbyStation>,
    pub special: Vec<NearbyStation>,
}

impl ModeGroups {
    /// Attach a departure list to whichever group holds this station id.
    pub fn attach_departures(&mut self, id: &str, departures: Vec<FlatDeparture>) {
        for group in [
            &mut self.train,
            &mut self.bus,
            &mut self.tram,
            &mut self.special,
        ] {
            for station in group.iter_mut() {
                if station.id == id {
                    station.departures = Some(departures.clone());
                }
            }
        }
    }
}

/// Filter candidates to MAX_DISTANCE, bucket by mode, sort by distance, truncate.
pub fn group_nearby<'a, I>(candidates: I, lat: f64, lon: f64) -> ModeGroups
where
    I: Iterator<Item = &'a Station>,
{
    let mut groups = ModeGroups::default();

    for row in candidates {
        let dist = haversine_distance(lat, lon, row.lat, row.lon);
        if dist > MAX_DISTANCE {
            continue;
        }

        let entry = NearbyStation {
            id: row.id.clone(),
            name: row.name.clone(),
            dist: dist.round() as i64,
            lat: row.lat,
            lon: row.lon,
            departures: None,
        };

        match row.mode.as_str() {
            "bus" => groups.bus.push(entry),
            "tram" => groups.tram.push(entry),
            "special" => groups.special.push(entry),
            _ => groups.train.push(entry),
        }
    }

    for group in [
        &mut groups.train,
        &mut groups.bus,
        &mut groups.tram,
        &mut groups.special,
    ] {
        group.sort_by_key(|e| e.dist);
        group.truncate(MAX_PER_MODE);
    }

    groups
}

/// Pick the station whose departures get embedded: prioritise the requested mode,
/// then fall back through the remaining groups.
pub fn default_station_id(groups: &ModeGroups, requested_mode: Option<&str>) -> Option<String> {
    let ordered: Vec<&Vec<NearbyStation>> = match requested_mode {
        Some("bus") => vec![&groups.bus, &groups.train, &groups.tram, &groups.special],
        Some("tram") => vec![&groups.tram, &groups.train, &groups.bus, &groups.special],
        Some("special") => vec![&groups.special, &groups.train, &groups.bus, &groups.tram],
        _ => vec![&groups.train, &groups.bus, &groups.tram, &groups.special],
    };
    ordered.iter().find_map(|g| g.first().map(|s| s.id.clone()))
}
