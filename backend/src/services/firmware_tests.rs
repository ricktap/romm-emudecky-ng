//! FirmwareService over real files with a fake RomM firmware list.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};

use super::*;
use crate::domain::errors::RommError;
use crate::services::testing::{FakeFiles, FakeLocator, FixedPaths};

/// SHA-1 of "abc".
const ABC_SHA1: &str = "a9993e364706816aba3e25717850c26c9cd0d89d";

struct FakeApi {
    list: Mutex<Result<Value, RommError>>,
    calls: Mutex<usize>,
}

#[async_trait]
impl RommFirmwareApi for FakeApi {
    async fn list_firmware(&self) -> Result<Value, RommError> {
        *self.calls.lock().unwrap() += 1;
        self.list.lock().unwrap().clone()
    }
}

struct TestClock(Mutex<f64>);

impl Clock for TestClock {
    fn now_iso(&self) -> String {
        "2026-01-01T00:00:00+00:00".into()
    }
    fn epoch_seconds(&self) -> f64 {
        *self.0.lock().unwrap()
    }
    fn monotonic(&self) -> f64 {
        0.0
    }
}

struct Env {
    dir: tempfile::TempDir,
    api: Arc<FakeApi>,
    clock: Arc<TestClock>,
    locator: Arc<FakeLocator>,
    svc: Arc<FirmwareService>,
}

impl Env {
    fn home(&self) -> String {
        format!("{}/home", self.dir.path().display())
    }
    fn bios(&self) -> String {
        format!("{}/Emulation/bios", self.dir.path().display())
    }
    fn write(&self, path: &str, bytes: &[u8]) {
        std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    fn set_list(&self, v: Value) {
        *self.api.list.lock().unwrap() = Ok(v);
    }
    fn calls(&self) -> usize {
        *self.api.calls.lock().unwrap()
    }
}

fn entry(id: i64, name: &str, size: u64, sha1: &str) -> Value {
    json!({"id": id, "platform_id": 9, "file_name": name, "file_path": "bios/Switch", "file_size_bytes": size, "sha1_hash": sha1, "md5_hash": "x"})
}

fn romm_list() -> Value {
    json!([
        entry(1, "console.keys", 451, "c1"),
        entry(5, "Firmware.23.0.1.zip", 1000, "f23"),
        entry(2, "key_retail.bin", 160, "k1"),
        entry(3, "prod.keys", 3, ABC_SHA1),
        entry(4, "title.keys", 1224, "t1"),
        entry(6, "Firmware.9.2.0.zip", 500, "f9"),
        {"id": 90, "file_name": "scph5501.bin", "file_path": "bios/ps", "file_size_bytes": 524288, "sha1_hash": "p"},
    ])
}

fn env(installed: &[&str]) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let api = Arc::new(FakeApi { list: Mutex::new(Ok(romm_list())), calls: Mutex::new(0) });
    let clock = Arc::new(TestClock(Mutex::new(1000.0)));
    let locator = Arc::new(FakeLocator {
        installed: Mutex::new(installed.iter().map(|id| ((*id).to_string(), format!("/apps/{id}"))).collect::<HashMap<_, _>>()),
        missing: Mutex::default(),
    });
    let home = format!("{}/home", dir.path().display());
    let bios = format!("{}/Emulation/bios", dir.path().display());
    let svc = FirmwareService::new(FirmwareServiceConfig {
        romm: api.clone(),
        files: Arc::new(FakeFiles::default()),
        locator: locator.clone(),
        paths: Arc::new(FixedPaths { roms: String::new(), bios }),
        clock: clock.clone(),
        home,
    });
    Env { dir, api, clock, locator, svc }
}

fn key_row<'a>(status: &'a Value, name: &str) -> &'a Value {
    status["keys"].as_array().unwrap().iter().find(|k| k["file_name"] == name).unwrap()
}

fn states(row: &Value) -> Vec<(String, String)> {
    row["emulators"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["id"].as_str().unwrap().into(), e["state"].as_str().unwrap().into()))
        .collect()
}

#[tokio::test]
async fn keys_are_compared_per_installed_emulator_with_romms_copy() {
    let e = env(&["eden", "yuzu", "ryujinx"]);
    let h = e.home();
    e.write(&format!("{h}/.local/share/eden/keys/prod.keys"), b"abc"); // same bytes as RomM's
    e.write(&format!("{h}/.local/share/eden/keys/title.keys"), b"my own title keys"); // user's own
    e.write(&format!("{h}/.config/Ryujinx/system/prod.keys"), b"something else");
    // yuzu has no keys directory at all
    let s = e.svc.get_firmware_status().await;
    assert_eq!(s["success"], true);
    assert_eq!(s["server_offline"], false);
    assert_eq!(s["platform_slug"], "switch");
    assert_eq!(
        states(key_row(&s, "prod.keys")),
        [("eden".into(), "matches".into()), ("yuzu".into(), "missing".into()), ("ryujinx".into(), "differs".into())]
    );
    assert_eq!(
        states(key_row(&s, "title.keys")),
        [("eden".into(), "differs".into()), ("yuzu".into(), "missing".into()), ("ryujinx".into(), "missing".into())]
    );
    // Ryujinx-only files appear for Ryujinx only
    assert_eq!(states(key_row(&s, "console.keys")), [("ryujinx".into(), "missing".into())]);
    assert_eq!(states(key_row(&s, "key_retail.bin")), [("ryujinx".into(), "missing".into())]);
    // required flags and RomM details
    assert_eq!(key_row(&s, "prod.keys")["required"], true);
    assert_eq!(key_row(&s, "title.keys")["required"], true);
    assert_eq!(key_row(&s, "console.keys")["required"], false);
    assert_eq!((key_row(&s, "prod.keys")["on_romm"].clone(), key_row(&s, "prod.keys")["firmware_id"].clone()), (json!(true), json!(3)));
    assert_eq!(s["emulators"][0], json!({"id": "eden", "label": "Eden", "keys_dir": format!("{h}/.local/share/eden/keys")}));
}

#[tokio::test]
async fn only_installed_emulators_are_listed() {
    let e = env(&["ryujinx"]);
    let s = e.svc.get_firmware_status().await;
    assert_eq!(s["emulators"].as_array().unwrap().len(), 1);
    assert_eq!(states(key_row(&s, "prod.keys")), [("ryujinx".into(), "missing".into())]);
    let none = env(&[]);
    let s = none.svc.get_firmware_status().await;
    assert!(s["emulators"].as_array().unwrap().is_empty());
    assert!(states(key_row(&s, "prod.keys")).is_empty(), "no emulator, nothing to compare");
    assert_eq!(key_row(&s, "prod.keys")["on_romm"], true, "RomM's files are still listed");
}

#[tokio::test]
async fn firmware_versions_are_listed_newest_first_with_the_newest_selected_and_the_local_archive_checked() {
    let e = env(&["eden"]);
    // 23.0.1 is on disk with the right size; 9.2.0 is on disk with the wrong size
    e.write(&format!("{}/switch/Firmware.23.0.1.zip", e.bios()), &vec![0u8; 1000]);
    e.write(&format!("{}/switch/Firmware.9.2.0.zip", e.bios()), &vec![0u8; 3]);
    let s = e.svc.get_firmware_status().await;
    let fw = s["firmware"].as_array().unwrap();
    let names: Vec<&str> = fw.iter().map(|f| f["file_name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Firmware.23.0.1.zip", "Firmware.9.2.0.zip"], "numeric order, 23 before 9");
    assert_eq!((fw[0]["version"].clone(), fw[0]["selected"].clone(), fw[0]["downloaded"].clone()), (json!("23.0.1"), json!(true), json!(true)));
    assert_eq!((fw[1]["selected"].clone(), fw[1]["downloaded"].clone(), fw[1]["local_size"].clone()), (json!(false), json!(false), json!(3)));
    assert_eq!(s["archive_dir"], format!("{}/switch", e.bios()));
    assert_eq!(fw[0]["firmware_id"], 5);
    assert_eq!(fw[0]["size"], 1000);
}

#[tokio::test]
async fn other_platforms_unsafe_names_and_unknown_files_are_not_listed() {
    let e = env(&["eden"]);
    e.set_list(json!({"items": [
        entry(3, "prod.keys", 3, ABC_SHA1),
        entry(7, "../evil.keys", 1, "x"),
        entry(8, "sub/dir.keys", 1, "x"),
        entry(9, "readme.txt", 1, "x"),
        entry(10, "title.keys_autogenerated", 1, "x"),
        {"id": 11, "file_name": "ps2.bin", "file_path": "bios/ps2", "file_size_bytes": 1, "sha1_hash": "x"},
    ]}));
    let s = e.svc.get_firmware_status().await;
    assert_eq!(key_row(&s, "prod.keys")["on_romm"], true);
    assert_eq!(key_row(&s, "title.keys")["on_romm"], false, "not offered by RomM");
    assert!(s["firmware"].as_array().unwrap().is_empty());
    assert_eq!(s["unrecognized"], 2, "readme.txt and the emulator's own autogenerated file; unsafe names are dropped");
}

#[tokio::test]
async fn offline_without_a_list_still_shows_the_known_keys_but_cannot_compare() {
    let e = env(&["eden"]);
    *e.api.list.lock().unwrap() = Err(RommError::Connection("down".into()));
    e.write(&format!("{}/.local/share/eden/keys/prod.keys", e.home()), b"abc");
    let s = e.svc.get_firmware_status().await;
    assert_eq!(s["success"], true);
    assert_eq!(s["server_offline"], true);
    assert!(s["firmware"].as_array().unwrap().is_empty());
    assert_eq!(key_row(&s, "prod.keys")["on_romm"], false);
    assert_eq!(states(key_row(&s, "prod.keys")), [("eden".into(), "present".into())], "nothing to compare with");
    assert_eq!(states(key_row(&s, "title.keys")), [("eden".into(), "missing".into())]);
}

#[tokio::test]
async fn the_list_is_cached_for_an_hour_and_a_stale_list_serves_an_offline_server() {
    let e = env(&["eden"]);
    e.svc.get_firmware_status().await;
    e.svc.get_firmware_status().await;
    assert_eq!(e.calls(), 1, "fresh cache");
    *e.clock.0.lock().unwrap() += 3599.0;
    e.svc.get_firmware_status().await;
    assert_eq!(e.calls(), 1, "still inside the hour");
    *e.clock.0.lock().unwrap() += 2.0;
    e.svc.get_firmware_status().await;
    assert_eq!(e.calls(), 2, "expired");
    e.svc.invalidate();
    e.svc.get_firmware_status().await;
    assert_eq!(e.calls(), 3, "invalidated");

    // RomM goes away after the hour: the stale list still answers, marked offline
    *e.clock.0.lock().unwrap() += 7200.0;
    *e.api.list.lock().unwrap() = Err(RommError::Timeout("slow".into()));
    let s = e.svc.get_firmware_status().await;
    assert_eq!(s["server_offline"], true);
    assert_eq!(s["firmware"].as_array().unwrap().len(), 2, "the last known list");
    let _ = &e.locator;
}

#[tokio::test]
async fn the_status_never_writes() {
    let e = env(&["eden", "yuzu", "ryujinx"]);
    e.svc.get_firmware_status().await;
    assert!(!std::path::Path::new(&format!("{}/.local", e.home())).exists());
    assert!(!std::path::Path::new(&format!("{}/.config", e.home())).exists());
    assert!(!std::path::Path::new(&e.bios()).exists());
}
