//! Where EmuDeck keeps its files, and what to say when that changed.
//!
//! EmuDeck's `storagePath` (the parent of `Emulation/`) can move: a new SD card, a reinstall that picked another
//! drive. The installs this plugin recorded still name the old folders, so after a move the games would silently
//! look "not installed" (or worse, the plugin would be reading a different tree). This service remembers the
//! location the user last accepted, reports a change next to the health state, counts how many recorded installs
//! are no longer on disk, and records the new location once the user accepts it. It never moves, copies or deletes
//! a file; getting the games over to the new place is up to the user.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::domain::errors::{failure, reason, unknown_failure};
use crate::services::ports::{DownloadFiles, EmuDeckPaths, KvConfigRepo, LibraryRepo};

/// `kv_config` key of the storage location the user last accepted.
const ACCEPTED_KEY: &str = "emudeck_storage_path";

pub struct EmuDeckLocationConfig {
    pub paths: Arc<dyn EmuDeckPaths>,
    pub kv: Arc<dyn KvConfigRepo>,
    pub library: Arc<dyn LibraryRepo>,
    pub files: Arc<dyn DownloadFiles>,
}

pub struct EmuDeckLocationService {
    c: EmuDeckLocationConfig,
}

impl EmuDeckLocationService {
    pub fn new(config: EmuDeckLocationConfig) -> Arc<Self> {
        Arc::new(Self { c: config })
    }

    /// The `get_emudeck_health` answer: the health state plus, when the storage location moved since it was last
    /// accepted, `moved: true` with both paths and how many recorded installs are no longer where they were.
    ///
    /// The first healthy reading is recorded silently (there is nothing to compare with yet).
    pub async fn status(&self) -> Value {
        let health = self.c.paths.health();
        let current = self.c.paths.storage_path();
        let mut value = health.to_value(current.as_deref());
        value["moved"] = json!(false);
        let accepted = match self.c.kv.get(ACCEPTED_KEY).await {
            Ok(accepted) => accepted,
            Err(e) => {
                tracing::warn!("Could not read the accepted EmuDeck storage path: {e:#}");
                return value;
            }
        };
        let Some(current) = current.filter(|_| health.is_ok()) else {
            return value;
        };
        match accepted {
            None => {
                if let Err(e) = self.c.kv.set(ACCEPTED_KEY, &current).await {
                    tracing::warn!("Could not remember the EmuDeck storage path: {e:#}");
                }
            }
            Some(accepted) if accepted != current => {
                let (installed, missing) = self.count_missing_installs().await;
                value["moved"] = json!(true);
                value["accepted_path"] = json!(accepted);
                value["installed"] = json!(installed);
                value["missing"] = json!(missing);
            }
            Some(_) => {}
        }
        value
    }

    /// `(recorded installs, those whose file is no longer on disk)`.
    async fn count_missing_installs(&self) -> (usize, usize) {
        let installs = match self.c.library.installs().await {
            Ok(installs) => installs,
            Err(e) => {
                tracing::warn!("Could not list the installs to verify them: {e:#}");
                return (0, 0);
            }
        };
        let mut missing = 0;
        for install in &installs {
            if !self.c.files.exists(&install.file_path).await {
                missing += 1;
            }
        }
        (installs.len(), missing)
    }

    /// Remember the current storage location as the accepted one. Refused while EmuDeck is not healthy: the user
    /// would be accepting a place that is not there.
    pub async fn accept(&self) -> Value {
        let health = self.c.paths.health();
        let Some(current) = self.c.paths.storage_path().filter(|_| health.is_ok()) else {
            return failure("emudeck_unavailable", health.message());
        };
        match self.c.kv.set(ACCEPTED_KEY, &current).await {
            Ok(()) => json!({"success": true, "message": "Using the new EmuDeck location", "storage_path": current}),
            Err(e) => {
                tracing::warn!("Could not remember the EmuDeck storage path: {e:#}");
                let mut v = unknown_failure(&e);
                v["reason"] = json!(reason::UNKNOWN);
                v
            }
        }
    }
}
