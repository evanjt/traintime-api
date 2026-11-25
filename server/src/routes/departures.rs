use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use traintime_core::CacheStatus;

use crate::fetch::Fetched;
use crate::source::fetch_departures_for;
use crate::routes::{respond, respond_error};
use crate::state::AppState;
use traintime_core::favourites::{parse_favourites, partition_favourites};
use traintime_core::ojp::upcoming_from_cache;
use traintime_core::region::region_for_station_id;

#[derive(Deserialize)]
pub struct DeparturesQuery {
    id: Option<String>,
    limit: Option<u32>,
    favourites: Option<String>,
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
    let fav_pairs = params
        .favourites
        .as_deref()
        .map(parse_favourites)
        .unwrap_or_default();
    let has_favourites = !fav_pairs.is_empty();

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
    let cache = fetched.cache_status();
    let (json, stale) = match fetched {
        Fetched::Fresh(v) | Fetched::Cached(v) => (v, None),
        Fetched::Stale(v, age) => (v, Some(age)),
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
    if has_favourites {
        let (favs, deps) = partition_favourites(&departures, &fav_pairs, limit as usize);
        return respond(
            StatusCode::OK,
            serde_json::json!({ "favourites": favs, "departures": deps }), stale, cache);
    }
    let mut departures = departures;
    departures.truncate(limit as usize);
    respond(StatusCode::OK, serde_json::json!({ "departures": departures }), stale, cache)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Path, RawQuery};
    use axum::routing::{get, post};
    use axum::Router;
    use traintime_core::stations::Station;

    async fn stub_ojp(status: StatusCode) -> String {
        let app = Router::new().route("/ojp20", post(move || async move { (status, "gateway error") }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/ojp20")
    }

    #[tokio::test]
    async fn ojp_502_answers_503_upstream_unavailable() {
        let state = AppState::for_test(&stub_ojp(StatusCode::BAD_GATEWAY).await);
        let query = DeparturesQuery { id: Some("8507000".into()), limit: None, favourites: None };

        let resp = handle_departures(State(state), Query(query)).await;

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(resp.headers().get("retry-after").unwrap(), "30");
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body, serde_json::json!({ "error": "upstream_unavailable" }));
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
        let query = DeparturesQuery { id: Some(id.into()), limit: Some(10), favourites: None };
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
