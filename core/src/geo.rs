pub const MAX_PER_MODE: usize = 5;
pub const MAX_DISTANCE: f64 = 5000.0;

pub fn haversine_distance(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R: f64 = 6371000.0;
    let to_rad = |deg: f64| deg * std::f64::consts::PI / 180.0;
    let d_lat = to_rad(lat2 - lat1);
    let d_lon = to_rad(lon2 - lon1);
    let a = (d_lat / 2.0).sin().powi(2)
        + to_rad(lat1).cos() * to_rad(lat2).cos() * (d_lon / 2.0).sin().powi(2);
    R * 2.0 * a.sqrt().atan2((1.0 - a).sqrt())
}

/// Metres per degree, rounded down from the haversine radius's 111195 so the
/// box errs a little wide and never drops a station at exactly MAX_DISTANCE.
const METRES_PER_DEGREE: f64 = 111000.0;

/// ~5km bounding box in degrees, returned as (lat_min, lat_max, lon_min, lon_max).
/// Both runtimes must filter on the same box or their candidate sets diverge.
pub fn bounding_box(lat: f64, lon: f64) -> (f64, f64, f64, f64) {
    let dlat = MAX_DISTANCE / METRES_PER_DEGREE;
    // A degree of longitude shrinks with cos(lat). The floor keeps the span
    // finite at the poles.
    let dlon = MAX_DISTANCE / (METRES_PER_DEGREE * lat.to_radians().cos().max(0.01));
    (lat - dlat, lat + dlat, lon - dlon, lon + dlon)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lon_east_at(lat: f64, metres: f64) -> f64 {
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..100 {
            let mid = (lo + hi) / 2.0;
            if haversine_distance(lat, 0.0, lat, mid) < metres { lo = mid } else { hi = mid }
        }
        hi
    }

    #[test]
    fn box_holds_a_station_max_distance_east_at_lat_60() {
        let lon = lon_east_at(60.0, MAX_DISTANCE);
        let (_, _, lon_min, lon_max) = bounding_box(60.0, 0.0);
        assert!(lon <= lon_max && -lon >= lon_min, "lon {lon} outside {lon_min}..{lon_max}");
    }

    #[test]
    fn box_holds_a_station_max_distance_east_at_any_latitude() {
        for lat in [-38.0, 0.0, 45.8, 47.8] {
            let lon = lon_east_at(lat, MAX_DISTANCE);
            let (_, _, _, lon_max) = bounding_box(lat, 0.0);
            assert!(lon <= lon_max, "lat {lat}: lon {lon} beyond {lon_max}");
        }
    }

    #[test]
    fn box_is_not_padded_beyond_a_percent() {
        let lon = lon_east_at(46.0, MAX_DISTANCE);
        let (_, _, _, lon_max) = bounding_box(46.0, 0.0);
        assert!(lon_max < lon * 1.01, "lon_max {lon_max} vs {lon}");
    }
}
