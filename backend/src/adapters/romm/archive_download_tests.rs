//! Multipart ("cached ZIP") transfer behaviour against a scriptable server.
//!
//! The server in `test_server.rs` imitates what RomM does: `Range` requests are
//! answered from one cached archive with an `ETag`, `If-Range` with another tag
//! gets the whole new representation, and archive preparation shows up as
//! silence before the response headers.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::adapters::romm::test_server::{Body, Reply, Req, TestServer, cached_zip_reply};
use super::*;
use crate::domain::archive_transfer::{ArchiveIdentity, ResumePlan};
use crate::domain::download_failure::ArchiveRecoveryNeeded;
use crate::services::ports::{TransferMode, TransferOutcome, TransferPhase};

const ETAG: &str = "\"cached-v1\"";
const LAST_MODIFIED: &str = "Mon, 01 Jan 2026 00:00:00 GMT";
const PATH: &str = "/api/roms/7/content/Game.zip";

fn zip_bytes(seed: u8) -> Vec<u8> {
    (0..200_000usize).map(|i| ((i * 7 + usize::from(seed)) % 251) as u8).collect()
}

fn identity(zip: &[u8]) -> ArchiveIdentity {
    ArchiveIdentity {
        etag: ETAG.into(),
        last_modified: Some(LAST_MODIFIED.into()),
        length: zip.len() as u64,
    }
}

fn fast_timeouts() -> TransferTimeouts {
    TransferTimeouts {
        connect: Duration::from_secs(2),
        headers_single: Duration::from_millis(300),
        headers_archive: Duration::from_secs(3),
        body_inactivity: Duration::from_millis(400),
    }
}

fn http_with(server: &TestServer, timeouts: TransferTimeouts, base_delay: Duration) -> RommHttp {
    let settings = SettingsStore::new(
        json!({"romm_url": server.url, "romm_api_token": "tok", "romm_api_token_origin": server.url})
            .as_object()
            .cloned()
            .unwrap(),
    );
    RommHttp::new(settings, "ua/1".into(), RetryPolicy { max_attempts: 3, base_delay }, None)
        .unwrap()
        .with_timeouts(timeouts)
        .unwrap()
}

fn http(server: &TestServer) -> RommHttp {
    http_with(server, fast_timeouts(), Duration::from_millis(1))
}

#[derive(Default)]
struct Seen {
    progress: Vec<(u64, u64)>,
    meta: Vec<bool>,
    phases: Vec<TransferPhase>,
    identities: Vec<Option<ArchiveIdentity>>,
}

fn hooks(cancel: CancellationToken) -> (TransferHooks, Arc<Mutex<Seen>>) {
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let (p, m, ph, id) = (seen.clone(), seen.clone(), seen.clone(), seen.clone());
    let hooks = TransferHooks {
        progress: Box::new(move |d, t| p.lock().unwrap().progress.push((d, t))),
        on_meta: Some(Box::new(move |r| m.lock().unwrap().meta.push(r))),
        on_phase: Box::new(move |phase| ph.lock().unwrap().phases.push(phase)),
        on_identity: Some(Box::new(move |identity| {
            id.lock().unwrap().identities.push(identity);
            Box::pin(async { Ok(()) })
        })),
        cancel,
    };
    (hooks, seen)
}

fn dest(dir: &tempfile::TempDir) -> String {
    dir.path().join("roms/Game.zip.tmp").to_string_lossy().into_owned()
}

fn seed_partial(path: &str, bytes: &[u8]) {
    std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

async fn run(
    server: &TestServer,
    dest: &str,
    plan: ResumePlan,
) -> (anyhow::Result<TransferOutcome>, Arc<Mutex<Seen>>) {
    let (h, seen) = hooks(CancellationToken::new());
    let r = http(server).download(PATH, dest, TransferMode::Archive(plan), h).await;
    (r, seen)
}

fn cached(zip: Vec<u8>) -> impl Fn(&Req, usize) -> Reply + Send + Sync + 'static {
    move |req, _| cached_zip_reply(req, &zip, ETAG, LAST_MODIFIED)
}

// ── the first request ─────────────────────────────────────────────────────

#[tokio::test]
async fn a_new_multipart_download_asks_for_the_cached_zip_from_the_first_byte() {
    let zip = zip_bytes(1);
    let server = TestServer::start(cached(zip.clone())).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dest(&dir);
    let (r, seen) = run(&server, &dest, ResumePlan::Fresh).await;
    assert_eq!(r.unwrap(), TransferOutcome::Downloaded);
    let reqs = server.requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].header("range"), Some("bytes=0-"));
    assert_eq!(reqs[0].header("accept-encoding"), Some("identity"));
    assert_eq!(reqs[0].header("if-range"), None);
    assert_eq!(reqs[0].header("authorization"), Some("Bearer tok"));
    assert_eq!(std::fs::read(&dest).unwrap(), zip);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.identities, vec![Some(identity(&zip))]);
    assert_eq!(seen.meta, vec![true]);
    assert_eq!(seen.progress.last(), Some(&(zip.len() as u64, zip.len() as u64)));
    assert_eq!(
        seen.phases.first(),
        Some(&TransferPhase::Waiting { attempt: 1, max_attempts: 3 })
    );
    assert_eq!(seen.phases.last(), Some(&TransferPhase::Transferring));
}

#[tokio::test]
async fn the_exact_etag_header_value_is_reported() {
    let zip = zip_bytes(1);
    let weird = "\"a/b+c=d==\"";
    let server = TestServer::start({
        let zip = zip.clone();
        move |req, _| cached_zip_reply(req, &zip, weird, LAST_MODIFIED)
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let (r, seen) = run(&server, &dest(&dir), ResumePlan::Fresh).await;
    r.unwrap();
    assert_eq!(seen.lock().unwrap().identities[0].as_ref().unwrap().etag, weird);
}

#[tokio::test]
async fn a_streamed_200_has_no_validator_and_is_not_promised_resumable() {
    let zip = zip_bytes(2);
    // RomM streams a generated ZIP when Range is not honoured (e.g. stripped by an edge).
    let server = TestServer::start({
        let zip = zip.clone();
        move |_, _| Reply::new(200, zip.clone())
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dest(&dir);
    let (r, seen) = run(&server, &dest, ResumePlan::Fresh).await;
    r.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), zip);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.identities, vec![None]);
    assert_eq!(seen.meta, vec![false]);
}

#[tokio::test]
async fn a_weak_etag_is_never_recorded() {
    let zip = zip_bytes(2);
    let server = TestServer::start({
        let zip = zip.clone();
        move |_, _| Reply::new(200, zip.clone()).header("etag", "W/\"weak\"")
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let (r, seen) = run(&server, &dest(&dir), ResumePlan::Fresh).await;
    r.unwrap();
    assert_eq!(seen.lock().unwrap().identities, vec![None]);
}

// ── timeouts and waiting ──────────────────────────────────────────────────

#[tokio::test]
async fn waiting_for_archive_headers_outlasts_the_single_file_deadline() {
    let zip = zip_bytes(1);
    let server = TestServer::start({
        let zip = zip.clone();
        move |req, _| cached_zip_reply(req, &zip, ETAG, LAST_MODIFIED).delayed(Duration::from_millis(900))
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    // 900 ms is three times the single-file header deadline (300 ms) ...
    let single = http(&server)
        .download("/api/roms/7/content/x.nsp", &dest(&dir), TransferMode::Single { resume: false }, hooks(CancellationToken::new()).0)
        .await
        .unwrap_err();
    assert!(matches!(single.downcast_ref::<RommError>(), Some(RommError::Timeout(_))), "{single:#}");
    // ... but the archive request waits (its deadline is 3 s).
    let dest = dest(&dir);
    let (r, _) = run(&server, &dest, ResumePlan::Fresh).await;
    assert_eq!(r.unwrap(), TransferOutcome::Downloaded);
    assert_eq!(std::fs::read(&dest).unwrap(), zip);
}

#[tokio::test]
async fn the_header_deadline_covers_the_whole_wait_and_is_a_timeout() {
    let server = TestServer::start(|_, _| Reply::new(200, vec![1]).delayed(Duration::from_secs(30))).await;
    let mut t = fast_timeouts();
    t.headers_archive = Duration::from_millis(300);
    let h = http_with(&server, t, Duration::from_millis(1));
    let dir = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let err = h
        .download(PATH, &dest(&dir), TransferMode::Archive(ResumePlan::Fresh), hooks(CancellationToken::new()).0)
        .await
        .unwrap_err();
    assert!(matches!(err.downcast_ref::<RommError>(), Some(RommError::Timeout(_))), "{err:#}");
    // three bounded attempts of ~300 ms, not 30 s
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(server.requests().len(), 3);
    assert!(server.requests().iter().all(|r| r.header("range") == Some("bytes=0-")));
}

#[tokio::test]
async fn cancel_while_headers_are_pending_returns_promptly_and_writes_nothing() {
    let server = TestServer::start(|_, _| Reply::new(200, vec![1]).delayed(Duration::from_secs(30))).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dest(&dir);
    let cancel = CancellationToken::new();
    let (h, _) = hooks(cancel.clone());
    let task = tokio::spawn({
        let dest = dest.clone();
        let http = http(&server);
        async move { http.download(PATH, &dest, TransferMode::Archive(ResumePlan::Fresh), h).await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = Instant::now();
    cancel.cancel();
    let err = task.await.unwrap().unwrap_err();
    assert!(err.downcast_ref::<TransferCancelled>().is_some(), "{err:#}");
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(!std::path::Path::new(&dest).exists());
}

#[tokio::test]
async fn cancel_during_retry_backoff_returns_promptly() {
    let server = TestServer::start(|_, _| Reply::new(503, Vec::new())).await;
    let h = http_with(&server, fast_timeouts(), Duration::from_secs(30));
    let dir = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let (hk, seen) = hooks(cancel.clone());
    let dest = dest(&dir);
    let task = tokio::spawn(async move { h.download(PATH, &dest, TransferMode::Archive(ResumePlan::Fresh), hk).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = Instant::now();
    cancel.cancel();
    let err = task.await.unwrap().unwrap_err();
    assert!(err.downcast_ref::<TransferCancelled>().is_some());
    assert!(started.elapsed() < Duration::from_secs(1));
    let phases = seen.lock().unwrap().phases.clone();
    assert!(
        phases.iter().any(|p| matches!(p, TransferPhase::Retrying { attempt: 2, max_attempts: 3, .. })),
        "{phases:?}"
    );
}

#[tokio::test]
async fn a_slow_but_healthy_body_is_not_cut_by_a_total_deadline() {
    let zip = zip_bytes(1);
    // 1.2 s in total, three times the 400 ms inactivity deadline, but data keeps arriving.
    let server = TestServer::start({
        let zip = zip.clone();
        move |req, _| {
            cached_zip_reply(req, &zip, ETAG, LAST_MODIFIED)
                .plan(Body::Drip { chunk: zip.len() / 8 + 1, every: Duration::from_millis(150) })
        }
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dest(&dir);
    let started = Instant::now();
    let (r, _) = run(&server, &dest, ResumePlan::Fresh).await;
    assert_eq!(r.unwrap(), TransferOutcome::Downloaded);
    assert!(started.elapsed() > Duration::from_millis(1000));
    assert_eq!(std::fs::read(&dest).unwrap(), zip);
    assert_eq!(server.requests().len(), 1, "no retry was needed");
}

#[tokio::test]
async fn a_stalled_body_ends_as_a_timeout_and_keeps_the_partial() {
    let zip = zip_bytes(1);
    let server = TestServer::start({
        let zip = zip.clone();
        move |req, _| cached_zip_reply(req, &zip, ETAG, LAST_MODIFIED).plan(Body::StallAfter(5000))
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dest(&dir);
    let (r, _) = run(&server, &dest, ResumePlan::Fresh).await;
    let err = r.unwrap_err();
    assert!(
        matches!(err.downcast_ref::<RommError>(), Some(RommError::Timeout(m)) if m.contains("stalled")),
        "{err:#}"
    );
    // every attempt resumed from the actual size and added its 5000 bytes; nothing was lost
    assert_eq!(std::fs::read(&dest).unwrap(), zip[..15_000]);
    let reqs = server.requests();
    assert_eq!(reqs[0].header("range"), Some("bytes=0-"));
    assert_eq!(reqs[1].header("range"), Some("bytes=5000-"));
    assert_eq!(reqs[1].header("if-range"), Some(ETAG));
    assert_eq!(reqs[2].header("range"), Some("bytes=10000-"));
}

// ── resuming ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_matching_etag_and_a_valid_206_append_exactly_the_missing_bytes() {
    let zip = zip_bytes(1);
    let server = TestServer::start(cached(zip.clone())).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dest(&dir);
    seed_partial(&dest, &zip[..60_000]);
    let (r, seen) = run(&server, &dest, ResumePlan::Append(identity(&zip))).await;
    assert_eq!(r.unwrap(), TransferOutcome::Downloaded);
    let reqs = server.requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].header("range"), Some("bytes=60000-"));
    assert_eq!(reqs[0].header("if-range"), Some(ETAG));
    assert_eq!(reqs[0].header("accept-encoding"), Some("identity"));
    // byte for byte, not just the same size
    assert_eq!(std::fs::read(&dest).unwrap(), zip);
    let seen = seen.lock().unwrap();
    assert!(seen.identities.is_empty(), "the saved identity is still valid; nothing to persist");
    assert!(seen.progress[0].0 > 60_000);
}

#[tokio::test]
async fn the_actual_file_size_is_the_resume_offset_not_a_counter() {
    let zip = zip_bytes(1);
    let server = TestServer::start(cached(zip.clone())).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dest(&dir);
    seed_partial(&dest, &zip[..12_345]);
    let (r, _) = run(&server, &dest, ResumePlan::Append(identity(&zip))).await;
    r.unwrap();
    assert_eq!(server.requests()[0].header("range"), Some("bytes=12345-"));
    assert_eq!(std::fs::read(&dest).unwrap(), zip);
}

#[tokio::test]
async fn a_changed_representation_is_replaced_and_never_appended() {
    let old = zip_bytes(1);
    let new = zip_bytes(9);
    let server = TestServer::start({
        let new = new.clone();
        move |req, _| cached_zip_reply(req, &new, "\"cached-v2\"", LAST_MODIFIED).delayed(Duration::from_millis(600))
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dest(&dir);
    seed_partial(&dest, &old[..60_000]);
    let (h, seen) = hooks(CancellationToken::new());
    let task = tokio::spawn({
        let (dest, http, id) = (dest.clone(), http(&server), identity(&old));
        async move { http.download(PATH, &dest, TransferMode::Archive(ResumePlan::Append(id)), h).await }
    });
    // While the response is still pending the old partial is untouched.
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(std::fs::read(&dest).unwrap(), old[..60_000]);
    assert_eq!(task.await.unwrap().unwrap(), TransferOutcome::Downloaded);
    // The 200 was never appended: the file is exactly the new archive.
    assert_eq!(std::fs::read(&dest).unwrap(), new);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.identities.len(), 1);
    assert_eq!(seen.identities[0].as_ref().unwrap().etag, "\"cached-v2\"");
    assert_eq!(seen.identities[0].as_ref().unwrap().length, new.len() as u64);
    assert_eq!(server.requests()[0].header("if-range"), Some(ETAG));
    // Right the first time: appending the 200 and repairing it on a retry would also end
    // with the right bytes, so the number of requests is what proves nothing was appended.
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn a_failed_replacement_leaves_the_old_partial_alone() {
    let old = zip_bytes(1);
    let server = TestServer::start(|_, _| Reply::new(502, Vec::new())).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dest(&dir);
    seed_partial(&dest, &old[..60_000]);
    let (r, _) = run(&server, &dest, ResumePlan::Append(identity(&old))).await;
    assert!(r.is_err());
    assert_eq!(std::fs::read(&dest).unwrap(), old[..60_000]);
}

#[tokio::test]
async fn a_fresh_restart_replaces_an_existing_partial_only_after_the_response_is_accepted() {
    let zip = zip_bytes(1);
    let server = TestServer::start({
        let zip = zip.clone();
        move |req, _| cached_zip_reply(req, &zip, ETAG, LAST_MODIFIED).delayed(Duration::from_millis(500))
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dest(&dir);
    seed_partial(&dest, b"old bytes of unknown origin");
    let (h, _) = hooks(CancellationToken::new());
    let task = tokio::spawn({
        let (dest, http) = (dest.clone(), http(&server));
        async move { http.download(PATH, &dest, TransferMode::Archive(ResumePlan::Fresh), h).await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(std::fs::read(&dest).unwrap(), b"old bytes of unknown origin");
    task.await.unwrap().unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), zip);
}

// ── responses that must be rejected ───────────────────────────────────────

fn is_recovery(err: &anyhow::Error) -> bool {
    err.chain().any(|c| c.downcast_ref::<ArchiveRecoveryNeeded>().is_some())
}

#[tokio::test]
async fn inconsistent_resume_responses_are_rejected_and_the_partial_is_untouched() {
    let zip = zip_bytes(1);
    let total = zip.len();
    let body = zip[60_000..].to_vec();
    let cases: Vec<(&str, Reply)> = vec![
        (
            "wrong start offset",
            Reply::new(206, body.clone())
                .header("content-range", &format!("bytes 59999-{}/{total}", total - 1))
                .header("etag", ETAG),
        ),
        (
            "inconsistent total",
            Reply::new(206, body.clone())
                .header("content-range", &format!("bytes 60000-{}/{}", total, total + 1))
                .header("etag", ETAG),
        ),
        (
            "contradicting validator",
            Reply::new(206, body.clone())
                .header("content-range", &format!("bytes 60000-{}/{total}", total - 1))
                .header("etag", "\"other\""),
        ),
        (
            "content encoding",
            Reply::new(206, body.clone())
                .header("content-range", &format!("bytes 60000-{}/{total}", total - 1))
                .header("content-encoding", "gzip")
                .header("etag", ETAG),
        ),
        ("no content-range", Reply::new(206, body.clone()).header("etag", ETAG)),
    ];
    for (name, reply) in cases {
        let server = TestServer::start(move |_, _| reply.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dest(&dir);
        seed_partial(&dest, &zip[..60_000]);
        let (r, _) = run(&server, &dest, ResumePlan::Append(identity(&zip))).await;
        let err = r.unwrap_err();
        assert!(is_recovery(&err), "{name}: {err:#}");
        assert_eq!(std::fs::read(&dest).unwrap(), zip[..60_000], "{name}: partial changed");
        assert_eq!(server.requests().len(), 1, "{name}: must not be retried blindly");
    }
}

#[tokio::test]
async fn a_416_with_the_full_saved_length_reports_a_complete_candidate_without_touching_it() {
    let zip = zip_bytes(1);
    let server = TestServer::start(cached(zip.clone())).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dest(&dir);
    seed_partial(&dest, &zip);
    let (r, _) = run(&server, &dest, ResumePlan::Append(identity(&zip))).await;
    assert_eq!(r.unwrap(), TransferOutcome::AlreadyComplete);
    assert_eq!(std::fs::read(&dest).unwrap(), zip);
    assert_eq!(server.requests()[0].header("range"), Some(&*format!("bytes={}-", zip.len())));
}

#[tokio::test]
async fn a_416_for_a_shorter_partial_is_not_success() {
    let zip = zip_bytes(1);
    let server = TestServer::start(|_, _| Reply::new(416, Vec::new()).header("content-range", "bytes */5")).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dest(&dir);
    seed_partial(&dest, &zip[..60_000]);
    let (r, _) = run(&server, &dest, ResumePlan::Append(identity(&zip))).await;
    assert!(is_recovery(&r.unwrap_err()));
    assert_eq!(std::fs::read(&dest).unwrap(), zip[..60_000]);
}

#[tokio::test]
async fn unexpected_responses_are_never_written_into_the_archive() {
    let json_error = || Reply::new(200, br#"{"detail":"boom"}"#.to_vec()).header("content-type", "application/json");
    let accepted = || Reply::new(202, br#"{"status":"converting"}"#.to_vec()).header("content-type", "application/json");
    for (name, reply, expect_requests) in [
        ("202 accepted", accepted(), 3),
        ("json 200", json_error(), 3),
        ("html 200", Reply::new(200, b"<html>".to_vec()).header("content-type", "text/html"), 3),
        ("401", Reply::new(401, Vec::new()), 1),
        ("404", Reply::new(404, Vec::new()), 1),
    ] {
        let server = TestServer::start(move |_, _| reply.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dest(&dir);
        let (r, _) = run(&server, &dest, ResumePlan::Fresh).await;
        let err = r.unwrap_err();
        assert!(!std::path::Path::new(&dest).exists(), "{name}: something was written");
        assert_eq!(server.requests().len(), expect_requests, "{name}");
        match name {
            "401" => assert!(matches!(err.downcast_ref::<RommError>(), Some(RommError::Auth(_))), "{err:#}"),
            "404" => assert!(matches!(err.downcast_ref::<RommError>(), Some(RommError::NotFound { .. })), "{err:#}"),
            _ => assert!(
                matches!(err.downcast_ref::<RommError>(), Some(RommError::Connection(m)) if m.contains("Unexpected response")),
                "{name}: {err:#}"
            ),
        }
    }
}

#[tokio::test]
async fn rejected_responses_do_not_destroy_an_existing_partial() {
    let zip = zip_bytes(1);
    let server = TestServer::start(|_, _| Reply::new(202, b"{}".to_vec())).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dest(&dir);
    seed_partial(&dest, &zip[..60_000]);
    let (r, _) = run(&server, &dest, ResumePlan::Fresh).await;
    assert!(r.is_err());
    assert_eq!(std::fs::read(&dest).unwrap(), zip[..60_000]);
}

// ── failures, retries and ownership ──────────────────────────────────────

#[tokio::test]
async fn failures_before_headers_retry_from_byte_zero_with_one_worker_at_a_time() {
    let zip = zip_bytes(1);
    let server = TestServer::start({
        let zip = zip.clone();
        move |req, i| match i {
            0 => Reply::new(503, Vec::new()),
            1 => Reply::dropped(),
            _ => cached_zip_reply(req, &zip, ETAG, LAST_MODIFIED),
        }
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dest(&dir);
    let (r, seen) = run(&server, &dest, ResumePlan::Fresh).await;
    assert_eq!(r.unwrap(), TransferOutcome::Downloaded);
    let reqs = server.requests();
    assert_eq!(reqs.len(), 3);
    assert!(reqs.iter().all(|r| r.header("range") == Some("bytes=0-") && r.header("if-range").is_none()));
    assert_eq!(server.peak_connections(), 1, "never two requests for one job at once");
    assert_eq!(std::fs::read(&dest).unwrap(), zip);
    let phases = seen.lock().unwrap().phases.clone();
    assert!(phases.iter().any(|p| matches!(p, TransferPhase::Retrying { attempt: 2, .. })), "{phases:?}");
    assert!(phases.iter().any(|p| matches!(p, TransferPhase::Retrying { attempt: 3, .. })), "{phases:?}");
    assert!(phases.contains(&TransferPhase::Waiting { attempt: 3, max_attempts: 3 }));
}

#[tokio::test]
async fn retries_are_bounded() {
    let server = TestServer::start(|_, _| Reply::dropped()).await;
    let dir = tempfile::tempdir().unwrap();
    let (r, _) = run(&server, &dest(&dir), ResumePlan::Fresh).await;
    assert!(r.is_err());
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn a_dropped_body_resumes_from_the_actual_size_in_the_same_call() {
    let zip = zip_bytes(1);
    let server = TestServer::start({
        let zip = zip.clone();
        move |req, i| {
            let reply = cached_zip_reply(req, &zip, ETAG, LAST_MODIFIED);
            if i == 0 { reply.plan(Body::AbortAfter(70_000)) } else { reply }
        }
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dest(&dir);
    let (r, seen) = run(&server, &dest, ResumePlan::Fresh).await;
    assert_eq!(r.unwrap(), TransferOutcome::Downloaded);
    let reqs = server.requests();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0].header("range"), Some("bytes=0-"));
    let resumed_from: u64 = reqs[1].header("range").unwrap().trim_start_matches("bytes=").trim_end_matches('-').parse().unwrap();
    assert!(resumed_from > 0 && resumed_from <= 70_000, "{resumed_from}");
    assert_eq!(reqs[1].header("if-range"), Some(ETAG));
    assert_eq!(std::fs::read(&dest).unwrap(), zip, "byte-identical after the interruption");
    assert_eq!(server.peak_connections(), 1);
    // The identity was saved once, before the first byte; the resume reused it.
    assert_eq!(seen.lock().unwrap().identities.len(), 1);
}

#[tokio::test]
async fn a_restarted_server_that_rebuilt_the_archive_is_recovered_without_appending() {
    let zip = zip_bytes(1);
    let rebuilt = zip_bytes(5);
    // The first response is cut off; when the client comes back RomM has rebuilt the
    // archive (new tag), so the conditional resume gets a whole new representation.
    let server = TestServer::start({
        let (zip, rebuilt) = (zip.clone(), rebuilt.clone());
        move |req, i| {
            if i == 0 {
                cached_zip_reply(req, &zip, ETAG, LAST_MODIFIED).plan(Body::AbortAfter(50_000))
            } else {
                cached_zip_reply(req, &rebuilt, "\"cached-v2\"", LAST_MODIFIED)
            }
        }
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dest(&dir);
    let (r, seen) = run(&server, &dest, ResumePlan::Fresh).await;
    assert_eq!(r.unwrap(), TransferOutcome::Downloaded);
    assert_eq!(std::fs::read(&dest).unwrap(), rebuilt);
    assert_eq!(server.requests().len(), 2, "the cut-off request, then one clean replacement");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.identities.len(), 2);
    assert_eq!(seen.identities[1].as_ref().unwrap().etag, "\"cached-v2\"");
    assert_eq!(server.peak_connections(), 1);
}

#[tokio::test]
async fn connection_setup_is_bounded_separately_from_the_header_wait() {
    // 10.255.255.1 is unroutable: depending on the host the connect either hangs
    // (bounded by the connect deadline) or fails at once. Never the 3 s header wait.
    let settings = SettingsStore::new(
        json!({"romm_url": "http://10.255.255.1:81", "romm_api_token": "t", "romm_api_token_origin": "http://10.255.255.1:81"})
            .as_object()
            .cloned()
            .unwrap(),
    );
    let mut t = fast_timeouts();
    t.connect = Duration::from_millis(300);
    t.headers_archive = Duration::from_secs(60);
    let h = RommHttp::new(settings, "ua/1".into(), RetryPolicy { max_attempts: 1, base_delay: Duration::from_millis(1) }, None)
        .unwrap()
        .with_timeouts(t)
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let err = h
        .download(PATH, &dest(&dir), TransferMode::Archive(ResumePlan::Fresh), hooks(CancellationToken::new()).0)
        .await
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(5), "{err:#}");
}
