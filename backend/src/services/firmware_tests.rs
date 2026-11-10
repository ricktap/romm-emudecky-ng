//! FirmwareService over real files with a fake RomM firmware list.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};

use super::*;
use crate::domain::errors::RommError;
use crate::services::ports::FirmwareFileKind;
use crate::services::testing::{FakeFiles, FakeLocator, FixedPaths};

/// SHA-1 of "abc".
const ABC_SHA1: &str = "a9993e364706816aba3e25717850c26c9cd0d89d";

struct FakeApi {
    list: Mutex<Result<Value, RommError>>,
    calls: Mutex<usize>,
    /// What a firmware download writes, by firmware id; absent = the download fails.
    content: Mutex<HashMap<i64, Vec<u8>>>,
}

#[async_trait]
impl RommFirmwareApi for FakeApi {
    async fn list_firmware(&self) -> Result<Value, RommError> {
        *self.calls.lock().unwrap() += 1;
        self.list.lock().unwrap().clone()
    }
    async fn download_firmware_content(
        &self,
        firmware_id: i64,
        _file_name: &str,
        dest: &str,
        _mode: crate::services::ports::TransferMode,
        mut hooks: crate::services::ports::TransferHooks,
    ) -> anyhow::Result<crate::services::ports::TransferOutcome> {
        let Some(bytes) = self.content.lock().unwrap().get(&firmware_id).cloned() else {
            anyhow::bail!("connection reset");
        };
        std::fs::write(dest, &bytes)?;
        (hooks.progress)(bytes.len() as u64, bytes.len() as u64);
        Ok(crate::services::ports::TransferOutcome::Downloaded)
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
    let api = Arc::new(FakeApi { list: Mutex::new(Ok(romm_list())), calls: Mutex::new(0), content: Mutex::default() });
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
        settings: SettingsStore::new(serde_json::Map::new()),
        persister: Arc::new(crate::services::testing::FakePersister::default()),
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

// ── Planning, targets and the version choice ─────────────────────────────

fn ids(plan: &Result<(Vec<i64>, Vec<String>), Value>) -> Vec<i64> {
    let mut v = plan.as_ref().unwrap().0.clone();
    v.sort_unstable();
    v
}

#[tokio::test]
async fn required_downloads_are_the_missing_required_keys_and_the_chosen_archive() {
    let e = env(&["eden"]);
    let plan = e.svc.plan_downloads(false).await;
    // prod.keys (3), title.keys (4) and the newest archive (5); the optional Ryujinx files are not needed for Eden
    assert_eq!(ids(&plan), [3, 4, 5]);
    assert!(plan.unwrap().1.is_empty());
}

#[tokio::test]
async fn all_adds_the_optional_keys_but_only_for_emulators_that_read_them() {
    let e = env(&["eden"]);
    assert_eq!(ids(&e.svc.plan_downloads(true).await), [3, 4, 5], "Eden does not read console.keys / key_retail.bin");
    let e = env(&["eden", "ryujinx"]);
    assert_eq!(ids(&e.svc.plan_downloads(false).await), [3, 4, 5]);
    assert_eq!(ids(&e.svc.plan_downloads(true).await), [1, 2, 3, 4, 5], "Ryujinx also reads console.keys (1) and key_retail.bin (2)");
}

#[tokio::test]
async fn keys_that_match_are_skipped_and_keys_that_differ_are_kept_and_named() {
    let e = env(&["eden", "ryujinx"]);
    let h = e.home();
    e.write(&format!("{h}/.local/share/eden/keys/prod.keys"), b"abc"); // matches RomM
    e.write(&format!("{h}/.config/Ryujinx/system/prod.keys"), b"mine"); // differs
    e.write(&format!("{h}/.local/share/eden/keys/title.keys"), b"own title keys"); // differs, Ryujinx lacks it
    let (ids, kept) = e.svc.plan_downloads(false).await.unwrap();
    let mut ids = ids;
    ids.sort_unstable();
    assert_eq!(ids, [4, 5], "title.keys for Ryujinx and the archive; prod.keys needs nothing");
    assert_eq!(kept, ["prod.keys", "title.keys"], "the user's own files are named, never replaced");
}

#[tokio::test]
async fn an_archive_that_is_already_saved_is_not_downloaded_again_and_the_chosen_version_decides() {
    let e = env(&["eden"]);
    e.write(&format!("{}/switch/Firmware.23.0.1.zip", e.bios()), &vec![0u8; 1000]);
    assert_eq!(ids(&e.svc.plan_downloads(false).await), [3, 4], "the newest is already there");
    assert_eq!(e.svc.set_firmware_version("9.2.0").await["success"], true);
    assert_eq!(ids(&e.svc.plan_downloads(false).await), [3, 4, 6], "the chosen 9.2.0 is not saved yet");
    let status = e.svc.get_firmware_status().await;
    assert_eq!(status["selected_version"], "9.2.0");
    let fw = status["firmware"].as_array().unwrap();
    assert_eq!((fw[0]["selected"].clone(), fw[1]["selected"].clone()), (json!(false), json!(true)));
}

#[tokio::test]
async fn the_version_choice_is_validated_normalised_persisted_and_falls_back_when_romm_dropped_it() {
    let e = env(&["eden"]);
    assert_eq!(e.svc.set_firmware_version(" 18.1.0 ").await["success"], true);
    assert_eq!(e.svc.c.settings.get_str("switch_firmware_version").as_deref(), Some("18.1.0"));
    let status = e.svc.get_firmware_status().await;
    assert_eq!(status["selected_version"], "23.0.1", "RomM does not have 18.1.0: the newest is used");
    assert_eq!(status["version_pinned"], false, "a version RomM dropped is not a pin");
    e.svc.set_firmware_version("9.2.0").await;
    assert_eq!(e.svc.get_firmware_status().await["version_pinned"], true);
    assert_eq!(e.svc.set_firmware_version("").await["success"], true, "empty means newest");
    assert_eq!(e.svc.c.settings.get_str("switch_firmware_version").as_deref(), Some(""));
    let bad = e.svc.set_firmware_version("latest").await;
    assert_eq!((bad["success"].clone(), bad["reason"].clone()), (json!(false), json!("invalid_version")));
    assert_eq!(e.svc.c.settings.get_str("switch_firmware_version").as_deref(), Some(""), "nothing saved for bad input");
}

#[tokio::test]
async fn a_failed_save_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let persister = Arc::new(crate::services::testing::FakePersister::default());
    persister.fail_with("disk full");
    let svc = FirmwareService::new(FirmwareServiceConfig {
        romm: Arc::new(FakeApi { list: Mutex::new(Ok(romm_list())), calls: Mutex::new(0), content: Mutex::default() }),
        files: Arc::new(FakeFiles::default()),
        locator: Arc::new(FakeLocator::default()),
        paths: Arc::new(FixedPaths { roms: String::new(), bios: dir.path().display().to_string() }),
        clock: Arc::new(TestClock(Mutex::new(0.0))),
        home: dir.path().display().to_string(),
        settings: SettingsStore::new(serde_json::Map::new()),
        persister,
    });
    let r = svc.set_firmware_version("23.0.1").await;
    assert_eq!((r["success"].clone(), r["reason"].clone()), (json!(false), json!("save_failed")));
}

#[tokio::test]
async fn planning_says_why_when_there_is_nothing_to_plan_from() {
    let e = env(&["eden"]);
    *e.api.list.lock().unwrap() = Err(RommError::Connection("down".into()));
    let err = e.svc.plan_downloads(false).await.unwrap_err();
    assert_eq!(err["reason"], "server_unreachable");
    e.set_list(json!([{"id": 90, "file_name": "scph.bin", "file_path": "bios/ps", "file_size_bytes": 1}]));
    e.svc.invalidate();
    assert_eq!(e.svc.plan_downloads(false).await.unwrap_err()["reason"], "not_found");
}

#[tokio::test]
async fn a_key_target_writes_to_the_first_emulator_that_lacks_it_and_copies_to_the_rest() {
    let e = env(&["eden", "yuzu", "ryujinx"]);
    let h = e.home();
    e.write(&format!("{h}/.local/share/yuzu/keys/prod.keys"), b"abc"); // yuzu already has RomM's
    let t = e.svc.target(3).await.unwrap();
    assert_eq!((t.kind, t.file_name.as_str(), t.size), (FirmwareFileKind::Key, "prod.keys", 3));
    assert_eq!(t.sha1.as_deref(), Some(ABC_SHA1));
    assert_eq!(t.dest_path, format!("{h}/.local/share/eden/keys/prod.keys"));
    assert_eq!(t.copy_to, [format!("{h}/.config/Ryujinx/system/prod.keys")]);
    // everyone has it now: nothing to do
    e.write(&format!("{h}/.local/share/eden/keys/prod.keys"), b"abc");
    e.write(&format!("{h}/.config/Ryujinx/system/prod.keys"), b"abc");
    assert!(e.svc.target(3).await.is_none());
    // a differing file is not "missing": it is not a download target either
    e.write(&format!("{h}/.config/Ryujinx/system/title.keys"), b"mine");
    let t = e.svc.target(4).await.unwrap();
    assert_eq!(t.dest_path, format!("{h}/.local/share/eden/keys/title.keys"));
    assert_eq!(t.copy_to, [format!("{h}/.local/share/yuzu/keys/title.keys")], "Ryujinx keeps its own");
}

#[tokio::test]
async fn an_archive_target_is_the_bios_switch_folder_and_other_files_have_none() {
    let e = env(&[]);
    let t = e.svc.target(5).await.unwrap();
    assert_eq!((t.kind, t.size), (FirmwareFileKind::Archive, 1000));
    assert_eq!(t.dest_path, format!("{}/switch/Firmware.23.0.1.zip", e.bios()));
    assert!(t.copy_to.is_empty());
    e.write(&t.dest_path, &vec![0u8; 1000]);
    assert!(e.svc.target(5).await.is_none(), "already saved");
    assert!(e.svc.target(999).await.is_none(), "not on RomM");
    assert!(e.svc.target(90).await.is_none(), "another platform's file");
    // a key needs an emulator to put it in
    assert!(e.svc.target(3).await.is_none(), "no emulator installed");
}


// ── Replacing a key file that differs from RomM's ────────────────────────

fn romm_prod_keys() -> Vec<u8> {
    b"abc".to_vec() // its SHA-1 is ABC_SHA1, what romm_list() reports for prod.keys
}

fn with_romm_keys(e: &Env) {
    e.api.content.lock().unwrap().insert(3, romm_prod_keys());
}

#[tokio::test]
async fn replacing_a_differing_key_keeps_the_users_file_as_a_backup_and_installs_romms_verified_one() {
    let e = env(&["eden", "ryujinx"]);
    let dest = format!("{}/.config/Ryujinx/system/prod.keys", e.home());
    e.write(&dest, b"the user's own keys");
    e.write(&format!("{}/.config/Ryujinx/system/title.keys_autogenerated", e.home()), b"emulator made");
    with_romm_keys(&e);
    let r = e.svc.replace_key(3, "ryujinx").await;
    assert_eq!(r["success"], true, "{r}");
    assert_eq!(std::fs::read(&dest).unwrap(), romm_prod_keys());
    assert_eq!(std::fs::read(format!("{dest}.romm-backup")).unwrap(), b"the user's own keys");
    assert_eq!(r["backup_path"], format!("{dest}.romm-backup"));
    assert!(r["message"].as_str().unwrap().contains("prod.keys.romm-backup"));
    assert!(!std::path::Path::new(&format!("{dest}.tmp")).exists());
    assert_eq!(
        std::fs::read(format!("{}/.config/Ryujinx/system/title.keys_autogenerated", e.home())).unwrap(),
        b"emulator made",
        "the emulator's own files are never touched"
    );
    // Eden (which had no prod.keys) is untouched by a replace for Ryujinx
    assert!(!std::path::Path::new(&format!("{}/.local/share/eden/keys/prod.keys", e.home())).exists());
    // and the status now agrees
    let status = e.svc.get_firmware_status().await;
    assert_eq!(states(key_row(&status, "prod.keys"))[1], ("ryujinx".into(), "matches".into()));
}

#[tokio::test]
async fn an_earlier_backup_is_never_overwritten() {
    let e = env(&["ryujinx"]);
    let dest = format!("{}/.config/Ryujinx/system/prod.keys", e.home());
    with_romm_keys(&e);
    e.write(&dest, b"first own file");
    assert_eq!(e.svc.replace_key(3, "ryujinx").await["success"], true);
    e.write(&dest, b"second own file");
    let r = e.svc.replace_key(3, "ryujinx").await;
    assert_eq!(r["success"], true);
    assert_eq!(std::fs::read(format!("{dest}.romm-backup")).unwrap(), b"first own file");
    assert_eq!(std::fs::read(format!("{dest}.romm-backup.1")).unwrap(), b"second own file");
    assert_eq!(r["backup_path"], format!("{dest}.romm-backup.1"));
}

#[tokio::test]
async fn a_download_that_does_not_match_romms_checksum_changes_nothing() {
    let e = env(&["ryujinx"]);
    let dest = format!("{}/.config/Ryujinx/system/prod.keys", e.home());
    e.write(&dest, b"the user's own keys");
    e.api.content.lock().unwrap().insert(3, b"corrupted".to_vec());
    let r = e.svc.replace_key(3, "ryujinx").await;
    assert_eq!((r["success"].clone(), r["reason"].clone()), (json!(false), json!("checksum_mismatch")));
    assert_eq!(std::fs::read(&dest).unwrap(), b"the user's own keys");
    assert!(!std::path::Path::new(&format!("{dest}.romm-backup")).exists(), "no backup for a replacement that never happened");
    assert!(!std::path::Path::new(&format!("{dest}.tmp")).exists());
}

#[tokio::test]
async fn a_failed_download_changes_nothing() {
    let e = env(&["ryujinx"]);
    let dest = format!("{}/.config/Ryujinx/system/prod.keys", e.home());
    e.write(&dest, b"the user's own keys");
    // no content registered: the fake download fails
    let r = e.svc.replace_key(3, "ryujinx").await;
    assert_eq!((r["success"].clone(), r["reason"].clone()), (json!(false), json!("download_failed")));
    assert_eq!(std::fs::read(&dest).unwrap(), b"the user's own keys");
    assert!(!std::path::Path::new(&format!("{dest}.tmp")).exists());
}

#[tokio::test]
async fn only_a_differing_key_of_an_installed_emulator_that_reads_it_is_replaced() {
    let e = env(&["eden", "ryujinx"]);
    with_romm_keys(&e);
    let h = e.home();
    // missing: that is a download, not a replace
    let r = e.svc.replace_key(3, "ryujinx").await;
    assert_eq!(r["reason"], "not_present");
    // already RomM's
    e.write(&format!("{h}/.local/share/eden/keys/prod.keys"), b"abc");
    assert_eq!(e.svc.replace_key(3, "eden").await["reason"], "already_matches");
    // an emulator that is not installed, one that does not read the file, a firmware archive, an unknown id
    assert_eq!(e.svc.replace_key(3, "yuzu").await["reason"], "emulator_not_found");
    assert_eq!(e.svc.replace_key(1, "eden").await["reason"], "not_used", "Eden does not read console.keys");
    assert_eq!(e.svc.replace_key(5, "eden").await["reason"], "not_a_key");
    assert_eq!(e.svc.replace_key(999, "eden").await["reason"], "not_found");
    // RomM without a checksum for the file: not verifiable, not replaced
    e.set_list(json!([{"id": 3, "file_name": "prod.keys", "file_path": "bios/Switch", "file_size_bytes": 3}]));
    e.svc.invalidate();
    e.write(&format!("{h}/.config/Ryujinx/system/prod.keys"), b"mine");
    assert_eq!(e.svc.replace_key(3, "ryujinx").await["reason"], "no_checksum");
    assert_eq!(std::fs::read(format!("{h}/.config/Ryujinx/system/prod.keys")).unwrap(), b"mine");
}

#[tokio::test]
async fn replacing_with_romm_offline_says_so() {
    let e = env(&["ryujinx"]);
    *e.api.list.lock().unwrap() = Err(RommError::Connection("down".into()));
    assert_eq!(e.svc.replace_key(3, "ryujinx").await["reason"], "server_unreachable");
}
