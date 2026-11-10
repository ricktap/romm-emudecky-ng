//! Transport + endpoint tests against a local fake RomM (wiremock) — the Rust
//! twin of the connection-relevant parts of `tests/adapters/romm/test_http.py`.

use std::sync::Mutex;

use serde_json::json;
use wiremock::matchers::{header, header_exists, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::*;
use crate::adapters::romm::api::{RommApi, TOKEN_SCOPES};
use crate::services::ports::RommConnectionApi;

const UA: &str = "romm-emudecky-ng/9.9.9";

#[tokio::test]
async fn native_ingest_preserves_422_detail_never_retries_and_checks_auth_origin(){
    use crate::services::ports::{RommPlaytimeApi,PlaytimeApiError};
    let server=MockServer::start().await;
    let settings=store(&server.uri(),Some("test-token"),Some(&server.uri()));let api=RommApi::new(Arc::new(http(settings.clone())));
    let rows=vec![json!({"rom_id":1,"duration_ms":1000,"start_time":"s","end_time":"e"})];
    for (status,body) in [(200,json!({"results":[{"index":0,"status":"created"}]})),(422,json!({"detail":[{"loc":["body","sessions",0]}]})),(500,json!({})),(403,json!({}))]{
        server.reset().await;Mock::given(method("POST")).and(path("/api/play-sessions")).and(header("authorization","Bearer test-token")).and(header("user-agent",UA)).respond_with(ResponseTemplate::new(status).set_body_json(&body)).expect(1).mount(&server).await;
        let r=api.ingest("device",rows.clone()).await;
        match status{200=>assert_eq!(r.unwrap(),body),422=>assert!(matches!(r,Err(PlaytimeApiError::Rejected(d)) if d==body["detail"])),_=>assert!(matches!(r,Err(PlaytimeApiError::Romm(_))))}
        let requests=server.received_requests().await.unwrap();assert_eq!(requests.len(),1);assert_eq!(requests[0].body_json::<Value>().unwrap(),json!({"device_id":"device","sessions":rows}));server.verify().await;
    }
    server.reset().await;settings.set("romm_api_token_origin",json!("https://different.invalid"));assert!(matches!(api.ingest("device",rows).await,Err(PlaytimeApiError::Romm(RommError::TokenHostMismatch))));assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn native_session_get_pages_with_auth_and_accepts_bare_or_items_arrays(){
    use crate::services::ports::RommPlaytimeApi;
    use wiremock::matchers::query_param;
    let server=MockServer::start().await;let api=RommApi::new(Arc::new(http(store(&server.uri(),Some("test-token"),Some(&server.uri())))));
    Mock::given(method("GET")).and(path("/api/play-sessions")).and(query_param("rom_id","7")).and(query_param("limit","100")).and(query_param("offset","0")).and(header("authorization","Bearer test-token")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"items":vec![json!({"duration_ms":1000});100]}))).expect(1).mount(&server).await;
    Mock::given(method("GET")).and(path("/api/play-sessions")).and(query_param("offset","100")).respond_with(ResponseTemplate::new(200).set_body_json(json!([{"duration_ms":2000}]))).expect(1).mount(&server).await;
    let rows=api.sessions(7).await.unwrap();assert_eq!(rows.len(),101);assert_eq!(rows[100]["duration_ms"],2000);
}

fn fast_retry() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 3,
        base_delay: Duration::from_millis(1),
    }
}

fn store(url: &str, token: Option<&str>, origin: Option<&str>) -> SettingsStore {
    SettingsStore::new(
        json!({"romm_url": url, "romm_api_token": token, "romm_api_token_origin": origin})
            .as_object()
            .cloned()
            .unwrap(),
    )
}

fn http(settings: SettingsStore) -> RommHttp {
    RommHttp::new(settings, UA.into(), fast_retry(), None).unwrap()
}

fn auth_of(req: &Request) -> Option<String> {
    req.headers
        .get("authorization")
        .map(|v| v.to_str().unwrap().to_string())
}

#[tokio::test]
async fn bearer_sent_to_matching_origin_with_user_agent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/heartbeat"))
        .and(header("user-agent", UA))
        .and(header("authorization", "Bearer rmm_tok"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"SYSTEM": {"VERSION": "4.9.0"}})))
        .expect(1)
        .mount(&server)
        .await;
    let h = http(store(
        &format!("{}/", server.uri()),
        Some("rmm_tok"),
        Some(&server.uri()),
    ));
    assert_eq!(
        h.get_json("/api/heartbeat").await.unwrap()["SYSTEM"]["VERSION"],
        "4.9.0"
    );
}

#[tokio::test]
async fn no_token_means_no_authorization_header() {
    let server = MockServer::start().await;
    Mock::given(path("/api/heartbeat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    for token in [None, Some("")] {
        let h = http(store(&server.uri(), token, None));
        h.get_json("/api/heartbeat").await.unwrap();
    }
    for req in server.received_requests().await.unwrap() {
        assert_eq!(auth_of(&req), None);
        assert_eq!(req.headers.get("user-agent").unwrap(), UA);
    }
}

#[tokio::test]
async fn legacy_token_without_origin_is_still_attached() {
    let server = MockServer::start().await;
    Mock::given(path("/x"))
        .and(header("authorization", "Bearer legacy"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;
    http(store(&server.uri(), Some("legacy"), None))
        .get_json("/x")
        .await
        .unwrap();
}

#[tokio::test]
async fn origin_mismatch_refuses_before_sending_and_is_not_retried() {
    let server = MockServer::start().await;
    Mock::given(path("/x"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    let listener: RetryListener = Arc::new(move |a, m, d| seen.lock().unwrap().push((a, m, d)));
    let h = RommHttp::new(
        store(&server.uri(), Some("rmm_tok"), Some("https://other.host")),
        UA.into(),
        fast_retry(),
        Some(listener),
    )
    .unwrap();
    assert_eq!(h.get_json("/x").await.unwrap_err(), RommError::TokenHostMismatch);
    assert_eq!(
        h.get_json_once("/x", Duration::from_secs(1)).await.unwrap_err(),
        RommError::TokenHostMismatch
    );
    assert!(calls.lock().unwrap().is_empty());
    // Scheme downgrade is a different origin too.
    let https_token = store("http://127.0.0.1:1", Some("t"), Some("https://127.0.0.1:1"));
    assert_eq!(
        http(https_token).auth_header().unwrap_err(),
        RommError::TokenHostMismatch
    );
}

#[tokio::test]
async fn transient_errors_retry_with_progress_events() {
    let server = MockServer::start().await;
    Mock::given(path("/api/platforms"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(path("/api/platforms"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"id": 1}])))
        .mount(&server)
        .await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    let listener: RetryListener = Arc::new(move |a, m, d| seen.lock().unwrap().push((a, m, d)));
    let h = RommHttp::new(
        store(&server.uri(), None, None),
        UA.into(),
        fast_retry(),
        Some(listener),
    )
    .unwrap();
    assert_eq!(h.get_json("/api/platforms").await.unwrap(), json!([{"id": 1}]));
    let events = events.lock().unwrap().clone();
    assert_eq!(events.len(), 2);
    assert_eq!((events[0].0, events[0].1), (2, 3));
    assert_eq!((events[1].0, events[1].1), (3, 3));
    assert!((events[1].2 / events[0].2 - 3.0).abs() < 1e-9, "exponential backoff x3");
}

#[tokio::test]
async fn retries_exhaust_and_non_transient_fail_fast() {
    let server = MockServer::start().await;
    Mock::given(path("/down"))
        .respond_with(ResponseTemplate::new(500))
        .expect(3)
        .mount(&server)
        .await;
    Mock::given(path("/auth"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    let h = http(store(&server.uri(), None, None));
    assert!(matches!(
        h.get_json("/down").await.unwrap_err(),
        RommError::Server { status: 500, .. }
    ));
    assert!(matches!(h.get_json("/auth").await.unwrap_err(), RommError::Auth(_)));
}

#[tokio::test]
async fn status_translation() {
    let server = MockServer::start().await;
    for code in [400u16, 401, 403, 404, 409, 422, 429, 502] {
        Mock::given(path(format!("/s{code}")))
            .respond_with(ResponseTemplate::new(code))
            .mount(&server)
            .await;
    }
    let h = RommHttp::new(
        store(&server.uri(), None, None),
        UA.into(),
        RetryPolicy {
            max_attempts: 1,
            base_delay: Duration::ZERO,
        },
        None,
    )
    .unwrap();
    let err = |code: u16| {
        let h = &h;
        async move { h.get_json(&format!("/s{code}")).await.unwrap_err() }
    };
    let e400 = err(400).await;
    assert!(matches!(&e400, RommError::Api(m) if m.starts_with("Bad request (GET ")));
    let e401 = err(401).await;
    assert!(matches!(&e401, RommError::Auth(m) if m.starts_with("HTTP 401: Unauthorized (GET ")));
    assert!(matches!(err(403).await, RommError::Forbidden(_)));
    assert!(matches!(err(404).await, RommError::NotFound { detail: None, .. }));
    assert!(matches!(err(409).await, RommError::Api(_)));
    assert!(matches!(err(422).await, RommError::Api(_)));
    let e429 = err(429).await;
    assert!(matches!(&e429, RommError::Server { status: 429, message } if message.starts_with("Rate limited")));
    assert!(matches!(err(502).await, RommError::Server { status: 502, .. }));
}

#[tokio::test]
async fn transport_failures() {
    // Nothing listens on a just-closed port.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let h = http(store(&format!("http://127.0.0.1:{port}"), None, None));
    assert!(matches!(h.get_json("/x").await.unwrap_err(), RommError::Connection(_)));

    let server = MockServer::start().await;
    Mock::given(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;
    Mock::given(path("/garbage"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>"))
        .mount(&server)
        .await;
    let h = http(store(&server.uri(), None, None));
    assert!(matches!(
        h.get_json_once("/slow", Duration::from_millis(100)).await.unwrap_err(),
        RommError::Timeout(_)
    ));
    let garbage = h.get_json("/garbage").await.unwrap_err();
    assert!(matches!(&garbage, RommError::Api(m) if m.contains("Unexpected error")));
    // An unset URL never reaches the network.
    let h = http(store("", None, None));
    assert!(h.get_json_once("/x", Duration::from_secs(1)).await.is_err());
}

#[tokio::test]
async fn basic_auth_mint_never_carries_the_bearer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/client-tokens"))
        .and(header("authorization", "Basic YWxpY2U6c2VjcmV0"))
        .and(header("user-agent", UA))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 42, "raw_token": "rmm_minted"})))
        .expect(1)
        .mount(&server)
        .await;
    let settings = store(&server.uri(), Some("rmm_old"), Some(&server.uri()));
    let api = RommApi::new(Arc::new(http(settings)));
    let minted = api
        .mint_client_token("alice", "secret", "romm-emudecky-ng (Deck)")
        .await
        .unwrap();
    assert_eq!(minted, json!({"id": 42, "raw_token": "rmm_minted"}));
    let req = &server.received_requests().await.unwrap()[0];
    let body: Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(
        body,
        json!({"name": "romm-emudecky-ng (Deck)", "scopes": TOKEN_SCOPES, "expires_in": "never"})
    );
    assert!(!TOKEN_SCOPES.contains(&"me.write"));
}

#[tokio::test]
async fn mint_is_not_retried() {
    let server = MockServer::start().await;
    Mock::given(path("/api/client-tokens"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let api = RommApi::new(Arc::new(http(store(&server.uri(), None, None))));
    assert!(api.mint_client_token("a", "b", "n").await.is_err());
}

#[tokio::test]
async fn delete_swallows_not_found_and_handles_204() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/api/client-tokens/7"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/client-tokens/8"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/client-tokens/9"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let api = RommApi::new(Arc::new(http(store(&server.uri(), None, None))));
    api.delete_client_token("a", "b", "7").await.unwrap();
    api.delete_client_token("a", "b", "8").await.unwrap();
    assert!(api.delete_client_token("a", "b", "9").await.is_err());
}

async fn exchange_with(response: ResponseTemplate) -> (MockServer, Result<Value, RommError>) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/client-tokens/exchange"))
        .respond_with(response)
        .mount(&server)
        .await;
    // A stored bearer must never ride along on the public exchange.
    let api = RommApi::new(Arc::new(http(store(
        &server.uri(),
        Some("rmm_old"),
        Some(&server.uri()),
    ))));
    let result = api.exchange_pairing_code("ABCD1234").await;
    (server, result)
}

#[tokio::test]
async fn pairing_exchange_success_without_bearer() {
    let (server, result) =
        exchange_with(ResponseTemplate::new(200).set_body_json(json!({"id": 3, "raw_token": "rmm_paired"}))).await;
    assert_eq!(result.unwrap()["raw_token"], "rmm_paired");
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1);
    assert_eq!(auth_of(&reqs[0]), None);
    assert_eq!(
        serde_json::from_slice::<Value>(&reqs[0].body).unwrap(),
        json!({"code": "ABCD1234"})
    );
}

#[tokio::test]
async fn pairing_exchange_failure_mapping() {
    let gone = ResponseTemplate::new(404).set_body_json(json!({"detail": "Token No Longer Exists."}));
    assert_eq!(exchange_with(gone).await.1.unwrap_err(), RommError::PairingTokenGone);
    let invalid = ResponseTemplate::new(404).set_body_json(json!({"detail": "Invalid or expired code"}));
    assert_eq!(
        exchange_with(invalid).await.1.unwrap_err(),
        RommError::PairingCodeInvalid
    );
    let non_string = ResponseTemplate::new(404).set_body_json(json!({"detail": ["token no longer exists"]}));
    assert_eq!(
        exchange_with(non_string).await.1.unwrap_err(),
        RommError::PairingCodeInvalid
    );
    let no_body = ResponseTemplate::new(404);
    assert_eq!(
        exchange_with(no_body).await.1.unwrap_err(),
        RommError::PairingCodeInvalid
    );
    assert_eq!(
        exchange_with(ResponseTemplate::new(403)).await.1.unwrap_err(),
        RommError::PairingOwnerDisabled
    );
    let (server, limited) = exchange_with(ResponseTemplate::new(429)).await;
    assert_eq!(limited.unwrap_err(), RommError::PairingRateLimited);
    assert_eq!(server.received_requests().await.unwrap().len(), 1, "never retried");
    let (server, err) = exchange_with(ResponseTemplate::new(503)).await;
    assert!(matches!(err.unwrap_err(), RommError::Server { status: 503, .. }));
    assert_eq!(server.received_requests().await.unwrap().len(), 1, "never retried");
}

#[tokio::test]
async fn heartbeat_once_is_single_attempt_and_version_cache() {
    let server = MockServer::start().await;
    Mock::given(path("/api/heartbeat"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;
    let api = RommApi::new(Arc::new(http(store(&server.uri(), None, None))));
    assert!(api.heartbeat_once().await.is_err());
    api.set_version(Some("4.9.0".into()));
    assert_eq!(api.version().as_deref(), Some("4.9.0"));
    api.set_version(None);
    assert_eq!(api.version(), None);
}

#[tokio::test]
async fn users_me_and_platforms_paths() {
    let server = MockServer::start().await;
    Mock::given(path("/api/users/me"))
        .and(header_exists("authorization"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 1})))
        .mount(&server)
        .await;
    Mock::given(path("/api/platforms"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    let api = RommApi::new(Arc::new(http(store(&server.uri(), Some("t"), Some(&server.uri())))));
    assert_eq!(api.get_current_user().await.unwrap(), json!({"id": 1}));
    assert_eq!(api.list_platforms().await.unwrap(), json!([]));
}

#[test]
fn insecure_flag_selects_client() {
    let settings = store("https://romm.local", None, None);
    let h = http(settings.clone());
    assert!(std::ptr::eq(h.client(), &h.secure));
    settings.set("romm_allow_insecure_ssl", json!(true));
    assert!(std::ptr::eq(h.client(), &h.insecure));
}
