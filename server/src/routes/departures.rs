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
use traintime_core::ojp::FlatDeparture;
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
    let departures: Vec<FlatDeparture> = match serde_json::from_str(&json) {
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
    use axum::routing::post;
    use axum::Router;

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
}
