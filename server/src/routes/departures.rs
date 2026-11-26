use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use traintime_core::CacheStatus;

use crate::fetch::Fetched;
use crate::source::fetch_departures_for;
use crate::routes::{respond, respond_error};
use crate::state::AppState;
use traintime_core::favourites::parse_favourites;
use traintime_core::ojp::upcoming_from_cache;
use traintime_core::{departures_body, departures_on_failure, stale_opt_in, UpstreamError};
use traintime_core::region::region_for_station_id;

#[derive(Deserialize)]
pub struct DeparturesQuery {
    id: Option<String>,
    limit: Option<u32>,
    favourites: Option<String>,
    /// `1` opts in to a list older than the short window, marked with its age.
    stale: Option<String>,
}

pub async fn handle_departures(
    State(state): State<AppState>,
    Query(params): Query<DeparturesQuery>,
) -> Response {
    let station_id = match params.id {
        Some(id) => id,
        None => {
            return respond(
                StatusCode::BAD_REQUEST,
                serde_json::json!({ "error": "Missing id parameter" }), None, CacheStatus::None);
        }
    };

    let limit = params.limit.unwrap_or(5);
    let opt_in = stale_opt_in(params.stale.as_deref());
    let fav_pairs = params
        .favourites
        .as_deref()
        .map(parse_favourites)
        .unwrap_or_default();

    // Always fetch 50 to populate cache; partition down to `limit` on return
    let fetch_limit = 50u32;
    let region = region_for_station_id(&station_id);
    let cache_key = format!("departures:{station_id}:{fetch_limit}");
    let fetched = state
        .inflight
        .cached(&state.cache, &cache_key, state.cache_ttl, || async {
            let deps = fetch_departures_for(&state, region, &station_id, fetch_limit).await?;
            Ok(serde_json::to_string(&deps).unwrap_or_default())
        })
        .await;
    let mut cache = fetched.cache_status();
    let (json, stale) = match fetched {
        Fetched::Fresh(v) | Fetched::Cached(v) => (v, None),
        Fetched::Stale(v, age) => match departures_on_failure(&v, age, unix_now_secs(), opt_in) {
            Some(s) => {
                println!("STALE {cache_key} {age}s");
                let json = serde_json::to_string(&s.departures).unwrap_or_default();
                (json, Some((s.fetched_at, age)))
            }
            None => {
                cache = CacheStatus::Miss;
                println!("Departures error: cached list past its window or all departed");
                return respond_error(UpstreamError::Timeout.response(), cache);
            }
        },
        Fetched::Failed(e) => {
            println!("Departures error: {e}");
            return respond_error(e.response(), cache);
        }
    };
    let departures = match upcoming_from_cache(&json, (state.now)()) {
        Ok(d) => d,
        Err(e) => {
            return respond(StatusCode::INTERNAL_SERVER_ERROR, serde_json::json!({ "error": e.to_string() }), None, cache);
        }
    };
    let body = departures_body(departures, &fav_pairs, limit as usize, stale.and_then(|(at, _)| at));
    respond(StatusCode::OK, body, stale.map(|(_, age)| age), cache)
}

pub fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use axum::extract::{Path, RawQuery};
    use axum::routing::{get, post};
    use axum::Router;
    use traintime_core::stations::Station;

    pub(crate) async fn stub_ojp(status: StatusCode) -> String {
        let app = Router::new().route("/ojp20", post(move || async move { (status, "gateway error") }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/ojp20")
    }

    #[tokio::test]
    async fn ojp_502_answers_503_upstream_unavailable() {
        let state = AppState::for_test(&stub_ojp(StatusCode::BAD_GATEWAY).await);
        let query = DeparturesQuery { id: Some("8507000".into()), limit: None, favourites: None, stale: None };

        let resp = handle_departures(State(state), Query(query)).await;

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(resp.headers().get("retry-after").unwrap(), "30");
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body, serde_json::json!({ "error": "upstream_unavailable" }));
    }

    fn unix_now() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
    }

    // A list cached `age` ago for a station, with one row gone and one to come.
    pub(crate) fn cached_list(state: &AppState, age: std::time::Duration) {
        let now = unix_now();
        let row = |to: &str, departure: i64| serde_json::json!({
            "to": to, "category": "IR", "number": "15", "departure": departure,
            "delay": null, "platform": "3", "platformChanged": false,
        });
        let list = serde_json::json!([row("Gone", now - 600), row("Next", now + 600)]).to_string();
        state.cache.put_at("departures:8507000:50", &list, 60, std::time::Instant::now() - age);
    }

    #[tokio::test]
    async fn outage_serves_twenty_minute_old_list_with_its_age() {
        let state = AppState::for_test(&stub_ojp(StatusCode::BAD_GATEWAY).await);
        cached_list(&state, std::time::Duration::from_secs(20 * 60));

        let (status, body) = departures_body_opted_in(state, "8507000", Some("1")).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["stale"], true);
        let age = unix_now() - body["fetchedAt"].as_i64().unwrap();
        assert!((20 * 60..20 * 60 + 5).contains(&age), "{age}");
        let to: Vec<&str> = body["departures"].as_array().unwrap().iter().map(|r| r["to"].as_str().unwrap()).collect();
        assert_eq!(to, vec!["Next"]);
    }

    #[tokio::test]
    async fn outage_past_the_window_answers_503() {
        let state = AppState::for_test(&stub_ojp(StatusCode::BAD_GATEWAY).await);
        cached_list(&state, std::time::Duration::from_secs(40 * 60));

        let (status, body) = departures_body_opted_in(state, "8507000", Some("1")).await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body, serde_json::json!({ "error": "upstream_unavailable" }));
    }

    // Scenario: a client that does not read the stale fields asks during an
    // outage, and the cached list is 20 minutes old.
    // Expected behaviour: it gets the 503 it got before the long window existed.
    #[tokio::test]
    async fn outage_without_the_opt_in_answers_503_for_a_twenty_minute_old_list() {
        let state = AppState::for_test(&stub_ojp(StatusCode::BAD_GATEWAY).await);
        cached_list(&state, std::time::Duration::from_secs(20 * 60));

        let (status, body) = departures_body(state, "8507000").await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body, serde_json::json!({ "error": "upstream_unavailable" }));
    }

    #[tokio::test]
    async fn outage_serves_four_minute_old_list_without_stale_fields_either_way() {
        for opt_in in [None, Some("1")] {
            let state = AppState::for_test(&stub_ojp(StatusCode::BAD_GATEWAY).await);
            cached_list(&state, std::time::Duration::from_secs(4 * 60));

            let (status, body) = departures_body_opted_in(state, "8507000", opt_in).await;

            assert_eq!(status, StatusCode::OK, "{opt_in:?}");
            assert!(body.get("stale").is_none() && body.get("fetchedAt").is_none(), "{opt_in:?} {body}");
            assert_eq!(body["departures"][0]["to"], "Next");
        }
    }

    const PTV_TRAINS: &str = include_str!("../../../core/fixtures/ptv_train_departures.json");

    // Serves the recorded metro board for route type 0, an empty V/Line board
    // for 3, and refuses any request without a signature.
    async fn stub_ptv() -> String {
        let app = Router::new().route(
            "/v3/departures/route_type/{rt}/stop/{stop}",
            get(|Path((rt, stop)): Path<(u8, String)>, RawQuery(q): RawQuery| async move {
                let q = q.unwrap_or_default();
                if stop != "1105" || !q.contains("devid=3000123") || !q.contains("&signature=") {
                    return (StatusCode::FORBIDDEN, String::new());
                }
                match rt {
                    0 => (StatusCode::OK, PTV_TRAINS.to_string()),
                    _ => (StatusCode::OK, r#"{"departures":[]}"#.to_string()),
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn jordanville() -> Station {
        Station { id: "au-vic-1105".into(), name: "Jordanville".into(), lat: -37.8735, lon: 145.1203, mode: "train".into(), route_types: None, stop_ids: None }
    }

    async fn departures_body(state: AppState, id: &str) -> (StatusCode, serde_json::Value) {
        departures_body_opted_in(state, id, None).await
    }

    async fn departures_body_opted_in(state: AppState, id: &str, stale: Option<&str>) -> (StatusCode, serde_json::Value) {
        let query = DeparturesQuery { id: Some(id.into()), limit: Some(10), favourites: None, stale: stale.map(Into::into) };
        let resp = handle_departures(State(state), Query(query)).await;
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn victorian_station_answers_from_ptv() {
        let state = AppState::for_test("http://127.0.0.1:9/ojp20")
            .with_ptv(&stub_ptv().await, "3000123", "key")
            .with_stations(vec![jordanville()]);

        let (status, body) = departures_body(state, "au-vic-1105").await;

        assert_eq!(status, StatusCode::OK);
        let rows = body["departures"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["to"], "City (Flinders Street)");
        assert_eq!(rows[0]["category"], "Metro");
        assert_eq!(rows[0]["delay"], 3);
        assert_eq!(rows[0]["platform"], "1");
    }

    // Scenario: the cache holds a board fetched earlier, and by now its first
    // row, planned 09:05 and 3 minutes late, is well past.
    // Expected behaviour: that row is not served, the later two are.
    #[tokio::test]
    async fn a_row_past_its_time_is_not_served() {
        let state = AppState::for_test("http://127.0.0.1:9/ojp20")
            .with_ptv(&stub_ptv().await, "3000123", "key")
            .with_stations(vec![jordanville()])
            .with_clock(|| 1_795_338_840); // 2026-11-22T09:14:00Z

        let (status, body) = departures_body(state, "au-vic-1105").await;

        assert_eq!(status, StatusCode::OK);
        let rows = body["departures"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["departure"], 1_795_338_720);
    }

    const PTV_TRAMS: &str = include_str!("../../../core/fixtures/ptv_tram_departures.json");

    // A tram stop whose two platforms are separate PTV stops. Each serves one
    // direction, and every request is recorded as "route_type/stop".
    async fn stub_ptv_two_platforms() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = asked.clone();
        let app = Router::new().route(
            "/v3/departures/route_type/{rt}/stop/{stop}",
            get(move |Path((rt, stop)): Path<(u8, String)>| {
                let log = log.clone();
                async move {
                    log.lock().unwrap().push(format!("{rt}/{stop}"));
                    match stop.as_str() {
                        "7001" => (StatusCode::OK, PTV_TRAMS.to_string()),
                        "7002" => (
                            StatusCode::OK,
                            PTV_TRAMS
                                .replace("\"direction_name\": \"St Kilda Beach\"", "\"direction_name\": \"Melbourne University\"")
                                .replace("\"direction_name\": \"Carnegie\"", "\"direction_name\": \"Melbourne University\"")
                                .replace("09:10:00Z", "09:06:00Z")
                                .replace("09:03:00Z", "09:12:00Z"),
                        ),
                        _ => (StatusCode::NOT_FOUND, String::new()),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), asked)
    }

    #[tokio::test]
    async fn merged_tram_stop_asks_both_stops_and_shows_both_directions() {
        let (endpoint, asked) = stub_ptv_two_platforms().await;
        let stop = Station {
            id: "au-vic-7001".into(),
            name: "Example Rd/Sample St".into(),
            lat: -37.8,
            lon: 144.97,
            mode: "tram".into(),
            route_types: Some("1".into()),
            stop_ids: Some("7001,7002".into()),
        };
        let state = AppState::for_test("http://127.0.0.1:9/ojp20")
            .with_ptv(&endpoint, "3000123", "key")
            .with_stations(vec![stop]);

        let (status, body) = departures_body(state, "au-vic-7001").await;

        assert_eq!(status, StatusCode::OK);
        let mut calls = asked.lock().unwrap().clone();
        calls.sort();
        assert_eq!(calls, vec!["1/7001", "1/7002"]);
        let to: Vec<&str> = body["departures"].as_array().unwrap().iter().map(|r| r["to"].as_str().unwrap()).collect();
        assert_eq!(to, vec!["Carnegie", "Melbourne University", "St Kilda Beach", "Melbourne University"]);
    }

    #[tokio::test]
    async fn victorian_station_without_ptv_credentials_answers_503() {
        let state = AppState::for_test("http://127.0.0.1:9/ojp20").with_stations(vec![jordanville()]);

        let (status, body) = departures_body(state, "au-vic-1105").await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body, serde_json::json!({ "error": "upstream_unavailable" }));
    }
}
