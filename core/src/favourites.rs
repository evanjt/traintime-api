use crate::ojp::FlatDeparture;

/// Parse "IC8:Brig,IR90:Visp" into vec of (lineNumber, destination)
pub fn parse_favourites(raw: &str) -> Vec<(String, String)> {
    raw.split(',')
        .filter_map(|pair| {
            let pair = pair.trim();
            let colon = pair.find(':')?;
            let line = pair[..colon].to_string();
            let dest = pair[colon + 1..].to_string();
            if line.is_empty() || dest.is_empty() {
                None
            } else {
                Some((line, dest))
            }
        })
        .collect()
}

/// Partition departures: clone first match per favourite pair, return (favourites, remaining)
/// Favourites are cloned (not removed) so they can still appear in the regular list.
/// Favourites are sorted by departure time.
pub fn partition_favourites(
    departures: &[FlatDeparture],
    fav_pairs: &[(String, String)],
    limit: usize,
) -> (Vec<FlatDeparture>, Vec<FlatDeparture>) {
    let mut favourites = Vec::new();

    for (line, dest) in fav_pairs {
        if let Some(d) = departures.iter().find(|d| d.number == *line && d.to == *dest) {
            favourites.push(d.clone());
        }
    }

    favourites.sort_by_key(|d| d.departure);

    let remaining = departures.iter().take(limit).cloned().collect();
    (favourites, remaining)
}
