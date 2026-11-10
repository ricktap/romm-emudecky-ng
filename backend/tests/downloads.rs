//! The durable download queue and install adoption through the real backend
//! — the Rust replacement of the Python contract tests
//! (`test_download_durable_queue`, `test_download_read`, `test_switch_launch`,
//! `test_install_adoption`). A raw-TCP fake RomM lets a transfer stall
//! mid-body so the binary can be killed, paused, and restarted over the same
//! data directory.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::fake_romm::FakeRomm;
use common::{Backend, Dirs};
use romm_backend::adapters::persistence::system_clock;
use romm_backend::adapters::romm::http::RetryPolicy;
use romm_backend::app::{Backend as InProcess, BackendEnv};
use romm_backend::rpc::server::{EventHub, Server};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};

const APP_ID: i64 = 4242;
const LEN: usize = 64 * 1024;
const PARTIAL: usize = 20_000;

fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

fn home(dirs: &Dirs) -> PathBuf {
    dirs.root.path().join("home")
}

fn roms(dirs: &Dirs) -> PathBuf {
    dirs.root.path().join("sd/Emulation/roms")
}

fn eden(dirs: &Dirs) -> String {
    home(dirs).join("Applications/Eden.AppImage").to_string_lossy().into_owned()
}

/// EmuDeck on an "SD card", Eden installed, signed in to `romm`.
fn prepare(dirs: &Dirs, romm: &FakeRomm) {
    let config = home(dirs).join(".config/EmuDeck");
    std::fs::create_dir_all(&config).unwrap();
    let storage = dirs.root.path().join("sd");
    std::fs::write(
        config.join("settings.json"),
        json!({"storagePath": storage}).to_string(),
    )
    .unwrap();
    let exe = PathBuf::from(eden(dirs));
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    dirs.write_settings(&json!({
        "romm_url": romm.url, "romm_api_token": "rmm_tok", "romm_api_token_origin": romm.url,
    }));
}

fn db(dirs: &Dirs) -> rusqlite::Connection {
    let c = rusqlite::Connection::open(dirs.db()).unwrap();
    c.busy_timeout(Duration::from_secs(5)).unwrap();
    c
}

fn seed_rom(dirs: &Dirs, id: i64, fs_name: &str, app_id: Option<i64>) {
    db(dirs)
        .execute(
            "INSERT OR REPLACE INTO roms (rom_id, platform_slug, name, fs_name, shortcut_app_id, last_synced_at) \
             VALUES (?1, 'switch', ?2, ?3, ?4, '2026-01-01T00:00:00')",
            rusqlite::params![id, format!("Game {id}"), fs_name, app_id],
        )
        .unwrap();
}

fn job_status(dirs: &Dirs, id: i64) -> Option<String> {
    db(dirs)
        .query_row("SELECT status FROM download_jobs WHERE rom_id = ?1", [id], |r| r.get(0))
        .ok()
}

fn switch_rom(id: i64, fs_name: &str, size: usize) -> Value {
    json!({
        "id": id, "name": format!("Game {id}"), "fs_name": fs_name, "fs_size_bytes": size,
        "platform_slug": "switch", "platform_fs_slug": "switch", "platform_name": "Nintendo Switch",
        "files": [{"file_name": fs_name, "file_size_bytes": size}],
    })
}

async fn wait_for_size(path: &Path, size: u64) {
    for _ in 0..1000 {
        if std::fs::metadata(path).is_ok_and(|m| m.len() == size) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{} never reached {size} bytes", path.display());
}

fn queue_entry(queue: &Value, id: i64) -> Value {
    queue["downloads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["rom_id"] == id)
        .cloned()
        .unwrap_or(Value::Null)
}

#[tokio::test]
async fn start_progress_complete_with_eden_launch_options() {
    let romm = FakeRomm::start().await;
    let dirs = Dirs::new();
    prepare(&dirs, &romm);
    let name = "Pokemon Shining Pearl [010018E011D92000][v0].nsp";
    romm.set_rom(switch_rom(7, name, LEN));
    romm.set_content(7, payload(LEN));
    let mut b = Backend::spawn(&dirs).await;
    seed_rom(&dirs, 7, name, Some(APP_ID));

    let r = b.call("start_download", json!([7])).await;
    assert_eq!(r, json!({"success": true, "message": "Download started"}));
    let complete = b.wait_event("download_complete", |p| p["rom_id"] == 7).await;
    let path = roms(&dirs).join("switch").join(name).to_string_lossy().into_owned();
    assert_eq!(
        complete,
        json!({
            "rom_id": 7, "rom_name": "Game 7", "platform_name": "Nintendo Switch", "file_path": path,
            "app_id": APP_ID,
            "launch_options": format!("vblank_mode=0 %command% \"{}\" -f -g \"{path}\"", eden(&dirs)),
            "resumable": true,
        })
    );
    let progress = b.seen("download_progress");
    let last = progress.iter().rev().find(|p| p["status"] == "downloading").unwrap();
    assert_eq!((last["bytes_downloaded"].clone(), last["total_bytes"].clone()), (json!(LEN), json!(LEN)));
    for frame in &progress {
        for key in ["rom_id", "rom_name", "platform_name", "file_name", "status", "progress", "bytes_downloaded", "total_bytes", "resumable"] {
            assert!(frame.get(key).is_some(), "{key} missing in {frame}");
        }
    }
    assert_eq!(std::fs::read(&path).unwrap(), payload(LEN));

    let installed = b.call("get_installed_rom", json!([7])).await;
    assert_eq!(installed["file_path"], path);
    assert_eq!(installed["file_name"], name);
    assert_eq!(b.call("get_installed_rom", json!([8])).await, Value::Null);
    assert_eq!(queue_entry(&b.call("get_download_queue", json!([])).await, 7)["status"], "completed");
    assert_eq!(job_status(&dirs, 7), None);
    // The file name is quoted twice on the wire, like the Python client.
    assert!(romm.content_requests()[0].path.ends_with("Shining%2520Pearl%2520%255B010018E011D92000%255D%255Bv0%255D.nsp"));

    let r = b.request("start_download", json!(["abc"])).await;
    assert!(r["error"]["message"].as_str().unwrap().contains("rom_id"));
    assert_eq!(b.call("cancel_download", json!([7])).await["reason"], "no_active_download");
    assert_eq!(b.call("resume_download", json!([7])).await["reason"], "not_paused");
    b.shutdown().await;
}

#[tokio::test]
async fn killed_mid_transfer_resumes_from_the_partial_after_restart() {
    let romm = FakeRomm::start().await;
    let dirs = Dirs::new();
    prepare(&dirs, &romm);
    romm.set_rom(switch_rom(7, "metroid.nsp", LEN));
    romm.set_content(7, payload(LEN));
    romm.stall_at(Some(PARTIAL));
    let mut first = Backend::spawn(&dirs).await;
    seed_rom(&dirs, 7, "metroid.nsp", Some(APP_ID));
    assert_eq!(first.call("start_download", json!([7])).await["success"], true);
    let tmp = roms(&dirs).join("switch/metroid.nsp.tmp");
    wait_for_size(&tmp, PARTIAL as u64).await;
    // Power-off style: no shutdown, the row still says "downloading".
    first.child.kill().await.unwrap();
    assert_eq!(job_status(&dirs, 7).as_deref(), Some("downloading"));
    assert_eq!(std::fs::metadata(&tmp).unwrap().len(), PARTIAL as u64);
    romm.stall_at(None);
    romm.release.notify_waiters();

    // A new process over the same data dir reconciles and auto-resumes.
    let mut second = Backend::spawn(&dirs).await;
    let complete = second.wait_event("download_complete", |p| p["rom_id"] == 7).await;
    assert_eq!(complete["app_id"], APP_ID);
    let requests = romm.content_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].range, None);
    assert_eq!(requests[1].range.as_deref(), Some(format!("bytes={PARTIAL}-").as_str()));
    let path = roms(&dirs).join("switch/metroid.nsp");
    assert_eq!(std::fs::read(&path).unwrap(), payload(LEN));
    assert!(!tmp.exists());
    assert_eq!(job_status(&dirs, 7), None);
    let entry = queue_entry(&second.call("get_download_queue", json!([])).await, 7);
    assert_eq!(entry["status"], "completed");
    assert_eq!(second.call("get_installed_rom", json!([7])).await["file_path"], path.to_string_lossy().as_ref());
    second.shutdown().await;
}

#[tokio::test]
async fn paused_download_survives_a_restart_and_resumes_on_request() {
    let romm = FakeRomm::start().await;
    let dirs = Dirs::new();
    prepare(&dirs, &romm);
    romm.set_rom(switch_rom(7, "metroid.nsp", LEN));
    romm.set_content(7, payload(LEN));
    romm.stall_at(Some(PARTIAL));
    let mut first = Backend::spawn(&dirs).await;
    seed_rom(&dirs, 7, "metroid.nsp", None);
    first.call("start_download", json!([7])).await;
    let tmp = roms(&dirs).join("switch/metroid.nsp.tmp");
    wait_for_size(&tmp, PARTIAL as u64).await;
    assert_eq!(
        first.call("pause_download", json!([7])).await,
        json!({"success": true, "message": "Download paused"})
    );
    let paused = first.wait_event("download_progress", |p| p["status"] == "paused").await;
    assert_eq!(paused["resumable"], true);
    first.shutdown().await;
    assert_eq!(job_status(&dirs, 7).as_deref(), Some("paused"));
    romm.stall_at(None);
    romm.release.notify_waiters();

    let mut second = Backend::spawn(&dirs).await;
    let entry = queue_entry(&second.call("get_download_queue", json!([])).await, 7);
    assert_eq!(entry["status"], "paused");
    assert_eq!(entry["bytes_downloaded"], PARTIAL);
    assert_eq!(entry["total_bytes"], LEN);
    assert_eq!(entry["resumable"], true);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(romm.content_requests().len(), 1, "a user pause never auto-resumes");

    assert_eq!(second.call("resume_download", json!([7])).await["success"], true);
    let complete = second.wait_event("download_complete", |p| p["rom_id"] == 7).await;
    assert_eq!(complete["app_id"], Value::Null);
    assert_eq!(
        romm.content_requests()[1].range.as_deref(),
        Some(format!("bytes={PARTIAL}-").as_str())
    );
    assert_eq!(std::fs::read(roms(&dirs).join("switch/metroid.nsp")).unwrap(), payload(LEN));
    second.shutdown().await;
}

#[tokio::test]
async fn cancel_deletes_the_partial_and_one_transfer_runs_at_a_time() {
    let romm = FakeRomm::start().await;
    let dirs = Dirs::new();
    prepare(&dirs, &romm);
    for id in [1, 2, 3] {
        romm.set_rom(switch_rom(id, &format!("g{id}.nsp"), LEN));
        romm.set_content(id, payload(LEN));
    }
    romm.stall_at(Some(PARTIAL));
    let mut b = Backend::spawn(&dirs).await;
    for id in [1, 2, 3] {
        seed_rom(&dirs, id, &format!("g{id}.nsp"), None);
    }
    for id in [1, 2, 3] {
        assert_eq!(b.call("start_download", json!([id])).await["success"], true);
    }
    b.wait_event("download_progress", |p| p["rom_id"] == 2 && p["status"] == "queued").await;
    b.wait_event("download_progress", |p| p["rom_id"] == 3 && p["status"] == "queued").await;
    wait_for_size(&roms(&dirs).join("switch/g1.nsp.tmp"), PARTIAL as u64).await;
    assert_eq!(romm.content_requests().len(), 1, "one transfer at a time");

    // Cancelling the running download deletes its partial and starts the next.
    romm.stall_at(None);
    assert_eq!(b.call("cancel_download", json!([1])).await["message"], "Download cancelled");
    b.wait_event("download_progress", |p| p["rom_id"] == 1 && p["status"] == "cancelled").await;
    b.wait_event("download_complete", |p| p["rom_id"] == 2).await;
    b.wait_event("download_complete", |p| p["rom_id"] == 3).await;
    assert!(!roms(&dirs).join("switch/g1.nsp.tmp").exists());
    assert!(!roms(&dirs).join("switch/g1.nsp").exists());
    let order: Vec<String> = romm.content_requests().iter().map(|r| r.path.clone()).collect();
    assert!(order[1].contains("/roms/2/") && order[2].contains("/roms/3/"), "{order:?}");
    assert_eq!(job_status(&dirs, 1), None);
    b.shutdown().await;
}

// ── Multipart ROMs as RomM's cached ZIP ──────────────────────────────────

const ETAG_V1: &str = "\"cached-v1\"";
const CUT: usize = 20_000;

fn multi_rom(id: i64) -> Value {
    json!({
        "id": id, "name": "Mario Kart 8 Deluxe", "fs_name": "Mario Kart 8 Deluxe",
        "fs_name_no_ext": "Mario Kart 8 Deluxe", "fs_path": "switch", "fs_size_bytes": 50_000,
        "platform_slug": "switch", "platform_fs_slug": "switch", "platform_name": "Nintendo Switch",
        "has_multiple_files": false, "has_nested_single_file": true,
        "files": [
            {"file_path": "switch/Mario Kart 8 Deluxe", "file_name": "base.nsp", "file_size_bytes": 40_000},
            {"file_path": "switch/Mario Kart 8 Deluxe/update", "file_name": "u.nsp", "file_size_bytes": 9_000},
        ],
    })
}

/// A ZIP (stored, so the bytes are easy to reason about) of two members.
fn archive(seed: u8) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    use std::io::Write;
    let base: Vec<u8> = (0..40_000usize).map(|i| ((i * 7 + usize::from(seed)) % 251) as u8).collect();
    let update: Vec<u8> = (0..9_000usize).map(|i| ((i * 13 + usize::from(seed)) % 241) as u8).collect();
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("base.nsp", opts).unwrap();
        zip.write_all(&base).unwrap();
        zip.start_file("update/u.nsp", opts).unwrap();
        zip.write_all(&update).unwrap();
        zip.finish().unwrap();
    }
    (buf.into_inner(), base, update)
}

fn job_identity(dirs: &Dirs, id: i64) -> (Option<String>, Option<i64>, i64, i64) {
    db(dirs)
        .query_row(
            "SELECT etag, representation_length, cached_zip, needs_recovery FROM download_jobs WHERE rom_id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap()
}

async fn wait_until_no_content_connection_is_open(romm: &FakeRomm) {
    for _ in 0..500 {
        if romm.open_content_connections() == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the first backend's connection never closed");
}

#[tokio::test]
async fn a_multipart_download_resumes_after_a_restart_into_a_byte_identical_archive() {
    let romm = FakeRomm::start().await;
    let dirs = Dirs::new();
    prepare(&dirs, &romm);
    let (zip, base, update) = archive(1);
    romm.set_rom(multi_rom(9));
    romm.set_cached(9, zip.clone(), ETAG_V1);
    romm.stall_at(Some(CUT));
    let mut first = Backend::spawn(&dirs).await;
    seed_rom(&dirs, 9, "Mario Kart 8 Deluxe", Some(APP_ID));
    assert_eq!(first.call("start_download", json!([9])).await["success"], true);
    let tmp = roms(&dirs).join("switch/base.nsp.zip.tmp");
    wait_for_size(&tmp, CUT as u64).await;

    // The very first request asks for the cached archive.
    let first_request = romm.content_requests()[0].clone();
    assert_eq!(first_request.range.as_deref(), Some("bytes=0-"));
    assert_eq!(first_request.accept_encoding.as_deref(), Some("identity"));
    assert_eq!(first_request.if_range, None);
    // Its identity is saved while the bytes arrive.
    assert_eq!(
        job_identity(&dirs, 9),
        (Some(ETAG_V1.to_string()), Some(zip.len() as i64), 1, 0)
    );
    assert_eq!(std::fs::read(&tmp).unwrap(), zip[..CUT]);
    first.shutdown().await;
    assert_eq!(job_status(&dirs, 9).as_deref(), Some("downloading"), "the row survives the stop");

    romm.stall_at(None);
    romm.release.notify_waiters();
    wait_until_no_content_connection_is_open(&romm).await;
    let mut second = Backend::spawn(&dirs).await;
    second.wait_event("download_complete", |p| p["rom_id"] == 9).await;

    let requests = romm.content_requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert_eq!(requests[1].range.as_deref(), Some(format!("bytes={CUT}-").as_str()));
    assert_eq!(requests[1].if_range.as_deref(), Some(ETAG_V1));
    assert_eq!(requests[1].accept_encoding.as_deref(), Some("identity"));
    // The archive was valid end to end (extraction checks every CRC) and byte-identical.
    let dir = roms(&dirs).join("switch/Mario Kart 8 Deluxe");
    assert_eq!(std::fs::read(dir.join("base.nsp")).unwrap(), base);
    assert_eq!(std::fs::read(dir.join("update/u.nsp")).unwrap(), update);
    assert!(!tmp.exists());
    assert_eq!(job_status(&dirs, 9), None);
    assert_eq!(romm.peak_content_connections(), 1, "never two writers for one job");
    second.shutdown().await;
}

#[tokio::test]
async fn a_rebuilt_archive_after_an_interruption_replaces_the_partial_instead_of_extending_it() {
    let romm = FakeRomm::start().await;
    let dirs = Dirs::new();
    prepare(&dirs, &romm);
    let (zip_v1, _, _) = archive(1);
    romm.set_rom(multi_rom(9));
    romm.set_cached(9, zip_v1.clone(), ETAG_V1);
    romm.stall_at(Some(CUT));
    let mut first = Backend::spawn(&dirs).await;
    seed_rom(&dirs, 9, "Mario Kart 8 Deluxe", None);
    first.call("start_download", json!([9])).await;
    let tmp = roms(&dirs).join("switch/base.nsp.zip.tmp");
    wait_for_size(&tmp, CUT as u64).await;
    first.shutdown().await;

    // RomM restarted and rebuilt the archive: another tag, other bytes, other length.
    let (zip_v2, base_v2, update_v2) = archive(9);
    assert_ne!(zip_v1[..CUT], zip_v2[..CUT]);
    romm.set_cached(9, zip_v2.clone(), "\"cached-v2\"");
    romm.stall_at(None);
    romm.release.notify_waiters();
    wait_until_no_content_connection_is_open(&romm).await;

    let mut second = Backend::spawn(&dirs).await;
    second.wait_event("download_complete", |p| p["rom_id"] == 9).await;
    let requests = romm.content_requests();
    assert_eq!(requests[1].if_range.as_deref(), Some(ETAG_V1), "the old validator was offered");
    let dir = roms(&dirs).join("switch/Mario Kart 8 Deluxe");
    // Appending the 200 onto the old partial would not have been a valid archive of either.
    assert_eq!(std::fs::read(dir.join("base.nsp")).unwrap(), base_v2);
    assert_eq!(std::fs::read(dir.join("update/u.nsp")).unwrap(), update_v2);
    assert_eq!(romm.peak_content_connections(), 1);
    second.shutdown().await;
}

// ── In-process: internal entry points without a callable yet ─────────────

async fn in_process(dirs: &Dirs, hub: Arc<EventHub>) -> InProcess {
    InProcess::start(
        BackendEnv {
            settings_dir: dirs.settings(),
            runtime_dir: dirs.runtime(),
            plugin_dir: dirs.root.path().join("plugin"),
            user_home: home(dirs),
            retry: RetryPolicy {
                max_attempts: 1,
                base_delay: Duration::from_millis(1),
            },
            clock: system_clock(),
        },
        hub,
    )
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn move_to_front_runs_that_download_next() {
    let romm = FakeRomm::start().await;
    let dirs = Dirs::new();
    prepare(&dirs, &romm);
    for id in [1, 2, 3] {
        romm.set_rom(switch_rom(id, &format!("g{id}.nsp"), LEN));
        romm.set_content(id, payload(LEN));
    }
    romm.stall_at(Some(PARTIAL));
    let backend = in_process(&dirs, Arc::new(EventHub::default())).await;
    for id in [1, 2, 3] {
        seed_rom(&dirs, id, &format!("g{id}.nsp"), None);
    }
    for id in [1, 2, 3] {
        assert_eq!(backend.downloads.start_download(id).await["success"], true);
    }
    wait_for_size(&roms(&dirs).join("switch/g1.nsp.tmp"), PARTIAL as u64).await;
    assert_eq!(
        backend.downloads.move_to_front(3).await,
        json!({"success": true, "message": "Moved to front"})
    );
    assert_eq!(backend.downloads.move_to_front(99).await["reason"], "not_queued");
    let positions: Vec<(i64, i64)> = {
        let c = db(&dirs);
        let mut stmt = c.prepare("SELECT rom_id, position FROM download_jobs ORDER BY position").unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(positions, vec![(3, 0), (1, 1), (2, 2)]);
    romm.stall_at(None);
    romm.release.notify_waiters();
    for _ in 0..1000 {
        if backend.downloads.active_download_rom_ids().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let order: Vec<String> = romm.content_requests().iter().map(|r| r.path.clone()).collect();
    assert!(order[1].contains("/roms/3/") && order[2].contains("/roms/2/"), "{order:?}");
    backend.downloads.shutdown().await;
}

/// The real Deck data (Phase 1.5): RomM keeps "Catherine_ Full Body" as a
/// folder game, the Deck has a hand-copied folder with a scene-named XCI.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adopts_a_hand_copied_switch_folder_by_normalized_title() {
    let romm = FakeRomm::start().await;
    let dirs = Dirs::new();
    prepare(&dirs, &romm);
    let xci = "Catherine Full Body [0100BF00112C0000]+[v1.0.1](US-EU).xci";
    let folder = roms(&dirs).join("switch/Catherine Full Body");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(xci), vec![0u8; 4096]).unwrap();
    std::fs::write(folder.join("update.nsp"), vec![0u8; 16]).unwrap();
    std::fs::write(roms(&dirs).join("switch/metadata.txt"), b"x").unwrap();
    romm.set_rom(json!({
        "id": 1, "name": "Catherine: Full Body", "fs_name": "Catherine_ Full Body",
        "fs_name_no_ext": "Catherine_ Full Body", "fs_path": "switch", "fs_size_bytes": 7_985_954_816_u64,
        "platform_slug": "switch", "platform_name": "Nintendo Switch",
        "has_multiple_files": false, "has_nested_single_file": true,
        "files": [
            {"file_path": "switch/Catherine_ Full Body", "file_name": "Catherine.xci", "file_size_bytes": 7_985_954_816_u64},
            {"file_path": "switch/Catherine_ Full Body/update", "file_name": "u.nsp", "file_size_bytes": 1},
        ],
    }));

    let hub = Arc::new(EventHub::default());
    let backend = in_process(&dirs, hub.clone()).await;
    seed_rom(&dirs, 1, "Catherine_ Full Body", Some(777));
    seed_rom(&dirs, 2, "Super Mario Galaxy", None);
    assert!(backend.adoption.adopt_rom(1).await);
    assert!(!backend.adoption.adopt_rom(1).await, "already installed");
    assert!(!backend.adoption.adopt_rom(2).await, "no copy on disk");

    let installed = backend.downloads.get_installed_rom(1).await.unwrap();
    let launch = folder.join(xci).to_string_lossy().into_owned();
    assert_eq!(installed["file_path"], launch);
    assert_eq!(installed["file_name"], xci);
    assert!(folder.join(xci).exists(), "adoption never touches the files");

    // The event a finished download emits, buffered for the next shim.
    let socket = dirs.root.path().join("adopt.sock");
    let server = Server::new(backend.registry(), hub, None);
    let listener = Server::bind(&socket).unwrap();
    let serving = server.clone();
    let path = socket.clone();
    tokio::spawn(async move { serving.serve(listener, path).await });
    let stream = tokio::net::UnixStream::connect(&socket).await.unwrap();
    let mut lines = BufReader::new(stream).lines();
    let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let frame: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(frame["event"], "download_complete");
    assert_eq!(
        frame["args"][0],
        json!({
            "rom_id": 1, "rom_name": "Catherine: Full Body", "platform_name": "Nintendo Switch",
            "file_path": launch, "app_id": 777,
            "launch_options": format!("vblank_mode=0 %command% \"{}\" -f -g \"{launch}\"", eden(&dirs)),
            "resumable": false, "adopted": true,
        })
    );
    server.stop();
}
