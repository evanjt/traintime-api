use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use traintime_core::{departures_on_failure, mark_stale, stale_opt_in, CacheStatus};

use crate::fetch::Fetched;
use crate::source::fetch_departures_for;
use crate::routes::departures::unix_now_secs;
use crate::routes::respond;
use crate::state::AppState;
use traintime_core::ojp::upcoming_from_cache;
use traintime_core::region::region_for_station_id;
use traintime_core::stations::{default_station_id, group_nearby};

#[derive(Deserialize)]
pub struct NearbyQuery {
    lat: Option<f64>,
    lon: Option<f64>,
    query: Option<String>,
    mode: Option<String>,
    /// `1` opts in to a list older than the short window.
    stale: Option<String>,
}

pub async fn handle_nearby(
    State(state): State<AppState>,
    Query(params): Query<NearbyQuery>,
) -> Response {
    let (lat, lon) = match (params.lat, params.lon) {
        (Some(lat), Some(lon)) if lat.is_finite() && lon.is_finite() => (lat, lon),
        _ => {
            return respond(
                StatusCode::BAD_REQUEST,
                serde_json::json!({ "error": "Missing or invalid lat/lon parameters" }), None, CacheStatus::None);
        }
    };

    let query_lower = params.query.map(|q| q.to_lowercase());
    let requested_mode = params.mode;
    let opt_in = stale_opt_in(params.stale.as_deref());

    let rows = state.db.candidates(lat, lon, query_lower.as_deref());
    let mut groups = group_nearby(rows.into_iter(), lat, lon);

    // Fetch departures for the default station — prioritize requested mode, then fall back
    // Fetch 50 to share cache key with /v1/departures, but only embed first 20 in response
    let fetch_limit = 50u32;
    let embed_limit = 20usize;
    let default_id = default_station_id(&groups, requested_mode.as_deref());

    let mut stale = None;
    let mut fetched_at = None;
    let mut cache = CacheStatus::None;
    if let Some(id) = default_id {
        let region = region_for_station_id(&id);
        let cache_key = format!("departures:{id}:{fetch_limit}");
        let fetched = state
            .inflight
            .cached(&state.cache, &cache_key, state.cache_ttl, || async {
                let deps = fetch_departures_for(&state, region, &id, fetch_limit).await?;
                Ok(serde_json::to_string(&deps).unwrap_or_default())
            })
            .await;
        cache = fetched.cache_status();
        let json = match fetched {
            Fetched::Fresh(v) | Fetched::Cached(v) => Some(v),
            Fetched::Stale(v, age) => {
                let left = departures_on_failure(&v, age, unix_now_secs(), opt_in);
                fetched_at = left.as_ref().and_then(|s| s.fetched_at);
                let left = left.and_then(|s| serde_json::to_string(&s.departures).ok());
                stale = left.as_ref().map(|_| age);
                left
            }
            Fetched::Failed(_) => None,
        };
        if let Some(mut d) = json.and_then(|j| upcoming_from_cache(&j, (state.now)()).ok()) {
            d.truncate(embed_limit);
            groups.attach_departures(&id, d);
        }
    }

    let mut body = serde_json::json!({
        "train": groups.train,
        "bus": groups.bus,
        "tram": groups.tram,
        "special": groups.special,
    });
    mark_stale(&mut body, fetched_at);
    respond(StatusCode::OK, body, stale, cache)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::departures;
    use crate::routes::departures::tests::{cached_list, stub_ojp};
    use traintime_core::stations::Station;

    fn station() -> Station {
        Station { id: "8507000".into(), name: "Testhausen".into(), lat: 46.9, lon: 7.4, mode: "train".into(), route_types: None, stop_ids: None }
    }

    // The nearby body during an outage, with the nearest station's list cached
    // `age_secs` ago.
    async fn nearby_body(age_secs: u64, stale: Option<&str>) -> serde_json::Value {
        let state = AppState::for_test(&stub_ojp(StatusCode::BAD_GATEWAY).await).with_stations(vec![station()]);
        cached_list(&state, std::time::Duration::from_secs(age_secs));
        let query = NearbyQuery { lat: Some(46.9), lon: Some(7.4), query: None, mode: None, stale: stale.map(Into::into) };
        let resp = handle_nearby(State(state), Query(query)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    async fn embedded(age_secs: u64, stale: Option<&str>) -> serde_json::Value {
        nearby_body(age_secs, stale).await["train"][0]["departures"].clone()
    }

    #[tokio::test]
    async fn twenty_minute_old_list_is_embedded_only_with_the_opt_in() {
        assert_eq!(embedded(20 * 60, Some("1")).await[0]["to"], "Next");
        assert!(embedded(20 * 60, None).await.is_null());
    }

    #[tokio::test]
    async fn twenty_minute_old_list_is_marked_with_its_age() {
        let body = nearby_body(20 * 60, Some("1")).await;
        assert_eq!(body["stale"], true);
        let age = departures::unix_now_secs() - body["fetchedAt"].as_i64().unwrap();
        assert!((20 * 60..20 * 60 + 5).contains(&age), "{age}");
    }

    #[tokio::test]
    async fn four_minute_old_list_is_embedded_either_way_without_stale_fields() {
        for opt_in in [None, Some("1")] {
            let body = nearby_body(4 * 60, opt_in).await;
            assert_eq!(body["train"][0]["departures"][0]["to"], "Next", "{opt_in:?}");
            assert!(body.get("stale").is_none() && body.get("fetchedAt").is_none(), "{opt_in:?}");
        }
    }
}
