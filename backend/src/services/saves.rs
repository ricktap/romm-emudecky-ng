//! Switch save sync (ADR-0017, ADR-0023): one deterministic zip per `(rom, slot)`, decided by `compute_sync_action`.
//!
//! The service owns what the callables of the Python reference did for RetroArch, rebuilt around the standalone Switch
//! emulators: it finds the game's save through the resolved emulator and a title ID (never guessed), compares the local
//! archive with RomM's saves of the active slot, and uploads, downloads or surfaces a conflict. `negotiate` is only the
//! session envelope; every upload is `overwrite=false` unless the user explicitly keeps their local save; every local write
//! keeps the old save as a `.romm-backup`. A game without a known title, emulator, profile or save folder is
//! **unsupported** and says why; it never fails a launch.
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::domain::{
    errors::RommError,
    iso_time,
    rom_save_state::{Baseline, RomSaveState},
    save_archive::{self, ArchiveEntry},
    save_status::{build_file_status, compute_save_sync_display, resolve_chosen_server, status_from_action, FileStatusInput},
    switch_save::{choose_user_profile, title_id_from_file_name, FolderStamp, ProfileChoice, ProfileInfo, TitleId},
    sync_action::{compute_sync_action, resolve_upload_conflict, FileState, LocalFile, ServerSave, SkipReason, SyncAction, UploadConflict},
};
use crate::services::ports::{
    Clock, EventSink, KvConfigRepo, LibraryRepo, RommConnectionApi, RommSavesApi, SaveSafety, SaveStateRepo, SaveUpload, SettingsPersister,
    SettingsStore, TitleIdRepo,
};
use crate::util::path_safety::basename;

pub const DEVICE_ID_KEY: &str = "device_id";
const SYNC_DISABLED: &str = "Save sync is disabled";

// ── Ports of this service ──────────────────────────────────────────────────

/// A save to read or write locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalRequest {
    pub emulator: String,
    pub title: TitleId,
    /// The chosen user profile folder (Eden/yuzu); ignored by Ryujinx.
    pub profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalRead {
    pub entries: Vec<ArchiveEntry>,
    /// Newest file modification time, seconds since the epoch (0 without files).
    pub newest_mtime: f64,
    /// Where the save lives, for display.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LocalError {
    /// The emulator's save location cannot be found, or the emulator is not supported.
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Io(String),
}

/// The emulators' save directories on this machine.
#[async_trait]
pub trait LocalSaves: Send + Sync {
    /// The profile folders of an emulator (Eden/yuzu) with their shown names; empty for emulators without profiles.
    async fn profiles(&self, emulator: &str) -> Result<Vec<ProfileInfo>, LocalError>;
    async fn read(&self, req: &LocalRequest) -> Result<LocalRead, LocalError>;
    /// Replace the title's save with `entries`; returns the backups made.
    async fn write(&self, req: &LocalRequest, entries: &[ArchiveEntry]) -> Result<Vec<String>, LocalError>;
    /// Move the title's save aside as `<TITLE>.romm-backup` (never deleted); returns the backups made.
    async fn quarantine(&self, req: &LocalRequest) -> Result<Vec<String>, LocalError>;
    /// Every title folder the emulator has, to learn which one a play session touched.
    async fn snapshot(&self, emulator: &str) -> Result<BTreeMap<TitleId, FolderStamp>, LocalError>;
}

/// The emulator a ROM launches with (the resolved one, including the per-game override).
#[async_trait]
pub trait RomEmulator: Send + Sync {
    async fn emulator_id(&self, rom_id: i64) -> Option<String>;
}

/// Who this machine is, for device registration.
pub trait HostIdentity: Send + Sync {
    fn hostname(&self) -> String;
    fn machine_id(&self) -> Option<String>;
}

pub struct SaveServiceConfig {
    pub settings: SettingsStore,
    pub persister: Arc<dyn SettingsPersister>,
    pub api: Arc<dyn RommSavesApi>,
    pub connection: Arc<dyn RommConnectionApi>,
    pub state: Arc<dyn SaveStateRepo>,
    pub titles: Arc<dyn TitleIdRepo>,
    pub library: Arc<dyn LibraryRepo>,
    pub kv: Arc<dyn KvConfigRepo>,
    pub emulators: Arc<dyn RomEmulator>,
    pub local: Arc<dyn LocalSaves>,
    pub clock: Arc<dyn Clock>,
    pub host: Arc<dyn HostIdentity>,
    pub plugin_version: String,
    /// Where `save_status_updated` goes (the frontend listens for it).
    pub events: Arc<dyn EventSink>,
}

// ── Settings (pure) ────────────────────────────────────────────────────────

/// The five save-sync settings as the frontend reads them; blank slot names collapse to `default`.
pub fn settings_view(settings: &SettingsStore) -> Value {
    json!({
        "save_sync_enabled": settings.truthy("save_sync_enabled"),
        "sync_before_launch": bool_setting(settings, "sync_before_launch", true),
        "sync_after_exit": bool_setting(settings, "sync_after_exit", true),
        "default_slot": default_slot(settings),
        "autocleanup_limit": autocleanup_limit(settings),
    })
}

fn bool_setting(settings: &SettingsStore, key: &str, default: bool) -> bool {
    settings.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}

pub fn default_slot(settings: &SettingsStore) -> String {
    let raw = settings.get("default_slot").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    let trimmed = raw.trim();
    if trimmed.is_empty() { "default".into() } else { trimmed.into() }
}

pub fn autocleanup_limit(settings: &SettingsStore) -> u32 {
    settings.get("autocleanup_limit").and_then(|v| v.as_u64()).filter(|n| *n > 0).map_or(10, |n| n.min(u64::from(u32::MAX)) as u32)
}

const SETTING_KEYS: [&str; 5] = ["save_sync_enabled", "sync_before_launch", "sync_after_exit", "default_slot", "autocleanup_limit"];

/// Coerce one incoming setting; `None` drops an unusable value.
pub fn sanitize_setting(key: &str, value: &Value) -> Option<Value> {
    match key {
        "default_slot" => {
            let text = value.as_str().map(str::trim).unwrap_or_default();
            Some(json!(if text.is_empty() { "default" } else { text }))
        }
        "autocleanup_limit" => value.as_i64().or_else(|| value.as_f64().map(|f| f as i64)).map(|n| json!(n.max(1))),
        "save_sync_enabled" | "sync_before_launch" | "sync_after_exit" => Some(json!(match value {
            Value::Bool(b) => *b,
            Value::Null => false,
            Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
            Value::String(s) => !s.is_empty(),
            _ => true,
        })),
        _ => None,
    }
}

// ── Subject: what to sync for one ROM ──────────────────────────────────────

/// Why a game's saves cannot be synced; each says what the user can do.
#[derive(Debug, Clone, PartialEq)]
pub enum Unsupported {
    NotInstalled,
    NoEmulator,
    EmulatorNotSupported(String),
    NoTitle,
    NoSaveFolder(String),
    NeedsProfile(Vec<ProfileInfo>),
}

impl Unsupported {
    pub fn reason(&self) -> &'static str {
        match self {
            Unsupported::NotInstalled => "not_installed",
            Unsupported::NoEmulator => "no_emulator",
            Unsupported::EmulatorNotSupported(_) => "emulator_not_supported",
            Unsupported::NoTitle => "needs_title_id",
            Unsupported::NoSaveFolder(_) => "no_save_folder",
            Unsupported::NeedsProfile(_) => "needs_profile",
        }
    }

    /// The label the game page shows.
    pub fn label(&self) -> String {
        match self {
            Unsupported::NotInstalled => "Not installed".into(),
            Unsupported::NoEmulator => "No emulator".into(),
            Unsupported::EmulatorNotSupported(e) => format!("{e}: saves not supported"),
            Unsupported::NoTitle => "Choose save folder".into(),
            Unsupported::NoSaveFolder(_) => "Emulator save folder not found".into(),
            Unsupported::NeedsProfile(_) => "Choose profile".into(),
        }
    }

    pub fn message(&self) -> String {
        match self {
            Unsupported::NoSaveFolder(detail) => format!("Emulator save folder not found: {detail}"),
            other => other.label(),
        }
    }
}

#[derive(Debug, Clone)]
struct Subject {
    rom_id: i64,
    emulator: String,
    title: TitleId,
    profile: Option<String>,
    file_name: String,
    system: String,
}

impl Subject {
    fn request(&self) -> LocalRequest {
        LocalRequest { emulator: self.emulator.clone(), title: self.title, profile: self.profile.clone() }
    }
}

/// An error inside one sync: unsupported, a local failure, or a RomM failure.
#[derive(Debug)]
enum SaveErr {
    Unsupported(Unsupported),
    Local(String),
    Server(RommError),
}

impl From<RommError> for SaveErr {
    fn from(e: RommError) -> Self {
        SaveErr::Server(e)
    }
}
impl From<LocalError> for SaveErr {
    fn from(e: LocalError) -> Self {
        match e {
            LocalError::Unsupported(d) => SaveErr::Unsupported(Unsupported::NoSaveFolder(d)),
            LocalError::Io(d) => SaveErr::Local(d),
        }
    }
}

/// The local save as the kernel sees it.
#[derive(Debug, Clone)]
struct LocalView {
    entries: Vec<ArchiveEntry>,
    hash: String,
    size: u64,
    mtime: f64,
    path: String,
}

impl LocalView {
    fn file(&self) -> LocalFile {
        LocalFile { size: self.size, mtime: self.mtime }
    }
}

/// The outcome of one ROM's run.
#[derive(Debug, Default)]
pub struct RunResult {
    pub synced: u32,
    pub errors: Vec<String>,
    pub conflicts: Vec<Value>,
}

pub struct SaveService {
    c: SaveServiceConfig,
    locks: Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>,
    /// Title folders at launch, to see which one a session touched (rom id -> snapshot).
    launch_snapshots: Mutex<HashMap<i64, BTreeMap<TitleId, FolderStamp>>>,
    /// The save folder a session touched, proposed once for confirmation (rom id -> title).
    proposals: Mutex<HashMap<i64, TitleId>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn mtime_iso(epoch: f64) -> Option<String> {
    if epoch <= 0.0 {
        return None;
    }
    let days = (epoch / 86400.0).floor() as i64;
    let secs = epoch - days as f64 * 86400.0;
    // civil-from-days (Howard Hinnant), UTC
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let (h, mi, s) = ((secs / 3600.0) as i64, ((secs % 3600.0) / 60.0) as i64, (secs % 60.0) as i64);
    Some(format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}+00:00"))
}

impl SaveService {
    pub fn new(config: SaveServiceConfig) -> Self {
        Self { c: config, locks: Mutex::new(HashMap::new()), launch_snapshots: Mutex::new(HashMap::new()), proposals: Mutex::new(HashMap::new()) }
    }

    fn rom_lock(&self, rom_id: i64) -> Arc<tokio::sync::Mutex<()>> {
        lock(&self.locks).entry(rom_id).or_default().clone()
    }

    fn enabled(&self) -> bool {
        self.c.settings.truthy("save_sync_enabled")
    }

    fn disabled_failure(extra: Value) -> Value {
        let mut v = json!({"success": false, "reason": "sync_disabled", "message": SYNC_DISABLED});
        if let (Some(base), Some(more)) = (v.as_object_mut(), extra.as_object()) {
            base.extend(more.clone());
        }
        v
    }

    // ── Device ──────────────────────────────────────────────────────────────

    async fn device_id(&self) -> Option<String> {
        self.c.kv.get(DEVICE_ID_KEY).await.ok().flatten().filter(|d| !d.is_empty())
    }

    pub async fn ensure_device_registered(&self) -> Value {
        if !self.enabled() {
            return Self::disabled_failure(json!({"device_id": "", "device_name": "", "disabled": true}));
        }
        if let Ok(version) = self.c.connection.heartbeat().await {
            if let Some(v) = version["SYSTEM"]["VERSION"].as_str().filter(|v| !v.is_empty()) {
                self.c.connection.set_version(Some(v.to_string()));
            }
        }
        let name = self.c.settings.get_str("device_name").unwrap_or_default();
        if let Some(id) = self.device_id().await {
            // Best effort: keep the server's client_version current.
            let _ = self.c.api.update_device(&id, &json!({"client_version": self.c.plugin_version})).await;
            return json!({"success": true, "device_id": id, "device_name": name, "server_device_id": id});
        }
        let hostname = self.c.host.hostname();
        let machine = self.c.host.machine_id();
        match self.c.api.register_device(&hostname, "linux", "romm-emudecky-ng", &self.c.plugin_version, machine.as_deref()).await {
            Ok(result) => {
                let id = result["id"].as_str().map(str::to_string).or_else(|| result["id"].as_i64().map(|n| n.to_string())).or_else(|| result["device_id"].as_str().map(str::to_string));
                match id.filter(|i| !i.is_empty()) {
                    Some(id) => {
                        if self.c.kv.set(DEVICE_ID_KEY, &id).await.is_err() {
                            return json!({"success": false, "reason": "unknown", "message": "Could not store the device id", "device_id": "", "device_name": ""});
                        }
                        // The label is best effort: the id already makes the device usable.
                        self.c.settings.set("device_name", json!(hostname));
                        let _ = self.c.persister.save_settings().await;
                        json!({"success": true, "device_id": id, "device_name": hostname, "server_device_id": id})
                    }
                    None => json!({"success": false, "reason": "server_unreachable", "message": "Could not register device", "device_id": "", "device_name": ""}),
                }
            }
            Err(e) => {
                let (reason, message) = e.classify();
                json!({"success": false, "reason": reason, "message": message, "device_id": "", "device_name": ""})
            }
        }
    }

    pub async fn list_devices(&self) -> Value {
        if !self.enabled() {
            return Self::disabled_failure(json!({"devices": [], "disabled": true}));
        }
        let own = self.device_id().await.unwrap_or_default();
        match self.c.api.list_devices().await {
            Ok(devices) => {
                let enriched: Vec<Value> = devices
                    .into_iter()
                    .map(|mut d| {
                        let id = d["id"].as_str().map(str::to_string).or_else(|| d["id"].as_i64().map(|n| n.to_string())).unwrap_or_default();
                        d["is_current_device"] = json!(!own.is_empty() && id == own);
                        d
                    })
                    .collect();
                json!({"success": true, "devices": enriched})
            }
            Err(_) => json!({"success": false, "reason": "server_unreachable", "message": "Could not load devices", "devices": []}),
        }
    }

    // ── Subject, local and server views ─────────────────────────────────────

    async fn state_of(&self, rom_id: i64) -> RomSaveState {
        self.c.state.get(rom_id).await.ok().flatten().unwrap_or_default()
    }

    async fn profile_setting(&self, emulator: &str) -> Option<String> {
        self.c.kv.get(&format!("save_profile:{emulator}")).await.ok().flatten().filter(|p| !p.is_empty())
    }

    /// The title ID of a ROM: the stored one, else a single tag in the file name (stored then), else unknown.
    pub async fn title_for(&self, rom_id: i64) -> Option<TitleId> {
        if let Ok(Some(stored)) = self.c.titles.get(rom_id).await {
            if let Some(id) = TitleId::parse(&stored.title_id) {
                return Some(id);
            }
        }
        let rom = self.c.library.rom(rom_id).await.ok().flatten()?;
        let from_install = self.c.library.install(rom_id).await.ok().flatten().and_then(|i| title_id_from_file_name(basename(&i.file_path)));
        let id = title_id_from_file_name(&rom.fs_name).or(from_install)?;
        let _ = self.c.titles.set(rom_id, &id.to_hex(), "file_name").await;
        Some(id)
    }

    async fn subject(&self, rom_id: i64) -> Result<Subject, Unsupported> {
        let rom = self.c.library.rom(rom_id).await.ok().flatten().ok_or(Unsupported::NotInstalled)?;
        let install = self.c.library.install(rom_id).await.ok().flatten().ok_or(Unsupported::NotInstalled)?;
        let emulator = self.c.emulators.emulator_id(rom_id).await.ok_or(Unsupported::NoEmulator)?;
        if !matches!(emulator.as_str(), "eden" | "yuzu" | "ryujinx") {
            return Err(Unsupported::EmulatorNotSupported(emulator));
        }
        let title = self.title_for(rom_id).await.ok_or(Unsupported::NoTitle)?;
        let profile = if emulator == "ryujinx" {
            None
        } else {
            let profiles = self.c.local.profiles(&emulator).await.map_err(|e| Unsupported::NoSaveFolder(e.to_string()))?;
            let folders: Vec<String> = profiles.iter().map(|p| p.id.clone()).collect();
            let configured = self.profile_setting(&emulator).await;
            match choose_user_profile(&folders, configured.as_deref()) {
                ProfileChoice::Use(p) => Some(p),
                ProfileChoice::NoneYet => None,
                ProfileChoice::NeedsChoice(ids) => {
                    return Err(Unsupported::NeedsProfile(ids.into_iter().map(|id| profiles.iter().find(|p| p.id.eq_ignore_ascii_case(&id)).cloned().unwrap_or(ProfileInfo { id, name: String::new() })).collect()));
                }
            }
        };
        let _ = rom;
        Ok(Subject { rom_id, file_name: format!("{}.zip", title.to_hex()), emulator, title, profile, system: install.system })
    }

    async fn read_local(&self, subject: &Subject) -> Result<Option<LocalView>, SaveErr> {
        let read = self.c.local.read(&subject.request()).await?;
        if read.entries.is_empty() {
            return Ok(None);
        }
        let size = read.entries.iter().map(|e| e.data.len() as u64).sum();
        Ok(Some(LocalView { hash: save_archive::content_hash(&read.entries), entries: read.entries, size, mtime: read.newest_mtime, path: read.path }))
    }

    async fn server_saves(&self, subject: &Subject, device_id: Option<&str>, slot: &str) -> Result<Vec<ServerSave>, RommError> {
        let raw = self.c.api.list_saves(subject.rom_id, device_id, Some(slot)).await?;
        Ok(raw
            .iter()
            .filter(|s| s["file_name"].as_str() == Some(subject.file_name.as_str()))
            .filter(|s| s["slot"].as_str().filter(|x| !x.is_empty()).map_or(slot.is_empty(), |x| x == slot))
            .map(ServerSave::from_json)
            .collect())
    }

    fn slot_of(&self, state: &RomSaveState) -> String {
        state.active_slot.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| default_slot(&self.c.settings))
    }

    fn file_state(state: &RomSaveState, file_name: &str) -> FileState {
        state.files.get(file_name).map_or_else(FileState::default, |f| FileState { last_sync_hash: f.last_sync_hash.clone(), last_sync_local_size: f.last_sync_local_size.and_then(|n| u64::try_from(n).ok()) })
    }

    fn conflict_entry(&self, subject: &Subject, server: &ServerSave, local: Option<&LocalView>) -> Value {
        json!({
            "type": "sync_conflict",
            "rom_id": subject.rom_id,
            "filename": subject.file_name,
            "server_save_id": server.id,
            "server_updated_at": server.updated_at.clone().unwrap_or_default(),
            "server_size": server.raw["file_size_bytes"].as_i64(),
            "local_path": local.map(|l| l.path.clone()),
            "local_hash": local.map(|l| l.hash.clone()),
            "local_mtime": local.and_then(|l| mtime_iso(l.mtime)),
            "local_size": local.map(|l| l.size),
            "created_at": self.c.clock.now_iso(),
        })
    }

    // ── The run: decide, then do ────────────────────────────────────────────

    /// Download `server` into the local save: verify the archive against RomM's hash before anything is written.
    async fn download(&self, subject: &Subject, server: &ServerSave, state: &mut RomSaveState, device_id: &str) -> Result<String, String> {
        let id = server.id.ok_or("the server save has no id")?;
        let bytes = self.c.api.download_save(id, Some(device_id)).await.map_err(|e| e.classify().1)?;
        let entries = save_archive::read(&bytes).map_err(|e| format!("the downloaded save is not a valid archive: {e}"))?;
        let hash = save_archive::content_hash(&entries);
        if let Some(expected) = server.content_hash.as_deref().filter(|h| !h.is_empty()) {
            if expected != hash {
                return Err("the downloaded save does not match the server's checksum".into());
            }
        }
        for entry in &entries {
            let valid_title = entry.path.split('/').nth(1) == Some(subject.title.to_hex().as_str());
            if !valid_title {
                return Err("the downloaded save belongs to another game".into());
            }
        }
        self.c.local.write(&subject.request(), &entries).await.map_err(|e| e.to_string())?;
        // Best effort: tell RomM this device now has this version.
        let _ = self.c.api.confirm_download(id, device_id).await;
        let size = entries.iter().map(|e| e.data.len() as i64).sum::<i64>();
        let mtime = self.c.local.read(&subject.request()).await.map(|r| r.newest_mtime).unwrap_or(0.0);
        state
            .adopt_baseline(
                &subject.file_name,
                Baseline {
                    tracked_save_id: id,
                    last_sync_hash: hash.clone(),
                    last_sync_at: self.c.clock.now_iso(),
                    last_sync_server_updated_at: server.updated_at.clone().unwrap_or_default(),
                    last_sync_server_save_id: Some(id),
                    last_sync_server_size: server.raw["file_size_bytes"].as_i64(),
                    last_sync_local_mtime: Some(mtime),
                    last_sync_local_size: Some(size),
                },
            )
            .map_err(|e| e.to_string())?;
        Ok(hash)
    }

    /// Upload the local save as a new version. `overwrite` only from an explicit keep-local.
    async fn upload(&self, subject: &Subject, local: &LocalView, state: &mut RomSaveState, device_id: &str, slot: &str, overwrite: bool) -> Result<(), RommError> {
        let data = save_archive::build(&local.entries).map_err(|e| RommError::Api(e.to_string()))?;
        let result = self
            .c
            .api
            .upload_save(SaveUpload {
                rom_id: subject.rom_id,
                file_name: subject.file_name.clone(),
                data,
                emulator: subject.emulator.clone(),
                device_id: Some(device_id.to_string()),
                slot: Some(slot.to_string()),
                overwrite,
                autocleanup_limit: Some(autocleanup_limit(&self.c.settings)),
                save_id: None,
            })
            .await?;
        let id = result["id"].as_i64().unwrap_or(0);
        let hash = result["content_hash"].as_str().filter(|h| !h.is_empty()).unwrap_or(&local.hash).to_string();
        if id > 0 {
            let _ = state.adopt_baseline(
                &subject.file_name,
                Baseline {
                    tracked_save_id: id,
                    last_sync_hash: hash,
                    last_sync_at: self.c.clock.now_iso(),
                    last_sync_server_updated_at: result["updated_at"].as_str().unwrap_or_default().to_string(),
                    last_sync_server_save_id: Some(id),
                    last_sync_server_size: result["file_size_bytes"].as_i64(),
                    last_sync_local_mtime: Some(local.mtime),
                    last_sync_local_size: Some(local.size as i64),
                },
            );
            state.track_own_upload(id);
        } else {
            let _ = state.update_baseline_hash(&subject.file_name, &local.hash);
        }
        let _ = state.promote_slot_to_server(slot);
        Ok(())
    }

    /// One ROM's decision and its execution. The state is mutated in memory; the caller persists it.
    async fn run_rom(&self, subject: &Subject, state: &mut RomSaveState, device_id: &str) -> Result<RunResult, SaveErr> {
        let slot = self.slot_of(state);
        let local = self.read_local(subject).await?;
        let servers = self.server_saves(subject, Some(device_id), &slot).await?;
        let file_state = Self::file_state(state, &subject.file_name);
        let action = compute_sync_action(local.as_ref().map(LocalView::file).as_ref(), &servers, &file_state, device_id, local.as_ref().map(|l| l.hash.as_str()));
        let mut out = RunResult::default();
        match action {
            SyncAction::Skip { reason, adopt_baseline } => {
                if adopt_baseline {
                    if let Some(l) = &local {
                        let _ = state.update_baseline_hash(&subject.file_name, &l.hash);
                    }
                }
                let _ = reason == SkipReason::Synced;
            }
            SyncAction::Upload { .. } => {
                let local = local.as_ref().ok_or_else(|| SaveErr::Local("nothing to upload".into()))?;
                match self.upload(subject, local, state, device_id, &slot, false).await {
                    Ok(()) => out.synced += 1,
                    Err(RommError::Conflict(_)) => {
                        // RomM's write-time guard: the head moved past this device. Re-decide from hashes only.
                        let head = self.server_saves(subject, Some(device_id), &slot).await?.into_iter().next();
                        match head {
                            None => out.errors.push("the upload was refused and the slot is empty".into()),
                            Some(head) => match resolve_upload_conflict(Some(&local.hash), file_state.last_sync_hash.as_deref(), head.content_hash.as_deref()) {
                                UploadConflict::Download => match self.download(subject, &head, state, device_id).await {
                                    Ok(_) => out.synced += 1,
                                    Err(e) => out.errors.push(e),
                                },
                                UploadConflict::Conflict => out.conflicts.push(self.conflict_entry(subject, &head, Some(local))),
                            },
                        }
                    }
                    Err(e) => out.errors.push(e.classify().1),
                }
            }
            SyncAction::Download(server) => match self.download(subject, &server, state, device_id).await {
                Ok(_) => out.synced += 1,
                Err(e) => out.errors.push(e),
            },
            SyncAction::Conflict(server) => out.conflicts.push(self.conflict_entry(subject, &server, local.as_ref())),
        }
        state.adopt_system(&subject.system);
        let _ = state.record_synced_core(None, &subject.emulator);
        state.mark_sync_evaluated(&self.c.clock.now_iso());
        Ok(out)
    }

    /// Open a transport-only negotiate session for a confirmed slot; `None` on any failure (the run proceeds without).
    async fn open_session(&self, rom_id: i64, device_id: &str, state: &RomSaveState) -> Option<i64> {
        if !(state.slot_confirmed && state.active_slot.is_some()) {
            return None;
        }
        let inventory = json!({"rom_id": rom_id, "slot": state.active_slot});
        self.c.api.negotiate(device_id, &[inventory]).await.ok().and_then(|r| r["session_id"].as_i64())
    }

    async fn close_session(&self, session: i64, completed: u32, failed: u32) {
        let _ = self.c.api.complete_session(session, completed, failed).await;
    }

    /// Read, run, persist, for one ROM under its lock. `Unsupported` is reported, never raised.
    async fn sync_locked(&self, rom_id: i64, require_confirmed: bool) -> Result<RunResult, Unsupported> {
        let subject = self.subject(rom_id).await?;
        let mut state = self.state_of(rom_id).await;
        if require_confirmed && !state.slot_confirmed {
            return Ok(RunResult::default());
        }
        let device_id = self.device_id().await.unwrap_or_default();
        let session = self.open_session(rom_id, &device_id, &state).await;
        let result = match self.run_rom(&subject, &mut state, &device_id).await {
            Ok(r) => r,
            Err(SaveErr::Unsupported(u)) => {
                if let Some(s) = session {
                    self.close_session(s, 0, 0).await;
                }
                return Err(u);
            }
            Err(SaveErr::Local(m)) => RunResult { errors: vec![m], ..RunResult::default() },
            Err(SaveErr::Server(e)) => RunResult { errors: vec![e.classify().1], ..RunResult::default() },
        };
        let _ = self.c.state.save(rom_id, &state).await;
        if let Some(s) = session {
            self.close_session(s, result.synced, result.errors.len() as u32).await;
        }
        Ok(result)
    }

    fn run_json(result: &RunResult, verb: &str) -> Value {
        let mut msg = format!("{verb} {} save(s)", result.synced);
        if !result.errors.is_empty() {
            msg.push_str(&format!(", {} error(s)", result.errors.len()));
        }
        if !result.conflicts.is_empty() {
            msg.push_str(&format!(", {} conflict(s)", result.conflicts.len()));
        }
        json!({"success": result.errors.is_empty(), "message": msg, "synced": result.synced, "errors": result.errors, "conflicts": result.conflicts})
    }

    fn unsupported_run(u: &Unsupported) -> Value {
        // A game whose saves cannot be located is not a failure: nothing to sync, and a launch must not warn about it.
        json!({"success": true, "message": format!("Save sync: {}", u.message()), "synced": 0, "errors": [], "conflicts": [], "unsupported": true, "unsupported_reason": u.reason()})
    }

    fn heartbeat_failure(e: &RommError) -> Value {
        if matches!(e, RommError::Connection(_) | RommError::Timeout(_)) {
            return json!({"success": false, "reason": "server_unreachable", "message": "Server offline", "synced": 0, "offline": true});
        }
        let (reason, message) = e.classify();
        json!({"success": false, "reason": reason, "message": message, "synced": 0})
    }

    async fn gate(&self) -> Result<String, Value> {
        if let Err(e) = self.c.connection.heartbeat().await {
            return Err(Self::heartbeat_failure(&e));
        }
        if let Some(id) = self.device_id().await {
            return Ok(id);
        }
        let registered = self.ensure_device_registered().await;
        if registered["success"] == true {
            return Ok(registered["device_id"].as_str().unwrap_or_default().to_string());
        }
        Err(json!({"success": false, "reason": "device_not_registered", "message": "Device is not registered with the server"}))
    }

    pub async fn sync_rom_saves(&self, rom_id: i64) -> Value {
        if !self.enabled() {
            return Self::disabled_failure(json!({"synced": 0}));
        }
        // A game whose saves cannot be located is not a failure and never asks the network.
        if let Err(u) = self.subject(rom_id).await {
            return Self::unsupported_run(&u);
        }
        let lock = self.rom_lock(rom_id);
        let _guard = lock.lock().await;
        if let Err(v) = self.gate().await {
            return v;
        }
        match self.sync_locked(rom_id, false).await {
            Ok(r) => Self::run_json(&r, "Synced"),
            Err(u) => Self::unsupported_run(&u),
        }
    }

    /// Download newer saves before a launch. Also remembers the save folders so a session can teach the title.
    pub async fn pre_launch_sync(&self, rom_id: i64) -> Value {
        if !self.enabled() {
            return json!({"success": true, "message": SYNC_DISABLED, "synced": 0});
        }
        self.take_launch_snapshot(rom_id).await;
        if !bool_setting(&self.c.settings, "sync_before_launch", true) {
            return json!({"success": true, "message": "Pre-launch sync disabled", "synced": 0});
        }
        // A game whose saves cannot be located is not a failure and never asks the network.
        if let Err(u) = self.subject(rom_id).await {
            return Self::unsupported_run(&u);
        }
        let lock = self.rom_lock(rom_id);
        let _guard = lock.lock().await;
        if let Err(v) = self.gate().await {
            return v;
        }
        match self.sync_locked(rom_id, false).await {
            Ok(r) => {
                let mut v = Self::run_json(&r, "Downloaded");
                if r.errors.is_empty() {
                    v["conflicts"] = json!(r.conflicts);
                }
                v
            }
            Err(u) => Self::unsupported_run(&u),
        }
    }

    /// Upload changed saves after the game exits; learns the title from a session that touched exactly one folder.
    pub async fn post_exit_sync(&self, rom_id: i64) -> Value {
        if !self.enabled() {
            return json!({"success": true, "message": SYNC_DISABLED, "synced": 0});
        }
        self.learn_from_session(rom_id).await;
        if !bool_setting(&self.c.settings, "sync_after_exit", true) {
            return json!({"success": true, "message": "Post-exit sync disabled", "synced": 0});
        }
        // A game whose saves cannot be located is not a failure and never asks the network.
        if let Err(u) = self.subject(rom_id).await {
            return Self::unsupported_run(&u);
        }
        let lock = self.rom_lock(rom_id);
        let _guard = lock.lock().await;
        if let Err(v) = self.gate().await {
            return v;
        }
        match self.sync_locked(rom_id, false).await {
            Ok(r) => Self::run_json(&r, "Uploaded"),
            Err(u) => Self::unsupported_run(&u),
        }
    }

    pub async fn sync_all_saves(&self) -> Value {
        if !self.enabled() {
            return Self::disabled_failure(json!({"synced": 0, "conflicts": 0}));
        }
        if let Err(v) = self.gate().await {
            return v;
        }
        let ids = self.c.state.rom_ids().await.unwrap_or_default();
        let (mut synced, mut conflicts, mut errors) = (0u32, 0usize, 0usize);
        for id in ids {
            let lock = self.rom_lock(id);
            let _guard = lock.lock().await;
            // A never-configured ROM's stale local save must not be auto-uploaded over another device's progress.
            if let Ok(r) = self.sync_locked(id, true).await {
                synced += r.synced;
                conflicts += r.conflicts.len();
                errors += r.errors.len();
            }
        }
        let mut msg = format!("Synced {synced} save(s)");
        if conflicts > 0 {
            msg.push_str(&format!(", {conflicts} conflict(s)"));
        }
        json!({"success": errors == 0, "message": msg, "synced": synced, "conflicts": conflicts})
    }
}


// ── Learning the title from play ───────────────────────────────────────────

impl SaveService {
    async fn snapshot_of(&self, rom_id: i64) -> Option<(String, BTreeMap<TitleId, FolderStamp>)> {
        let emulator = self.c.emulators.emulator_id(rom_id).await?;
        let snap = self.c.local.snapshot(&emulator).await.ok()?;
        Some((emulator, snap))
    }

    /// Remember the title folders before a launch of a game whose title is not known yet.
    async fn take_launch_snapshot(&self, rom_id: i64) {
        if self.title_for(rom_id).await.is_some() {
            return;
        }
        if let Some((_, snap)) = self.snapshot_of(rom_id).await {
            lock(&self.launch_snapshots).insert(rom_id, snap);
        }
    }

    /// After the game exits: when exactly one title folder changed, propose it (the user confirms; nothing is bound here).
    async fn learn_from_session(&self, rom_id: i64) {
        let Some(before) = lock(&self.launch_snapshots).remove(&rom_id) else { return };
        let Some((_, after)) = self.snapshot_of(rom_id).await else { return };
        let changed = crate::domain::switch_save::changed_titles(&before, &after);
        match crate::domain::switch_save::propose_title(&changed) {
            Some(title) => {
                lock(&self.proposals).insert(rom_id, title);
            }
            None => {
                lock(&self.proposals).remove(&rom_id);
            }
        }
    }

    /// Everything the "choose save folder / profile" UI needs for a ROM.
    pub async fn get_save_title_info(&self, rom_id: i64) -> Value {
        let stored = self.c.titles.get(rom_id).await.ok().flatten();
        let title = self.title_for(rom_id).await;
        let source = stored.map(|s| s.source).or_else(|| title.map(|_| "file_name".to_string()));
        let candidates: Vec<Value> = match self.snapshot_of(rom_id).await {
            Some((_, snap)) => snap
                .iter()
                .map(|(id, stamp)| json!({"title_id": id.to_hex(), "files": stamp.files, "bytes": stamp.bytes, "newest": stamp.newest}))
                .collect(),
            None => vec![],
        };
        let proposal = lock(&self.proposals).get(&rom_id).map(|t| t.to_hex());
        let (profiles, profile_choice) = match self.c.emulators.emulator_id(rom_id).await {
            Some(e) if e != "ryujinx" => {
                let list = self.c.local.profiles(&e).await.unwrap_or_default();
                let configured = self.profile_setting(&e).await;
                let folders: Vec<String> = list.iter().map(|p| p.id.clone()).collect();
                let chosen = match choose_user_profile(&folders, configured.as_deref()) {
                    ProfileChoice::Use(p) => Some(p),
                    _ => None,
                };
                (list.into_iter().filter(|p| p.id != crate::domain::switch_save::ZERO_PROFILE).map(|p| json!({"id": p.id, "name": p.name})).collect::<Vec<_>>(), chosen)
            }
            _ => (vec![], None),
        };
        json!({
            "success": true,
            "title_id": title.map(|t| t.to_hex()),
            "source": source,
            "proposal": proposal,
            "candidates": candidates,
            "profiles": profiles,
            "profile": profile_choice,
        })
    }

    /// Bind (or with `None` unbind) a ROM's title ID after the user's confirmation.
    pub async fn set_save_title_id(&self, rom_id: i64, title_id: Option<&str>, source: &str) -> Value {
        let lock_ = self.rom_lock(rom_id);
        let _guard = lock_.lock().await;
        let Some(text) = title_id.filter(|t| !t.is_empty()) else {
            let _ = self.c.titles.clear(rom_id).await;
            lock(&self.proposals).remove(&rom_id);
            return json!({"success": true, "message": "Save folder cleared"});
        };
        let Some(id) = TitleId::parse(text) else {
            return json!({"success": false, "reason": "invalid_title_id", "message": "Not a Switch title ID"});
        };
        let source = if source == "learned" { "learned" } else { "manual" };
        // A different title invalidates the baselines of the old one.
        let previous = self.c.titles.get(rom_id).await.ok().flatten().map(|s| s.title_id);
        if self.c.titles.set(rom_id, &id.base().to_hex(), source).await.is_err() {
            return json!({"success": false, "reason": "unknown", "message": "Could not store the save folder"});
        }
        if previous.as_deref() != Some(id.base().to_hex().as_str()) {
            let mut state = self.state_of(rom_id).await;
            state.clear_baselines();
            let _ = self.c.state.save(rom_id, &state).await;
        }
        lock(&self.proposals).remove(&rom_id);
        json!({"success": true, "message": "Save folder set", "title_id": id.base().to_hex()})
    }

    /// Remember which user profile a machine's saves live under (Eden/yuzu), after the user chose.
    pub async fn set_save_profile(&self, rom_id: i64, profile: &str) -> Value {
        let Some(emulator) = self.c.emulators.emulator_id(rom_id).await else {
            return json!({"success": false, "reason": "no_emulator", "message": "No emulator"});
        };
        let list = self.c.local.profiles(&emulator).await.unwrap_or_default();
        let Some(found) = list.iter().find(|p| p.id.eq_ignore_ascii_case(profile)) else {
            return json!({"success": false, "reason": "unknown_profile", "message": "That profile does not exist"});
        };
        if self.c.kv.set(&format!("save_profile:{emulator}"), &found.id).await.is_err() {
            return json!({"success": false, "reason": "unknown", "message": "Could not store the profile"});
        }
        json!({"success": true, "message": "Profile set", "profile": found.id})
    }
}

// ── Status ─────────────────────────────────────────────────────────────────

impl SaveService {
    fn empty_status(&self, rom_id: i64, device_id: &str, state: &RomSaveState, label: &str, extra: Value) -> Value {
        let mut v = json!({
            "rom_id": rom_id,
            "files": [],
            "playtime": {"total_seconds": 0, "session_count": 0, "last_session_start": null, "last_session_duration_sec": null, "last_played": null},
            "device_id": device_id,
            "last_sync_check_at": state.last_sync_check_at,
            "conflicts": [],
            "active_slot": state.active_slot,
            "save_sync_display": {"status": "none", "label": label, "last_sync_check_at": null},
            "multi_file": false,
            "component_files": [],
            "rollback_supported": true,
        });
        if let (Some(base), Some(more)) = (v.as_object_mut(), extra.as_object()) {
            base.extend(more.clone());
        }
        v
    }

    /// The `SaveStatus` the game page and the launch gate read: one file row (the title's archive), its verdict, conflicts.
    pub async fn get_save_status(&self, rom_id: i64) -> Value {
        let state = self.state_of(rom_id).await;
        let device_id = self.device_id().await.unwrap_or_default();
        if !self.enabled() {
            return self.empty_status(rom_id, &device_id, &state, "Save sync off", json!({}));
        }
        let subject = match self.subject(rom_id).await {
            Ok(s) => s,
            Err(u) => {
                let extra = match &u {
                    Unsupported::NeedsProfile(list) => json!({"unsupported_reason": u.reason(), "profiles": list.iter().map(|p| json!({"id": p.id, "name": p.name})).collect::<Vec<_>>()}),
                    _ => json!({"unsupported_reason": u.reason()}),
                };
                return self.empty_status(rom_id, &device_id, &state, &u.label(), extra);
            }
        };
        let slot = self.slot_of(&state);
        let local = match self.read_local(&subject).await {
            Ok(l) => l,
            Err(SaveErr::Unsupported(u)) => return self.empty_status(rom_id, &device_id, &state, &u.label(), json!({"unsupported_reason": u.reason()})),
            Err(_) => None,
        };
        let servers = match self.server_saves(&subject, Some(&device_id), &slot).await {
            Ok(s) => s,
            Err(_) => {
                // A connectivity blip must not read as "ready to upload".
                let mut row = build_file_status(FileStatusInput {
                    filename: &subject.file_name,
                    local_path: local.as_ref().map(|l| l.path.as_str()),
                    local_hash: local.as_ref().map(|l| l.hash.as_str()),
                    local_mtime: local.as_ref().and_then(|l| mtime_iso(l.mtime)),
                    local_size: local.as_ref().map(|l| l.size),
                    server: None,
                    last_sync_at: state.files.get(&subject.file_name).map(|f| f.last_sync_at.as_str()),
                    status: "unknown",
                    own_device_id: Some(&device_id),
                    uploaded_by_us: None,
                });
                row["status"] = json!("unknown");
                let display = compute_save_sync_display(&[row.clone()], state.last_sync_check_at.as_deref(), true);
                let mut v = self.empty_status(rom_id, &device_id, &state, "", json!({}));
                v["files"] = json!([row]);
                v["save_sync_display"] = display.to_json();
                v["server_query_failed"] = json!(true);
                return v;
            }
        };
        let file_state = Self::file_state(&state, &subject.file_name);
        let action = compute_sync_action(local.as_ref().map(LocalView::file).as_ref(), &servers, &file_state, &device_id, local.as_ref().map(|l| l.hash.as_str()));
        let chosen = resolve_chosen_server(&action, &servers);
        let uploaded_by_us = chosen.and_then(|s| s.id).and_then(|id| state.own_upload_ids.as_ref().map(|ids| ids.contains(&id)));
        let status = status_from_action(&action);
        let has_content = local.is_some() || chosen.is_some();
        let files: Vec<Value> = if has_content {
            vec![build_file_status(FileStatusInput {
                filename: &subject.file_name,
                local_path: local.as_ref().map(|l| l.path.as_str()),
                local_hash: local.as_ref().map(|l| l.hash.as_str()),
                local_mtime: local.as_ref().and_then(|l| mtime_iso(l.mtime)),
                local_size: local.as_ref().map(|l| l.size),
                server: chosen,
                last_sync_at: state.files.get(&subject.file_name).map(|f| f.last_sync_at.as_str()).filter(|t| !t.is_empty()),
                status,
                own_device_id: Some(&device_id),
                uploaded_by_us,
            })]
        } else {
            vec![]
        };
        let conflicts: Vec<Value> = match &action {
            SyncAction::Conflict(server) => vec![self.conflict_entry(&subject, server, local.as_ref())],
            _ => vec![],
        };
        let display = compute_save_sync_display(&files, state.last_sync_check_at.as_deref(), false);
        let mut v = self.empty_status(rom_id, &device_id, &state, "", json!({}));
        v["files"] = json!(files);
        v["conflicts"] = json!(conflicts);
        v["save_sync_display"] = display.to_json();
        v["component_files"] = json!([subject.file_name]);
        v["active_slot"] = json!(slot);
        v
    }

    /// Recompute the status and push it to the frontend as `save_status_updated`.
    pub async fn refresh_save_status(&self, rom_id: i64) -> Value {
        let status = self.get_save_status(rom_id).await;
        self.c.events.emit("save_status_updated", status);
        json!({"success": true})
    }
}

// ── Conflict resolution ────────────────────────────────────────────────────

impl SaveService {
    /// Commit one side of a two-sided conflict. `server_save_id` is what the user saw: if the slot's head moved since, the
    /// answer is `stale_conflict` and nothing is written (a third device's work is never silently overwritten).
    pub async fn resolve_sync_conflict(&self, rom_id: i64, filename: &str, server_save_id: i64, action: &str) -> Value {
        if !matches!(action, "keep_local" | "use_server") {
            return json!({"success": false, "reason": "invalid_action", "message": format!("Invalid action: {action}")});
        }
        let lock_ = self.rom_lock(rom_id);
        let _guard = lock_.lock().await;
        let subject = match self.subject(rom_id).await {
            Ok(s) => s,
            Err(u) => return json!({"success": false, "reason": u.reason(), "message": u.message()}),
        };
        if filename != subject.file_name {
            return json!({"success": false, "reason": "invalid_filename", "message": "That is not this game's save"});
        }
        let mut state = self.state_of(rom_id).await;
        let device_id = self.device_id().await.unwrap_or_default();
        let slot = self.slot_of(&state);
        let servers = match self.server_saves(&subject, Some(&device_id), &slot).await {
            Ok(s) => s,
            Err(e) => {
                let (reason, message) = e.classify();
                return json!({"success": false, "reason": reason, "message": message});
            }
        };
        let head = servers.iter().fold(None::<&ServerSave>, |best, s| {
            let epoch = |x: &ServerSave| x.updated_at.as_deref().and_then(iso_time::parse).map_or(0.0, |t| t.seconds);
            match best {
                Some(b) if epoch(b) >= epoch(s) => Some(b),
                _ => Some(s),
            }
        });
        let Some(head) = head else {
            return json!({"success": false, "reason": "stale_conflict", "message": "The server save is gone"});
        };
        if head.id != Some(server_save_id) {
            return json!({"success": false, "reason": "stale_conflict", "message": "The server save changed since you looked at it"});
        }
        let result = if action == "use_server" {
            self.download(&subject, head, &mut state, &device_id).await.map(|_| ())
        } else {
            match self.read_local(&subject).await {
                Ok(Some(local)) if head.content_hash.as_deref() == Some(local.hash.as_str()) => {
                    // Already byte-identical: adopt silently instead of re-uploading.
                    let head_id = head.id.unwrap_or(0);
                    state
                        .adopt_baseline(&subject.file_name, Baseline { tracked_save_id: head_id.max(1), last_sync_hash: local.hash.clone(), last_sync_at: self.c.clock.now_iso(), last_sync_server_updated_at: head.updated_at.clone().unwrap_or_default(), last_sync_server_save_id: head.id, last_sync_local_mtime: Some(local.mtime), last_sync_local_size: Some(local.size as i64), ..Baseline::default() })
                        .map_err(|e| e.to_string())
                }
                Ok(Some(local)) => self.upload(&subject, &local, &mut state, &device_id, &slot, true).await.map_err(|e| e.classify().1),
                Ok(None) => Err("There is no local save to keep".to_string()),
                Err(_) => Err("The local save could not be read".to_string()),
            }
        };
        match result {
            Ok(()) => {
                state.mark_sync_evaluated(&self.c.clock.now_iso());
                let _ = self.c.state.save(rom_id, &state).await;
                json!({"success": true, "action": action, "message": if action == "use_server" { "Server save restored" } else { "Local save kept" }})
            }
            Err(message) => json!({"success": false, "message": message}),
        }
    }
}


// ── Settings ───────────────────────────────────────────────────────────────

impl SaveService {
    pub fn get_save_sync_settings(&self) -> Value {
        settings_view(&self.c.settings)
    }

    pub async fn update_save_sync_settings(&self, incoming: &Value) -> Value {
        let Some(map) = incoming.as_object() else {
            return json!({"success": false, "reason": "invalid_settings", "message": "Settings must be an object"});
        };
        for key in SETTING_KEYS {
            if let Some(value) = map.get(key).and_then(|v| sanitize_setting(key, v)) {
                self.c.settings.set(key, value);
            }
        }
        match self.c.persister.save_settings().await {
            Ok(()) => json!({"success": true}),
            Err(e) => json!({"success": false, "reason": "unknown", "message": e.to_string()}),
        }
    }
}

// ── Slots ──────────────────────────────────────────────────────────────────

fn save_in_slot(save: &Value, slot: &str) -> bool {
    let s = save["slot"].as_str().unwrap_or("");
    s == slot
}

impl SaveService {
    pub async fn is_save_tracking_configured(&self, rom_id: i64) -> Value {
        let state = self.c.state.get(rom_id).await.ok().flatten();
        let configured = state.as_ref().is_some_and(|s| s.slot_confirmed);
        json!({"configured": configured, "active_slot": if configured { state.and_then(|s| s.active_slot) } else { None }})
    }

    /// The first-sync wizard's input: what is local, what the server holds per slot, and what to do.
    pub async fn get_save_setup_info(&self, rom_id: i64) -> Value {
        let subject = self.subject(rom_id).await;
        let local = match &subject {
            Ok(s) => self.read_local(s).await.ok().flatten(),
            Err(_) => None,
        };
        let local_files: Vec<Value> = local.iter().map(|l| json!({"filename": subject.as_ref().map(|s| s.file_name.clone()).unwrap_or_default(), "size": l.size})).collect();
        let state = self.c.state.get(rom_id).await.ok().flatten();
        let confirmed = state.as_ref().is_some_and(|s| s.slot_confirmed);
        let active = if confirmed { state.and_then(|s| s.active_slot) } else { None };
        let default = default_slot(&self.c.settings);
        let device_id = self.device_id().await;
        let all = match self.c.api.list_saves(rom_id, device_id.as_deref(), None).await {
            Ok(v) => v,
            Err(_) => {
                return json!({
                    "has_local_saves": local.is_some(), "local_files": local_files, "server_slots": [], "default_slot": default,
                    "slot_confirmed": confirmed, "active_slot": active, "recommended_action": "server_unreachable", "server_query_failed": true,
                });
            }
        };
        let mut slots: BTreeMap<Option<String>, Vec<&Value>> = BTreeMap::new();
        for save in &all {
            slots.entry(save["slot"].as_str().filter(|x| !x.is_empty()).map(str::to_string)).or_default().push(save);
        }
        let server_slots: Vec<Value> = slots
            .iter()
            .map(|(slot, saves)| {
                let latest = saves.iter().filter_map(|s| s["updated_at"].as_str()).max_by(|a, b| {
                    let e = |x: &&str| iso_time::parse(x).map_or(0.0, |t| t.seconds);
                    e(a).partial_cmp(&e(b)).unwrap_or(std::cmp::Ordering::Equal)
                });
                json!({
                    "slot": slot,
                    "saves": saves.iter().map(|s| json!({"id": s["id"], "file_name": s["file_name"].as_str().unwrap_or(""), "emulator": s["emulator"].as_str().unwrap_or(""), "updated_at": s["updated_at"].as_str().unwrap_or(""), "file_size_bytes": s["file_size_bytes"].as_i64().unwrap_or(0)})).collect::<Vec<_>>(),
                    "count": saves.len(),
                    "latest_updated_at": latest,
                })
            })
            .collect();
        let recommended = if local.is_some() && server_slots.is_empty() { "auto_confirm_default" } else { "show_wizard" };
        json!({
            "has_local_saves": local.is_some(), "local_files": local_files, "server_slots": server_slots, "default_slot": default,
            "slot_confirmed": confirmed, "active_slot": active, "recommended_action": recommended, "server_query_failed": false,
        })
    }

    /// Confirm the slot a game syncs into. A migration (carrying the local save into the chosen slot, then retiring the
    /// old slot's copy of this game's file) runs only when asked, and never deletes a server save it could not carry.
    pub async fn confirm_slot_choice(&self, rom_id: i64, chosen_slot: Option<&str>, migrate: bool, migrate_from: Option<&str>) -> Value {
        let slot = chosen_slot.map(str::trim).unwrap_or_default();
        if slot.is_empty() {
            return json!({"success": false, "reason": "invalid_slot_name", "needs_conflict_resolution": false, "message": "Slot name cannot be empty"});
        }
        let lock_ = self.rom_lock(rom_id);
        let _guard = lock_.lock().await;
        let mut state = self.state_of(rom_id).await;
        if self.c.library.rom(rom_id).await.ok().flatten().is_none() {
            return json!({"success": false, "reason": "not_installed", "needs_conflict_resolution": false, "message": "ROM is not known"});
        }
        let previous = state.active_slot.clone();
        let _ = state.confirm_slot(slot);
        // A baseline of the old slot means nothing in the new one.
        if previous.as_deref() != Some(slot) {
            state.clear_baselines();
        }
        let mut message = "Slot confirmed".to_string();
        if migrate {
            if let Err(e) = self.migrate_slot(rom_id, slot, migrate_from).await {
                message = format!("Slot confirmed but migration failed: {e}");
            }
        }
        if self.c.state.save(rom_id, &state).await.is_err() {
            return json!({"success": false, "reason": "unknown", "needs_conflict_resolution": false, "message": "Could not store the slot"});
        }
        json!({"success": true, "needs_conflict_resolution": false, "message": message})
    }

    async fn migrate_slot(&self, rom_id: i64, to: &str, from: Option<&str>) -> Result<(), String> {
        let subject = self.subject(rom_id).await.map_err(|u| u.message())?;
        let device_id = self.device_id().await;
        let old: Vec<ServerSave> = self.server_saves(&subject, device_id.as_deref(), from.unwrap_or("")).await.map_err(|e| e.classify().1)?;
        if old.is_empty() {
            return Ok(());
        }
        let Some(local) = self.read_local(&subject).await.map_err(|_| "the local save could not be read".to_string())? else {
            return Err("there is no local save to carry over; the old saves were left in place".into());
        };
        let mut scratch = RomSaveState::default();
        self.upload(&subject, &local, &mut scratch, device_id.as_deref().unwrap_or(""), to, false).await.map_err(|e| e.classify().1)?;
        // Only after the new copy is safely on the server is the old one retired.
        let ids: Vec<i64> = old.iter().filter_map(|s| s.id).collect();
        if !ids.is_empty() {
            self.c.api.delete_saves(&ids).await.map_err(|e| e.classify().1)?;
        }
        Ok(())
    }

    pub async fn get_save_slots(&self, rom_id: i64) -> Value {
        let state = self.c.state.get(rom_id).await.ok().flatten();
        let default = default_slot(&self.c.settings);
        let active = state.as_ref().and_then(|s| s.active_slot.clone()).unwrap_or_else(|| default.clone());
        if !self.enabled() {
            return json!({"success": false, "reason": "sync_disabled", "message": SYNC_DISABLED, "slots": [], "active_slot": "default"});
        }
        let device_id = self.device_id().await;
        let summary = match self.c.api.save_summary(rom_id, device_id.as_deref()).await {
            Ok(v) => v,
            Err(e) => return json!({"success": false, "reason": "server_unreachable", "message": e.to_string(), "slots": [], "active_slot": active}),
        };
        let mut merged: BTreeMap<String, Value> = BTreeMap::new();
        for s in summary["slots"].as_array().cloned().unwrap_or_default() {
            let name = s["slot"].as_str().or_else(|| s["slot_name"].as_str()).unwrap_or("").to_string();
            merged.insert(name, json!({"source": "server", "count": s["count"].as_i64().unwrap_or(0), "latest_updated_at": s["latest"]["updated_at"]}));
        }
        if let Some(st) = &state {
            for (name, info) in &st.slots {
                if merged.contains_key(name) {
                    continue;
                }
                match info["source"].as_str() {
                    Some("local") => {
                        merged.insert(name.clone(), json!({"source": "local", "count": 0, "latest_updated_at": null}));
                    }
                    Some("server") if *name == active => {
                        merged.insert(name.clone(), json!({"source": "server", "count": 0, "latest_updated_at": null}));
                    }
                    _ => {}
                }
            }
        }
        let mut next = state.unwrap_or_default();
        if next.active_slot.is_none() {
            next.switch_active_slot(Some(&active));
        }
        next.refresh_slot_listing(merged.clone());
        let _ = self.c.state.save(rom_id, &next).await;
        let slots: Vec<Value> = merged.iter().map(|(name, i)| json!({"slot": name, "source": i["source"], "count": i["count"], "latest_updated_at": i["latest_updated_at"]})).collect();
        json!({"success": true, "slots": slots, "active_slot": active})
    }

    pub async fn get_slot_saves(&self, rom_id: i64, slot: &str) -> Value {
        let slot = slot.trim();
        if !self.enabled() {
            return json!({"success": false, "reason": "sync_disabled", "message": SYNC_DISABLED, "slot": slot, "saves": []});
        }
        let device_id = self.device_id().await;
        match self.c.api.list_saves(rom_id, device_id.as_deref(), Some(slot)).await {
            Ok(saves) => {
                let rows: Vec<Value> = saves
                    .iter()
                    .filter(|s| save_in_slot(s, slot))
                    .map(|s| json!({"filename": s["file_name"], "id": s["id"], "size": s["file_size_bytes"], "updated_at": s["updated_at"].as_str().unwrap_or(""), "emulator": s["emulator"].as_str().unwrap_or("")}))
                    .collect();
                json!({"success": true, "slot": slot, "saves": rows})
            }
            Err(e) => json!({"success": false, "reason": "server_unreachable", "message": e.to_string(), "slot": slot, "saves": []}),
        }
    }

    /// Switch the active slot. Refused while the local save has changes that were never uploaded (they would be replaced).
    pub async fn switch_slot(&self, rom_id: i64, new_slot: &str) -> Value {
        if !self.enabled() {
            return json!({"success": false, "reason": "sync_disabled", "message": SYNC_DISABLED});
        }
        let slot = new_slot.trim();
        if slot.is_empty() {
            return json!({"success": false, "reason": "invalid_slot_name", "message": "Slot name cannot be empty"});
        }
        let subject = match self.subject(rom_id).await {
            Ok(s) => s,
            Err(Unsupported::NotInstalled) => return json!({"success": false, "reason": "not_installed", "message": "ROM is not installed"}),
            Err(u) => return json!({"success": false, "reason": u.reason(), "message": u.message()}),
        };
        let lock_ = self.rom_lock(rom_id);
        let guard = lock_.lock().await;
        let mut state = self.state_of(rom_id).await;
        let local = self.read_local(&subject).await.ok().flatten();
        if let (Some(l), Some(base)) = (&local, state.files.get(&subject.file_name).and_then(|f| f.last_sync_hash.clone())) {
            if l.hash != base {
                return json!({"success": false, "reason": "pending_uploads", "message": "Pending local changes: upload or discard first", "files": [subject.file_name]});
            }
        }
        let device_id = self.device_id().await;
        let servers = match self.server_saves(&subject, device_id.as_deref(), slot).await {
            Ok(s) => s,
            Err(e) => return json!({"success": false, "reason": "server_unreachable", "message": e.to_string()}),
        };
        state.switch_active_slot(Some(slot));
        state.clear_baselines();
        let head = servers.iter().max_by(|a, b| {
            let e = |x: &ServerSave| x.updated_at.as_deref().and_then(iso_time::parse).map_or(0.0, |t| t.seconds);
            e(a).partial_cmp(&e(b)).unwrap_or(std::cmp::Ordering::Equal)
        });
        if let Some(head) = head {
            // The new slot's newest save replaces the local one (the old one is kept as a backup by the write).
            if let Err(e) = self.download(&subject, head, &mut state, device_id.as_deref().unwrap_or("")).await {
                let _ = self.c.state.save(rom_id, &state).await;
                return json!({"success": false, "reason": "switch_incomplete", "message": format!("Switched to the slot but the save failed to download: {e}")});
            }
        } else if local.is_some() {
            // An empty slot: keep the local save out of the way so it is not uploaded into the new slot unnoticed.
            let _ = self.c.local.quarantine(&subject.request()).await;
        }
        state.mark_sync_evaluated(&self.c.clock.now_iso());
        let _ = self.c.state.save(rom_id, &state).await;
        drop(guard);
        let status = self.get_save_status(rom_id).await;
        json!({"success": true, "save_status": status})
    }

    async fn validated_slot(&self, rom_id: i64, slot: &str) -> Result<(RomSaveState, Value), Value> {
        if !self.enabled() {
            return Err(json!({"success": false, "reason": "disabled", "message": SYNC_DISABLED}));
        }
        if self.c.library.install(rom_id).await.ok().flatten().is_none() {
            return Err(json!({"success": false, "reason": "not_installed", "message": "ROM is not installed"}));
        }
        let Some(state) = self.c.state.get(rom_id).await.ok().flatten() else {
            return Err(json!({"success": false, "reason": "not_found", "message": "No save state found"}));
        };
        let Some(info) = state.slots.get(slot).cloned() else {
            return Err(json!({"success": false, "reason": "not_found", "message": "Slot not found"}));
        };
        Ok((state, info))
    }

    pub async fn get_slot_delete_info(&self, rom_id: i64, slot: &str) -> Value {
        let slot = slot.trim();
        let (state, info) = match self.validated_slot(rom_id, slot).await {
            Ok(v) => v,
            Err(f) => return f,
        };
        let source = info["source"].as_str().unwrap_or("server").to_string();
        let mut ids: Vec<i64> = vec![];
        if source == "server" {
            let device_id = self.device_id().await;
            match self.c.api.list_saves(rom_id, device_id.as_deref(), Some(slot)).await {
                Ok(saves) => ids = saves.iter().filter(|s| save_in_slot(s, slot)).filter_map(|s| s["id"].as_i64()).collect(),
                Err(_) => return json!({"success": false, "reason": "server_unreachable", "message": "Cannot inspect slot: server unreachable"}),
            }
        }
        let local_filenames: Vec<&String> = state.files.iter().filter(|(_, f)| f.tracked_save_id.is_some_and(|t| ids.contains(&t))).map(|(n, _)| n).collect();
        json!({
            "success": true, "slot": slot, "source": source, "server_save_count": ids.len(), "server_save_ids": ids,
            "local_file_count": local_filenames.len(), "local_filenames": local_filenames,
            "is_active": slot == state.active_slot.clone().unwrap_or_default(),
        })
    }

    pub async fn delete_slot(&self, rom_id: i64, slot: &str) -> Value {
        let slot = slot.trim();
        let lock_ = self.rom_lock(rom_id);
        let _guard = lock_.lock().await;
        let (mut state, info) = match self.validated_slot(rom_id, slot).await {
            Ok(v) => v,
            Err(f) => return f,
        };
        if slot == state.active_slot.clone().unwrap_or_default() {
            return json!({"success": false, "reason": "active_slot", "message": "Cannot delete the active slot. Switch to a different slot first."});
        }
        let (mut deleted, mut cleaned) = (0usize, 0usize);
        if info["source"].as_str().unwrap_or("server") == "server" {
            let device_id = self.device_id().await;
            let ids: Vec<i64> = match self.c.api.list_saves(rom_id, device_id.as_deref(), Some(slot)).await {
                Ok(saves) => saves.iter().filter(|s| save_in_slot(s, slot)).filter_map(|s| s["id"].as_i64()).collect(),
                Err(e) => return json!({"success": false, "reason": "server_unreachable", "message": format!("Failed to delete server saves: {e}")}),
            };
            if !ids.is_empty() {
                if let Err(e) = self.c.api.delete_saves(&ids).await {
                    return json!({"success": false, "reason": "server_unreachable", "message": format!("Failed to delete server saves: {e}")});
                }
            }
            deleted = ids.len();
            let stale: Vec<String> = state.files.iter().filter(|(_, f)| f.tracked_save_id.is_some_and(|t| ids.contains(&t))).map(|(n, _)| n.clone()).collect();
            for name in stale {
                state.delete_file_tracking(&name);
                cleaned += 1;
            }
        }
        state.delete_slot_tracking(slot);
        let _ = self.c.state.save(rom_id, &state).await;
        json!({"success": true, "deleted_server_saves": deleted, "cleaned_files": cleaned})
    }
}

// ── Versions and rollback ──────────────────────────────────────────────────

impl SaveService {
    /// The other versions in a slot, newest first (the tracked one is the current save, not history).
    pub async fn list_file_versions(&self, rom_id: i64, slot: &str, filename: &str) -> Value {
        let state = self.state_of(rom_id).await;
        let device_id = self.device_id().await;
        let saves = match self.c.api.list_saves(rom_id, device_id.as_deref(), Some(slot)).await {
            Ok(s) => s,
            Err(e) => return json!({"status": "server_unreachable", "message": e.to_string()}),
        };
        let tracked = state.files.get(filename).and_then(|f| f.tracked_save_id);
        let mut versions: Vec<Value> = saves
            .iter()
            .filter(|s| s["id"].as_i64() != tracked && save_in_slot(s, slot))
            .map(|s| {
                let id = s["id"].as_i64().unwrap_or(0);
                json!({
                    "id": id, "file_name": s["file_name"].as_str().unwrap_or(""), "emulator": s["emulator"], "updated_at": s["updated_at"].as_str().unwrap_or(""),
                    "file_size_bytes": s["file_size_bytes"], "device_syncs": s["device_syncs"].as_array().cloned().unwrap_or_default(),
                    "uploaded_by_us": state.own_upload_ids.as_ref().map(|ids| ids.contains(&id)),
                })
            })
            .collect();
        let epoch = |v: &Value| v["updated_at"].as_str().and_then(iso_time::parse).map_or(0.0, |t| t.seconds);
        versions.sort_by(|a, b| epoch(b).partial_cmp(&epoch(a)).unwrap_or(std::cmp::Ordering::Equal));
        json!({"status": "ok", "versions": versions})
    }

    /// Make an older version the current save: sync first (a conflict or error blocks it), download it, then bump it on the
    /// server so it becomes the newest and every other device follows.
    pub async fn rollback_to_version(&self, rom_id: i64, slot: &str, save_id: i64) -> Value {
        let lock_ = self.rom_lock(rom_id);
        let _guard = lock_.lock().await;
        let subject = match self.subject(rom_id).await {
            Ok(s) => s,
            Err(Unsupported::NotInstalled) => return json!({"status": "rom_not_installed"}),
            Err(u) => return json!({"status": "unsupported", "reason": u.reason()}),
        };
        let mut state = self.state_of(rom_id).await;
        let device_id = self.device_id().await.unwrap_or_default();
        // Pre-flight: the current save must be in sync, or the switch could lose local progress.
        match self.run_rom(&subject, &mut state, &device_id).await {
            Ok(run) if !run.conflicts.is_empty() => {
                let _ = self.c.state.save(rom_id, &state).await;
                return json!({"status": "conflict_blocked", "conflicts": run.conflicts});
            }
            Ok(run) if !run.errors.is_empty() => {
                let _ = self.c.state.save(rom_id, &state).await;
                return json!({"status": "preflight_failed", "errors": run.errors});
            }
            Ok(_) => {}
            Err(SaveErr::Server(e)) => return json!({"status": "server_unreachable", "message": e.to_string()}),
            Err(SaveErr::Unsupported(u)) => return json!({"status": "unsupported", "reason": u.reason()}),
            Err(SaveErr::Local(m)) => return json!({"status": "preflight_failed", "errors": [m]}),
        }
        let servers = match self.server_saves(&subject, Some(&device_id), slot).await {
            Ok(s) => s,
            Err(e) => {
                let _ = self.c.state.save(rom_id, &state).await;
                return json!({"status": "server_unreachable", "message": e.to_string()});
            }
        };
        let Some(target) = servers.iter().find(|s| s.id == Some(save_id)) else {
            let _ = self.c.state.save(rom_id, &state).await;
            return json!({"status": "version_deleted"});
        };
        let result = match self.download(&subject, target, &mut state, &device_id).await {
            Err(e) => json!({"status": "preflight_failed", "errors": [e]}),
            Ok(_) => {
                // PUT the same bytes so this version's `updated_at` becomes now and it is the newest in the slot.
                let bytes = self.read_local(&subject).await.ok().flatten().and_then(|l| save_archive::build(&l.entries).ok());
                match bytes {
                    None => json!({"status": "put_failed", "message": "The restored save could not be read back"}),
                    Some(data) => match self
                        .c
                        .api
                        .upload_save(SaveUpload { rom_id, file_name: subject.file_name.clone(), data, emulator: subject.emulator.clone(), device_id: Some(device_id.clone()), slot: Some(slot.to_string()), overwrite: false, autocleanup_limit: None, save_id: Some(save_id) })
                        .await
                    {
                        Ok(_) => {
                            let _ = self.c.api.confirm_download(save_id, &device_id).await;
                            json!({"status": "ok"})
                        }
                        Err(e) => json!({"status": "put_failed", "message": e.to_string()}),
                    },
                }
            }
        };
        let _ = self.c.state.save(rom_id, &state).await;
        result
    }
}

// ── Deleting local saves ───────────────────────────────────────────────────

impl SaveService {
    /// Put one ROM's local save aside (`.romm-backup`, never deleted) and forget its baselines.
    pub async fn delete_local_saves(&self, rom_id: i64) -> Value {
        let lock_ = self.rom_lock(rom_id);
        let _guard = lock_.lock().await;
        match self.quarantine_one(rom_id).await {
            Ok(count) => json!({"success": true, "deleted_count": count, "message": if count == 0 { "No local saves to delete".to_string() } else { format!("Moved {count} save folder(s) aside as .romm-backup") }}),
            Err(m) => json!({"success": false, "deleted_count": 0, "message": m}),
        }
    }

    async fn quarantine_one(&self, rom_id: i64) -> Result<usize, String> {
        let subject = match self.subject(rom_id).await {
            Ok(s) => s,
            Err(Unsupported::NotInstalled) => return Ok(0),
            Err(u) => return Err(u.message()),
        };
        let backups = self.c.local.quarantine(&subject.request()).await.map_err(|e| e.to_string())?;
        let mut state = self.state_of(rom_id).await;
        state.clear_baselines();
        let _ = self.c.state.save(rom_id, &state).await;
        Ok(backups.len())
    }

    pub async fn delete_platform_saves(&self, platform_slug: &str) -> Value {
        let roms = self.c.library.roms_on_platform(platform_slug).await.unwrap_or_default();
        let mut count = 0usize;
        for rom in roms {
            let lock_ = self.rom_lock(rom.rom_id);
            let _guard = lock_.lock().await;
            if let Ok(n) = self.quarantine_one(rom.rom_id).await {
                count += n;
            }
        }
        json!({"success": true, "deleted_count": count, "message": format!("Moved {count} save folder(s) aside as .romm-backup")})
    }
}

// ── The launch gate's view ─────────────────────────────────────────────────

#[async_trait]
impl SaveSafety for SaveService {
    async fn status(&self, rom_id: i64) -> anyhow::Result<Value> {
        if !self.enabled() {
            anyhow::bail!("Save sync is disabled");
        }
        Ok(self.get_save_status(rom_id).await)
    }

    async fn tracked(&self, rom_id: i64) -> anyhow::Result<bool> {
        self.c.state.tracked(rom_id).await
    }

    /// Whether the local save differs from the last synced baseline (an offline edit), without any network.
    async fn drift(&self, rom_id: i64) -> anyhow::Result<bool> {
        let Ok(subject) = self.subject(rom_id).await else { return Ok(false) };
        let state = self.state_of(rom_id).await;
        let Some(baseline) = state.files.get(&subject.file_name).and_then(|f| f.last_sync_hash.clone()) else { return Ok(false) };
        Ok(self.read_local(&subject).await.ok().flatten().is_some_and(|l| l.hash != baseline))
    }

    /// Standalone emulators have no libretro core.
    async fn core_change(&self, _rom_id: i64) -> anyhow::Result<Value> {
        Ok(json!({"changed": false}))
    }

    async fn post_exit(&self, rom_id: i64) -> anyhow::Result<Value> {
        Ok(self.post_exit_sync(rom_id).await)
    }
}

#[cfg(test)]
#[path = "saves_tests.rs"]
mod tests;
