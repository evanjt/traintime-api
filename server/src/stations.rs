use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use traintime_core::geo::bounding_box;
use traintime_core::stations::Station;

/// Stand-in for the D1 stations table. The whole dataset is ~4.7 MB of JSON, so
/// a linear scan over a Vec replaces the bounding-box SELECT.
pub struct Stations {
    rows: Vec<Station>,
}

impl Stations {
    /// Loads `path` and every `stations*.json` beside it, so each region's
    /// file is updated on its own schedule. The named file must exist.
    pub fn load(path: &Path) -> Result<Self, String> {
        let mut rows = read_file(path)?;
        for sibling in sibling_files(path)? {
            rows.extend(read_file(&sibling)?);
        }

        if rows.is_empty() {
            return Err(format!("{}: contains no stations", path.display()));
        }

        Ok(Self { rows })
    }

    #[cfg(test)]
    pub fn from_rows(rows: Vec<Station>) -> Self {
        Self { rows }
    }

    /// The station row, whose mode, route types and stop ids decide what its source is asked.
    pub fn station(&self, id: &str) -> Option<&Station> {
        self.rows.iter().find(|s| s.id == id)
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

fn read_file(path: &Path) -> Result<Vec<Station>, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_reader(BufReader::new(file)).map_err(|e| format!("{}: {e}", path.display()))
}

/// Other `stations*.json` files in the same directory, in name order.
fn sibling_files(path: &Path) -> Result<Vec<PathBuf>, String> {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut found: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.file_name() != path.file_name())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("stations") && n.ends_with(".json"))
        })
        .collect();
    found.sort();
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("stations-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    const SWISS: &str =
        r#"[{"id":"8500001","name":"Testdorf","lat":47.1,"lon":7.1,"mode":"train"}]"#;
    const OTHER: &str = r#"[{"id":"xx-north-9001","name":"Lakeside Station","lat":-37.8,"lon":145.0,"mode":"train","route_types":"0,3","stop_ids":"9001"}]"#;

    #[test]
    fn load_reads_every_station_file_beside_the_path() {
        let dir = scratch("both");
        fs::write(dir.join("stations.json"), SWISS).unwrap();
        fs::write(dir.join("stations-xx-north.json"), OTHER).unwrap();
        fs::write(dir.join("unrelated.json"), "not json").unwrap();

        let db = Stations::load(&dir.join("stations.json")).unwrap();

        assert_eq!(db.len(), 2);
        let near = db.candidates(-37.8, 145.0, None);
        assert_eq!(near.len(), 1);
        assert_eq!(near[0].id, "xx-north-9001");
        assert_eq!(near[0].route_types.as_deref(), Some("0,3"));
        assert_eq!(near[0].stop_ids.as_deref(), Some("9001"));
        assert_eq!(db.candidates(47.1, 7.1, None)[0].route_types, None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn load_fails_when_the_named_file_is_missing() {
        let dir = scratch("missing");
        fs::write(dir.join("stations-xx-north.json"), OTHER).unwrap();

        assert!(Stations::load(&dir.join("stations.json")).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_malformed_sibling_fails_the_load() {
        let dir = scratch("malformed");
        fs::write(dir.join("stations.json"), SWISS).unwrap();
        fs::write(dir.join("stations-broken.json"), "[{").unwrap();

        let err = Stations::load(&dir.join("stations.json")).err().unwrap();
        assert!(err.contains("stations-broken.json"), "{err}");
        fs::remove_dir_all(dir).unwrap();
    }
}
