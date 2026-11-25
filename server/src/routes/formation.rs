use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use traintime_core::CacheStatus;

use crate::fetch::Fetched;
use crate::formation::fetch_formation;
use crate::routes::{respond, respond_error};
use crate::state::AppState;
use traintime_core::formation::{extract_train_number, is_operation_date, operator_ref_to_evu};
use traintime_core::region::region_for_station_id;

const FORMATION_CACHE_TTL: u64 = 300;

#[derive(Deserialize)]
pub struct FormationQuery {
    train: Option<String>,
    date: Option<String>,
    stop: Option<String>,
    evu: Option<String>,
    #[serde(rename = "operatorRef")]
    operator_ref: Option<String>,
}

pub async fn handle_formation(
    State(state): State<AppState>,
    Query(params): Query<FormationQuery>,
) -> Response {
    let train_raw = match params.train {
        Some(t) if !t.is_empty() => t,
        _ => {
            return respond(
                StatusCode::BAD_REQUEST,
                serde_json::json!({ "error": "Missing train parameter" }), None, CacheStatus::None);
        }
    };

    let date = match params.date {
        Some(d) if !d.is_empty() => d,
        _ => {
            return respond(
                StatusCode::BAD_REQUEST,
                serde_json::json!({ "error": "Missing date parameter" }), None, CacheStatus::None);
        }
    };
    if !is_operation_date(&date) {
        return respond(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "error": "Invalid date parameter" }), None, CacheStatus::None);
    }

    let train_number = extract_train_number(&train_raw);
    if train_number.is_empty() {
        return respond(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "error": "Invalid train number" }), None, CacheStatus::None);
    }

    // A stop in a region without formation data has nothing to look up. With
    // no stop the train is Swiss, as before regions existed.
    if !region_for_station_id(params.stop.as_deref().unwrap_or("")).formation {
        return respond(
            StatusCode::NOT_FOUND,
            serde_json::json!({ "error": "No formation data" }), None, CacheStatus::None);
    }

    let evu = if let Some(ref e) = params.evu {
        e.clone()
    } else if let Some(ref op) = params.operator_ref {
        match operator_ref_to_evu(op) {
            Some(mapped) => mapped.to_string(),
            None => {
                return respond(
                    StatusCode::NOT_FOUND,
                    serde_json::json!({ "error": "No formation data" }), None, CacheStatus::None);
            }
        }
    } else {
        "SBBP".to_string()
    };
    let stop_key = params.stop.as_deref().unwrap_or("all");
    let cache_key = format!("formation:{evu}:{date}:{train_number}:{stop_key}");

    let fetched = state
        .formation_inflight
        .cached(&state.cache, &cache_key, FORMATION_CACHE_TTL, || async {
            let result = fetch_formation(
                &state.http,
                &state.formation_endpoint,
                &state.formation_api_key,
                &evu,
                &date,
                &train_number,
                params.stop.as_deref(),
            )
            .await?;
            Ok(serde_json::to_string(&result).unwrap_or_default())
        })
        .await;
    let cache = fetched.cache_status();
    let (json, stale) = match fetched {
        Fetched::Fresh(v) | Fetched::Cached(v) => (v, None),
        Fetched::Stale(v, age) => (v, Some(age)),
        Fetched::Failed(e) => {
            println!("Formation error: {e}");
            return respond_error(e.response(), cache);
        }
    };
    match serde_json::from_str::<serde_json::Value>(&json) {
        Ok(v) => respond(StatusCode::OK, v, stale, cache),
        Err(e) => respond(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }), None, cache),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use axum::Router;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    // Answers every formation request with `status` and counts the calls.
    async fn stub_formation(status: StatusCode) -> (String, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let app = Router::new().route(
            "/formations_stop_based",
            get(move || {
                seen.fetch_add(1, Ordering::SeqCst);
                async move { (status, "{}") }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/formations_stop_based"), calls)
    }

    async fn formation_body(state: AppState, date: &str) -> (StatusCode, serde_json::Value) {
        let query = FormationQuery {
            train: Some("4321".into()),
            date: Some(date.into()),
            stop: Some("8507000".into()),
            evu: None,
            operator_ref: None,
        };
        let resp = handle_formation(State(state), Query(query)).await;
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn upstream_400_answers_404_no_formation_data() {
        let (endpoint, calls) = stub_formation(StatusCode::BAD_REQUEST).await;
        let state = AppState::for_test("http://127.0.0.1:9/ojp20").with_formation(&endpoint);

        let (status, body) = formation_body(state, "2031-04-02").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body, serde_json::json!({ "error": "No formation data" }));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn upstream_500_still_answers_503() {
        let (endpoint, _) = stub_formation(StatusCode::INTERNAL_SERVER_ERROR).await;
        let state = AppState::for_test("http://127.0.0.1:9/ojp20").with_formation(&endpoint);

        let (status, body) = formation_body(state, "2031-04-02").await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body, serde_json::json!({ "error": "upstream_unavailable" }));
    }

    #[tokio::test]
    async fn malformed_date_answers_400_without_asking_upstream() {
        let (endpoint, calls) = stub_formation(StatusCode::OK).await;
        let state = AppState::for_test("http://127.0.0.1:9/ojp20").with_formation(&endpoint);

        let (status, body) = formation_body(state, "tomorrow").await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, serde_json::json!({ "error": "Invalid date parameter" }));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
