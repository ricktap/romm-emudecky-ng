//! `settings.json` on disk: crash-safe writes, corrupt-file quarantine, and the
//! legacy `save_sync_state.json` read that feeds the one-time settings fold.
//!
//! Writes follow write-tmp → fsync(tmp) → rename → fsync(dir) under an
//! exclusive `flock` on `settings.json.lock`, so a power loss can never leave a
//! truncated file. An unparseable file is never silently reset: it is renamed
//! to `settings.json.corrupt-<ts>` and the caller is told so it can raise the
//! user-facing notice.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tokio::sync::Mutex as AsyncMutex;

use crate::domain::settings::{SETTINGS_VERSION, apply_defaults, stored_version};
use crate::services::ports::{SettingsPersister, SettingsStore};

const SETTINGS_FILE: &str = "settings.json";
const LEGACY_SAVE_SYNC_FILE: &str = "save_sync_state.json";

/// Wall-clock seconds since the epoch (injected so the backup stamp is testable).
pub type EpochClock = Arc<dyn Fn() -> u64 + Send + Sync>;

pub fn system_clock() -> EpochClock {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    })
}

/// What [`SettingsFile::load`] found.
pub struct LoadedSettings {
    /// The document with every default filled in (`version: 0` when absent).
    pub settings: Map<String, Value>,
    /// The backup file name when an unparseable file was quarantined this load.
    pub corrupt_backup: Option<String>,
}

/// The settings directory (`DECKY_PLUGIN_SETTINGS_DIR`) and its files.
pub struct SettingsFile {
    settings_dir: PathBuf,
    runtime_dir: PathBuf,
    clock: EpochClock,
}

impl SettingsFile {
    pub fn new(settings_dir: impl Into<PathBuf>, runtime_dir: impl Into<PathBuf>, clock: EpochClock) -> Self {
        Self {
            settings_dir: settings_dir.into(),
            runtime_dir: runtime_dir.into(),
            clock,
        }
    }

    pub fn path(&self) -> PathBuf {
        self.settings_dir.join(SETTINGS_FILE)
    }

    /// Read `settings.json`, apply defaults and enforce `0600`.
    ///
    /// A missing file is a first run (defaults, no backup). An unparseable or
    /// non-object file is quarantined to `settings.json.corrupt-<ts>`; when the
    /// rename itself fails the error is logged and defaults are still returned.
    pub fn load(&self) -> anyhow::Result<LoadedSettings> {
        let path = self.path();
        let mut corrupt_backup = None;
        let mut settings = match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(Value::Object(map)) => map,
                _ => {
                    tracing::error!(
                        "settings.json is corrupt/unparseable \u{2014} backing up and resetting to defaults"
                    );
                    corrupt_backup = self.quarantine(&path);
                    Map::new()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Map::new(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        apply_defaults(&mut settings);
        if let Ok(meta) = fs::metadata(&path)
            && meta.permissions().mode() & 0o777 != 0o600
        {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .with_context(|| format!("chmod 0600 {}", path.display()))?;
        }
        Ok(LoadedSettings {
            settings,
            corrupt_backup,
        })
    }

    /// Rename the corrupt file aside; never clobbers an older backup.
    fn quarantine(&self, path: &Path) -> Option<String> {
        let stamp = (self.clock)();
        let mut name = format!("{SETTINGS_FILE}.corrupt-{stamp}");
        let mut n = 1;
        while self.settings_dir.join(&name).exists() {
            name = format!("{SETTINGS_FILE}.corrupt-{stamp}-{n}");
            n += 1;
        }
        match fs::rename(path, self.settings_dir.join(&name)) {
            Ok(()) => Some(name),
            Err(e) => {
                tracing::error!("could not back up corrupt settings.json to {name}: {e}");
                None
            }
        }
    }

    /// Crash-safe write of `data`, stamping `version = max(stored, current)`
    /// into it first (never down-stamping a file from a newer plugin).
    pub fn save(&self, data: &mut Map<String, Value>) -> anyhow::Result<()> {
        let version = stored_version(data).max(SETTINGS_VERSION);
        data.insert("version".into(), json!(version));
        let bytes = serde_json::to_vec_pretty(&Value::Object(data.clone()))?;
        locked_write(&self.path(), &bytes)
    }

    /// The legacy `save_sync_state.json` document, when it exists and is a JSON object.
    pub fn load_legacy_save_sync_state(&self) -> Option<Map<String, Value>> {
        let bytes = fs::read(self.runtime_dir.join(LEGACY_SAVE_SYNC_FILE)).ok()?;
        match serde_json::from_slice(&bytes).ok()? {
            Value::Object(map) => Some(map),
            _ => None,
        }
    }
}

fn locked_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let dir = path.parent().context("settings path has no parent")?;
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path.with_extension("json.lock"))
        .context("opening settings lock file")?;
    lock.lock().context("locking settings")?;
    let tmp = path.with_extension("json.tmp");
    let written = (|| -> anyhow::Result<()> {
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e.context(format!("writing {}", path.display())));
    }
    // Make the rename durable. Best-effort: some filesystems reject a dir fsync.
    if let Err(e) = File::open(dir).and_then(|d| d.sync_all()) {
        tracing::debug!("directory fsync after settings write failed (non-fatal): {e}");
    }
    Ok(())
}

/// [`SettingsPersister`] over the live store: snapshots and writes it.
///
/// Saves are serialized, and the snapshot is taken inside the critical section,
/// so a later save can never be overtaken on disk by an earlier snapshot.
pub struct FileSettingsPersister {
    file: Arc<SettingsFile>,
    store: SettingsStore,
    order: AsyncMutex<()>,
}

impl FileSettingsPersister {
    pub fn new(file: Arc<SettingsFile>, store: SettingsStore) -> Self {
        Self {
            file,
            store,
            order: AsyncMutex::new(()),
        }
    }
}

#[async_trait]
impl SettingsPersister for FileSettingsPersister {
    async fn save_settings(&self) -> anyhow::Result<()> {
        let _guard = self.order.lock().await;
        let mut snapshot = self.store.with(|m| {
            let version = stored_version(m).max(SETTINGS_VERSION);
            m.insert("version".into(), json!(version));
            m.clone()
        });
        let file = self.file.clone();
        tokio::task::spawn_blocking(move || file.save(&mut snapshot)).await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_at(dir: &Path, stamp: u64) -> SettingsFile {
        SettingsFile::new(dir, dir.join("runtime"), Arc::new(move || stamp))
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn fresh_load_is_defaults_without_backup() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = file_at(dir.path(), 1).load().unwrap();
        assert!(loaded.corrupt_backup.is_none());
        assert_eq!(loaded.settings["version"], 0);
        assert_eq!(loaded.settings["romm_api_token"], Value::Null);
        assert_eq!(loaded.settings["log_level"], "warn");
        assert!(!loaded.settings.contains_key("romm_user"));
        assert!(fs::read_dir(dir.path()).unwrap().next().is_none(), "nothing written");
    }

    #[test]
    fn load_preserves_values_and_backfills() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(SETTINGS_FILE),
            r#"{"version": 7, "romm_url": "http://x", "custom": 1}"#,
        )
        .unwrap();
        fs::set_permissions(dir.path().join(SETTINGS_FILE), fs::Permissions::from_mode(0o644)).unwrap();
        let loaded = file_at(dir.path(), 1).load().unwrap();
        assert_eq!(loaded.settings["version"], 7);
        assert_eq!(loaded.settings["romm_url"], "http://x");
        assert_eq!(loaded.settings["custom"], 1);
        assert_eq!(loaded.settings["platform_cores"], json!({}));
        assert_eq!(mode(&dir.path().join(SETTINGS_FILE)), 0o600, "permissions fixed");
    }

    #[test]
    fn corrupt_file_is_backed_up_never_lost() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SETTINGS_FILE);
        fs::write(&path, b"{\"romm_url\": \"http://x\", trunc").unwrap();
        let loaded = file_at(dir.path(), 1781697600).load().unwrap();
        assert_eq!(
            loaded.corrupt_backup.as_deref(),
            Some("settings.json.corrupt-1781697600")
        );
        assert_eq!(loaded.settings["romm_url"], "");
        let backup = dir.path().join("settings.json.corrupt-1781697600");
        assert_eq!(fs::read(&backup).unwrap(), b"{\"romm_url\": \"http://x\", trunc");
        assert!(!path.exists());
    }

    #[test]
    fn two_corruptions_same_second_keep_both() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SETTINGS_FILE);
        let file = file_at(dir.path(), 42);
        fs::write(&path, b"first{").unwrap();
        assert_eq!(
            file.load().unwrap().corrupt_backup.as_deref(),
            Some("settings.json.corrupt-42")
        );
        fs::write(&path, b"second{").unwrap();
        assert_eq!(
            file.load().unwrap().corrupt_backup.as_deref(),
            Some("settings.json.corrupt-42-1")
        );
        assert_eq!(
            fs::read(dir.path().join("settings.json.corrupt-42")).unwrap(),
            b"first{"
        );
        assert_eq!(
            fs::read(dir.path().join("settings.json.corrupt-42-1")).unwrap(),
            b"second{"
        );
    }

    #[test]
    fn non_object_and_invalid_utf8_count_as_corrupt() {
        for bytes in [&b"[1, 2]"[..], &b"\xff\xfe"[..], &b""[..]] {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join(SETTINGS_FILE), bytes).unwrap();
            assert!(file_at(dir.path(), 5).load().unwrap().corrupt_backup.is_some());
        }
    }

    #[test]
    fn failed_backup_rename_still_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SETTINGS_FILE);
        fs::write(&path, b"{bad").unwrap();
        // A directory squatting on every candidate name makes the rename fail.
        let file = SettingsFile::new(dir.path(), dir.path(), Arc::new(|| 9));
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o500)).unwrap();
        let loaded = file.load();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let loaded = loaded.unwrap();
        assert!(loaded.corrupt_backup.is_none());
        assert_eq!(loaded.settings["version"], 0);
    }

    #[test]
    fn save_is_atomic_stamped_and_private() {
        let dir = tempfile::tempdir().unwrap();
        let file = file_at(&dir.path().join("sub"), 1);
        let mut data = json!({"romm_url": "http://x"}).as_object().cloned().unwrap();
        file.save(&mut data).unwrap();
        assert_eq!(data["version"], 10);
        let path = dir.path().join("sub").join(SETTINGS_FILE);
        let on_disk: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(on_disk, json!({"romm_url": "http://x", "version": 10}));
        assert_eq!(mode(&path), 0o600);
        assert!(dir.path().join("sub").join("settings.json.lock").exists());
        assert!(!dir.path().join("sub").join("settings.json.tmp").exists());
    }

    #[test]
    fn version_never_down_stamped() {
        let dir = tempfile::tempdir().unwrap();
        let file = file_at(dir.path(), 1);
        for (stored, expected) in [
            (json!(12), 12),
            (json!(3), 10),
            (json!(10), 10),
            (json!("abc"), 10),
            (json!(null), 10),
        ] {
            let mut data = json!({"version": stored}).as_object().cloned().unwrap();
            file.save(&mut data).unwrap();
            assert_eq!(data["version"], expected);
        }
        let mut absent = Map::new();
        file.save(&mut absent).unwrap();
        assert_eq!(absent["version"], 10);
    }

    #[test]
    fn write_failure_cleans_up_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let file = file_at(dir.path(), 1);
        // settings.json is a directory → the rename fails.
        fs::create_dir(dir.path().join(SETTINGS_FILE)).unwrap();
        fs::write(dir.path().join(SETTINGS_FILE).join("x"), b"x").unwrap();
        assert!(file.save(&mut Map::new()).is_err());
        assert!(!dir.path().join("settings.json.tmp").exists());
    }

    #[test]
    fn legacy_save_sync_state() {
        let dir = tempfile::tempdir().unwrap();
        let file = file_at(dir.path(), 1);
        assert!(file.load_legacy_save_sync_state().is_none());
        fs::create_dir_all(dir.path().join("runtime")).unwrap();
        let legacy = dir.path().join("runtime").join(LEGACY_SAVE_SYNC_FILE);
        fs::write(&legacy, br#"{"device_name": "Deck"}"#).unwrap();
        assert_eq!(file.load_legacy_save_sync_state().unwrap()["device_name"], "Deck");
        fs::write(&legacy, b"{broken").unwrap();
        assert!(file.load_legacy_save_sync_state().is_none());
        fs::write(&legacy, b"[1]").unwrap();
        assert!(file.load_legacy_save_sync_state().is_none());
    }

    #[tokio::test]
    async fn persister_writes_live_store_and_stamps_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = Arc::new(file_at(dir.path(), 1));
        let store = SettingsStore::new(json!({"version": 4, "a": 1}).as_object().cloned().unwrap());
        let persister = FileSettingsPersister::new(file.clone(), store.clone());
        persister.save_settings().await.unwrap();
        assert_eq!(store.get("version"), Some(json!(10)));
        store.set("a", json!(2));
        persister.save_settings().await.unwrap();
        let on_disk: Value = serde_json::from_slice(&fs::read(file.path()).unwrap()).unwrap();
        assert_eq!(on_disk, json!({"version": 10, "a": 2}));
    }

    #[tokio::test]
    async fn concurrent_saves_land_last_write_wins() {
        let dir = tempfile::tempdir().unwrap();
        let file = Arc::new(file_at(dir.path(), 1));
        let store = SettingsStore::new(Map::new());
        let persister = Arc::new(FileSettingsPersister::new(file.clone(), store.clone()));
        let mut tasks = Vec::new();
        for i in 0..20 {
            store.set("n", json!(i));
            let p = persister.clone();
            tasks.push(tokio::spawn(async move { p.save_settings().await.unwrap() }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        persister.save_settings().await.unwrap();
        let on_disk: Value = serde_json::from_slice(&fs::read(file.path()).unwrap()).unwrap();
        assert_eq!(on_disk["n"], 19);
    }
}
