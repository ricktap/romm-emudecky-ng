//! FirmwareService — what Switch firmware and keys RomM hosts and how this machine compares.
//!
//! `get_firmware_status` lists RomM's Switch files (`bios/Switch`), classifies them (keys, firmware archives),
//! and for every installed emulator says whether its copy of each key file is missing, the same as RomM's,
//! different, or present without anything to compare with. It never writes. Downloads join the queue later
//! (0.45.0); until then a differing key file is reported, never touched.
//! Design and device facts: docs/plans/0.45.0-switch-firmware.md.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::domain::emulator_table::find_emulator;
use crate::domain::switch_firmware::{
    self as sw, FileKind, KEY_FILES, Version, classify, is_switch_path, key_state, keys_dir, newest,
    version_text,
};
use crate::services::ports::{Clock, DownloadFiles, EmuDeckPaths, EmulatorLocator, RommFirmwareApi};
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

    pub async fn get_firmware_status(&self) -> Value {
        let (list, offline) = match self.list().await {
            Ok(list) => (list, false),
            Err(stale) => (stale, true),
        };
        let switch: Vec<Entry> = list
            .iter()
            .filter(|v| v.get("file_path").and_then(Value::as_str).is_some_and(is_switch_path))
            .filter_map(entry_of)
            .collect();

        // The emulators found on this machine that have a keys directory.
        let installed = self.c.locator.installed(SYSTEM);
        let emulators: Vec<(&str, String, String)> = sw::EMULATORS
            .iter()
            .filter(|id| installed.contains_key(**id))
            .filter_map(|id| {
                let dir = keys_dir(id, &self.c.home)?;
                let label = find_emulator(Some(id)).map_or_else(|| (*id).to_string(), |e| e.label.to_string());
                Some((*id, label, dir))
            })
            .collect();

        let mut keys = Vec::new();
        for key in KEY_FILES {
            let on_romm = switch.iter().find(|e| e.file_name == key.file_name);
            let mut per_emulator = Vec::new();
            for (id, label, dir) in &emulators {
                if !key.emulators.contains(id) {
                    continue;
                }
                let local = self.c.files.sha1_hex(&join(dir, &[key.file_name])).await;
                let state = key_state(local.as_deref(), on_romm.and_then(|e| e.sha1.as_deref()));
                per_emulator.push(json!({"id": id, "label": label, "state": state.as_str()}));
            }
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

        let archive_dir = join(&self.c.paths.bios_path(), &[SYSTEM]);
        let mut versions: Vec<(Version, &Entry)> = switch
            .iter()
            .filter_map(|e| match classify(&e.file_name) {
                FileKind::Firmware(v) => Some((v, e)),
                _ => None,
            })
            .collect();
        let newest_version = newest(versions.iter().map(|(v, _)| v)).cloned();
        versions.sort_by(|a, b| b.0.cmp(&a.0));
        let mut firmware = Vec::new();
        for (version, e) in &versions {
            let local = self.c.files.regular_file_size(&join(&archive_dir, &[&e.file_name])).await;
            firmware.push(json!({
                "firmware_id": e.id,
                "file_name": e.file_name,
                "version": version_text(version),
                "size": e.size,
                "selected": newest_version.as_ref() == Some(version),
                "downloaded": local.is_some() && local == e.size,
                "local_size": local,
            }));
        }
        let unrecognized = switch.iter().filter(|e| matches!(classify(&e.file_name), FileKind::Other)).count();

        json!({
            "success": true,
            "server_offline": offline,
            "platform_slug": SYSTEM,
            "archive_dir": archive_dir,
            "emulators": emulators.iter().map(|(id, label, dir)| json!({"id": id, "label": label, "keys_dir": dir})).collect::<Vec<_>>(),
            "keys": keys,
            "firmware": firmware,
            "unrecognized": unrecognized,
        })
    }
}

#[cfg(test)]
#[path = "firmware_tests.rs"]
mod tests;
