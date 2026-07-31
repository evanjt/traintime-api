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

/// ~5km bounding box in degrees, returned as (lat_min, lat_max, lon_min, lon_max).
/// Both runtimes must filter on the same box or their candidate sets diverge.
pub fn bounding_box(lat: f64, lon: f64) -> (f64, f64, f64, f64) {
    let dlat = MAX_DISTANCE / 111000.0;
    let dlon = MAX_DISTANCE / 75700.0;
    (lat - dlat, lat + dlat, lon - dlon, lon + dlon)
}
