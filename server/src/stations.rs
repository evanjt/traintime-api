use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use traintime_core::geo::bounding_box;
use traintime_core::stations::Station;

/// Stand-in for the D1 stations table. The whole dataset is ~4.7 MB of JSON, so
/// a linear scan over a Vec replaces the bounding-box SELECT.
pub struct Stations {
    rows: Vec<Station>,
}

impl Stations {
    pub fn load(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let rows: Vec<Station> = serde_json::from_reader(BufReader::new(file))
            .map_err(|e| format!("{}: {e}", path.display()))?;

        if rows.is_empty() {
            return Err(format!("{}: contains no stations", path.display()));
        }

        Ok(Self { rows })
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Candidates inside the ~5km bounding box, optionally filtered by a
    /// lowercase substring of the name. Mirrors the Worker's D1 WHERE clause.
    pub fn candidates(&self, lat: f64, lon: f64, name_like: Option<&str>) -> Vec<&Station> {
        let (lat_min, lat_max, lon_min, lon_max) = bounding_box(lat, lon);

        self.rows
            .iter()
            .filter(|s| {
                s.lat >= lat_min && s.lat <= lat_max && s.lon >= lon_min && s.lon <= lon_max
            })
            .filter(|s| match name_like {
                Some(q) => s.name.to_lowercase().contains(q),
                None => true,
            })
            .collect()
    }
}
