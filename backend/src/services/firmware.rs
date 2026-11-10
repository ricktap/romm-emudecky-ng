//! FirmwareService — what Switch firmware and keys RomM hosts and how this machine compares.
//!
//! `get_firmware_status` lists RomM's Switch files (`bios/Switch`), classifies them (keys, firmware archives),
//! and for every installed emulator says whether its copy of each key file is missing, the same as RomM's,
//! different, or present without anything to compare with. Listing never writes. Downloads go through the shared
//! download queue (`DownloadService`, kind firmware) and only ever fill gaps: a key file that differs from RomM's is
//! reported and kept, never replaced.
//! Design and device facts: docs/plans/0.45.0-switch-firmware.md.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use async_trait::async_trait;

use crate::domain::emulator_table::find_emulator;
use crate::domain::switch_firmware::{
    self as sw, FileKind, KEY_FILES, KeyState, Version, classify, is_switch_path, key_state, keys_dir,
    parse_version_text, selected_version, version_text,
};
use crate::services::downloads::DownloadService;
use crate::services::ports::{
    Clock, DownloadFiles, EmuDeckPaths, EmulatorLocator, FirmwareFileKind, FirmwareJobs, FirmwareTarget,
    RommFirmwareApi, SettingsPersister, SettingsStore,
};
use crate::domain::errors::failure;
use crate::util::path_safety::join;

/// How long a fetched firmware list is reused.
const LIST_TTL_S: f64 = 3600.0;
/// The system whose firmware this service handles.
const SYSTEM: &str = "switch";

pub struct FirmwareServiceConfig {
    pub romm: Arc<dyn RommFirmwareApi>,
    pub files: Arc<dyn DownloadFiles>,
    pub locator: Arc<dyn EmulatorLocator>,
    pub paths: Arc<dyn EmuDeckPaths>,
    pub clock: Arc<dyn Clock>,
    /// The user's home directory; the emulators' key directories live under it.
    pub home: String,
    /// Holds `switch_firmware_version`.
    pub settings: SettingsStore,
    pub persister: Arc<dyn SettingsPersister>,
}

pub struct FirmwareService {
    c: FirmwareServiceConfig,
    cache: Mutex<Option<(f64, Vec<Value>)>>,
}

/// One RomM firmware entry, reduced to what the status needs.
#[derive(Debug, Clone)]
struct Entry {
    id: Option<i64>,
    file_name: String,
    size: Option<u64>,
    sha1: Option<String>,
}

fn entry_of(v: &Value) -> Option<Entry> {
    let file_name = v.get("file_name")?.as_str()?.to_string();
    // RomM names a file by `file_name` alone; refuse anything that is not a plain file name.
    if file_name.is_empty() || file_name.contains('/') || file_name.contains('\\') || file_name == "." || file_name == ".." {
        return None;
    }
    Some(Entry {
        id: v.get("id").and_then(Value::as_i64),
        file_name,
        size: v.get("file_size_bytes").and_then(Value::as_u64),
        sha1: v.get("sha1_hash").and_then(Value::as_str).map(str::to_ascii_lowercase).filter(|h| !h.is_empty()),
    })
}

impl FirmwareService {
    pub fn new(config: FirmwareServiceConfig) -> Arc<Self> {
        Arc::new(Self { c: config, cache: Mutex::new(None) })
    }

    /// The firmware list: the cache while fresh, else RomM. `Err(stale)` carries the old list (possibly empty)
    /// when RomM cannot be reached.
    async fn list(&self) -> Result<Vec<Value>, Vec<Value>> {
        let now = self.c.clock.epoch_seconds();
        let stale = {
            let cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
            match &*cache {
                Some((at, list)) if now - at < LIST_TTL_S => return Ok(list.clone()),
                Some((_, list)) => list.clone(),
                None => Vec::new(),
            }
        };
        match self.c.romm.list_firmware().await {
            Ok(value) => {
                let list = match value {
                    Value::Array(items) => items,
                    Value::Object(mut obj) => match obj.remove("items") {
                        Some(Value::Array(items)) => items,
                        _ => Vec::new(),
                    },
                    _ => Vec::new(),
                };
                *self.cache.lock().unwrap_or_else(|p| p.into_inner()) = Some((now, list.clone()));
                Ok(list)
            }
            Err(e) => {
                tracing::warn!("Failed to fetch the firmware list from RomM: {e}");
                Err(stale)
            }
        }
    }

    /// Forget the cached list (the next status asks RomM again).
    pub fn invalidate(&self) {
        *self.cache.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    /// The Switch files RomM lists, and whether RomM answered.
    async fn switch_entries(&self) -> (Vec<Entry>, bool) {
        let (list, offline) = match self.list().await {
            Ok(list) => (list, false),
            Err(stale) => (stale, true),
        };
        let entries = list
            .iter()
            .filter(|v| v.get("file_path").and_then(Value::as_str).is_some_and(is_switch_path))
            .filter_map(entry_of)
            .collect();
        (entries, offline)
    }

    /// The emulators found on this machine that have a keys directory: `(id, label, keys dir)`.
    fn emulators(&self) -> Vec<(&'static str, String, String)> {
        let installed = self.c.locator.installed(SYSTEM);
        sw::EMULATORS
            .iter()
            .filter(|id| installed.contains_key(**id))
            .filter_map(|id| {
                let dir = keys_dir(id, &self.c.home)?;
                let label = find_emulator(Some(id)).map_or_else(|| (*id).to_string(), |e| e.label.to_string());
                Some((*id, label, dir))
            })
            .collect()
    }

    /// `(emulator id, label, full path, state)` of one key file for every installed emulator that reads it.
    async fn key_states(
        &self,
        key: &sw::KeyFile,
        on_romm: Option<&Entry>,
        emulators: &[(&'static str, String, String)],
    ) -> Vec<(&'static str, String, String, KeyState)> {
        let mut out = Vec::new();
        for (id, label, dir) in emulators {
            if !key.emulators.contains(id) {
                continue;
            }
            let path = join(dir, &[key.file_name]);
            let local = self.c.files.sha1_hex(&path).await;
            let state = key_state(local.as_deref(), on_romm.and_then(|e| e.sha1.as_deref()));
            out.push((*id, label.clone(), path, state));
        }
        out
    }

    fn archive_dir(&self) -> String {
        join(&self.c.paths.bios_path(), &[SYSTEM])
    }

    /// The firmware archives in the list, newest first: `(version, entry)`.
    fn archives(entries: &[Entry]) -> Vec<(Version, &Entry)> {
        let mut versions: Vec<(Version, &Entry)> = entries
            .iter()
            .filter_map(|e| match classify(&e.file_name) {
                FileKind::Firmware(v) => Some((v, e)),
                _ => None,
            })
            .collect();
        versions.sort_by(|a, b| b.0.cmp(&a.0));
        versions
    }

    /// The version the user chose (or the newest), among the archives RomM lists.
    fn chosen(&self, versions: &[(Version, &Entry)]) -> Option<Version> {
        let available: Vec<Version> = versions.iter().map(|(v, _)| v.clone()).collect();
        let setting = self.c.settings.get_str("switch_firmware_version").unwrap_or_default();
        selected_version(&available, &setting).cloned()
    }

    fn pinned(&self, versions: &[(Version, &Entry)]) -> bool {
        let setting = self.c.settings.get_str("switch_firmware_version").unwrap_or_default();
        parse_version_text(&setting).is_some_and(|wanted| versions.iter().any(|(v, _)| *v == wanted))
    }

    async fn archive_downloaded(&self, e: &Entry) -> (bool, Option<u64>) {
        let local = self.c.files.regular_file_size(&join(&self.archive_dir(), &[&e.file_name])).await;
        (local.is_some() && local == e.size, local)
    }

    pub async fn get_firmware_status(&self) -> Value {
        let (switch, offline) = self.switch_entries().await;
        let emulators = self.emulators();

        let mut keys = Vec::new();
        for key in KEY_FILES {
            let on_romm = switch.iter().find(|e| e.file_name == key.file_name);
            let per_emulator: Vec<Value> = self
                .key_states(key, on_romm, &emulators)
                .await
                .into_iter()
                .map(|(id, label, _, state)| json!({"id": id, "label": label, "state": state.as_str()}))
                .collect();
            keys.push(json!({
                "file_name": key.file_name,
                "description": key.description,
                "required": key.required,
                "on_romm": on_romm.is_some(),
                "firmware_id": on_romm.and_then(|e| e.id),
                "size": on_romm.and_then(|e| e.size),
                "emulators": per_emulator,
            }));
        }

        let versions = Self::archives(&switch);
        let chosen = self.chosen(&versions);
        let mut firmware = Vec::new();
        for (version, e) in &versions {
            let (downloaded, local) = self.archive_downloaded(e).await;
            firmware.push(json!({
                "firmware_id": e.id,
                "file_name": e.file_name,
                "version": version_text(version),
                "size": e.size,
                "selected": chosen.as_ref() == Some(version),
                "downloaded": downloaded,
                "local_size": local,
            }));
        }
        let unrecognized = switch.iter().filter(|e| matches!(classify(&e.file_name), FileKind::Other)).count();

        json!({
            "success": true,
            "server_offline": offline,
            "platform_slug": SYSTEM,
            "archive_dir": self.archive_dir(),
            "emulators": emulators.iter().map(|(id, label, dir)| json!({"id": id, "label": label, "keys_dir": dir})).collect::<Vec<_>>(),
            "keys": keys,
            "firmware": firmware,
            "selected_version": chosen.as_ref().map(|v| version_text(v)),
            // Whether the user chose a version (and RomM still has it) rather than following the newest.
            "version_pinned": self.pinned(&versions),
            "unrecognized": unrecognized,
        })
    }

    /// Save the firmware version to download (`""` = the newest RomM has). A version RomM does not list is accepted
    /// (it may reappear); only text that is not a version is refused.
    pub async fn set_firmware_version(&self, version: &str) -> Value {
        let trimmed = version.trim();
        let value = if trimmed.is_empty() {
            String::new()
        } else if let Some(v) = parse_version_text(trimmed) {
            version_text(&v)
        } else {
            return failure("invalid_version", &format!("Not a firmware version: {trimmed}"));
        };
        self.c.settings.set("switch_firmware_version", json!(value));
        match self.c.persister.save_settings().await {
            Ok(()) => json!({"success": true, "message": "Firmware version saved"}),
            Err(e) => {
                tracing::error!("Failed to save the firmware version: {e:#}");
                failure("save_failed", &format!("Save failed: {e}"))
            }
        }
    }

    /// The downloads that fill this machine's gaps, without starting them: missing key files (the required ones, or
    /// every one with `all`) and the chosen firmware version's archive, as `(firmware ids, key files kept because
    /// they differ from RomM's)`. `Err` is the failure to report (RomM unreachable / no Switch files).
    pub async fn plan_downloads(&self, all: bool) -> Result<(Vec<i64>, Vec<String>), Value> {
        let (switch, offline) = self.switch_entries().await;
        if switch.is_empty() {
            return Err(if offline {
                failure("server_unreachable", "RomM is unreachable — firmware cannot be downloaded")
            } else {
                failure("not_found", "RomM has no Switch firmware")
            });
        }
        let emulators = self.emulators();
        let mut ids: Vec<i64> = Vec::new();
        let mut kept: Vec<String> = Vec::new();
        for key in KEY_FILES.iter().filter(|k| k.required || all) {
            let Some(entry) = switch.iter().find(|e| e.file_name == key.file_name) else {
                continue;
            };
            let states = self.key_states(key, Some(entry), &emulators).await;
            if states.iter().any(|(_, _, _, s)| *s == KeyState::Differs) {
                kept.push(key.file_name.to_string());
            }
            if let (Some(id), true) = (entry.id, states.iter().any(|(_, _, _, s)| *s == KeyState::Missing)) {
                ids.push(id);
            }
        }
        let versions = Self::archives(&switch);
        if let Some(chosen) = self.chosen(&versions)
            && let Some((_, entry)) = versions.iter().find(|(v, _)| *v == chosen)
            && let Some(id) = entry.id
            && !self.archive_downloaded(entry).await.0
        {
            ids.push(id);
        }
        Ok((ids, kept))
    }

    /// Queue what [`plan_downloads`](Self::plan_downloads) found. Returns at once; the transfers run in the shared
    /// download queue. Key files that exist but differ from RomM's are named in the answer and never replaced.
    pub async fn start_downloads(&self, downloads: &Arc<DownloadService>, all: bool) -> Value {
        let (ids, kept) = match self.plan_downloads(all).await {
            Ok(plan) => plan,
            Err(failure) => return failure,
        };
        let mut queued = 0;
        let mut problems: Vec<String> = Vec::new();
        for id in &ids {
            let r = downloads.start_firmware_download(*id).await;
            if r["success"] == true || r["reason"] == "already_downloading" {
                queued += 1;
            } else {
                problems.push(r["message"].as_str().unwrap_or("download failed").to_string());
            }
        }
        let mut message = match queued {
            0 if problems.is_empty() => "Nothing to download — everything is already in place".to_string(),
            0 => "Nothing could be queued".to_string(),
            1 => "Queued 1 download".to_string(),
            n => format!("Queued {n} downloads"),
        };
        if !kept.is_empty() {
            message.push_str(&format!(" · kept your own {} (differs from RomM's)", kept.join(", ")));
        }
        if !problems.is_empty() {
            message.push_str(&format!(" · {}", problems.join("; ")));
        }
        json!({"success": problems.is_empty() || queued > 0, "message": message, "queued": queued, "kept": kept})
    }
}

#[async_trait]
impl FirmwareJobs for FirmwareService {
    async fn target(&self, firmware_id: i64) -> Option<FirmwareTarget> {
        let (switch, _) = self.switch_entries().await;
        let entry = switch.iter().find(|e| e.id == Some(firmware_id))?;
        match classify(&entry.file_name) {
            FileKind::Key(key) => {
                let emulators = self.emulators();
                let missing: Vec<String> = self
                    .key_states(&key, Some(entry), &emulators)
                    .await
                    .into_iter()
                    .filter(|(_, _, _, s)| *s == KeyState::Missing)
                    .map(|(_, _, path, _)| path)
                    .collect();
                let (dest_path, copy_to) = missing.split_first()?;
                Some(FirmwareTarget {
                    firmware_id,
                    file_name: entry.file_name.clone(),
                    size: entry.size.unwrap_or(0),
                    sha1: entry.sha1.clone(),
                    kind: FirmwareFileKind::Key,
                    dest_path: dest_path.clone(),
                    copy_to: copy_to.to_vec(),
                })
            }
            FileKind::Firmware(_) => {
                if self.archive_downloaded(entry).await.0 {
                    return None;
                }
                Some(FirmwareTarget {
                    firmware_id,
                    file_name: entry.file_name.clone(),
                    size: entry.size.unwrap_or(0),
                    sha1: entry.sha1.clone(),
                    kind: FirmwareFileKind::Archive,
                    dest_path: join(&self.archive_dir(), &[&entry.file_name]),
                    copy_to: Vec::new(),
                })
            }
            FileKind::Other => None,
        }
    }
}

#[cfg(test)]
#[path = "firmware_tests.rs"]
mod tests;
