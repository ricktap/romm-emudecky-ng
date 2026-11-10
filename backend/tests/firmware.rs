//! The Switch firmware status through the real backend: RomM's `bios/Switch` list (the shape a real
//! server returned) compared with the keys on this machine.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use common::fake_romm::FakeRomm;
use common::{Backend, Dirs};
use serde_json::{Value, json};

fn home(dirs: &Dirs) -> PathBuf {
    dirs.root.path().join("home")
}

fn install_emulator(dirs: &Dirs, relative: &str) {
    let exe = home(dirs).join(relative);
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn rom_entry(id: i64, name: &str, size: u64, sha1: &str) -> Value {
    json!({
        "id": id, "platform_id": 9, "file_name": name, "file_name_no_ext": name, "file_path": "bios/Switch",
        "file_size_bytes": size, "full_path": format!("bios/Switch/{name}"), "is_verified": false,
        "crc_hash": "00", "md5_hash": "00", "sha1_hash": sha1, "missing_from_fs": false,
    })
}

#[tokio::test]
async fn firmware_status_compares_romms_switch_files_with_the_emulators_keys_and_never_writes() {
    let romm = FakeRomm::start().await;
    let dirs = Dirs::new();
    let storage = dirs.root.path().join("sd");
    let config = home(&dirs).join(".config/EmuDeck");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("settings.json"), json!({"storagePath": storage}).to_string()).unwrap();
    install_emulator(&dirs, "Applications/Eden.AppImage");
    install_emulator(&dirs, "Applications/publish/Ryujinx.sh");
    dirs.write_settings(&json!({"romm_url": romm.url, "romm_api_token": "rmm_tok", "romm_api_token_origin": romm.url}));
    // The user's own keys: Eden has RomM's prod.keys ("abc"), Ryujinx has different ones.
    let eden_keys = home(&dirs).join(".local/share/eden/keys");
    std::fs::create_dir_all(&eden_keys).unwrap();
    std::fs::write(eden_keys.join("prod.keys"), b"abc").unwrap();
    let ryu_keys = home(&dirs).join(".config/Ryujinx/system");
    std::fs::create_dir_all(&ryu_keys).unwrap();
    std::fs::write(ryu_keys.join("prod.keys"), b"mine").unwrap();
    romm.set_firmware(json!([
        rom_entry(1, "console.keys", 451, "c1"),
        rom_entry(5, "Firmware.23.0.1.zip", 340_422_248, "f23"),
        rom_entry(3, "prod.keys", 3, "a9993e364706816aba3e25717850c26c9cd0d89d"),
        rom_entry(4, "title.keys", 1224, "t1"),
    ]));

    let mut b = Backend::spawn(&dirs).await;
    let status = b.call("get_firmware_status", json!([])).await;
    assert_eq!(status["success"], true);
    assert_eq!(status["server_offline"], false);
    let emulators: Vec<&str> = status["emulators"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert_eq!(emulators, ["eden", "ryujinx"], "only the emulators that are installed");
    let prod = status["keys"].as_array().unwrap().iter().find(|k| k["file_name"] == "prod.keys").unwrap();
    assert_eq!(prod["emulators"][0]["state"], "matches");
    assert_eq!(prod["emulators"][1]["state"], "differs");
    assert_eq!(status["firmware"][0]["version"], "23.0.1");
    assert_eq!(status["firmware"][0]["selected"], true);
    assert_eq!(status["firmware"][0]["downloaded"], false);

    // Reading changed nothing.
    assert_eq!(std::fs::read(eden_keys.join("prod.keys")).unwrap(), b"abc");
    assert_eq!(std::fs::read(ryu_keys.join("prod.keys")).unwrap(), b"mine");
    assert!(!eden_keys.join("title.keys").exists());
    assert!(!storage.join("Emulation/bios").exists());
    b.shutdown().await;
}

#[tokio::test]
async fn firmware_status_says_offline_when_romm_has_no_firmware_endpoint() {
    let romm = FakeRomm::start().await; // no firmware list: 404
    let dirs = Dirs::new();
    dirs.write_settings(&json!({"romm_url": romm.url, "romm_api_token": "rmm_tok", "romm_api_token_origin": romm.url}));
    let mut b = Backend::spawn(&dirs).await;
    let status = b.call("get_firmware_status", json!([])).await;
    assert_eq!(status["success"], true);
    assert_eq!(status["server_offline"], true);
    assert!(status["firmware"].as_array().unwrap().is_empty());
    assert_eq!(status["keys"].as_array().unwrap().len(), 4, "the known key files are still listed");
    b.shutdown().await;
}

// ── Downloads through the shared queue ───────────────────────────────────

fn sha1(bytes: &[u8]) -> String {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
    ctx.update(bytes);
    ctx.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

fn file_entry(id: i64, name: &str, bytes: &[u8]) -> Value {
    rom_entry(id, name, bytes.len() as u64, &sha1(bytes))
}

struct Setup {
    romm: FakeRomm,
    dirs: Dirs,
    storage: PathBuf,
    eden_keys: PathBuf,
    ryu_keys: PathBuf,
    archive_23: Vec<u8>,
    archive_9: Vec<u8>,
}

/// Eden and Ryujinx installed; Eden has RomM's `prod.keys`, Ryujinx has its own different ones; RomM hosts the
/// real five-file Switch layout (two firmware versions).
fn setup() -> impl std::future::Future<Output = Setup> {
    async {
        let romm = FakeRomm::start().await;
        let dirs = Dirs::new();
        let storage = dirs.root.path().join("sd");
        let config = home(&dirs).join(".config/EmuDeck");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("settings.json"), json!({"storagePath": storage}).to_string()).unwrap();
        install_emulator(&dirs, "Applications/Eden.AppImage");
        install_emulator(&dirs, "Applications/publish/Ryujinx.sh");
        dirs.write_settings(&json!({"romm_url": romm.url, "romm_api_token": "rmm_tok", "romm_api_token_origin": romm.url}));
        let eden_keys = home(&dirs).join(".local/share/eden/keys");
        std::fs::create_dir_all(&eden_keys).unwrap();
        std::fs::write(eden_keys.join("prod.keys"), b"romm prod keys").unwrap();
        let ryu_keys = home(&dirs).join(".config/Ryujinx/system");
        std::fs::create_dir_all(&ryu_keys).unwrap();
        std::fs::write(ryu_keys.join("prod.keys"), b"the user's own prod keys").unwrap();
        let archive_23: Vec<u8> = (0..60_000u32).map(|i| (i % 251) as u8).collect();
        let archive_9: Vec<u8> = (0..30_000u32).map(|i| (i % 241) as u8).collect();
        let files: [(i64, &str, &[u8]); 4] = [
            (3, "prod.keys", b"romm prod keys"),
            (4, "title.keys", b"romm title keys"),
            (5, "Firmware.23.0.1.zip", &archive_23),
            (6, "Firmware.9.2.0.zip", &archive_9),
        ];
        romm.set_firmware(Value::Array(files.iter().map(|(id, n, b)| file_entry(*id, n, b)).collect()));
        for (id, _, bytes) in files {
            romm.set_firmware_file(id, bytes.to_vec());
        }
        Setup { romm, dirs, storage, eden_keys, ryu_keys, archive_23, archive_9 }
    }
}

async fn wait_firmware_complete(b: &mut Backend, id: i64) -> Value {
    b.wait_event("download_complete", move |p| p["kind"] == "firmware" && p["firmware_id"] == id).await
}

#[tokio::test]
async fn download_required_fills_the_gaps_keeps_the_users_own_keys_and_installs_the_chosen_archive() {
    let t = setup().await;
    let mut b = Backend::spawn(&t.dirs).await;
    let r = b.call("download_required_firmware", json!(["switch"])).await;
    assert_eq!(r["success"], true, "{r}");
    assert_eq!(r["queued"], 2, "title.keys and the newest archive: {r}");
    assert_eq!(r["kept"], json!(["prod.keys"]));
    assert!(r["message"].as_str().unwrap().contains("kept your own prod.keys"));
    wait_firmware_complete(&mut b, 4).await;
    wait_firmware_complete(&mut b, 5).await;

    // title.keys: verified, stored for Eden, and copied to Ryujinx (which lacked it)
    assert_eq!(std::fs::read(t.eden_keys.join("title.keys")).unwrap(), b"romm title keys");
    assert_eq!(std::fs::read(t.ryu_keys.join("title.keys")).unwrap(), b"romm title keys");
    // the user's differing prod.keys is untouched
    assert_eq!(std::fs::read(t.ryu_keys.join("prod.keys")).unwrap(), b"the user's own prod keys");
    assert_eq!(std::fs::read(t.eden_keys.join("prod.keys")).unwrap(), b"romm prod keys");
    // the archive is byte-identical, in the BIOS folder, and no temp file is left
    let archive = t.storage.join("Emulation/bios/switch/Firmware.23.0.1.zip");
    assert_eq!(std::fs::read(&archive).unwrap(), t.archive_23);
    assert!(!t.storage.join("Emulation/bios/switch/Firmware.23.0.1.zip.tmp").exists());
    assert!(!t.eden_keys.join("title.keys.tmp").exists());
    // 9.2.0 was not chosen, so it was not downloaded
    assert!(!t.storage.join("Emulation/bios/switch/Firmware.9.2.0.zip").exists());
    let status = b.call("get_firmware_status", json!([])).await;
    assert_eq!(status["firmware"][0]["downloaded"], true);
    let title = status["keys"].as_array().unwrap().iter().find(|k| k["file_name"] == "title.keys").unwrap();
    assert_eq!(title["emulators"][0]["state"], "matches");
    // asking again finds nothing to do
    let again = b.call("download_required_firmware", json!(["switch"])).await;
    assert_eq!(again["queued"], 0);
    assert!(again["message"].as_str().unwrap().starts_with("Nothing to download"));
    // a platform other than the Switch is refused
    assert_eq!(b.call("download_all_firmware", json!(["ps2"])).await["reason"], "unsupported_platform");
    b.shutdown().await;
}

#[tokio::test]
async fn a_chosen_version_is_the_one_that_downloads_and_the_choice_survives_a_restart() {
    let t = setup().await;
    let mut b = Backend::spawn(&t.dirs).await;
    assert_eq!(b.call("save_firmware_version", json!(["9.2.0"])).await["success"], true);
    assert_eq!(b.call("save_firmware_version", json!(["latest"])).await["reason"], "invalid_version");
    b.shutdown().await;
    let mut b = Backend::spawn(&t.dirs).await;
    let status = b.call("get_firmware_status", json!([])).await;
    assert_eq!(status["selected_version"], "9.2.0", "persisted in settings.json");
    b.call("download_required_firmware", json!(["switch"])).await;
    wait_firmware_complete(&mut b, 6).await;
    assert_eq!(std::fs::read(t.storage.join("Emulation/bios/switch/Firmware.9.2.0.zip")).unwrap(), t.archive_9);
    assert!(!t.storage.join("Emulation/bios/switch/Firmware.23.0.1.zip").exists());
    b.shutdown().await;
}

#[tokio::test]
async fn a_firmware_download_can_be_paused_survives_a_restart_and_resumes_into_a_byte_identical_archive() {
    let t = setup().await;
    t.romm.stall_at(Some(20_000));
    let mut b = Backend::spawn(&t.dirs).await;
    b.call("download_required_firmware", json!(["switch"])).await;
    let partial = t.storage.join("Emulation/bios/switch/Firmware.23.0.1.zip.tmp");
    // the transfer slot is single: title.keys (4 KB < stall point) runs first, then the archive stalls
    wait_for_size(&partial, 20_000).await;
    assert_eq!(
        b.call("pause_download", json!([5, "firmware"])).await,
        json!({"success": true, "message": "Download paused"})
    );
    let paused = b.wait_event("download_progress", |p| p["firmware_id"] == 5 && p["status"] == "paused").await;
    assert_eq!(paused["kind"], "firmware");
    b.shutdown().await;
    let row: String = db(&t.dirs)
        .query_row("SELECT status FROM download_jobs WHERE kind = 'firmware' AND item_id = 5", [], |r| r.get(0))
        .unwrap();
    assert_eq!(row, "paused");

    t.romm.stall_at(None);
    t.romm.release.notify_waiters();
    let mut b = Backend::spawn(&t.dirs).await;
    let entry = queue_entry_kind(&b.call("get_download_queue", json!([])).await, "firmware", 5);
    assert_eq!((entry["status"].clone(), entry["bytes_downloaded"].clone()), (json!("paused"), json!(20_000)));
    assert_eq!(b.call("resume_download", json!([5, "firmware"])).await["success"], true);
    wait_firmware_complete(&mut b, 5).await;
    let archive = t.storage.join("Emulation/bios/switch/Firmware.23.0.1.zip");
    assert_eq!(std::fs::read(&archive).unwrap(), t.archive_23, "byte-identical after the pause and the restart");
    let ranges: Vec<Option<String>> = t.romm.content_requests().iter().map(|r| r.range.clone()).collect();
    assert!(ranges.contains(&Some("bytes=20000-".to_string())), "{ranges:?}");
    b.shutdown().await;
}

fn queue_entry_kind(queue: &Value, kind: &str, id: i64) -> Value {
    let key = if kind == "firmware" { "firmware_id" } else { "rom_id" };
    queue["downloads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == kind && e[key] == id)
        .cloned()
        .unwrap_or(Value::Null)
}

async fn wait_for_size(path: &std::path::Path, size: u64) {
    for _ in 0..1000 {
        if std::fs::metadata(path).is_ok_and(|m| m.len() == size) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("{} never reached {size} bytes", path.display());
}

fn db(dirs: &Dirs) -> rusqlite::Connection {
    let c = rusqlite::Connection::open(dirs.db()).unwrap();
    c.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
    c
}
