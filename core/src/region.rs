//! The regions the API serves. Adding a region is adding one entry to
//! `REGIONS` plus its station data: no route names a country.

/// Where a region's departures come from. Each build dispatches on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Ojp,
}

/// Coverage box in degrees, inclusive on every edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub lat_min: f64,
    pub lat_max: f64,
    pub lon_min: f64,
    pub lon_max: f64,
}

impl Bounds {
    pub fn contains(&self, lat: f64, lon: f64) -> bool {
        lat >= self.lat_min && lat <= self.lat_max && lon >= self.lon_min && lon <= self.lon_max
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    pub id: &'static str,
    pub bounds: Bounds,
    /// IANA zone the region's departure times are shown in.
    pub timezone: &'static str,
    /// Station id prefix. Swiss ids stay bare so saved favourites and caches
    /// keep working, every other region's ids carry a prefix with no `:` or `,`.
    pub id_prefix: &'static str,
    pub source: Source,
    /// Whether `/v1/formation` has data for this region's trains.
    pub formation: bool,
}

pub const SWITZERLAND: Region = Region {
    id: "ch",
    bounds: Bounds { lat_min: 45.8, lat_max: 47.8, lon_min: 5.9, lon_max: 10.5 },
    timezone: "Europe/Zurich",
    id_prefix: "",
    source: Source::Ojp,
    formation: true,
};

/// Switzerland stays first: it is the fallback for any id no prefix claims.
pub static REGIONS: &[Region] = &[SWITZERLAND];

pub fn region_for_station_id(id: &str) -> &'static Region {
    region_for_station_id_in(REGIONS, id)
}

pub fn region_for_point(lat: f64, lon: f64) -> Option<&'static Region> {
    region_for_point_in(REGIONS, lat, lon)
}

/// Longest matching prefix wins, so a bare id falls to the empty prefix.
/// `regions` must be non-empty with the fallback first.
pub fn region_for_station_id_in<'a>(regions: &'a [Region], id: &str) -> &'a Region {
    regions
        .iter()
        .filter(|r| id.starts_with(r.id_prefix))
        .max_by_key(|r| r.id_prefix.len())
        .unwrap_or(&regions[0])
}

pub fn region_for_point_in(regions: &[Region], lat: f64, lon: f64) -> Option<&Region> {
    regions.iter().find(|r| r.bounds.contains(lat, lon))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ELSEWHERE: Region = Region {
        id: "xx",
        bounds: Bounds { lat_min: -10.0, lat_max: -9.0, lon_min: 20.0, lon_max: 21.0 },
        timezone: "Etc/UTC",
        id_prefix: "xx-north-",
        source: Source::Ojp,
        formation: false,
    };
    const TEST_REGIONS: &[Region] = &[SWITZERLAND, ELSEWHERE];

    #[test]
    fn bare_id_is_switzerland() {
        assert_eq!(region_for_station_id("8503000").id, "ch");
    }

    #[test]
    fn prefixed_id_is_its_region() {
        assert_eq!(region_for_station_id_in(TEST_REGIONS, "xx-north-1071").id, "xx");
    }

    #[test]
    fn unknown_prefix_falls_back_to_switzerland() {
        assert_eq!(region_for_station_id_in(TEST_REGIONS, "yy-south-1071").id, "ch");
        assert_eq!(region_for_station_id("yy-south-1071").id, "ch");
    }

    #[test]
    fn switzerland_is_the_fallback_and_has_formation() {
        assert_eq!(REGIONS[0], SWITZERLAND);
        assert_eq!(SWITZERLAND.source, Source::Ojp);
        assert!(SWITZERLAND.formation);
    }

    #[test]
    fn bern_is_switzerland_and_sydney_is_nowhere() {
        assert_eq!(region_for_point(46.9480, 7.4474).map(|r| r.id), Some("ch"));
        assert_eq!(region_for_point(-33.8688, 151.2093), None);
    }

    #[test]
    fn point_inside_another_box_is_that_region() {
        assert_eq!(region_for_point_in(TEST_REGIONS, -9.5, 20.5).map(|r| r.id), Some("xx"));
    }

    #[test]
    fn every_prefix_is_free_of_separators() {
        for r in REGIONS {
            assert!(!r.id_prefix.contains(':') && !r.id_prefix.contains(','), "{}", r.id);
        }
    }
}
