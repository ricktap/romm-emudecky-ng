//! The seams services depend on: the live settings document and the traits
//! adapters implement. Services never name a concrete adapter.

use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::domain::errors::RommError;
use crate::domain::settings::truthy;

/// Durable history used by the library sync orchestrator.
#[async_trait]
pub trait SyncRunRepo: Send + Sync {
    async fn get(&self, id: &str) -> anyhow::Result<Option<crate::domain::sync_run::SyncRun>>;
    async fn save(&self, run: &crate::domain::sync_run::SyncRun) -> anyhow::Result<()>;
    async fn latest_completed(&self) -> anyhow::Result<Option<crate::domain::sync_run::SyncRun>>;
    async fn running(&self) -> anyhow::Result<Option<crate::domain::sync_run::SyncRun>>;
}

/// The live, shared `settings.json` document.
///
/// Every service reads and mutates the same map (as the Python services shared
/// one dict); the persister writes snapshots of it. The lock is a plain mutex
/// and is never held across an `.await`.
#[derive(Clone, Default)]
pub struct SettingsStore(Arc<Mutex<Map<String, Value>>>);

impl SettingsStore {
    pub fn new(map: Map<String, Value>) -> Self {
        Self(Arc::new(Mutex::new(map)))
    }

    fn lock(&self) -> MutexGuard<'_, Map<String, Value>> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The value at `key` (`None` when absent).
    pub fn get(&self, key: &str) -> Option<Value> {
        self.lock().get(key).cloned()
    }

    /// The value at `key` when it is a string.
    pub fn get_str(&self, key: &str) -> Option<String> {
        self.lock().get(key).and_then(Value::as_str).map(str::to_string)
    }

    /// Python-style truthiness of `key` (`false` when absent).
    pub fn truthy(&self, key: &str) -> bool {
        self.lock().get(key).is_some_and(truthy)
    }

    pub fn set(&self, key: &str, value: Value) {
        self.lock().insert(key.to_string(), value);
    }

    pub fn remove(&self, key: &str) -> Option<Value> {
        self.lock().shift_remove(key)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.lock().contains_key(key)
    }

    /// Run `f` with exclusive access to the whole document.
    pub fn with<R>(&self, f: impl FnOnce(&mut Map<String, Value>) -> R) -> R {
        f(&mut self.lock())
    }

    /// A copy of the whole document.
    pub fn snapshot(&self) -> Map<String, Value> {
        self.lock().clone()
    }
}

/// Persists the live settings document to disk.
#[async_trait]
pub trait SettingsPersister: Send + Sync {
    async fn save_settings(&self) -> anyhow::Result<()>;
}

/// The RomM endpoints the connection flows use.
#[async_trait]
pub trait RommConnectionApi: Send + Sync {
    /// Cache the detected server version (`None` clears it).
    fn set_version(&self, version: Option<String>);
    /// `GET /api/heartbeat`, retried on transient failures.
    async fn heartbeat(&self) -> Result<Value, RommError>;
    /// `GET /api/heartbeat` once, with a short timeout.
    async fn heartbeat_once(&self) -> Result<Value, RommError>;
    async fn list_platforms(&self) -> Result<Value, RommError>;
    /// `GET /api/users/me` — validates the bearer token.
    async fn get_current_user(&self) -> Result<Value, RommError>;
    /// Mint a scoped Client API Token via one-off Basic auth.
    async fn mint_client_token(&self, username: &str, password: &str, token_name: &str) -> Result<Value, RommError>;
    /// Delete a minted token via Basic auth (a 404 counts as success).
    async fn delete_client_token(&self, username: &str, password: &str, token_id: &str) -> Result<(), RommError>;
    /// Exchange a one-time pairing code for a token (public endpoint, never retried).
    async fn exchange_pairing_code(&self, code: &str) -> Result<Value, RommError>;
}

/// The `kv_config` singleton-scalar store.
#[async_trait]
pub trait KvConfigRepo: Send + Sync {
    async fn get(&self, key: &str) -> anyhow::Result<Option<String>>;
    async fn set(&self, key: &str, value: &str) -> anyhow::Result<()>;
    async fn delete(&self, key: &str) -> anyhow::Result<()>;
}

/// The `regions` list of every ROM row in the local library.
#[async_trait]
pub trait RomRegionsReader: Send + Sync {
    async fn all_rom_regions(&self) -> anyhow::Result<Vec<Vec<String>>>;
}

// ── Downloads, installs, launch ────────────────────────────────────────────

/// The RomM endpoints library sync uses for platform / collection metadata.
#[async_trait]
pub trait RommLibraryApi: Send + Sync {
    async fn list_platforms(&self) -> Result<Value, RommError>;
    async fn list_collections(&self) -> Result<Value, RommError>;
    async fn list_smart_collections(&self) -> Result<Value, RommError>;
    async fn list_virtual_collections(&self, collection_type: &str) -> Result<Value, RommError>;
}

/// Pushes a backend event (`{event, args: [payload]}`) to the frontend.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: &str, payload: Value);
}

/// Wall-clock timestamps for persisted rows and a monotonic clock for throttling.
pub trait Clock: Send + Sync {
    /// Now as ISO-8601 UTC.
    fn now_iso(&self) -> String;
    fn epoch_seconds(&self) -> f64;
    /// Seconds on a monotonic clock.
    fn monotonic(&self) -> f64;
}

/// Callbacks and the stop signal of one ROM transfer.
pub struct TransferHooks {
    /// `(bytes on disk, full size)` as the body streams (only when the size is known).
    pub progress: Box<dyn FnMut(u64, u64) + Send>,
    /// Called once, when the response headers arrive, with whether the
    /// transfer can be resumed later.
    pub on_meta: Option<Box<dyn FnOnce(bool) + Send>>,
    /// Cancelled by a user cancel / pause or plugin shutdown; the transfer
    /// then fails with `TransferCancelled` and keeps whatever it wrote.
    pub cancel: tokio_util::sync::CancellationToken,
}

/// The RomM ROM endpoints the download and adoption flows use.
#[async_trait]
pub trait RommRomApi: Send + Sync {
    /// `GET /api/roms/{id}` — the ROM detail (names, sizes, files).
    async fn get_rom(&self, rom_id: i64) -> Result<Value, RommError>;
    /// Stream the ROM content into `dest`. With `resume`, an existing partial
    /// is continued through an HTTP Range request when the server honours it.
    async fn download_rom_content(
        &self,
        rom_id: i64,
        file_name: &str,
        dest: &str,
        resume: bool,
        hooks: TransferHooks,
    ) -> anyhow::Result<()>;
}

/// `(extracted bytes, total bytes)` while a ZIP extracts.
pub type ExtractProgress = Box<dyn FnMut(u64, u64) + Send>;

/// Filesystem operations on ROM files. Missing paths are not errors where noted.
#[async_trait]
pub trait DownloadFiles: Send + Sync {
    async fn exists(&self, path: &str) -> bool;
    /// Delete a file; a missing file is fine.
    async fn remove_file(&self, path: &str) -> std::io::Result<()>;
    /// Delete a directory tree; a missing directory is fine.
    async fn remove_tree(&self, path: &str) -> std::io::Result<()>;
    async fn make_dirs(&self, path: &str) -> std::io::Result<()>;
    /// Atomically replace `dst` with `src`.
    async fn rename(&self, src: &str, dst: &str) -> std::io::Result<()>;
    /// Free bytes on the filesystem holding `path`.
    async fn disk_free(&self, path: &str) -> std::io::Result<u64>;
    /// Size of the file at `path`, 0 when missing.
    async fn file_size(&self, path: &str) -> u64;
    /// Size of the regular file at `path`, `None` when missing or not a regular file.
    async fn regular_file_size(&self, path: &str) -> Option<u64>;
    async fn is_dir(&self, path: &str) -> bool;
    /// `(name, is_dir, size)` of the top-level files and directories (symlinks
    /// followed); empty when unreadable.
    async fn list_entries(&self, dir: &str) -> Vec<(String, bool, u64)>;
    /// Every file under `base` whose name ends with one of `suffixes`.
    async fn walk_files_matching_suffixes(&self, base: &str, suffixes: &[&str]) -> Vec<String>;
    /// Extract `archive` into `dest`, refusing any member (or `dest` itself)
    /// that would land outside `safe_root` / `dest`.
    async fn extract_zip(
        &self,
        archive: &str,
        dest: &str,
        safe_root: &str,
        progress: ExtractProgress,
    ) -> anyhow::Result<()>;
    /// Rename URL-encoded names under `dir` to their decoded form, refusing
    /// any decoded name that is not a single safe component.
    async fn decode_url_encoded_names(&self, dir: &str) -> anyhow::Result<()>;
    /// `(path, size)` of every file under `dir`, recursively.
    async fn scan_files_with_sizes(&self, dir: &str) -> Vec<(String, u64)>;
}

/// EmuDeck's storage roots (`<storagePath>/Emulation/...`).
pub trait EmuDeckPaths: Send + Sync {
    fn roms_path(&self) -> String;
    fn bios_path(&self) -> String;
}

/// RomM platform slug → ROM folder / system name.
pub trait SystemResolver: Send + Sync {
    fn resolve_system(&self, platform_slug: &str, platform_fs_slug: Option<&str>) -> String;
}

/// Finds the emulator-table emulators on this machine.
pub trait EmulatorLocator: Send + Sync {
    /// Installed emulator id → executable path for `system`.
    fn installed(&self, system: &str) -> std::collections::HashMap<String, String>;
    /// Whether `path` is an executable regular file.
    fn is_executable(&self, path: &str) -> bool;
}

/// The persisted download queue (`download_jobs`).
#[async_trait]
pub trait DownloadJobRepo: Send + Sync {
    async fn get(&self, rom_id: i64) -> anyhow::Result<Option<crate::domain::download_job::DownloadJob>>;
    async fn save(&self, job: &crate::domain::download_job::DownloadJob) -> anyhow::Result<()>;
    async fn delete(&self, rom_id: i64) -> anyhow::Result<()>;
    /// Every job, in queue order.
    async fn all(&self) -> anyhow::Result<Vec<crate::domain::download_job::DownloadJob>>;
    /// `max(position) + 1`, or 1 for an empty queue.
    async fn next_position(&self) -> anyhow::Result<i64>;
    async fn min_position(&self) -> anyhow::Result<Option<i64>>;
}

/// The fields of a `roms` row the download, adoption and launch flows read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RomRow {
    pub rom_id: i64,
    pub platform_slug: String,
    pub name: String,
    pub fs_name: String,
    pub shortcut_app_id: Option<i64>,
    pub emulator_override: Option<String>,
    pub selected_disc: Option<String>,
    pub sibling_group_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryPlatformRow {
    pub platform_slug: String,
    pub count: i64,
}

/// A consistent ROM/install pair, read before filesystem or emulator probes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSnapshot {
    pub rom: RomRow,
    pub install: Option<crate::domain::rom_install::RomInstall>,
}

/// Launch preferences use narrow writes: sync metadata and history survive.
/// Conditional writes refuse a stale snapshot (e.g. uninstall during a scan).
#[async_trait]
pub trait LaunchPreferencesRepo: Send + Sync {
    async fn launch_snapshot(&self, rom_id: i64) -> anyhow::Result<Option<LaunchSnapshot>>;
    async fn installed_launch_snapshots(&self) -> anyhow::Result<Vec<LaunchSnapshot>>;
    async fn change_emulator_override(&self, expected: &LaunchSnapshot, label: Option<&str>) -> anyhow::Result<bool>;
    async fn change_selected_disc(&self, expected: &LaunchSnapshot, filename: Option<&str>) -> anyhow::Result<bool>;
}

/// The synced library (`roms`) and its install records (`rom_installs`).
#[async_trait]
pub trait LibraryRepo: Send + Sync {
    async fn rom(&self, rom_id: i64) -> anyhow::Result<Option<RomRow>>;
    async fn roms_in_group(&self, group_key: &str) -> anyhow::Result<Vec<RomRow>>;
    async fn roms_on_platform(&self, platform_slug: &str) -> anyhow::Result<Vec<RomRow>>;
    async fn install(&self, rom_id: i64) -> anyhow::Result<Option<crate::domain::rom_install::RomInstall>>;
    async fn installs(&self) -> anyhow::Result<Vec<crate::domain::rom_install::RomInstall>>;
    async fn save_install(&self, install: &crate::domain::rom_install::RomInstall) -> anyhow::Result<()>;
    async fn delete_install(&self, rom_id: i64) -> anyhow::Result<()>;
    /// Write an adopted install in one transaction, only when the ROM row
    /// exists and it has no install, no download job, and no installed member
    /// of `group_key`. Returns whether it was written.
    async fn adopt_install(
        &self,
        install: &crate::domain::rom_install::RomInstall,
        group_key: Option<&str>,
    ) -> anyhow::Result<bool>;
    async fn bound_rom_count(&self) -> anyhow::Result<i64>;
    async fn registry_platforms(&self) -> anyhow::Result<Vec<RegistryPlatformRow>>;
    async fn app_id_rom_id_map(&self) -> anyhow::Result<Vec<(i64, i64)>>;
    async fn latest_completed_sync_finished_at(&self) -> anyhow::Result<Option<String>>;
    async fn delete_completed_sync_runs(&self) -> anyhow::Result<()>;
    /// Startup only: terminalize runs left in flight by the previous process.
    async fn interrupt_running_syncs(&self, finished_at: &str) -> anyhow::Result<usize>;
}

/// Read-only paginated ROM endpoint used by library sync.
#[async_trait]
pub trait RommLibraryPages: Send + Sync {
    async fn page(&self, unit: &crate::domain::work_unit::WorkUnit, offset: usize) -> Result<Value, RommError>;
}

/// Consistent read of the completed-run watermark and all persisted siblings.
#[derive(Debug, Clone, Default)]
pub struct IncrementalBaseline {
    pub last_sync: Option<String>,
    pub bound_roms: Vec<Value>,
    pub persisted_count: usize,
    pub needs_backfill: bool,
    pub missing_shortcuts: bool,
}

#[async_trait]
pub trait LibraryBaselineRepo: Send + Sync {
    async fn baseline(&self, platform_slug: &str) -> anyhow::Result<IncrementalBaseline>;
}

#[async_trait]
pub trait RommLibraryDelta: Send + Sync {
    async fn updated_after(&self, platform_id: i64, timestamp: &str) -> Result<Value, RommError>;
}

/// Atomic unit persistence. Every sibling is saved; only acknowledged targets bind.
#[derive(Debug, Clone)]
pub struct SyncUnitCommit {
    pub built: Vec<Value>,
    pub raw: Vec<Value>,
    pub bindings: std::collections::BTreeMap<i64,i64>,
    pub covers: std::collections::BTreeMap<i64,String>,
    pub synced_at: String,
    pub cached_at: f64,
}
#[async_trait]
pub trait SyncLibraryRepo: Send + Sync {
    async fn registry(&self) -> anyhow::Result<std::collections::BTreeMap<i64,Value>>;
    async fn commit_unit(&self, commit: SyncUnitCommit) -> anyhow::Result<()>;
}

pub trait SyncIds: Send + Sync { fn next_id(&self) -> String; }
#[async_trait]
pub trait SyncSleeper: Send + Sync { async fn sleep(&self, seconds:f64); }
#[derive(Clone,Default)]
pub struct SyncFinalization {
    pub stale_ids:Vec<i64>,
    pub platform_names:std::collections::BTreeMap<String,String>,
    pub platform_rom_ids:std::collections::BTreeSet<i64>,
    pub memberships:std::collections::BTreeMap<String,Vec<i64>>,
    pub create_platform_groups:bool,
}
#[async_trait]
pub trait SyncFinalizeRepo: Send + Sync {
    async fn finalize(&self, request:SyncFinalization)->anyhow::Result<Value>;
}

#[async_trait]
pub trait SteamAssets: Send + Sync {
    async fn grid_dir(&self)->anyhow::Result<Option<String>>;
    fn cache_path(&self,rom_id:i64)->String;
    fn cache_dir(&self)->String;
    async fn files_in(&self,dir:&str)->anyhow::Result<Vec<String>>;
    async fn exists(&self,path:&str)->bool;
    async fn read(&self,path:&str)->anyhow::Result<Vec<u8>>;
    async fn copy_atomic(&self,src:&str,dest:&str)->anyhow::Result<()>;
    async fn rename(&self,src:&str,dest:&str)->anyhow::Result<()>;
    async fn remove(&self,path:&str)->anyhow::Result<()>;
    async fn write_icon(&self,app_id:i64,bytes:&[u8])->anyhow::Result<Option<String>>;
    async fn steam_input(&self,app_ids:&[i64],mode:&str)->anyhow::Result<()>;
}
#[async_trait]
pub trait RommCoverApi: Send + Sync {
    async fn download_cover(&self,url:&str,dest:&str)->anyhow::Result<()>;
}
#[async_trait]
pub trait ShortcutBindings: Send + Sync {
    async fn unbind(&self,ids:Vec<i64>)->anyhow::Result<()>;
    async fn reconcile(&self,live:std::collections::BTreeSet<i64>)->anyhow::Result<usize>;
}

/// A game page is read in one transaction, without HTTP or filesystem probes.
pub struct GameDetailSnapshot {
    pub rom: Value,
    pub install_file: Option<String>,
    pub metadata: Option<Value>,
    pub save_status: Option<Value>,
    pub platform_names: Value,
}

#[async_trait]
pub trait GameReadRepo: Send + Sync {
    async fn game_detail(&self, app_id: i64) -> anyhow::Result<Option<GameDetailSnapshot>>;
    async fn metadata(&self, rom_id: i64) -> anyhow::Result<Option<Value>>;
    async fn all_metadata(&self) -> anyhow::Result<Value>;
    async fn remember_sgdb_id(&self, rom_id: i64, sgdb_id: i64) -> anyhow::Result<()>;
    /// Only persist if this ROM still owns the shortcut captured before HTTP.
    async fn remember_cover(&self, rom_id: i64, app_id: i64, path: &str) -> anyhow::Result<bool>;
}

#[async_trait]
pub trait PendingRomReader: Send + Sync {
    async fn pending_rom(&self, rom_id: i64) -> Option<Value>;
}

#[derive(Debug, thiserror::Error)]
pub enum SgdbError {
    #[error("HTTP {0}")]
    Http(u16),
    #[error("SteamGridDB connection failed")]
    Connection,
    #[error("SteamGridDB returned an unreadable response")]
    BadResponse,
}

#[async_trait]
pub trait SteamGridApi: Send + Sync {
    async fn request(&self, path: &str, key: &str) -> Result<Value, SgdbError>;
    /// Image requests are unauthenticated, even on a different CDN origin.
    async fn download_image(&self, url: &str, dest: &str) -> anyhow::Result<()>;
}

/// The version picker snapshots all local siblings before any HTTP request.
#[derive(Clone, Debug, PartialEq)]
pub struct VersionGroup {
    pub bound: Value,
    pub members: Vec<Value>,
}
#[async_trait]
pub trait VersionRepo: Send + Sync {
    async fn group(&self, app_id: i64) -> anyhow::Result<Option<VersionGroup>>;
    async fn member(&self, rom_id: i64) -> anyhow::Result<Option<Value>>;
    /// Conditional, narrow binding move; a server-only row is inserted, never replaced.
    async fn bind_version(&self, expected: &VersionGroup, target: i64, server: Option<Value>, now: &str) -> anyhow::Result<Value>;
}

/// Step 5 supplies standalone save providers here. No guessed save paths.
#[async_trait]
pub trait SaveSafety: Send + Sync {
    async fn status(&self, rom_id: i64) -> anyhow::Result<Value>;
    async fn tracked(&self, rom_id: i64) -> anyhow::Result<bool>;
    async fn drift(&self, rom_id: i64) -> anyhow::Result<bool>;
    async fn core_change(&self, rom_id: i64) -> anyhow::Result<Value>;
    async fn post_exit(&self, rom_id: i64) -> anyhow::Result<Value>;
}

#[derive(Debug, thiserror::Error)]
pub enum ReleaseError {
    #[error("GitHub rate limit reached")] RateLimited,
    #[error("GitHub returned an unreadable response")] BadResponse,
    #[error("Could not reach GitHub")] Unreachable,
}
#[async_trait]
pub trait ReleaseFeed: Send + Sync {
    async fn latest(&self) -> Result<Value, ReleaseError>;
}
#[async_trait]
pub trait PluginMetadata: Send + Sync {
    async fn metadata(&self) -> anyhow::Result<(String,String)>;
}

pub use crate::domain::playtime::{Playtime, PendingSession};
#[derive(Debug, thiserror::Error)]
pub enum PlaytimeApiError {
    #[error("RomM play-session request failed")] Romm(#[from] RommError),
    #[error("RomM rejected the play-session batch")] Rejected(Value),
}
#[async_trait]
pub trait RommPlaytimeApi: Send + Sync {
    async fn ingest(&self, device: &str, sessions: Vec<Value>) -> Result<Value, PlaytimeApiError>;
    async fn sessions(&self, rom_id: i64) -> Result<Vec<Value>, RommError>;
}
#[async_trait]
pub trait PlaytimeRepo: Send + Sync {
    async fn begin(&self, rom_id: i64, now: &str, monotonic: f64) -> anyhow::Result<bool>;
    async fn finish(&self, rom_id: i64, now: &str, monotonic: f64) -> anyhow::Result<Value>;
    async fn get(&self, rom_id: i64) -> anyhow::Result<Option<Playtime>>;
    async fn all(&self) -> anyhow::Result<Value>;
    async fn pending(&self, limit: usize) -> anyhow::Result<Vec<PendingSession>>;
    /// Outcomes only touch byte-identical submitted rows; concurrent recording survives.
    async fn apply_outcomes(&self, outcomes: Vec<(PendingSession, bool)>) -> anyhow::Result<()>;
    async fn reconcile(&self, rom_id: i64, sessions: Vec<Value>) -> anyhow::Result<Playtime>;
}
