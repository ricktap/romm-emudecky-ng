//! Per-ROM save-sync state: the active slot, whether the user confirmed it, the emulator the ROM runs under, our upload
//! attribution, the merged slot listing the UI reads, and the per-file sync baselines the decision kernel compares against.
//! A port of the Python aggregate (`domain/rom_save_state.py`); the same invariants hold: a tracked file always carries a
//! non-empty hash baseline, a non-legacy active slot always has its key in `slots`, attribution never mutates `None`.
use std::collections::BTreeMap;

use serde_json::{json, Value};

/// The per-file baseline: what was last observed when this device synced the file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileSyncState {
    pub tracked_save_id: Option<i64>,
    pub last_sync_hash: Option<String>,
    pub last_sync_at: String,
    pub last_sync_server_updated_at: String,
    pub last_sync_server_save_id: Option<i64>,
    pub last_sync_server_size: Option<i64>,
    pub last_sync_local_mtime: Option<f64>,
    pub last_sync_local_size: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RomSaveState {
    pub active_slot: Option<String>,
    pub slot_confirmed: bool,
    pub emulator: String,
    pub system: String,
    pub last_synced_core: Option<String>,
    /// `None` = attribution unknown (legacy), `Some([])` = we uploaded nothing; both are meaningful.
    pub own_upload_ids: Option<Vec<i64>>,
    /// Slot name (`""` = the legacy no-slot key) -> `{source, count, latest_updated_at}`.
    pub slots: BTreeMap<String, Value>,
    pub files: BTreeMap<String, FileSyncState>,
    pub last_sync_check_at: Option<String>,
}

impl Default for RomSaveState {
    fn default() -> Self {
        RomSaveState {
            active_slot: None,
            slot_confirmed: false,
            emulator: "retroarch".into(),
            system: String::new(),
            last_synced_core: None,
            own_upload_ids: None,
            slots: BTreeMap::new(),
            files: BTreeMap::new(),
            last_sync_check_at: None,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StateError {
    #[error("tracked_save_id must be positive")]
    BadSaveId,
    #[error("last_sync_hash is required")]
    EmptyHash,
    #[error("a slot needs a non-empty name")]
    EmptySlot,
    #[error("emulator is required")]
    EmptyEmulator,
}

fn local_slot() -> Value {
    json!({"source": "local", "count": 0, "latest_updated_at": null})
}

/// The values of one full baseline (`adopt_baseline`).
#[derive(Debug, Clone, Default)]
pub struct Baseline {
    pub tracked_save_id: i64,
    pub last_sync_hash: String,
    pub last_sync_at: String,
    pub last_sync_server_updated_at: String,
    pub last_sync_server_save_id: Option<i64>,
    pub last_sync_server_size: Option<i64>,
    pub last_sync_local_mtime: Option<f64>,
    pub last_sync_local_size: Option<i64>,
}

impl RomSaveState {
    /// Record `filename`'s baseline, replacing any existing entry. The only way to add a tracked file with a server save.
    pub fn adopt_baseline(&mut self, filename: &str, b: Baseline) -> Result<(), StateError> {
        if b.tracked_save_id <= 0 {
            return Err(StateError::BadSaveId);
        }
        if b.last_sync_hash.is_empty() {
            return Err(StateError::EmptyHash);
        }
        self.files.insert(
            filename.to_string(),
            FileSyncState {
                tracked_save_id: Some(b.tracked_save_id),
                last_sync_hash: Some(b.last_sync_hash),
                last_sync_at: b.last_sync_at,
                last_sync_server_updated_at: b.last_sync_server_updated_at,
                last_sync_server_save_id: b.last_sync_server_save_id,
                last_sync_server_size: b.last_sync_server_size,
                last_sync_local_mtime: b.last_sync_local_mtime,
                last_sync_local_size: b.last_sync_local_size,
            },
        );
        Ok(())
    }

    /// Record only the hash baseline (the skip-adopt path), keeping every other anchor of an existing entry.
    pub fn update_baseline_hash(&mut self, filename: &str, hash: &str) -> Result<(), StateError> {
        if hash.is_empty() {
            return Err(StateError::EmptyHash);
        }
        let entry = self.files.entry(filename.to_string()).or_default();
        entry.last_sync_hash = Some(hash.to_string());
        Ok(())
    }

    /// Attribute `save_id` to an upload we made (idempotent; starts the list when attribution was unknown).
    pub fn track_own_upload(&mut self, save_id: i64) {
        match &mut self.own_upload_ids {
            None => self.own_upload_ids = Some(vec![save_id]),
            Some(ids) if !ids.contains(&save_id) => ids.push(save_id),
            Some(_) => {}
        }
    }

    /// Confirm `name` as the active slot (legacy `slot:null` confirmation is retired: an empty name is refused).
    pub fn confirm_slot(&mut self, name: &str) -> Result<(), StateError> {
        if name.is_empty() {
            return Err(StateError::EmptySlot);
        }
        self.active_slot = Some(name.to_string());
        self.slot_confirmed = true;
        self.slots.entry(name.to_string()).or_insert_with(local_slot);
        Ok(())
    }

    /// Switch the active slot without confirming it (an empty name means the legacy no-slot key).
    pub fn switch_active_slot(&mut self, name: Option<&str>) {
        let normalized = name.filter(|n| !n.is_empty()).map(str::to_string);
        self.slots.entry(normalized.clone().unwrap_or_default()).or_insert_with(local_slot);
        self.active_slot = normalized;
    }

    /// A local-only slot got its first save on the server: flip `source` to `server` and seed the count at 1.
    pub fn promote_slot_to_server(&mut self, slot: &str) -> Result<(), StateError> {
        if slot.is_empty() {
            return Err(StateError::EmptySlot);
        }
        if let Some(entry) = self.slots.get_mut(slot) {
            if entry["source"] == "local" {
                entry["source"] = json!("server");
                entry["count"] = json!(1);
            }
        }
        Ok(())
    }

    pub fn mark_sync_evaluated(&mut self, at: &str) {
        self.last_sync_check_at = Some(at.to_string());
    }

    /// Stamp the system the ROM runs under; an empty value never clobbers a known one.
    pub fn adopt_system(&mut self, system: &str) {
        if !system.is_empty() {
            self.system = system.to_string();
        }
    }

    /// Stamp the emulator (and optionally the core) the last sync ran under; `core: None` keeps a known core.
    pub fn record_synced_core(&mut self, core: Option<&str>, emulator: &str) -> Result<(), StateError> {
        if emulator.is_empty() {
            return Err(StateError::EmptyEmulator);
        }
        self.emulator = emulator.to_string();
        if let Some(c) = core {
            self.last_synced_core = Some(c.to_string());
        }
        Ok(())
    }

    pub fn refresh_slot_listing(&mut self, merged: BTreeMap<String, Value>) {
        self.slots = merged;
    }

    pub fn delete_file_tracking(&mut self, filename: &str) {
        self.files.remove(filename);
    }

    pub fn delete_slot_tracking(&mut self, slot: &str) {
        self.slots.remove(slot);
    }

    /// Drop every baseline (the active slot changed, which invalidates them).
    pub fn clear_baselines(&mut self) {
        self.files.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baseline(id: i64, hash: &str) -> Baseline {
        Baseline { tracked_save_id: id, last_sync_hash: hash.into(), ..Baseline::default() }
    }

    #[test]
    fn a_baseline_needs_a_positive_save_id_and_a_hash() {
        let mut s = RomSaveState::default();
        assert_eq!(s.adopt_baseline("a", baseline(0, "h")), Err(StateError::BadSaveId));
        assert_eq!(s.adopt_baseline("a", baseline(1, "")), Err(StateError::EmptyHash));
        assert!(s.files.is_empty(), "a refused baseline tracks nothing");
        s.adopt_baseline("a", baseline(7, "h")).unwrap();
        assert_eq!(s.files["a"].tracked_save_id, Some(7));
        s.adopt_baseline("a", baseline(8, "h2")).unwrap();
        assert_eq!(s.files["a"].last_sync_hash.as_deref(), Some("h2"), "re-adopting replaces");
    }

    #[test]
    fn updating_only_the_hash_keeps_the_other_anchors() {
        let mut s = RomSaveState::default();
        s.update_baseline_hash("a", "h1").unwrap();
        assert_eq!(s.files["a"], FileSyncState { last_sync_hash: Some("h1".into()), ..FileSyncState::default() });
        s.adopt_baseline("a", baseline(7, "h1")).unwrap();
        s.update_baseline_hash("a", "h2").unwrap();
        assert_eq!((s.files["a"].tracked_save_id, s.files["a"].last_sync_hash.as_deref()), (Some(7), Some("h2")));
        assert_eq!(s.update_baseline_hash("a", ""), Err(StateError::EmptyHash));
    }

    #[test]
    fn own_uploads_start_a_list_from_unknown_and_never_duplicate() {
        let mut s = RomSaveState::default();
        assert_eq!(s.own_upload_ids, None);
        s.track_own_upload(3);
        s.track_own_upload(3);
        s.track_own_upload(4);
        assert_eq!(s.own_upload_ids, Some(vec![3, 4]));
    }

    #[test]
    fn confirming_a_slot_needs_a_name_and_seeds_its_listing_key() {
        let mut s = RomSaveState::default();
        assert_eq!(s.confirm_slot(""), Err(StateError::EmptySlot));
        assert!(!s.slot_confirmed);
        s.confirm_slot("default").unwrap();
        assert_eq!((s.active_slot.as_deref(), s.slot_confirmed), (Some("default"), true));
        assert_eq!(s.slots["default"]["source"], "local");
        s.slots.get_mut("default").unwrap()["count"] = json!(5);
        s.confirm_slot("default").unwrap();
        assert_eq!(s.slots["default"]["count"], 5, "an existing listing is kept");
    }

    #[test]
    fn switching_a_slot_does_not_confirm_and_an_empty_name_is_the_legacy_key() {
        let mut s = RomSaveState::default();
        s.switch_active_slot(Some("other"));
        assert_eq!((s.active_slot.as_deref(), s.slot_confirmed), (Some("other"), false));
        s.switch_active_slot(Some(""));
        assert_eq!(s.active_slot, None);
        assert!(s.slots.contains_key(""));
        s.switch_active_slot(None);
        assert_eq!(s.active_slot, None);
    }

    #[test]
    fn promoting_flips_a_local_slot_once() {
        let mut s = RomSaveState::default();
        s.confirm_slot("a").unwrap();
        s.promote_slot_to_server("a").unwrap();
        assert_eq!((s.slots["a"]["source"].clone(), s.slots["a"]["count"].clone()), (json!("server"), json!(1)));
        s.slots.get_mut("a").unwrap()["count"] = json!(4);
        s.promote_slot_to_server("a").unwrap();
        assert_eq!(s.slots["a"]["count"], 4, "no double counting");
        s.promote_slot_to_server("untracked").unwrap();
        assert_eq!(s.promote_slot_to_server(""), Err(StateError::EmptySlot));
    }

    #[test]
    fn system_and_core_stamps_never_clobber_with_empty_values() {
        let mut s = RomSaveState::default();
        s.adopt_system("switch");
        s.adopt_system("");
        assert_eq!(s.system, "switch");
        s.record_synced_core(Some("c"), "eden").unwrap();
        s.record_synced_core(None, "yuzu").unwrap();
        assert_eq!((s.emulator.as_str(), s.last_synced_core.as_deref()), ("yuzu", Some("c")));
        assert_eq!(s.record_synced_core(None, ""), Err(StateError::EmptyEmulator));
    }

    #[test]
    fn tracking_and_slot_housekeeping() {
        let mut s = RomSaveState::default();
        s.adopt_baseline("a", baseline(1, "h")).unwrap();
        s.adopt_baseline("b", baseline(2, "h")).unwrap();
        s.delete_file_tracking("a");
        s.delete_file_tracking("nope");
        assert_eq!(s.files.keys().collect::<Vec<_>>(), vec!["b"]);
        s.clear_baselines();
        assert!(s.files.is_empty());
        s.confirm_slot("x").unwrap();
        s.delete_slot_tracking("x");
        assert!(s.slots.is_empty());
        s.mark_sync_evaluated("2026-01-01T00:00:00Z");
        assert_eq!(s.last_sync_check_at.as_deref(), Some("2026-01-01T00:00:00Z"));
        s.refresh_slot_listing(BTreeMap::from([("z".to_string(), json!({}))]));
        assert!(s.slots.contains_key("z"));
    }
}
