//! Range-resume transfer tests against a local fake RomM (wiremock) — the Rust
//! twin of the download parts of `tests/adapters/romm/test_http.py`.

use std::sync::{Arc, Mutex};

use serde_json::json;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::*;
use crate::adapters::romm::api::RommApi;
use crate::domain::download_failure::{Outcome, classify_download_failure};
use crate::services::ports::{RommRomApi, TransferMode, TransferOutcome};

const PAYLOAD_LEN: usize = 4096;

fn payload() -> Vec<u8> {
    (0..PAYLOAD_LEN).map(|i| (i % 251) as u8).collect()
}

fn http(server: &MockServer) -> RommHttp {
    let settings = SettingsStore::new(
        json!({"romm_url": server.uri(), "romm_api_token": "tok", "romm_api_token_origin": server.uri()})
            .as_object()
            .cloned()
            .unwrap(),
    );
    RommHttp::new(
        settings,
        "ua/1".into(),
        RetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(1),
        },
        None,
    )
    .unwrap()
}

type Seen = Arc<Mutex<Vec<(u64, u64)>>>;
type Meta = Arc<Mutex<Vec<bool>>>;

fn hooks(cancel: CancellationToken) -> (TransferHooks, Seen, Meta) {
    let seen: Seen = Arc::default();
    let meta: Meta = Arc::default();
    let (s, m) = (seen.clone(), meta.clone());
    (
        TransferHooks {
            progress: Box::new(move |d, t| s.lock().unwrap().push((d, t))),
            on_meta: Some(Box::new(move |r| m.lock().unwrap().push(r))),
            on_phase: Box::new(|_| {}),
            on_identity: None,
            cancel,
        },
        seen,
        meta,
    )
}

fn range_of(req: &Request) -> Option<String> {
    req.headers.get("range").map(|v| v.to_str().unwrap().to_string())
}

async fn run(server: &MockServer, dest: &str, resume: bool) -> (anyhow::Result<TransferOutcome>, Seen, Meta) {
    let (h, seen, meta) = hooks(CancellationToken::new());
    let r = http(server).download("/api/roms/7/content/x.nsp", dest, TransferMode::Single { resume }, h).await;
    (r, seen, meta)
}

fn tmp(dir: &tempfile::TempDir) -> String {
    dir.path().join("sub/x.nsp.tmp").to_string_lossy().into_owned()
}

#[tokio::test]
async fn fresh_download_writes_file_and_reports_progress() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/roms/7/content/x.nsp"))
        .and(header("authorization", "Bearer tok"))
        .and(header("user-agent", "ua/1"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(payload()))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = tmp(&dir);
    let (r, seen, meta) = run(&server, &dest, false).await;
    r.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), payload());
    assert_eq!(seen.lock().unwrap().last(), Some(&(4096, 4096)));
    assert_eq!(*meta.lock().unwrap(), vec![false]);
    assert!(range_of(&server.received_requests().await.unwrap()[0]).is_none());
}

#[tokio::test]
async fn resume_206_appends_from_partial() {
    let server = MockServer::start().await;
    let body = payload();
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(206)
                .insert_header("content-range", "bytes 1500-4095/4096")
                .set_body_bytes(body[1500..].to_vec()),
        )
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = tmp(&dir);
    std::fs::create_dir_all(dir.path().join("sub")).unwrap();
    std::fs::write(&dest, &body[..1500]).unwrap();
    let (r, seen, meta) = run(&server, &dest, true).await;
    r.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body);
    assert_eq!(
        range_of(&server.received_requests().await.unwrap()[0]).as_deref(),
        Some("bytes=1500-")
    );
    assert_eq!(seen.lock().unwrap()[0].1, 4096);
    assert!(seen.lock().unwrap()[0].0 > 1500);
    assert_eq!(*meta.lock().unwrap(), vec![true]);
}

#[tokio::test]
async fn resume_206_at_wrong_offset_rewrites_and_fails_loudly_when_short() {
    let server = MockServer::start().await;
    let body = payload();
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(206)
                .insert_header("content-range", "bytes 0-4095/4096")
                .set_body_bytes(body.clone()),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = tmp(&dir);
    std::fs::create_dir_all(dir.path().join("sub")).unwrap();
    std::fs::write(&dest, b"stale partial bytes").unwrap();
    let (r, _, _) = run(&server, &dest, true).await;
    r.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body, "truncated and rewritten, never appended");

    // A 206 at an offset we did not ask for restarts and validates against
    // the full total: the short body fails as incomplete (resumable).
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(206)
                .insert_header("content-range", "bytes 100-4095/4096")
                .set_body_bytes(body[100..].to_vec()),
        )
        .expect(3)
        .mount(&server)
        .await;
    std::fs::write(&dest, &body[..50]).unwrap();
    let (r, _, _) = run(&server, &dest, true).await;
    let err = r.unwrap_err();
    assert!(err.to_string().contains("got 3996 bytes, expected 4096"), "{err}");
    assert_eq!(classify_download_failure(&err), Outcome::Interrupted("server_unreachable"));
}

#[tokio::test]
async fn plain_200_on_resume_restarts_from_zero() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("accept-ranges", "bytes")
                .set_body_bytes(payload()),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = tmp(&dir);
    std::fs::create_dir_all(dir.path().join("sub")).unwrap();
    std::fs::write(&dest, vec![9u8; 3000]).unwrap();
    let (r, _, meta) = run(&server, &dest, true).await;
    r.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), payload());
    assert_eq!(*meta.lock().unwrap(), vec![true]);
}

#[tokio::test]
async fn cloudflare_edge_vetoes_resumability() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("accept-ranges", "bytes")
                .insert_header("cf-ray", "8abc-FRA")
                .set_body_bytes(payload()),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let (r, _, meta) = run(&server, &tmp(&dir), false).await;
    r.unwrap();
    assert_eq!(*meta.lock().unwrap(), vec![false]);
    let mut h = HeaderMap::new();
    h.insert("server", "cloudflare".parse().unwrap());
    assert!(!range_supported(StatusCode::PARTIAL_CONTENT, &h));
    assert!(range_supported(StatusCode::PARTIAL_CONTENT, &HeaderMap::new()));
}

#[tokio::test]
async fn short_stream_is_interrupted_and_overshoot_fails() {
    let body = payload();
    let dir = tempfile::tempdir().unwrap();
    let dest = tmp(&dir);
    std::fs::create_dir_all(dir.path().join("sub")).unwrap();
    for (total, expected) in [
        (5000, Outcome::Interrupted("server_unreachable")),
        (3000, Outcome::Failed),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("content-range", format!("bytes 10-4095/{total}"))
                    .set_body_bytes(body[10..].to_vec()),
            )
            .mount(&server)
            .await;
        std::fs::write(&dest, &body[..10]).unwrap();
        let (r, _, _) = run(&server, &dest, true).await;
        assert_eq!(classify_download_failure(&r.unwrap_err()), expected, "total {total}");
    }
}

#[tokio::test]
async fn empty_body_and_http_errors() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let (r, _, _) = run(&server, &tmp(&dir), false).await;
    let err = r.unwrap_err();
    assert!(err.to_string().contains("Download produced 0 bytes"));
    assert_eq!(classify_download_failure(&err), Outcome::Failed);

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;
    let (r, _, meta) = run(&server, &tmp(&dir), false).await;
    assert_eq!(classify_download_failure(&r.unwrap_err()), Outcome::Failed);
    assert!(meta.lock().unwrap().is_empty());

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .expect(3)
        .mount(&server)
        .await;
    let (r, _, _) = run(&server, &tmp(&dir), false).await;
    assert_eq!(
        classify_download_failure(&r.unwrap_err()),
        Outcome::Interrupted("server_unreachable")
    );

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    let (r, _, _) = run(&server, &tmp(&dir), false).await;
    assert_eq!(classify_download_failure(&r.unwrap_err()), Outcome::Interrupted("auth_failed"));
}

#[tokio::test]
async fn unreachable_server_is_interrupted() {
    let uri = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let settings = SettingsStore::new(json!({"romm_url": uri}).as_object().cloned().unwrap());
    let h = RommHttp::new(
        settings,
        "ua".into(),
        RetryPolicy {
            max_attempts: 2,
            base_delay: Duration::from_millis(1),
        },
        None,
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (hk, _, _) = hooks(CancellationToken::new());
    let err = h.download("/x", &tmp(&dir), TransferMode::Single { resume: false }, hk).await.unwrap_err();
    assert_eq!(classify_download_failure(&err), Outcome::Interrupted("server_unreachable"));
}

#[tokio::test]
async fn local_write_failure_keeps_the_io_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(payload()))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    // The destination is a directory: opening it for writing fails locally.
    let dest = dir.path().join("sub/x.nsp.tmp");
    std::fs::create_dir_all(&dest).unwrap();
    let (r, _, _) = run(&server, &dest.to_string_lossy(), false).await;
    let err = r.unwrap_err();
    assert!(err.chain().any(|c| c.downcast_ref::<std::io::Error>().is_some()));
    // A full disk anywhere in that chain keeps the partial (interrupted).
    let enospc = local_io(std::io::Error::from_raw_os_error(libc::ENOSPC), "writing the download file");
    assert_eq!(classify_download_failure(&enospc), Outcome::Interrupted("disk_full"));
    assert!(matches!(enospc.downcast_ref::<RommError>(), Some(RommError::Connection(_))));
}

#[tokio::test]
async fn cancellation_stops_the_transfer() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(payload())
                .set_delay(Duration::from_secs(5)),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let (h, _, _) = hooks(cancel.clone());
    let client = http(&server);
    let dest = tmp(&dir);
    let task = async { client.download("/a", &dest, TransferMode::Single { resume: false }, h).await };
    let stopper = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();
    };
    let (r, ()) = tokio::join!(task, stopper);
    assert!(r.unwrap_err().downcast_ref::<TransferCancelled>().is_some());

    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let (h, _, _) = hooks(cancelled);
    let err = http(&server).download("/a", &dest, TransferMode::Single { resume: false }, h).await.unwrap_err();
    assert!(err.downcast_ref::<TransferCancelled>().is_some());
}

#[tokio::test]
async fn api_quotes_the_file_name_like_python() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/roms/7/content/Mario%2520Kart%2520%255Bv0%255D\.nsp$"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"abc".to_vec()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/roms/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 7})))
        .mount(&server)
        .await;
    let api = RommApi::new(Arc::new(http(&server)));
    assert_eq!(api.get_rom(7).await.unwrap()["id"], 7);
    let dir = tempfile::tempdir().unwrap();
    let (h, _, _) = hooks(CancellationToken::new());
    api.download_rom_content(7, "Mario Kart [v0].nsp", &tmp(&dir), TransferMode::Single { resume: false }, h)
        .await
        .unwrap();
}

#[test]
fn content_range_parsing() {
    assert_eq!(parse_content_range(Some("bytes 10-99/100")), Some((10, 100)));
    assert_eq!(parse_content_range(Some("BYTES 0-0/1")), Some((0, 1)));
    assert_eq!(parse_content_range(Some("bytes 0-9/*")), None);
    assert_eq!(parse_content_range(Some("items 0-9/10")), None);
    assert_eq!(parse_content_range(Some("bytes x-9/10")), None);
    assert_eq!(parse_content_range(Some("garbage")), None);
    assert_eq!(parse_content_range(None), None);
    let mut h = HeaderMap::new();
    h.insert("content-length", "77".parse().unwrap());
    assert_eq!(resume_branch(StatusCode::OK, &h, 50), (false, 0, 77));
    assert_eq!(resume_branch(StatusCode::PARTIAL_CONTENT, &h, 50), (false, 0, 77));
    h.insert("content-range", "bytes 50-99/100".parse().unwrap());
    assert_eq!(resume_branch(StatusCode::PARTIAL_CONTENT, &h, 50), (true, 50, 100));
    assert_eq!(resume_branch(StatusCode::PARTIAL_CONTENT, &h, 0), (false, 0, 100));
    assert!(validate_download(10, 10).is_ok());
    assert!(validate_download(0, 5).is_ok());
}
