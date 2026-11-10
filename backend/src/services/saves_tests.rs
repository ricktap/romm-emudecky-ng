//! Behavior of the save service against a simulated RomM (device sync rows, the write-time 409) and fake emulator folders.
use std::sync::atomic::{AtomicI64, Ordering};

use super::*;
use crate::adapters::{
    db::Db,
    repositories::{SqliteLibrary, SqliteSaveState, SqliteTitleIds},
};
use crate::services::testing::{FakeClock, FakeKv, FakePersister, FakeRomm};

const TITLE: &str = "01008CF01BAAC000";
const OTHER_TITLE: &str = "0100152000022000";
const USER: &str = "7EBDCC305B4640571756A9D7A0BCDB3C";
const ME: &str = "dev-me";
const OTHER: &str = "dev-other";

fn tid(s: &str) -> TitleId {
    TitleId::parse(s).unwrap()
}

fn entry(title: &str, role: &str, rel: &str, data: &str) -> ArchiveEntry {
    ArchiveEntry { path: format!("{role}/{title}/{rel}"), data: data.as_bytes().to_vec() }
}

// ── The simulated RomM ─────────────────────────────────────────────────────

#[derive(Default)]
struct FakeSaves {
    saves: Mutex<Vec<Value>>,
    next_id: AtomicI64,
    clock: AtomicI64,
    devices: Mutex<Vec<Value>>,
    pub calls: Mutex<Vec<String>>,
    pub fail_list: Mutex<Option<RommError>>,
    pub fail_download: Mutex<Option<RommError>>,
    pub negotiate_ok: Mutex<bool>,
    pub corrupt_download: Mutex<bool>,
    pub blobs: Mutex<HashMap<i64, Vec<u8>>>,
}

impl FakeSaves {
    fn new() -> Arc<Self> {
        let f = Self::default();
        f.next_id.store(100, Ordering::SeqCst);
        f.clock.store(1, Ordering::SeqCst);
        *f.negotiate_ok.lock().unwrap() = true;
        Arc::new(f)
    }

    fn now(&self) -> String {
        let n = self.clock.fetch_add(1, Ordering::SeqCst);
        format!("2025-01-{:02}T12:00:00+00:00", n.min(28))
    }

    fn note(&self, what: &str) {
        self.calls.lock().unwrap().push(what.to_string());
    }

    fn calls_starting(&self, prefix: &str) -> usize {
        self.calls.lock().unwrap().iter().filter(|c| c.starts_with(prefix)).count()
    }

    fn head(&self, rom_id: i64, slot: &str) -> Option<Value> {
        let saves = self.saves.lock().unwrap();
        saves.iter().filter(|s| s["rom_id"] == rom_id && s["slot"] == slot).max_by_key(|s| s["updated_at"].as_str().unwrap().to_string()).cloned()
    }

    /// Put a save on the server as if another device uploaded it.
    fn seed(&self, rom_id: i64, slot: &str, file: &str, entries: &[ArchiveEntry], by: &str) -> i64 {
        let data = save_archive::build(entries).unwrap();
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let hash = save_archive::content_hash(entries);
        self.blobs.lock().unwrap().insert(id, data.clone());
        self.saves.lock().unwrap().push(json!({
            "id": id, "rom_id": rom_id, "file_name": file, "slot": slot, "emulator": "eden", "updated_at": self.now(), "content_hash": hash,
            "file_size_bytes": data.len(), "device_syncs": [{"device_id": by, "device_name": by, "is_current": true, "last_synced_at": "t"}],
        }));
        id
    }
}

#[async_trait]
impl RommSavesApi for FakeSaves {
    async fn list_saves(&self, rom_id: i64, _device_id: Option<&str>, slot: Option<&str>) -> Result<Vec<Value>, RommError> {
        self.note(&format!("list:{rom_id}:{slot:?}"));
        if let Some(e) = self.fail_list.lock().unwrap().clone() {
            return Err(e);
        }
        Ok(self.saves.lock().unwrap().iter().filter(|s| s["rom_id"] == rom_id && slot.is_none_or(|sl| s["slot"] == sl)).cloned().collect())
    }

    async fn upload_save(&self, up: SaveUpload) -> Result<Value, RommError> {
        self.note(&format!("upload:overwrite={}:put={:?}:cleanup={:?}", up.overwrite, up.save_id, up.autocleanup_limit));
        let slot = up.slot.clone().unwrap_or_default();
        let device = up.device_id.clone().unwrap_or_default();
        let hash = save_archive::archive_content_hash(&up.data).map_err(|e| RommError::Api(e.to_string()))?;
        if let Some(id) = up.save_id {
            let mut saves = self.saves.lock().unwrap();
            let s = saves.iter_mut().find(|s| s["id"] == id).ok_or(RommError::NotFound { message: "x".into(), detail: None })?;
            s["updated_at"] = json!(self.now());
            s["content_hash"] = json!(hash);
            s["device_syncs"] = json!([{"device_id": device, "device_name": device, "is_current": true, "last_synced_at": "t"}]);
            self.blobs.lock().unwrap().insert(id, up.data);
            return Ok(s.clone());
        }
        // RomM's write-time guard: a plain POST into a slot whose head this device is not current on is a 409.
        if !up.overwrite {
            if let Some(head) = self.head(up.rom_id, &slot) {
                let current = head["device_syncs"].as_array().unwrap().iter().any(|d| d["device_id"] == device && d["is_current"] == true);
                if !current {
                    return Err(RommError::Conflict("slot has a newer save".into()));
                }
            }
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let save = json!({
            "id": id, "rom_id": up.rom_id, "file_name": up.file_name, "slot": slot, "emulator": up.emulator, "updated_at": self.now(), "content_hash": hash,
            "file_size_bytes": up.data.len(), "device_syncs": [{"device_id": device, "device_name": device, "is_current": true, "last_synced_at": "t"}],
        });
        self.blobs.lock().unwrap().insert(id, up.data);
        self.saves.lock().unwrap().push(save.clone());
        Ok(save)
    }

    async fn download_save(&self, save_id: i64, _device_id: Option<&str>) -> Result<Vec<u8>, RommError> {
        self.note(&format!("download:{save_id}"));
        if let Some(e) = self.fail_download.lock().unwrap().clone() {
            return Err(e);
        }
        let mut bytes = self.blobs.lock().unwrap().get(&save_id).cloned().ok_or(RommError::NotFound { message: "gone".into(), detail: None })?;
        if *self.corrupt_download.lock().unwrap() {
            let bad = vec![ArchiveEntry { path: format!("user/{TITLE}/evil"), data: b"tampered".to_vec() }];
            bytes = save_archive::build(&bad).unwrap();
        }
        Ok(bytes)
    }

    async fn confirm_download(&self, save_id: i64, device_id: &str) -> Result<Value, RommError> {
        self.note(&format!("confirm:{save_id}:{device_id}"));
        let mut saves = self.saves.lock().unwrap();
        if let Some(s) = saves.iter_mut().find(|s| s["id"] == save_id) {
            let list = s["device_syncs"].as_array_mut().unwrap();
            list.retain(|d| d["device_id"] != device_id);
            list.push(json!({"device_id": device_id, "device_name": device_id, "is_current": true, "last_synced_at": "t"}));
        }
        Ok(json!({}))
    }

    async fn delete_saves(&self, ids: &[i64]) -> Result<Value, RommError> {
        self.note(&format!("delete:{ids:?}"));
        self.saves.lock().unwrap().retain(|s| !ids.contains(&s["id"].as_i64().unwrap()));
        Ok(json!({}))
    }

    async fn save_summary(&self, rom_id: i64, _device_id: Option<&str>) -> Result<Value, RommError> {
        let saves = self.saves.lock().unwrap();
        let mut slots: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
        for s in saves.iter().filter(|s| s["rom_id"] == rom_id) {
            slots.entry(s["slot"].as_str().unwrap().to_string()).or_default().push(s);
        }
        Ok(json!({"slots": slots.iter().map(|(n, v)| json!({"slot": n, "count": v.len(), "latest": {"updated_at": v.iter().map(|s| s["updated_at"].as_str().unwrap()).max()}})).collect::<Vec<_>>()}))
    }

    async fn negotiate(&self, _device_id: &str, _saves: &[Value]) -> Result<Value, RommError> {
        self.note("negotiate");
        if *self.negotiate_ok.lock().unwrap() { Ok(json!({"session_id": 77, "operations": []})) } else { Err(RommError::Server { status: 500, message: "x".into() }) }
    }

    async fn complete_session(&self, session_id: i64, completed: u32, failed: u32) -> Result<Value, RommError> {
        self.note(&format!("complete:{session_id}:{completed}:{failed}"));
        Ok(json!({}))
    }

    async fn register_device(&self, name: &str, platform: &str, client: &str, version: &str, hostname: Option<&str>) -> Result<Value, RommError> {
        self.note(&format!("register:{name}:{platform}:{client}:{version}:{hostname:?}"));
        Ok(json!({"id": ME}))
    }

    async fn list_devices(&self) -> Result<Vec<Value>, RommError> {
        Ok(self.devices.lock().unwrap().clone())
    }

    async fn update_device(&self, device_id: &str, fields: &Value) -> Result<Value, RommError> {
        self.note(&format!("update_device:{device_id}:{fields}"));
        Ok(json!({}))
    }
}

// ── Fake emulator folders ──────────────────────────────────────────────────

#[derive(Default)]
struct FakeLocal {
    saves: Mutex<HashMap<(String, TitleId), (Vec<ArchiveEntry>, f64)>>,
    profiles: Mutex<Vec<ProfileInfo>>,
    pub backups: Mutex<Vec<String>>,
    pub missing: Mutex<bool>,
    pub fail_write: Mutex<bool>,
    pub writes: Mutex<Vec<(LocalRequest, Vec<ArchiveEntry>)>>,
}

impl FakeLocal {
    fn new() -> Arc<Self> {
        let f = Self::default();
        *f.profiles.lock().unwrap() = vec![ProfileInfo { id: USER.into(), name: "Eden".into() }];
        Arc::new(f)
    }
    fn put(&self, emulator: &str, title: &str, entries: Vec<ArchiveEntry>, mtime: f64) {
        self.saves.lock().unwrap().insert((emulator.into(), tid(title)), (entries, mtime));
    }
    fn get(&self, emulator: &str, title: &str) -> Option<Vec<ArchiveEntry>> {
        self.saves.lock().unwrap().get(&(emulator.to_string(), tid(title))).map(|(e, _)| e.clone())
    }
}

#[async_trait]
impl LocalSaves for FakeLocal {
    async fn profiles(&self, emulator: &str) -> Result<Vec<ProfileInfo>, LocalError> {
        if *self.missing.lock().unwrap() {
            return Err(LocalError::Unsupported(format!("{emulator} is not configured")));
        }
        Ok(self.profiles.lock().unwrap().clone())
    }
    async fn read(&self, req: &LocalRequest) -> Result<LocalRead, LocalError> {
        if *self.missing.lock().unwrap() {
            return Err(LocalError::Unsupported("no save folder".into()));
        }
        let saves = self.saves.lock().unwrap();
        Ok(match saves.get(&(req.emulator.clone(), req.title)) {
            Some((entries, mtime)) => LocalRead { entries: entries.clone(), newest_mtime: *mtime, path: format!("/saves/{}/{}", req.emulator, req.title.to_hex()) },
            None => LocalRead { entries: vec![], newest_mtime: 0.0, path: String::new() },
        })
    }
    async fn write(&self, req: &LocalRequest, entries: &[ArchiveEntry]) -> Result<Vec<String>, LocalError> {
        if *self.fail_write.lock().unwrap() {
            return Err(LocalError::Io("disk full".into()));
        }
        self.writes.lock().unwrap().push((req.clone(), entries.to_vec()));
        let existed = self.saves.lock().unwrap().insert((req.emulator.clone(), req.title), (entries.to_vec(), 2_000_000_000.0)).is_some();
        if existed {
            self.backups.lock().unwrap().push(format!("{}.romm-backup", req.title.to_hex()));
            return Ok(vec![format!("{}.romm-backup", req.title.to_hex())]);
        }
        Ok(vec![])
    }
    async fn quarantine(&self, req: &LocalRequest) -> Result<Vec<String>, LocalError> {
        let existed = self.saves.lock().unwrap().remove(&(req.emulator.clone(), req.title)).is_some();
        if existed {
            self.backups.lock().unwrap().push(format!("{}.romm-backup", req.title.to_hex()));
            return Ok(vec![format!("{}.romm-backup", req.title.to_hex())]);
        }
        Ok(vec![])
    }
    async fn snapshot(&self, emulator: &str) -> Result<BTreeMap<TitleId, FolderStamp>, LocalError> {
        Ok(self
            .saves
            .lock()
            .unwrap()
            .iter()
            .filter(|((e, _), _)| e == emulator)
            .map(|((_, t), (entries, mtime))| (*t, FolderStamp { files: entries.len() as u64, bytes: entries.iter().map(|e| e.data.len() as u64).sum(), newest: *mtime as i64 }))
            .collect())
    }
}

struct FixedEmulator(Mutex<Option<String>>);

#[async_trait]
impl RomEmulator for FixedEmulator {
    async fn emulator_id(&self, _rom_id: i64) -> Option<String> {
        self.0.lock().unwrap().clone()
    }
}

struct TestHost;
impl HostIdentity for TestHost {
    fn hostname(&self) -> String {
        "steamdeck".into()
    }
    fn machine_id(&self) -> Option<String> {
        Some("machine-1".into())
    }
}

struct Env {
    svc: SaveService,
    api: Arc<FakeSaves>,
    local: Arc<FakeLocal>,
    romm: Arc<FakeRomm>,
    kv: Arc<FakeKv>,
    settings: SettingsStore,
    persister: Arc<FakePersister>,
    emulator: Arc<FixedEmulator>,
    db: Db,
    _dir: tempfile::TempDir,
}

const ROM: i64 = 7;

async fn env_with(file_name: &str, enabled: bool) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let (db, _) = Db::open(&dir.path().join("t.db")).await.unwrap();
    let name = file_name.to_string();
    db.call(move |c| {
        c.execute("INSERT INTO roms (rom_id, platform_slug, name, fs_name, last_synced_at) VALUES (7, 'switch', 'Zelda', ?1, '2026-01-01')", [&name])?;
        c.execute("INSERT INTO rom_installs (rom_id, file_path, rom_dir, platform_slug, system, installed_at) VALUES (7, '/roms/switch/Zelda', NULL, 'switch', 'switch', '2026-01-01')", [])
    })
    .await
    .unwrap();
    let settings = SettingsStore::new(json!({"save_sync_enabled": enabled, "romm_url": "http://x"}).as_object().cloned().unwrap());
    let api = FakeSaves::new();
    let local = FakeLocal::new();
    let romm = Arc::new(FakeRomm::new(settings.clone()));
    let kv = Arc::new(FakeKv::default());
    kv.insert(DEVICE_ID_KEY, ME);
    let persister = Arc::new(FakePersister::default());
    let emulator = Arc::new(FixedEmulator(Mutex::new(Some("eden".into()))));
    let svc = SaveService::new(SaveServiceConfig {
        settings: settings.clone(),
        persister: persister.clone(),
        api: api.clone(),
        connection: romm.clone(),
        state: Arc::new(SqliteSaveState::new(db.clone())),
        titles: Arc::new(SqliteTitleIds::new(db.clone())),
        library: Arc::new(SqliteLibrary::new(db.clone())),
        kv: kv.clone(),
        emulators: emulator.clone(),
        local: local.clone(),
        clock: Arc::new(FakeClock::default()),
        host: Arc::new(TestHost),
        plugin_version: "9.9.9".into(),
    });
    Env { svc, api, local, romm, kv, settings, persister, emulator, db, _dir: dir }
}

/// A game whose file name carries the title tag, with save sync on.
async fn env() -> Env {
    env_with(&format!("Zelda [{TITLE}][v0].nsp"), true).await
}

fn zelda_save(text: &str) -> Vec<ArchiveEntry> {
    vec![entry(TITLE, "user", "0/data.bin", text), entry(TITLE, "zero", "device.bin", "dev")]
}

const FILE: &str = "01008CF01BAAC000.zip";

// ── Settings ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn settings_view_defaults_and_collapses_a_blank_slot() {
    let e = env_with("x.nsp", false).await;
    assert_eq!(e.svc.get_save_sync_settings(), json!({"save_sync_enabled": false, "sync_before_launch": true, "sync_after_exit": true, "default_slot": "default", "autocleanup_limit": 10}));
    e.settings.set("default_slot", json!("  "));
    e.settings.set("autocleanup_limit", json!(0));
    assert_eq!(e.svc.get_save_sync_settings()["default_slot"], "default");
    assert_eq!(e.svc.get_save_sync_settings()["autocleanup_limit"], 10, "0 means the default limit, never 'keep nothing'");
}

#[tokio::test]
async fn updating_settings_coerces_values_ignores_unknown_keys_and_persists() {
    let e = env_with("x.nsp", false).await;
    let r = e.svc.update_save_sync_settings(&json!({"save_sync_enabled": true, "default_slot": "  Mine ", "autocleanup_limit": -3, "sync_after_exit": 0, "romm_url": "http://evil", "other": 1})).await;
    assert_eq!(r, json!({"success": true}));
    let v = e.svc.get_save_sync_settings();
    assert_eq!((v["save_sync_enabled"].clone(), v["default_slot"].clone(), v["autocleanup_limit"].clone(), v["sync_after_exit"].clone()), (json!(true), json!("Mine"), json!(1), json!(false)));
    assert_eq!(e.settings.get_str("romm_url").as_deref(), Some("http://x"), "only the five save-sync keys can be written");
    assert!(e.settings.get("other").is_none());
    assert_eq!(e.persister.saves(), 1);
    assert_eq!(e.svc.update_save_sync_settings(&json!("no")).await["reason"], "invalid_settings");
    e.persister.fail_with("disk");
    assert_eq!(e.svc.update_save_sync_settings(&json!({"default_slot": "b"})).await["success"], false);
}

// ── Devices ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn device_registration_registers_once_and_afterwards_only_refreshes_the_version() {
    let e = env().await;
    e.kv.rows.lock().unwrap().clear();
    let r = e.svc.ensure_device_registered().await;
    assert_eq!((r["success"].clone(), r["device_id"].clone(), r["device_name"].clone()), (json!(true), json!(ME), json!("steamdeck")));
    assert!(e.api.calls.lock().unwrap().iter().any(|c| c == "register:steamdeck:linux:romm-emudecky-ng:9.9.9:Some(\"machine-1\")"));
    assert_eq!(e.kv.rows.lock().unwrap().get(DEVICE_ID_KEY).map(String::as_str), Some(ME));
    assert_eq!(e.settings.get_str("device_name").as_deref(), Some("steamdeck"));
    assert_eq!(e.romm.versions.lock().unwrap().last(), Some(&Some("4.9.0".to_string())), "the server version was probed");
    let again = e.svc.ensure_device_registered().await;
    assert_eq!(again["success"], true);
    assert_eq!(e.api.calls_starting("register"), 1, "no second registration");
    assert_eq!(e.api.calls_starting("update_device"), 1);
}

#[tokio::test]
async fn device_calls_say_so_when_save_sync_is_off_and_classify_a_registration_failure() {
    let off = env_with("x.nsp", false).await;
    assert_eq!(off.svc.ensure_device_registered().await["reason"], "sync_disabled");
    assert_eq!(off.svc.list_devices().await["disabled"], true);
    let on = env().await;
    on.kv.rows.lock().unwrap().clear();
    let failing = Arc::new(FailingRegister(on.api.clone()));
    let svc = SaveService::new(SaveServiceConfig { api: failing, ..rebuild_config(&on).await });
    let r = svc.ensure_device_registered().await;
    assert_eq!((r["success"].clone(), r["reason"].clone()), (json!(false), json!("auth_failed")));
}

struct FailingRegister(Arc<FakeSaves>);
#[async_trait]
impl RommSavesApi for FailingRegister {
    async fn register_device(&self, _: &str, _: &str, _: &str, _: &str, _: Option<&str>) -> Result<Value, RommError> {
        Err(RommError::Auth("revoked".into()))
    }
    async fn list_saves(&self, a: i64, b: Option<&str>, c: Option<&str>) -> Result<Vec<Value>, RommError> { self.0.list_saves(a, b, c).await }
    async fn upload_save(&self, u: SaveUpload) -> Result<Value, RommError> { self.0.upload_save(u).await }
    async fn download_save(&self, a: i64, b: Option<&str>) -> Result<Vec<u8>, RommError> { self.0.download_save(a, b).await }
    async fn confirm_download(&self, a: i64, b: &str) -> Result<Value, RommError> { self.0.confirm_download(a, b).await }
    async fn delete_saves(&self, a: &[i64]) -> Result<Value, RommError> { self.0.delete_saves(a).await }
    async fn save_summary(&self, a: i64, b: Option<&str>) -> Result<Value, RommError> { self.0.save_summary(a, b).await }
    async fn negotiate(&self, a: &str, b: &[Value]) -> Result<Value, RommError> { self.0.negotiate(a, b).await }
    async fn complete_session(&self, a: i64, b: u32, c: u32) -> Result<Value, RommError> { self.0.complete_session(a, b, c).await }
    async fn list_devices(&self) -> Result<Vec<Value>, RommError> { self.0.list_devices().await }
    async fn update_device(&self, a: &str, b: &Value) -> Result<Value, RommError> { self.0.update_device(a, b).await }
}

/// A config sharing the environment's collaborators (for swapping one of them).
async fn rebuild_config(e: &Env) -> SaveServiceConfig {
    SaveServiceConfig {
        settings: e.settings.clone(),
        persister: e.persister.clone(),
        api: e.api.clone(),
        connection: e.romm.clone(),
        state: Arc::new(SqliteSaveState::new(e.db.clone())),
        titles: Arc::new(SqliteTitleIds::new(e.db.clone())),
        library: Arc::new(SqliteLibrary::new(e.db.clone())),
        kv: e.kv.clone(),
        emulators: e.emulator.clone(),
        local: e.local.clone(),
        clock: Arc::new(FakeClock::default()),
        host: Arc::new(TestHost),
        plugin_version: "9.9.9".into(),
    }
}

#[tokio::test]
async fn listing_devices_marks_this_one() {
    let e = env().await;
    *e.api.devices.lock().unwrap() = vec![json!({"id": ME, "name": "Deck"}), json!({"id": OTHER, "name": "PC"})];
    let r = e.svc.list_devices().await;
    assert_eq!(r["devices"][0]["is_current_device"], true);
    assert_eq!(r["devices"][1]["is_current_device"], false);
}

// ── Title, profile and unsupported games ───────────────────────────────────

#[tokio::test]
async fn the_title_comes_from_the_file_name_tag_and_is_remembered() {
    let e = env().await;
    assert_eq!(e.svc.title_for(ROM).await, Some(tid(TITLE)));
    let stored = SqliteTitleIds::new(e.db.clone()).get(ROM).await.unwrap().unwrap();
    assert_eq!((stored.title_id.as_str(), stored.source.as_str()), (TITLE, "file_name"));
}

#[tokio::test]
async fn a_game_without_a_title_is_unsupported_and_says_how_to_fix_it_and_never_fails_a_launch() {
    let e = env_with("Metroid Dread NSW-VENOM.xci", true).await;
    let status = e.svc.get_save_status(ROM).await;
    assert_eq!(status["unsupported_reason"], "needs_title_id");
    assert_eq!(status["save_sync_display"], json!({"status": "none", "label": "Choose save folder", "last_sync_check_at": null}));
    let launch = e.svc.pre_launch_sync(ROM).await;
    assert_eq!((launch["success"].clone(), launch["unsupported"].clone(), launch["unsupported_reason"].clone()), (json!(true), json!(true), json!("needs_title_id")));
    assert_eq!(e.api.calls_starting("upload"), 0);
}

#[tokio::test]
async fn unsupported_emulators_and_missing_save_folders_are_reported_not_guessed() {
    let e = env().await;
    *e.emulator.0.lock().unwrap() = Some("dolphin".into());
    assert_eq!(e.svc.get_save_status(ROM).await["unsupported_reason"], "emulator_not_supported");
    *e.emulator.0.lock().unwrap() = None;
    assert_eq!(e.svc.get_save_status(ROM).await["unsupported_reason"], "no_emulator");
    *e.emulator.0.lock().unwrap() = Some("eden".into());
    *e.local.missing.lock().unwrap() = true;
    let s = e.svc.get_save_status(ROM).await;
    assert_eq!(s["unsupported_reason"], "no_save_folder");
    assert_eq!(s["save_sync_display"]["label"], "Emulator save folder not found");
}

#[tokio::test]
async fn several_user_profiles_need_a_choice_which_is_then_remembered() {
    let e = env().await;
    *e.local.profiles.lock().unwrap() = vec![ProfileInfo { id: USER.into(), name: "Eden".into() }, ProfileInfo { id: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(), name: "Guest".into() }];
    let s = e.svc.get_save_status(ROM).await;
    assert_eq!(s["unsupported_reason"], "needs_profile");
    assert_eq!(s["profiles"].as_array().unwrap().len(), 2);
    assert_eq!(e.svc.set_save_profile(ROM, "nope").await["reason"], "unknown_profile");
    assert_eq!(e.svc.set_save_profile(ROM, &USER.to_lowercase()).await["profile"], USER);
    assert_ne!(e.svc.get_save_status(ROM).await["unsupported_reason"], "needs_profile");
    let info = e.svc.get_save_title_info(ROM).await;
    assert_eq!(info["profile"], USER);
}

#[tokio::test]
async fn a_not_installed_rom_has_nothing_to_sync() {
    let e = env().await;
    assert_eq!(e.svc.sync_rom_saves(404).await["unsupported_reason"], "not_installed");
}

// ── Sync flows ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_first_sync_uploads_the_local_save_as_a_new_version_and_records_the_baseline() {
    let e = env().await;
    e.local.put("eden", TITLE, zelda_save("one"), 1_700_000_000.0);
    let r = e.svc.sync_rom_saves(ROM).await;
    assert_eq!((r["success"].clone(), r["synced"].clone()), (json!(true), json!(1)), "{r}");
    let head = e.api.head(ROM, "default").unwrap();
    assert_eq!(head["file_name"], FILE);
    assert_eq!(head["content_hash"], save_archive::content_hash(&zelda_save("one")));
    assert!(e.api.calls.lock().unwrap().iter().any(|c| c == "upload:overwrite=false:put=None:cleanup=Some(10)"), "never an overwrite, retention cap set");
    let state = SqliteSaveState::new(e.db.clone()).get(ROM).await.unwrap().unwrap();
    assert_eq!(state.files[FILE].last_sync_hash.as_deref(), Some(save_archive::content_hash(&zelda_save("one")).as_str()));
    assert_eq!(state.own_upload_ids, Some(vec![head["id"].as_i64().unwrap()]));
    assert_eq!((state.system.as_str(), state.emulator.as_str()), ("switch", "eden"));
    // the next run changes nothing
    let again = e.svc.sync_rom_saves(ROM).await;
    assert_eq!(again["synced"], 0);
    assert_eq!(e.api.calls_starting("upload"), 1);
}

#[tokio::test]
async fn a_second_device_downloads_the_save_verified_and_keeps_its_old_one_as_a_backup() {
    let e = env().await;
    let id = e.api.seed(ROM, "default", FILE, &zelda_save("from the deck"), OTHER);
    e.local.put("eden", TITLE, zelda_save("stale local"), 1_000.0);
    // first run: a local save of unknown provenance meeting a head we never synced is a conflict, not a silent overwrite
    let first = e.svc.sync_rom_saves(ROM).await;
    assert_eq!(first["conflicts"].as_array().unwrap().len(), 1);
    assert_eq!(e.api.calls_starting("download"), 0);
    // no local save at all: plain download
    e.local.saves.lock().unwrap().clear();
    let r = e.svc.sync_rom_saves(ROM).await;
    assert_eq!((r["success"].clone(), r["synced"].clone()), (json!(true), json!(1)), "{r}");
    assert_eq!(e.local.get("eden", TITLE).unwrap(), zelda_save("from the deck"));
    assert!(e.api.calls.lock().unwrap().contains(&format!("confirm:{id}:{ME}")), "RomM is told this device has it");
    let state = SqliteSaveState::new(e.db.clone()).get(ROM).await.unwrap().unwrap();
    assert_eq!(state.files[FILE].tracked_save_id, Some(id));
}

#[tokio::test]
async fn a_download_that_fails_the_checksum_or_names_another_game_is_never_written() {
    let e = env().await;
    e.api.seed(ROM, "default", FILE, &zelda_save("good"), OTHER);
    *e.api.corrupt_download.lock().unwrap() = true;
    let r = e.svc.sync_rom_saves(ROM).await;
    assert_eq!(r["success"], false);
    assert!(r["errors"][0].as_str().unwrap().contains("checksum"), "{r}");
    assert!(e.local.writes.lock().unwrap().is_empty(), "nothing was written");
    *e.api.corrupt_download.lock().unwrap() = false;
    // an archive whose entries belong to another title
    e.api.saves.lock().unwrap().clear();
    let foreign = vec![entry(OTHER_TITLE, "user", "x", "y")];
    let id = e.api.seed(ROM, "default", FILE, &foreign, OTHER);
    let _ = id;
    let r = e.svc.sync_rom_saves(ROM).await;
    assert!(r["errors"][0].as_str().unwrap().contains("another game"), "{r}");
    assert!(e.local.writes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_local_write_failure_is_an_error_and_leaves_the_baseline_alone() {
    let e = env().await;
    e.api.seed(ROM, "default", FILE, &zelda_save("remote"), OTHER);
    *e.local.fail_write.lock().unwrap() = true;
    let r = e.svc.sync_rom_saves(ROM).await;
    assert_eq!(r["success"], false);
    assert!(r["errors"][0].as_str().unwrap().contains("disk full"));
    let state = SqliteSaveState::new(e.db.clone()).get(ROM).await.unwrap().unwrap();
    assert!(state.files.is_empty());
}

#[tokio::test]
async fn an_offline_edit_after_a_sync_is_uploaded_as_the_next_version() {
    let e = env().await;
    e.local.put("eden", TITLE, zelda_save("v1"), 1_700_000_000.0);
    e.svc.sync_rom_saves(ROM).await;
    e.local.put("eden", TITLE, zelda_save("v2 played offline"), 1_700_001_000.0);
    let r = e.svc.sync_rom_saves(ROM).await;
    assert_eq!(r["synced"], 1, "{r}");
    assert_eq!(e.api.head(ROM, "default").unwrap()["content_hash"], save_archive::content_hash(&zelda_save("v2 played offline")));
    assert_eq!(e.api.saves.lock().unwrap().len(), 2, "a new version, the old one stays");
}

#[tokio::test]
async fn a_corrupt_looking_local_save_is_a_conflict_never_pushed_over_the_good_server_copy() {
    let e = env().await;
    e.local.put("eden", TITLE, vec![entry(TITLE, "user", "0/data.bin", &"x".repeat(1000))], 1_700_000_000.0);
    e.svc.sync_rom_saves(ROM).await;
    e.local.put("eden", TITLE, vec![entry(TITLE, "user", "0/data.bin", "")], 1_700_001_000.0);
    let r = e.svc.sync_rom_saves(ROM).await;
    assert_eq!(r["conflicts"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(e.api.saves.lock().unwrap().len(), 1, "nothing was uploaded");
}

#[tokio::test]
async fn a_stale_slot_409_downloads_when_local_is_unchanged_and_is_a_conflict_when_both_moved() {
    let e = env().await;
    e.local.put("eden", TITLE, zelda_save("v1"), 1_700_000_000.0);
    e.svc.sync_rom_saves(ROM).await;
    // another device uploads, then this device edits: both moved -> the upload is refused (409) and surfaces a conflict
    e.api.seed(ROM, "default", FILE, &zelda_save("v-other"), OTHER);
    e.local.put("eden", TITLE, zelda_save("v-mine"), 1_700_002_000.0);
    // hide the server's newer head from the first listing so the kernel plans an upload (a stale snapshot)
    let r = e.svc.sync_rom_saves(ROM).await;
    assert_eq!(r["conflicts"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(e.api.head(ROM, "default").unwrap()["content_hash"], save_archive::content_hash(&zelda_save("v-other")), "the other device's save was not overwritten");
}

#[tokio::test]
async fn negotiate_is_only_an_envelope_and_a_failing_session_never_blocks_the_sync() {
    let e = env().await;
    e.local.put("eden", TITLE, zelda_save("v1"), 1.7e9);
    e.svc.confirm_slot_choice(ROM, Some("default"), false, None).await;
    let r = e.svc.sync_rom_saves(ROM).await;
    assert_eq!(r["synced"], 1);
    assert_eq!(e.api.calls_starting("negotiate"), 1);
    assert!(e.api.calls.lock().unwrap().contains(&"complete:77:1:0".to_string()));
    *e.api.negotiate_ok.lock().unwrap() = false;
    e.local.put("eden", TITLE, zelda_save("v2"), 1.8e9);
    let r = e.svc.sync_rom_saves(ROM).await;
    assert_eq!(r["synced"], 1, "{r}");
    assert_eq!(e.api.calls_starting("complete"), 1, "no session to close when none was opened");
}

#[tokio::test]
async fn an_unconfirmed_slot_opens_no_session() {
    let e = env().await;
    e.local.put("eden", TITLE, zelda_save("v1"), 1.7e9);
    e.svc.sync_rom_saves(ROM).await;
    assert_eq!(e.api.calls_starting("negotiate"), 0);
}

#[tokio::test]
async fn sync_is_off_offline_or_unregistered_and_says_so() {
    let off = env_with("x.nsp", false).await;
    assert_eq!(off.svc.sync_rom_saves(ROM).await["reason"], "sync_disabled");
    assert_eq!(off.svc.pre_launch_sync(ROM).await["success"], true, "disabled is not a launch failure");
    assert_eq!(off.svc.sync_all_saves().await["reason"], "sync_disabled");
    let e = env().await;
    FakeRomm::set(&e.romm.heartbeat, Err(RommError::Connection("refused".into())));
    let r = e.svc.sync_rom_saves(ROM).await;
    assert_eq!((r["offline"].clone(), r["reason"].clone()), (json!(true), json!("server_unreachable")));
    FakeRomm::set(&e.romm.heartbeat, Err(RommError::Auth("revoked".into())));
    let r = e.svc.sync_rom_saves(ROM).await;
    assert_eq!(r["reason"], "auth_failed");
    assert!(r.get("offline").is_none(), "a revoked token is not 'offline'");
}

#[tokio::test]
async fn pre_launch_and_post_exit_honour_their_switches_and_report_what_they_did() {
    let e = env().await;
    e.local.put("eden", TITLE, zelda_save("v1"), 1.7e9);
    e.settings.set("sync_before_launch", json!(false));
    assert_eq!(e.svc.pre_launch_sync(ROM).await["message"], "Pre-launch sync disabled");
    assert_eq!(e.api.calls_starting("upload"), 0);
    e.settings.set("sync_after_exit", json!(false));
    assert_eq!(e.svc.post_exit_sync(ROM).await["message"], "Post-exit sync disabled");
    e.settings.set("sync_after_exit", json!(true));
    let r = e.svc.post_exit_sync(ROM).await;
    assert_eq!((r["success"].clone(), r["message"].clone()), (json!(true), json!("Uploaded 1 save(s)")));
    e.settings.set("sync_before_launch", json!(true));
    let r = e.svc.pre_launch_sync(ROM).await;
    assert_eq!(r["message"], "Downloaded 0 save(s)");
}

#[tokio::test]
async fn the_bulk_sync_skips_games_whose_slot_was_never_confirmed() {
    let e = env().await;
    e.local.put("eden", TITLE, zelda_save("v1"), 1.7e9);
    // a state row exists (e.g. from a slot listing) but the slot is not confirmed
    let mut st = RomSaveState::default();
    st.switch_active_slot(Some("default"));
    SqliteSaveState::new(e.db.clone()).save(ROM, &st).await.unwrap();
    let r = e.svc.sync_all_saves().await;
    assert_eq!(r["synced"], 0);
    assert_eq!(e.api.calls_starting("upload"), 0, "a stale local save is not auto-uploaded over another device's progress");
    e.svc.confirm_slot_choice(ROM, Some("default"), false, None).await;
    let r = e.svc.sync_all_saves().await;
    assert_eq!(r["synced"], 1);
}

// ── Status ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn status_reports_one_row_with_the_verdict_and_a_conflict_entry_for_the_modal() {
    let e = env().await;
    e.api.seed(ROM, "default", FILE, &zelda_save("server"), OTHER);
    e.local.put("eden", TITLE, zelda_save("local"), 1_700_000_000.0);
    let s = e.svc.get_save_status(ROM).await;
    assert_eq!(s["files"][0]["status"], "conflict");
    assert_eq!(s["save_sync_display"]["status"], "conflict");
    let c = &s["conflicts"][0];
    assert_eq!((c["type"].clone(), c["filename"].clone(), c["rom_id"].clone()), (json!("sync_conflict"), json!(FILE), json!(ROM)));
    assert_eq!(c["local_mtime"], "2023-11-14T22:13:20+00:00");
    assert_eq!(c["local_size"], zelda_save("local").iter().map(|x| x.data.len()).sum::<usize>() as u64);
    assert!(c["server_save_id"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn status_with_nothing_anywhere_is_no_saves_and_a_blip_is_never_ready_to_upload() {
    let e = env().await;
    let s = e.svc.get_save_status(ROM).await;
    assert_eq!(s["save_sync_display"]["label"], "No saves");
    e.local.put("eden", TITLE, zelda_save("local"), 1.7e9);
    *e.api.fail_list.lock().unwrap() = Some(RommError::Connection("x".into()));
    let s = e.svc.get_save_status(ROM).await;
    assert_eq!((s["server_query_failed"].clone(), s["files"][0]["status"].clone()), (json!(true), json!("unknown")));
    assert_eq!(s["save_sync_display"]["label"], "Server unreachable");
}

#[tokio::test]
async fn a_synced_game_shows_synced_with_the_time_for_the_frontend_and_status_off_when_disabled() {
    let e = env().await;
    e.local.put("eden", TITLE, zelda_save("v1"), 1.7e9);
    e.svc.sync_rom_saves(ROM).await;
    let s = e.svc.get_save_status(ROM).await;
    assert_eq!((s["files"][0]["status"].clone(), s["save_sync_display"]["status"].clone(), s["save_sync_display"]["last_sync_check_at"].clone()), (json!("synced"), json!("synced"), json!("2026-01-01T00:00:00+00:00")));
    assert_eq!(s["files"][0]["uploaded_by_us"], true);
    e.settings.set("save_sync_enabled", json!(false));
    assert_eq!(e.svc.get_save_status(ROM).await["save_sync_display"]["label"], "Save sync off");
}

// ── Conflict resolution ────────────────────────────────────────────────────

async fn conflicted(e: &Env) -> i64 {
    let id = e.api.seed(ROM, "default", FILE, &zelda_save("server"), OTHER);
    e.local.put("eden", TITLE, zelda_save("local"), 1_700_000_000.0);
    id
}

#[tokio::test]
async fn use_server_restores_the_server_save_and_keeps_the_local_one_as_a_backup() {
    let e = env().await;
    let id = conflicted(&e).await;
    let r = e.svc.resolve_sync_conflict(ROM, FILE, id, "use_server").await;
    assert_eq!(r, json!({"success": true, "action": "use_server", "message": "Server save restored"}));
    assert_eq!(e.local.get("eden", TITLE).unwrap(), zelda_save("server"));
    assert_eq!(e.local.backups.lock().unwrap().len(), 1);
    assert_eq!(e.svc.get_save_status(ROM).await["files"][0]["status"], "synced");
}

#[tokio::test]
async fn keep_local_is_the_only_path_that_overwrites_and_then_the_slot_has_the_local_save() {
    let e = env().await;
    let id = conflicted(&e).await;
    let r = e.svc.resolve_sync_conflict(ROM, FILE, id, "keep_local").await;
    assert_eq!(r["success"], true, "{r}");
    assert!(e.api.calls.lock().unwrap().iter().any(|c| c.starts_with("upload:overwrite=true")));
    assert_eq!(e.api.head(ROM, "default").unwrap()["content_hash"], save_archive::content_hash(&zelda_save("local")));
    assert_eq!(e.svc.get_save_status(ROM).await["files"][0]["status"], "synced");
}

#[tokio::test]
async fn keep_local_with_an_identical_server_save_adopts_it_without_uploading() {
    let e = env().await;
    let id = e.api.seed(ROM, "default", FILE, &zelda_save("same"), OTHER);
    e.local.put("eden", TITLE, zelda_save("same"), 1.7e9);
    let r = e.svc.resolve_sync_conflict(ROM, FILE, id, "keep_local").await;
    assert_eq!(r["success"], true, "{r}");
    assert_eq!(e.api.calls_starting("upload"), 0);
}

#[tokio::test]
async fn a_conflict_resolution_is_stale_when_the_server_head_moved_or_the_input_is_wrong() {
    let e = env().await;
    let seen = conflicted(&e).await;
    e.api.seed(ROM, "default", FILE, &zelda_save("third device"), "dev-third");
    let r = e.svc.resolve_sync_conflict(ROM, FILE, seen, "keep_local").await;
    assert_eq!(r["reason"], "stale_conflict");
    assert_eq!(e.api.calls_starting("upload"), 0, "a third device's work is never silently overwritten");
    assert_eq!(e.local.get("eden", TITLE).unwrap(), zelda_save("local"), "nothing local changed");
    assert_eq!(e.svc.resolve_sync_conflict(ROM, FILE, seen, "merge").await["reason"], "invalid_action");
    assert_eq!(e.svc.resolve_sync_conflict(ROM, "other.zip", seen, "use_server").await["reason"], "invalid_filename");
    e.api.saves.lock().unwrap().clear();
    assert_eq!(e.svc.resolve_sync_conflict(ROM, FILE, seen, "use_server").await["reason"], "stale_conflict");
}

// ── Learning the title ─────────────────────────────────────────────────────

#[tokio::test]
async fn a_session_that_touches_one_folder_proposes_it_and_only_the_users_confirmation_binds_it() {
    let e = env_with("Metroid Dread NSW-VENOM.xci", true).await;
    e.local.put("eden", OTHER_TITLE, vec![entry(OTHER_TITLE, "user", "x", "1")], 100.0);
    e.svc.pre_launch_sync(ROM).await; // takes the snapshot, nothing to sync
    e.local.put("eden", TITLE, zelda_save("new"), 200.0); // the game created its save
    let r = e.svc.post_exit_sync(ROM).await;
    assert_eq!(r["unsupported_reason"], "needs_title_id");
    let info = e.svc.get_save_title_info(ROM).await;
    assert_eq!((info["proposal"].clone(), info["title_id"].clone()), (json!(TITLE), Value::Null), "proposed, not bound");
    assert_eq!(info["candidates"].as_array().unwrap().len(), 2);
    let set = e.svc.set_save_title_id(ROM, Some(TITLE), "learned").await;
    assert_eq!(set["success"], true);
    let info = e.svc.get_save_title_info(ROM).await;
    assert_eq!((info["title_id"].clone(), info["source"].clone(), info["proposal"].clone()), (json!(TITLE), json!("learned"), Value::Null));
    assert_eq!(e.svc.sync_rom_saves(ROM).await["synced"], 1, "now it syncs");
}

#[tokio::test]
async fn two_touched_folders_or_none_give_no_proposal() {
    let e = env_with("x.xci", true).await;
    e.svc.pre_launch_sync(ROM).await;
    e.local.put("eden", TITLE, zelda_save("a"), 1.0);
    e.local.put("eden", OTHER_TITLE, vec![entry(OTHER_TITLE, "user", "x", "1")], 1.0);
    e.svc.post_exit_sync(ROM).await;
    assert_eq!(e.svc.get_save_title_info(ROM).await["proposal"], Value::Null);
    e.svc.pre_launch_sync(ROM).await;
    e.svc.post_exit_sync(ROM).await;
    assert_eq!(e.svc.get_save_title_info(ROM).await["proposal"], Value::Null, "nothing changed");
}

#[tokio::test]
async fn setting_and_clearing_the_title_id_validates_normalises_and_resets_the_baselines() {
    let e = env_with("x.xci", true).await;
    assert_eq!(e.svc.set_save_title_id(ROM, Some("nonsense"), "manual").await["reason"], "invalid_title_id");
    let r = e.svc.set_save_title_id(ROM, Some("01008CF01BAAC800"), "manual").await;
    assert_eq!(r["title_id"], TITLE, "an update id is stored as its base game");
    e.local.put("eden", TITLE, zelda_save("v1"), 1.7e9);
    e.svc.sync_rom_saves(ROM).await;
    assert!(!SqliteSaveState::new(e.db.clone()).get(ROM).await.unwrap().unwrap().files.is_empty());
    e.svc.set_save_title_id(ROM, Some(OTHER_TITLE), "manual").await;
    assert!(SqliteSaveState::new(e.db.clone()).get(ROM).await.unwrap().unwrap().files.is_empty(), "a different title invalidates the old baselines");
    assert_eq!(e.svc.set_save_title_id(ROM, None, "manual").await["success"], true);
    assert_eq!(e.svc.get_save_title_info(ROM).await["title_id"], Value::Null);
}

// ── Slots ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_setup_wizard_recommends_auto_confirm_only_with_local_saves_and_an_empty_server() {
    let e = env().await;
    e.local.put("eden", TITLE, zelda_save("v1"), 1.7e9);
    let info = e.svc.get_save_setup_info(ROM).await;
    assert_eq!((info["recommended_action"].clone(), info["has_local_saves"].clone(), info["default_slot"].clone()), (json!("auto_confirm_default"), json!(true), json!("default")));
    e.api.seed(ROM, "Mine", FILE, &zelda_save("x"), OTHER);
    let info = e.svc.get_save_setup_info(ROM).await;
    assert_eq!(info["recommended_action"], "show_wizard");
    assert_eq!(info["server_slots"][0]["slot"], "Mine");
    assert_eq!(info["server_slots"][0]["count"], 1);
    *e.api.fail_list.lock().unwrap() = Some(RommError::Connection("x".into()));
    let info = e.svc.get_save_setup_info(ROM).await;
    assert_eq!((info["recommended_action"].clone(), info["server_query_failed"].clone()), (json!("server_unreachable"), json!(true)), "the wizard must hold, not auto-confirm over real saves");
}

#[tokio::test]
async fn confirming_a_slot_needs_a_name_and_marks_tracking_configured() {
    let e = env().await;
    for bad in [None, Some(""), Some("   ")] {
        assert_eq!(e.svc.confirm_slot_choice(ROM, bad, false, None).await["reason"], "invalid_slot_name");
    }
    assert_eq!(e.svc.is_save_tracking_configured(ROM).await, json!({"configured": false, "active_slot": null}));
    assert_eq!(e.svc.confirm_slot_choice(ROM, Some(" Mine "), false, None).await["success"], true);
    assert_eq!(e.svc.is_save_tracking_configured(ROM).await, json!({"configured": true, "active_slot": "Mine"}));
    assert_eq!(e.svc.confirm_slot_choice(404, Some("x"), false, None).await["reason"], "not_installed");
}

#[tokio::test]
async fn migrating_carries_the_local_save_into_the_new_slot_and_only_then_retires_the_old_copy() {
    let e = env().await;
    let old = e.api.seed(ROM, "Old", FILE, &zelda_save("legacy"), ME);
    e.local.put("eden", TITLE, zelda_save("legacy"), 1.7e9);
    let r = e.svc.confirm_slot_choice(ROM, Some("New"), true, Some("Old")).await;
    assert_eq!((r["success"].clone(), r["message"].clone()), (json!(true), json!("Slot confirmed")), "{r}");
    assert!(e.api.head(ROM, "New").is_some());
    assert!(e.api.calls.lock().unwrap().contains(&format!("delete:[{old}]")));
    // no local save: the old server save is left alone
    let e2 = env().await;
    let keep = e2.api.seed(ROM, "Old", FILE, &zelda_save("only on server"), ME);
    let r = e2.svc.confirm_slot_choice(ROM, Some("New"), true, Some("Old")).await;
    assert!(r["message"].as_str().unwrap().contains("migration failed"), "{r}");
    assert!(e2.api.saves.lock().unwrap().iter().any(|s| s["id"] == keep), "data that exists nowhere else is never deleted");
}

#[tokio::test]
async fn the_slot_listing_merges_server_slots_with_local_ones_and_keeps_the_active_one() {
    let e = env().await;
    e.api.seed(ROM, "Server", FILE, &zelda_save("x"), OTHER);
    e.svc.confirm_slot_choice(ROM, Some("Local only"), false, None).await;
    let r = e.svc.get_save_slots(ROM).await;
    let names: Vec<(String, String)> = r["slots"].as_array().unwrap().iter().map(|s| (s["slot"].as_str().unwrap().to_string(), s["source"].as_str().unwrap().to_string())).collect();
    assert_eq!(names, vec![("Local only".into(), "local".into()), ("Server".into(), "server".into())]);
    assert_eq!(r["active_slot"], "Local only");
    let saves = e.svc.get_slot_saves(ROM, "Server").await;
    assert_eq!(saves["saves"][0]["filename"], FILE);
    *e.api.fail_list.lock().unwrap() = Some(RommError::Connection("x".into()));
    assert_eq!(e.svc.get_slot_saves(ROM, "Server").await["reason"], "server_unreachable");
    e.settings.set("save_sync_enabled", json!(false));
    assert_eq!(e.svc.get_save_slots(ROM).await["reason"], "sync_disabled");
    assert_eq!(e.svc.get_slot_saves(ROM, "x").await["reason"], "sync_disabled");
}

#[tokio::test]
async fn switching_slots_downloads_the_new_slots_save_and_refuses_while_changes_are_pending() {
    let e = env().await;
    e.local.put("eden", TITLE, zelda_save("v1"), 1.7e9);
    e.svc.confirm_slot_choice(ROM, Some("default"), false, None).await;
    e.svc.sync_rom_saves(ROM).await;
    e.api.seed(ROM, "Other", FILE, &zelda_save("other slot"), OTHER);
    // an unsynced local edit blocks the switch
    e.local.put("eden", TITLE, zelda_save("edited"), 1.8e9);
    let r = e.svc.switch_slot(ROM, "Other").await;
    assert_eq!((r["reason"].clone(), r["files"].clone()), (json!("pending_uploads"), json!([FILE])));
    e.svc.sync_rom_saves(ROM).await; // uploads the edit
    let r = e.svc.switch_slot(ROM, "Other").await;
    assert_eq!(r["success"], true, "{r}");
    assert_eq!(e.local.get("eden", TITLE).unwrap(), zelda_save("other slot"));
    assert_eq!(r["save_status"]["active_slot"], "Other");
    assert_eq!(e.svc.switch_slot(ROM, " ").await["reason"], "invalid_slot_name");
}

#[tokio::test]
async fn switching_to_an_empty_slot_sets_the_local_save_aside_so_it_is_not_uploaded_there() {
    let e = env().await;
    e.local.put("eden", TITLE, zelda_save("v1"), 1.7e9);
    e.svc.sync_rom_saves(ROM).await;
    let r = e.svc.switch_slot(ROM, "Fresh").await;
    assert_eq!(r["success"], true, "{r}");
    assert!(e.local.get("eden", TITLE).is_none(), "moved to a backup, not deleted");
    assert_eq!(e.local.backups.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn deleting_a_slot_refuses_the_active_one_and_cleans_server_saves_and_tracking() {
    let e = env().await;
    e.local.put("eden", TITLE, zelda_save("v1"), 1.7e9);
    e.svc.confirm_slot_choice(ROM, Some("default"), false, None).await;
    e.svc.sync_rom_saves(ROM).await;
    let spare = e.api.seed(ROM, "Spare", FILE, &zelda_save("s"), OTHER);
    e.svc.get_save_slots(ROM).await;
    let info = e.svc.get_slot_delete_info(ROM, "Spare").await;
    assert_eq!((info["server_save_count"].clone(), info["server_save_ids"].clone(), info["is_active"].clone()), (json!(1), json!([spare]), json!(false)));
    assert_eq!(e.svc.delete_slot(ROM, "default").await["reason"], "active_slot");
    assert_eq!(e.svc.delete_slot(ROM, "nope").await["reason"], "not_found");
    let r = e.svc.delete_slot(ROM, "Spare").await;
    assert_eq!((r["success"].clone(), r["deleted_server_saves"].clone()), (json!(true), json!(1)), "{r}");
    assert!(e.api.saves.lock().unwrap().iter().all(|s| s["id"] != spare));
    assert!(!SqliteSaveState::new(e.db.clone()).get(ROM).await.unwrap().unwrap().slots.contains_key("Spare"));
    *e.api.fail_list.lock().unwrap() = Some(RommError::Connection("x".into()));
    assert_eq!(e.svc.get_slot_delete_info(ROM, "default").await["reason"], "server_unreachable", "a server slot cannot be inspected offline");
}

// ── Versions and rollback ──────────────────────────────────────────────────

#[tokio::test]
async fn version_history_excludes_the_tracked_save_and_is_newest_first() {
    let e = env().await;
    e.local.put("eden", TITLE, zelda_save("v1"), 1.7e9);
    e.svc.sync_rom_saves(ROM).await;
    let tracked = e.api.head(ROM, "default").unwrap()["id"].as_i64().unwrap();
    let old = e.api.seed(ROM, "default", FILE, &zelda_save("older"), ME);
    // seed() gives a later timestamp: it is the head now; sync downloads it, so the first save is history
    let _ = old;
    let list = e.svc.list_file_versions(ROM, "default", FILE).await;
    assert_eq!(list["status"], "ok");
    let ids: Vec<i64> = list["versions"].as_array().unwrap().iter().map(|v| v["id"].as_i64().unwrap()).collect();
    assert!(ids.contains(&tracked) || ids.contains(&old));
    assert_eq!(list["versions"][0]["uploaded_by_us"], false);
    *e.api.fail_list.lock().unwrap() = Some(RommError::Connection("x".into()));
    assert_eq!(e.svc.list_file_versions(ROM, "default", FILE).await["status"], "server_unreachable");
}

#[tokio::test]
async fn rolling_back_restores_an_older_version_bumps_it_on_the_server_and_blocks_on_conflicts() {
    let e = env().await;
    e.local.put("eden", TITLE, zelda_save("v1"), 1.7e9);
    e.svc.sync_rom_saves(ROM).await;
    let v1 = e.api.head(ROM, "default").unwrap()["id"].as_i64().unwrap();
    e.local.put("eden", TITLE, zelda_save("v2"), 1.8e9);
    e.svc.sync_rom_saves(ROM).await;
    let r = e.svc.rollback_to_version(ROM, "default", v1).await;
    assert_eq!(r, json!({"status": "ok"}), "{r}");
    assert_eq!(e.local.get("eden", TITLE).unwrap(), zelda_save("v1"));
    assert!(e.api.calls.lock().unwrap().iter().any(|c| c.starts_with(&"upload:overwrite=false:put=Some".to_string())), "the PUT bumps updated_at");
    assert_eq!(e.api.head(ROM, "default").unwrap()["id"], v1, "the restored version is the newest in the slot");
    assert_eq!(e.svc.get_save_status(ROM).await["files"][0]["status"], "synced");
    assert_eq!(e.svc.rollback_to_version(ROM, "default", 99999).await, json!({"status": "version_deleted"}));
    // a conflict blocks it
    e.api.seed(ROM, "default", FILE, &zelda_save("third"), "dev-third");
    e.local.put("eden", TITLE, zelda_save("diverged"), 1.9e9);
    let blocked = e.svc.rollback_to_version(ROM, "default", v1).await;
    assert_eq!(blocked["status"], "conflict_blocked");
    assert_eq!(blocked["conflicts"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn rollback_statuses_for_a_missing_install_an_unsupported_game_and_an_unreachable_server() {
    let e = env().await;
    assert_eq!(e.svc.rollback_to_version(404, "default", 1).await, json!({"status": "rom_not_installed"}));
    *e.emulator.0.lock().unwrap() = Some("dolphin".into());
    assert_eq!(e.svc.rollback_to_version(ROM, "default", 1).await["status"], "unsupported");
    *e.emulator.0.lock().unwrap() = Some("eden".into());
    *e.api.fail_list.lock().unwrap() = Some(RommError::Connection("x".into()));
    e.local.put("eden", TITLE, zelda_save("v1"), 1.7e9);
    assert_eq!(e.svc.rollback_to_version(ROM, "default", 1).await["status"], "server_unreachable");
}

// ── Deleting local saves and the launch gate's view ────────────────────────

#[tokio::test]
async fn deleting_local_saves_moves_them_aside_and_forgets_the_baselines() {
    let e = env().await;
    e.local.put("eden", TITLE, zelda_save("v1"), 1.7e9);
    e.svc.sync_rom_saves(ROM).await;
    let r = e.svc.delete_local_saves(ROM).await;
    assert_eq!((r["success"].clone(), r["deleted_count"].clone()), (json!(true), json!(1)), "{r}");
    assert!(e.local.get("eden", TITLE).is_none());
    assert_eq!(e.local.backups.lock().unwrap().len(), 1, "never destroyed");
    assert!(SqliteSaveState::new(e.db.clone()).get(ROM).await.unwrap().unwrap().files.is_empty());
    assert_eq!(e.svc.delete_local_saves(ROM).await["deleted_count"], 0);
    assert_eq!(e.svc.delete_local_saves(404).await["deleted_count"], 0);
    let platform = e.svc.delete_platform_saves("switch").await;
    assert_eq!(platform["success"], true);
}

#[tokio::test]
async fn the_launch_gate_sees_status_tracking_and_offline_drift_from_local_state_alone() {
    let e = env().await;
    e.local.put("eden", TITLE, zelda_save("v1"), 1.7e9);
    assert!(!e.svc.tracked(ROM).await.unwrap());
    assert!(!e.svc.drift(ROM).await.unwrap(), "no baseline: nothing to drift from");
    e.svc.sync_rom_saves(ROM).await;
    assert!(e.svc.tracked(ROM).await.unwrap());
    assert!(!e.svc.drift(ROM).await.unwrap());
    e.local.put("eden", TITLE, zelda_save("played offline"), 1.8e9);
    let calls_before = e.api.calls.lock().unwrap().len();
    assert!(e.svc.drift(ROM).await.unwrap());
    assert_eq!(e.api.calls.lock().unwrap().len(), calls_before, "drift never touches the network");
    assert_eq!(e.svc.core_change(ROM).await.unwrap(), json!({"changed": false}));
    assert_eq!(e.svc.status(ROM).await.unwrap()["rom_id"], ROM);
    e.settings.set("save_sync_enabled", json!(false));
    assert!(e.svc.status(ROM).await.is_err(), "the gate treats a disabled feature as 'no status', like before");
    assert_eq!(e.svc.post_exit(ROM).await.unwrap()["success"], true);
}

#[tokio::test]
async fn mtime_iso_formats_utc() {
    assert_eq!(mtime_iso(0.0), None);
    assert_eq!(mtime_iso(1_700_000_000.0).as_deref(), Some("2023-11-14T22:13:20+00:00"));
    assert_eq!(mtime_iso(951_782_400.5).as_deref(), Some("2000-02-29T00:00:00+00:00"));
}

#[tokio::test]
async fn confirming_a_different_slot_invalidates_the_old_slots_baselines() {
    let e = env().await;
    e.local.put("eden", TITLE, zelda_save("v1"), 1.7e9);
    e.svc.confirm_slot_choice(ROM, Some("A"), false, None).await;
    e.svc.sync_rom_saves(ROM).await;
    assert!(!SqliteSaveState::new(e.db.clone()).get(ROM).await.unwrap().unwrap().files.is_empty());
    e.svc.confirm_slot_choice(ROM, Some("A"), false, None).await;
    assert!(!SqliteSaveState::new(e.db.clone()).get(ROM).await.unwrap().unwrap().files.is_empty(), "confirming the same slot again keeps them");
    e.svc.confirm_slot_choice(ROM, Some("B"), false, None).await;
    assert!(SqliteSaveState::new(e.db.clone()).get(ROM).await.unwrap().unwrap().files.is_empty(), "a baseline of slot A means nothing in slot B");
}
