//! In-memory fakes of the service ports, for unit tests.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::domain::errors::RommError;
use crate::services::ports::{
    KvConfigRepo, RomRegionsReader, RommConnectionApi, RommLibraryApi, SettingsPersister, SettingsStore,
};

/// Counts saves; can be told to fail. Optionally records the settings
/// snapshot each save would have written.
#[derive(Default)]
pub struct FakePersister {
    saves: AtomicUsize,
    fail: Mutex<Option<String>>,
}

impl FakePersister {
    pub fn saves(&self) -> usize {
        self.saves.load(Ordering::SeqCst)
    }
    pub fn fail_with(&self, message: &str) {
        *self.fail.lock().unwrap() = Some(message.to_string());
    }
}

#[async_trait]
impl SettingsPersister for FakePersister {
    async fn save_settings(&self) -> anyhow::Result<()> {
        if let Some(msg) = self.fail.lock().unwrap().clone() {
            anyhow::bail!(msg);
        }
        self.saves.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Default)]
pub struct FakeRegions(Mutex<Vec<Vec<String>>>);

impl FakeRegions {
    pub fn set(&self, regions: Vec<Vec<String>>) {
        *self.0.lock().unwrap() = regions;
    }
}

#[async_trait]
impl RomRegionsReader for FakeRegions {
    async fn all_rom_regions(&self) -> anyhow::Result<Vec<Vec<String>>> {
        Ok(self.0.lock().unwrap().clone())
    }
}

#[derive(Default)]
pub struct FakeKv {
    pub rows: Mutex<HashMap<String, String>>,
    fail: Mutex<bool>,
}

impl FakeKv {
    pub fn insert(&self, k: &str, v: &str) {
        self.rows.lock().unwrap().insert(k.into(), v.into());
    }
    pub fn has(&self, k: &str) -> bool {
        self.rows.lock().unwrap().contains_key(k)
    }
    pub fn fail(&self) {
        *self.fail.lock().unwrap() = true;
    }
    fn check(&self) -> anyhow::Result<()> {
        if *self.fail.lock().unwrap() {
            anyhow::bail!("database is locked");
        }
        Ok(())
    }
}

#[async_trait]
impl KvConfigRepo for FakeKv {
    async fn get(&self, key: &str) -> anyhow::Result<Option<String>> {
        self.check()?;
        Ok(self.rows.lock().unwrap().get(key).cloned())
    }
    async fn set(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.check()?;
        self.insert(key, value);
        Ok(())
    }
    async fn delete(&self, key: &str) -> anyhow::Result<()> {
        self.check()?;
        self.rows.lock().unwrap().remove(key);
        Ok(())
    }
}

/// One recorded call: method name, arguments, and the auth-relevant settings
/// (token, origin) visible at call time.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub method: &'static str,
    pub args: Vec<String>,
    pub token: Value,
    pub origin: Value,
}

/// Scriptable RomM API. Responses default to a healthy 4.9.0 server.
pub struct FakeRomm {
    settings: SettingsStore,
    pub calls: Mutex<Vec<Call>>,
    pub versions: Mutex<Vec<Option<String>>>,
    pub heartbeat: Mutex<Result<Value, RommError>>,
    pub heartbeat_once: Mutex<Result<Value, RommError>>,
    pub list_platforms: Mutex<Result<Value, RommError>>,
    pub list_collections: Mutex<Result<Value, RommError>>,
    pub list_smart_collections: Mutex<Result<Value, RommError>>,
    pub list_virtual_collections: Mutex<Result<Value, RommError>>,
    pub current_user: Mutex<Result<Value, RommError>>,
    pub mint: Mutex<Result<Value, RommError>>,
    pub delete: Mutex<Result<(), RommError>>,
    pub exchange: Mutex<Result<Value, RommError>>,
}

impl FakeRomm {
    pub fn new(settings: SettingsStore) -> Self {
        Self {
            settings,
            calls: Mutex::new(Vec::new()),
            versions: Mutex::new(Vec::new()),
            heartbeat: Mutex::new(Ok(json!({"SYSTEM": {"VERSION": "4.9.0"}}))),
            heartbeat_once: Mutex::new(Ok(json!({"SYSTEM": {"VERSION": "4.9.0"}}))),
            list_platforms: Mutex::new(Ok(json!([{"id": 1, "slug": "n64"}]))),
            list_collections: Mutex::new(Ok(json!([]))),
            list_smart_collections: Mutex::new(Ok(json!([]))),
            list_virtual_collections: Mutex::new(Ok(json!([]))),
            current_user: Mutex::new(Ok(json!({"id": 1, "username": "tester"}))),
            mint: Mutex::new(Ok(json!({"id": 42, "raw_token": "rmm_minted"}))),
            delete: Mutex::new(Ok(())),
            exchange: Mutex::new(Ok(json!({"id": 99, "raw_token": "rmm_paired"}))),
        }
    }

    fn record(&self, method: &'static str, args: &[&str]) {
        self.calls.lock().unwrap().push(Call {
            method,
            args: args.iter().map(|s| s.to_string()).collect(),
            token: self.settings.get("romm_api_token").unwrap_or(Value::Null),
            origin: self.settings.get("romm_api_token_origin").unwrap_or(Value::Null),
        });
    }

    pub fn calls_to(&self, method: &str) -> Vec<Call> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.method == method)
            .cloned()
            .collect()
    }

    pub fn set<T>(slot: &Mutex<T>, value: T) {
        *slot.lock().unwrap() = value;
    }
}

#[async_trait]
impl RommConnectionApi for FakeRomm {
    fn set_version(&self, version: Option<String>) {
        self.versions.lock().unwrap().push(version);
    }
    async fn heartbeat(&self) -> Result<Value, RommError> {
        self.record("heartbeat", &[]);
        self.heartbeat.lock().unwrap().clone()
    }
    async fn heartbeat_once(&self) -> Result<Value, RommError> {
        self.record("heartbeat_once", &[]);
        self.heartbeat_once.lock().unwrap().clone()
    }
    async fn list_platforms(&self) -> Result<Value, RommError> {
        self.record("list_platforms", &[]);
        self.list_platforms.lock().unwrap().clone()
    }
    async fn get_current_user(&self) -> Result<Value, RommError> {
        self.record("get_current_user", &[]);
        self.current_user.lock().unwrap().clone()
    }
    async fn mint_client_token(&self, username: &str, password: &str, token_name: &str) -> Result<Value, RommError> {
        self.record("mint_client_token", &[username, password, token_name]);
        self.mint.lock().unwrap().clone()
    }
    async fn delete_client_token(&self, username: &str, password: &str, token_id: &str) -> Result<(), RommError> {
        self.record("delete_client_token", &[username, password, token_id]);
        self.delete.lock().unwrap().clone()
    }
    async fn exchange_pairing_code(&self, code: &str) -> Result<Value, RommError> {
        self.record("exchange_pairing_code", &[code]);
        self.exchange.lock().unwrap().clone()
    }
}

#[async_trait]
impl RommLibraryApi for FakeRomm {
    async fn list_platforms(&self) -> Result<Value, RommError> {
        self.record("list_platforms", &[]);
        self.list_platforms.lock().unwrap().clone()
    }

    async fn list_collections(&self) -> Result<Value, RommError> {
        self.record("list_collections", &[]);
        self.list_collections.lock().unwrap().clone()
    }

    async fn list_smart_collections(&self) -> Result<Value, RommError> {
        self.record("list_smart_collections", &[]);
        self.list_smart_collections.lock().unwrap().clone()
    }

    async fn list_virtual_collections(&self, collection_type: &str) -> Result<Value, RommError> {
        self.record("list_virtual_collections", &[collection_type]);
        self.list_virtual_collections.lock().unwrap().clone()
    }
}

// ── Downloads / adoption / launch fakes ───────────────────────────────────

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use crate::adapters::db::Db;
use crate::adapters::download_files::LocalFiles;
use crate::adapters::repositories::{SqliteDownloadJobs, SqliteLibrary};
use crate::domain::download_failure::TransferCancelled;
use crate::services::adoption::{InstallAdoptionService, InstallAdoptionServiceConfig};
use crate::services::downloads::{DownloadService, DownloadServiceConfig};
use crate::services::launch::{LaunchService, LaunchServiceConfig};
use crate::services::ports::{
    Clock, DownloadFiles, EmuDeckPaths, EmulatorLocator, EventSink, ExtractProgress, RommRomApi, SystemResolver,
    TransferHooks, TransferMode, TransferOutcome, TransferPhase,
};
use crate::services::rom_removal::{RomRemovalService, RomRemovalServiceConfig};

/// Records every emitted event.
#[derive(Default)]
pub struct RecordingEvents(pub Mutex<Vec<(String, Value)>>);

impl EventSink for RecordingEvents {
    fn emit(&self, event: &str, payload: Value) {
        self.0.lock().unwrap().push((event.to_string(), payload));
    }
}

impl RecordingEvents {
    pub fn named(&self, event: &str) -> Vec<Value> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(e, _)| e == event)
            .map(|(_, v)| v.clone())
            .collect()
    }
    pub fn statuses(&self, rom_id: i64) -> Vec<String> {
        self.named("download_progress")
            .iter()
            .filter(|v| v["rom_id"] == rom_id)
            .map(|v| v["status"].as_str().unwrap().to_string())
            .collect()
    }
}

/// Fixed wall clock; a monotonic clock that advances by `step` per read.
pub struct FakeClock {
    pub mono: Mutex<f64>,
    pub step: f64,
}

impl Default for FakeClock {
    fn default() -> Self {
        Self {
            mono: Mutex::new(0.0),
            step: 1.0,
        }
    }
}

impl Clock for FakeClock {
    fn epoch_seconds(&self)->f64 {1767225600.0}
    fn now_iso(&self) -> String {
        "2026-01-01T00:00:00+00:00".into()
    }
    fn monotonic(&self) -> f64 {
        let mut m = self.mono.lock().unwrap();
        *m += self.step;
        *m
    }
}

/// One recorded transfer call.
#[derive(Debug, Clone, PartialEq)]
pub struct TransferCall {
    pub rom_id: i64,
    pub file_name: String,
    pub dest: String,
    pub resume: bool,
    pub offset: u64,
    pub mode: TransferMode,
}

/// Scriptable RomM: ROM payloads per id, and transfers that write
/// `content[rom_id]` honouring resume (append from the partial's size).
/// With `stall_at`, a transfer writes that many bytes and then waits for
/// `release` (or its cancel token).
#[derive(Default)]
pub struct FakeRomRomm {
    pub roms: Mutex<HashMap<i64, Result<Value, RommError>>>,
    pub content: Mutex<HashMap<i64, Vec<u8>>>,
    pub transfer_error: Mutex<Option<Arc<dyn Fn() -> anyhow::Error + Send + Sync>>>,
    pub range_supported: Mutex<bool>,
    pub stall_at: Mutex<Option<u64>>,
    pub stalled: Notify,
    pub release: Notify,
    pub calls: Mutex<Vec<TransferCall>>,
    pub get_rom_calls: Mutex<Vec<i64>>,
    /// The identity an archive transfer reports for `content` (a cached ZIP response).
    pub archive_identity: Mutex<Option<crate::domain::archive_transfer::ArchiveIdentity>>,
    /// Records `(identity reported, bytes on disk at that moment)` per archive transfer.
    pub identity_events: Mutex<Vec<(Option<crate::domain::archive_transfer::ArchiveIdentity>, u64)>>,
    /// Phases the next transfer reports before it writes (e.g. waiting for headers).
    pub phases: Mutex<Vec<TransferPhase>>,
    /// Hold a transfer in its header wait until released (or cancelled).
    pub hold_headers: Mutex<bool>,
}

impl FakeRomRomm {
    pub fn set_rom(&self, rom: Value) {
        let id = rom["id"].as_i64().unwrap();
        self.roms.lock().unwrap().insert(id, Ok(rom));
    }
    pub fn fail_rom(&self, id: i64, err: RommError) {
        self.roms.lock().unwrap().insert(id, Err(err));
    }
    pub fn set_content(&self, id: i64, bytes: Vec<u8>) {
        self.content.lock().unwrap().insert(id, bytes);
    }
    pub fn fail_transfers(&self, f: impl Fn() -> anyhow::Error + Send + Sync + 'static) {
        *self.transfer_error.lock().unwrap() = Some(Arc::new(f));
    }
    pub fn calls(&self) -> Vec<TransferCall> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl RommRomApi for FakeRomRomm {
    async fn get_rom(&self, rom_id: i64) -> Result<Value, RommError> {
        self.get_rom_calls.lock().unwrap().push(rom_id);
        self.roms.lock().unwrap().get(&rom_id).cloned().unwrap_or_else(|| {
            Err(RommError::NotFound {
                message: "HTTP 404".into(),
                detail: None,
            })
        })
    }

    async fn download_rom_content(
        &self,
        rom_id: i64,
        file_name: &str,
        dest: &str,
        mode: TransferMode,
        mut hooks: TransferHooks,
    ) -> anyhow::Result<TransferOutcome> {
        use crate::domain::archive_transfer::ResumePlan;
        // How much of the partial this transfer continues (an archive Fresh/replace starts over).
        let continues = match &mode {
            TransferMode::Single { resume } => *resume,
            TransferMode::Archive(ResumePlan::Append(_)) => true,
            TransferMode::Archive(_) => false,
        };
        let offset = if continues {
            std::fs::metadata(dest).map_or(0, |m| m.len())
        } else {
            0
        };
        self.calls.lock().unwrap().push(TransferCall {
            rom_id,
            file_name: file_name.into(),
            dest: dest.into(),
            resume: continues,
            offset,
            mode: mode.clone(),
        });
        for phase in self.phases.lock().unwrap().clone() {
            (hooks.on_phase)(phase);
        }
        if *self.hold_headers.lock().unwrap() {
            (hooks.on_phase)(TransferPhase::Waiting { attempt: 1, max_attempts: 3 });
            tokio::select! {
                () = hooks.cancel.cancelled() => return Err(TransferCancelled.into()),
                () = self.release.notified() => {}
            }
        }
        if let Some(f) = self.transfer_error.lock().unwrap().clone() {
            return Err(f());
        }
        if let Some(meta) = hooks.on_meta.take() {
            meta(*self.range_supported.lock().unwrap());
        }
        let body = self.content.lock().unwrap().get(&rom_id).cloned().unwrap_or_default();
        let total = body.len() as u64;
        if let Some(parent) = std::path::Path::new(dest).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut written = offset;
        if offset == 0 {
            std::fs::write(dest, b"")?;
        }
        if matches!(mode, TransferMode::Archive(ResumePlan::Fresh))
            && let Some(save) = hooks.on_identity.as_mut()
        {
            let identity = self.archive_identity.lock().unwrap().clone();
            let on_disk = std::fs::metadata(dest).map_or(0, |m| m.len());
            self.identity_events.lock().unwrap().push((identity.clone(), on_disk));
            save(identity).await?;
        }
        (hooks.on_phase)(TransferPhase::Transferring);
        let stall = *self.stall_at.lock().unwrap();
        if let Some(stall) = stall.filter(|s| *s > written) {
            let mut f = std::fs::OpenOptions::new().append(true).open(dest)?;
            std::io::Write::write_all(&mut f, &body[written as usize..stall as usize])?;
            written = stall;
            (hooks.progress)(written, total);
            self.stalled.notify_one();
            tokio::select! {
                () = hooks.cancel.cancelled() => return Err(TransferCancelled.into()),
                () = self.release.notified() => {}
            }
        }
        if hooks.cancel.is_cancelled() {
            return Err(TransferCancelled.into());
        }
        let mut f = std::fs::OpenOptions::new().append(true).open(dest)?;
        std::io::Write::write_all(&mut f, &body[written as usize..])?;
        (hooks.progress)(total, total);
        Ok(TransferOutcome::Downloaded)
    }
}

/// The real filesystem with an overridable free-space answer.
#[derive(Default)]
pub struct FakeFiles {
    pub inner: LocalFiles,
    pub free: Mutex<Option<u64>>,
}

#[async_trait]
impl DownloadFiles for FakeFiles {
    async fn exists(&self, path: &str) -> bool {
        self.inner.exists(path).await
    }
    async fn remove_file(&self, path: &str) -> std::io::Result<()> {
        self.inner.remove_file(path).await
    }
    async fn remove_tree(&self, path: &str) -> std::io::Result<()> {
        self.inner.remove_tree(path).await
    }
    async fn make_dirs(&self, path: &str) -> std::io::Result<()> {
        self.inner.make_dirs(path).await
    }
    async fn rename(&self, src: &str, dst: &str) -> std::io::Result<()> {
        self.inner.rename(src, dst).await
    }
    async fn disk_free(&self, path: &str) -> std::io::Result<u64> {
        let free = *self.free.lock().unwrap();
        match free {
            Some(free) => Ok(free),
            None => self.inner.disk_free(path).await,
        }
    }
    async fn file_size(&self, path: &str) -> u64 {
        self.inner.file_size(path).await
    }
    async fn regular_file_size(&self, path: &str) -> Option<u64> {
        self.inner.regular_file_size(path).await
    }
    async fn is_dir(&self, path: &str) -> bool {
        self.inner.is_dir(path).await
    }
    async fn list_entries(&self, dir: &str) -> Vec<(String, bool, u64)> {
        self.inner.list_entries(dir).await
    }
    async fn walk_files_matching_suffixes(&self, base: &str, suffixes: &[&str]) -> Vec<String> {
        self.inner.walk_files_matching_suffixes(base, suffixes).await
    }
    async fn extract_zip(&self, a: &str, d: &str, s: &str, p: ExtractProgress) -> anyhow::Result<()> {
        self.inner.extract_zip(a, d, s, p).await
    }
    async fn verify_zip(&self, archive: &str) -> anyhow::Result<()> {
        self.inner.verify_zip(archive).await
    }
    async fn decode_url_encoded_names(&self, dir: &str) -> anyhow::Result<()> {
        self.inner.decode_url_encoded_names(dir).await
    }
    async fn scan_files_with_sizes(&self, dir: &str) -> Vec<(String, u64)> {
        self.inner.scan_files_with_sizes(dir).await
    }
}

pub struct FixedPaths {
    pub roms: String,
    pub bios: String,
}

impl EmuDeckPaths for FixedPaths {
    fn roms_path(&self) -> String {
        self.roms.clone()
    }
    fn bios_path(&self) -> String {
        self.bios.clone()
    }
}

/// `nintendo-switch` → `switch`; everything else verbatim.
pub struct TestSystems;

impl SystemResolver for TestSystems {
    fn resolve_system(&self, slug: &str, fs_slug: Option<&str>) -> String {
        match slug {
            "nintendo-switch" => "switch".into(),
            "" => fs_slug.unwrap_or_default().into(),
            other => other.into(),
        }
    }
}

/// Installed emulators by id; every listed path counts as executable.
#[derive(Default)]
pub struct FakeLocator {
    pub installed: Mutex<HashMap<String, String>>,
    pub missing: Mutex<HashSet<String>>,
}

impl EmulatorLocator for FakeLocator {
    fn installed(&self, system: &str) -> HashMap<String, String> {
        if system == "switch" {
            self.installed.lock().unwrap().clone()
        } else {
            HashMap::new()
        }
    }
    fn is_executable(&self, path: &str) -> bool {
        !self.missing.lock().unwrap().contains(path)
    }
}

pub const EDEN: &str = "/home/deck/Applications/Eden.AppImage";

/// Real SQLite + real files under a temp dir, fake RomM / events / clock.
pub struct DownloadEnv {
    pub dir: tempfile::TempDir,
    pub db: Db,
    pub jobs: Arc<SqliteDownloadJobs>,
    pub library: Arc<SqliteLibrary>,
    pub romm: Arc<FakeRomRomm>,
    pub events: Arc<RecordingEvents>,
    pub files: Arc<FakeFiles>,
    pub locator: Arc<FakeLocator>,
    pub settings: SettingsStore,
    pub launch: Arc<LaunchService>,
    pub svc: Arc<DownloadService>,
    pub adoption: Arc<InstallAdoptionService>,
    pub clock: Arc<FakeClock>,
}

impl DownloadEnv {
    pub async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        Self::at(dir).await
    }

    pub async fn at(dir: tempfile::TempDir) -> Self {
        let (db, _) = Db::open(&dir.path().join("db.sqlite")).await.unwrap();
        let roms = dir.path().join("Emulation/roms");
        let bios = dir.path().join("Emulation/bios");
        std::fs::create_dir_all(&roms).unwrap();
        std::fs::create_dir_all(&bios).unwrap();
        let paths = Arc::new(FixedPaths {
            roms: roms.to_string_lossy().into_owned(),
            bios: bios.to_string_lossy().into_owned(),
        });
        let jobs = Arc::new(SqliteDownloadJobs::new(db.clone()));
        let library = Arc::new(SqliteLibrary::new(db.clone()));
        let romm = Arc::new(FakeRomRomm::default());
        *romm.range_supported.lock().unwrap() = true;
        let events = Arc::new(RecordingEvents::default());
        let files = Arc::new(FakeFiles::default());
        let locator = Arc::new(FakeLocator::default());
        locator.installed.lock().unwrap().insert("eden".into(), EDEN.into());
        let settings = SettingsStore::new(serde_json::Map::new());
        let clock = Arc::new(FakeClock::default());
        let launch = Arc::new(LaunchService::new(LaunchServiceConfig {
            library: library.clone(),
            locator: locator.clone(),
            systems: Arc::new(TestSystems),
            files: files.clone(),
            settings: settings.clone(),
        }));
        let removal = Arc::new(RomRemovalService::new(RomRemovalServiceConfig {
            library: library.clone(),
            files: files.clone(),
            paths: paths.clone(),
        }));
        let svc = DownloadService::new(DownloadServiceConfig {
            romm: romm.clone(),
            files: files.clone(),
            jobs: jobs.clone(),
            library: library.clone(),
            paths: paths.clone(),
            systems: Arc::new(TestSystems),
            launch: launch.clone(),
            removal,
            events: events.clone(),
            clock: clock.clone(),
        });
        let active = svc.clone();
        let adoption = InstallAdoptionService::new(InstallAdoptionServiceConfig {
            romm: romm.clone(),
            files: files.clone(),
            systems: Arc::new(TestSystems),
            paths,
            launch: launch.clone(),
            library: library.clone(),
            jobs: jobs.clone(),
            active_downloads: Arc::new(move || active.active_download_rom_ids()),
            events: events.clone(),
            clock: clock.clone(),
        });
        Self {
            dir,
            db,
            jobs,
            library,
            romm,
            events,
            files,
            locator,
            settings,
            launch,
            svc,
            adoption,
            clock,
        }
    }

    pub fn roms(&self) -> PathBuf {
        self.dir.path().join("Emulation/roms")
    }

    pub fn roms_s(&self) -> String {
        self.roms().to_string_lossy().into_owned()
    }

    /// Insert a synced `roms` row.
    pub async fn seed_rom(&self, rom_id: i64, slug: &str, fs_name: &str, app_id: Option<i64>, group: Option<&str>) {
        let (slug, fs_name, group) = (slug.to_string(), fs_name.to_string(), group.map(str::to_string));
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT OR REPLACE INTO roms (rom_id, platform_slug, name, fs_name, shortcut_app_id, \
                     last_synced_at, sibling_group_key) VALUES (?1, ?2, ?3, ?4, ?5, 't', ?6)",
                    rusqlite::params![rom_id, slug, format!("Game {rom_id}"), fs_name, app_id, group],
                )
                .map(|_| ())
            })
            .await
            .unwrap();
    }

    pub async fn sql(&self, sql: &'static str) {
        self.db.call(move |c| c.execute_batch(sql)).await.unwrap();
    }

    pub fn entry(&self, rom_id: i64) -> Option<Value> {
        self.svc.get_download_queue()["downloads"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["rom_id"] == rom_id)
            .cloned()
    }

    /// Wait until the queue entry of `rom_id` has `status` and no attempt runs.
    pub async fn wait_status(&self, rom_id: i64, status: &str) -> Value {
        for _ in 0..500 {
            if let Some(e) = self.entry(rom_id)
                && e["status"] == status
                && !self.svc.active_download_rom_ids().contains(&rom_id)
            {
                // Let the attempt's final bookkeeping land.
                tokio::time::sleep(Duration::from_millis(5)).await;
                return e;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("rom {rom_id} never reached {status}: {:?}", self.entry(rom_id));
    }

    pub async fn wait_until(&self, what: &str, f: impl Fn() -> bool) {
        for _ in 0..500 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {what}");
    }
}

/// A RomM single-file Switch ROM payload.
pub fn switch_rom(id: i64, fs_name: &str, size: usize) -> Value {
    json!({
        "id": id, "name": format!("Game {id}"), "fs_name": fs_name, "fs_size_bytes": size,
        "platform_slug": "switch", "platform_fs_slug": "switch", "platform_name": "Nintendo Switch",
        "files": [{"file_name": fs_name, "file_size_bytes": size}],
    })
}

pub fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

/// Build a ZIP with `members` (names ending in `/` are directories).
pub fn zip_bytes(members: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, data) in members {
            if name.ends_with('/') {
                zip.add_directory(*name, opts).unwrap();
            } else {
                zip.start_file(*name, opts).unwrap();
                zip.write_all(data).unwrap();
            }
        }
        zip.finish().unwrap();
    }
    buf.into_inner()
}
