//! Pure save-sync display: the status/label the game page shows and the per-file status rows (port of
//! `domain/save_status.py` and `domain/save_status_builders.py`). Time-relative text is the frontend's job: for the one
//! time-relative case the label is `None` and `last_sync_check_at` is passed through.
use serde_json::{json, Value};

use crate::domain::{
    iso_time,
    sync_action::{ServerSave, SyncAction},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveSyncDisplay {
    /// `synced`, `conflict` or `none`.
    pub status: &'static str,
    pub label: Option<String>,
    pub last_sync_check_at: Option<String>,
}

impl SaveSyncDisplay {
    pub fn to_json(&self) -> Value {
        json!({"status": self.status, "label": self.label, "last_sync_check_at": self.last_sync_check_at})
    }
}

fn fixed(status: &'static str, label: &str) -> SaveSyncDisplay {
    SaveSyncDisplay { status, label: Some(label.to_string()), last_sync_check_at: None }
}

/// Status and label from the file rows (`status`, `local_path` keys). A failed server query never reports a false "synced".
pub fn compute_save_sync_display(files: &[Value], last_sync_check_at: Option<&str>, server_query_failed: bool) -> SaveSyncDisplay {
    if server_query_failed {
        return fixed("none", "Server unreachable");
    }
    if files.is_empty() {
        return fixed("none", "No saves");
    }
    if files.iter().any(|f| f["status"] == "conflict") {
        return fixed("conflict", "Conflict");
    }
    let has_local = files.iter().any(|f| f["local_path"].as_str().is_some_and(|p| !p.is_empty()) || matches!(f["status"].as_str(), Some("synced" | "upload")));
    if has_local {
        return match last_sync_check_at.filter(|t| !t.is_empty()) {
            Some(at) => SaveSyncDisplay { status: "synced", label: None, last_sync_check_at: Some(at.to_string()) },
            None => fixed("synced", "Not synced"),
        };
    }
    fixed("none", "No local saves")
}

/// The legacy file-status string of a decision.
pub fn status_from_action(action: &SyncAction) -> &'static str {
    match action {
        SyncAction::Skip { .. } => "synced",
        SyncAction::Upload { .. } => "upload",
        SyncAction::Download(_) => "download",
        SyncAction::Conflict(_) => "conflict",
    }
}

/// The server save shown next to a file row: the decision's own save for download/conflict, none for a fresh upload, else
/// the newest candidate (a synced row keeps showing the server's metadata).
pub fn resolve_chosen_server<'a>(action: &'a SyncAction, candidates: &'a [ServerSave]) -> Option<&'a ServerSave> {
    match action {
        SyncAction::Download(s) | SyncAction::Conflict(s) => Some(s),
        SyncAction::Upload { target_save_id: None } => None,
        _ => {
            let mut best: Option<&ServerSave> = None;
            let mut best_epoch = f64::NEG_INFINITY;
            for s in candidates {
                let e = s.updated_at.as_deref().and_then(iso_time::parse).map_or(0.0, |t| t.seconds);
                if e > best_epoch {
                    best = Some(s);
                    best_epoch = e;
                }
            }
            best
        }
    }
}

/// What the frontend's `SaveFileStatus` carries for one file.
pub struct FileStatusInput<'a> {
    pub filename: &'a str,
    pub local_path: Option<&'a str>,
    pub local_hash: Option<&'a str>,
    pub local_mtime: Option<String>,
    pub local_size: Option<u64>,
    pub server: Option<&'a ServerSave>,
    pub last_sync_at: Option<&'a str>,
    pub status: &'a str,
    pub own_device_id: Option<&'a str>,
    pub uploaded_by_us: Option<bool>,
}

pub fn build_file_status(i: FileStatusInput<'_>) -> Value {
    let raw = i.server.map(|s| &s.raw);
    let device_syncs: Vec<Value> = raw
        .and_then(|r| r["device_syncs"].as_array())
        .map(|list| {
            list.iter()
                .map(|d| {
                    json!({
                        "device_id": d["device_id"].as_str().unwrap_or(""),
                        "device_name": d["device_name"].as_str().unwrap_or(""),
                        "is_current": d["is_current"].as_bool().unwrap_or(false),
                        "last_synced_at": d["last_synced_at"],
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let is_current = i
        .own_device_id
        .and_then(|own| device_syncs.iter().find(|d| d["device_id"] == own))
        .map_or(true, |d| d["is_current"].as_bool().unwrap_or(false));
    json!({
        "filename": i.filename,
        "local_path": i.local_path,
        "local_hash": i.local_hash,
        "local_mtime": i.local_mtime,
        "local_size": i.local_size,
        "server_save_id": i.server.and_then(|s| s.id),
        "server_file_name": raw.and_then(|r| r["file_name"].as_str()),
        "server_emulator": raw.and_then(|r| r["emulator"].as_str()),
        "server_updated_at": i.server.map(|s| s.updated_at.clone().unwrap_or_default()),
        "server_size": raw.and_then(|r| r["file_size_bytes"].as_i64()),
        "last_sync_at": i.last_sync_at,
        "status": i.status,
        "device_syncs": device_syncs,
        "is_current": is_current,
        "uploaded_by_us": i.uploaded_by_us,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::sync_action::SkipReason;

    fn file(status: &str, path: Option<&str>) -> Value {
        json!({"status": status, "local_path": path})
    }
    fn save(id: i64, updated: &str) -> ServerSave {
        ServerSave::from_json(&json!({"id": id, "updated_at": updated, "file_name": "T.zip", "emulator": "eden", "file_size_bytes": 12,
            "device_syncs": [{"device_id": "me", "device_name": "Deck", "is_current": false, "last_synced_at": "t"}]}))
    }

    #[test]
    fn display_follows_the_reference_rules_in_order() {
        let d = |files: &[Value], at: Option<&str>, failed| compute_save_sync_display(files, at, failed);
        assert_eq!(d(&[file("synced", Some("/p"))], Some("t"), true), fixed("none", "Server unreachable"), "a failed query beats everything");
        assert_eq!(d(&[], Some("t"), false), fixed("none", "No saves"));
        assert_eq!(d(&[file("synced", Some("/p")), file("conflict", None)], Some("t"), false), fixed("conflict", "Conflict"));
        assert_eq!(d(&[file("synced", Some("/p"))], Some("2026-01-01T00:00:00Z"), false), SaveSyncDisplay { status: "synced", label: None, last_sync_check_at: Some("2026-01-01T00:00:00Z".into()) });
        assert_eq!(d(&[file("upload", None)], None, false), fixed("synced", "Not synced"));
        assert_eq!(d(&[file("download", None)], Some("t"), false), fixed("none", "No local saves"));
        assert_eq!(d(&[file("download", Some(""))], Some("t"), false), fixed("none", "No local saves"), "an empty path is no local file");
    }

    #[test]
    fn the_file_status_string_follows_the_action() {
        let s = save(1, "2024-01-01T00:00:00+00:00");
        assert_eq!(status_from_action(&SyncAction::Skip { reason: SkipReason::Synced, adopt_baseline: false }), "synced");
        assert_eq!(status_from_action(&SyncAction::Upload { target_save_id: None }), "upload");
        assert_eq!(status_from_action(&SyncAction::Download(s.clone())), "download");
        assert_eq!(status_from_action(&SyncAction::Conflict(s)), "conflict");
    }

    #[test]
    fn the_chosen_server_save_is_the_actions_own_or_the_newest_candidate() {
        let old = save(1, "2024-01-01T00:00:00+00:00");
        let new = save(2, "2025-01-01T00:00:00+00:00");
        let both = [old.clone(), new.clone()];
        assert_eq!(resolve_chosen_server(&SyncAction::Download(old.clone()), &both).unwrap().id, Some(1));
        assert_eq!(resolve_chosen_server(&SyncAction::Conflict(old.clone()), &both).unwrap().id, Some(1));
        assert!(resolve_chosen_server(&SyncAction::Upload { target_save_id: None }, &both).is_none());
        assert_eq!(resolve_chosen_server(&SyncAction::Upload { target_save_id: Some(1) }, &both).unwrap().id, Some(2));
        assert_eq!(resolve_chosen_server(&SyncAction::Skip { reason: SkipReason::Synced, adopt_baseline: false }, &both).unwrap().id, Some(2));
        assert!(resolve_chosen_server(&SyncAction::Skip { reason: SkipReason::NothingToSync, adopt_baseline: false }, &[]).is_none());
    }

    #[test]
    fn a_file_row_carries_the_servers_metadata_and_this_devices_currency() {
        let s = save(3, "2025-01-01T00:00:00+00:00");
        let row = build_file_status(FileStatusInput { filename: "T.zip", local_path: Some("/p"), local_hash: Some("h"), local_mtime: Some("m".into()), local_size: Some(5), server: Some(&s), last_sync_at: Some("t"), status: "download", own_device_id: Some("me"), uploaded_by_us: Some(true) });
        assert_eq!((row["server_save_id"].clone(), row["server_file_name"].clone(), row["server_emulator"].clone(), row["server_size"].clone()), (json!(3), json!("T.zip"), json!("eden"), json!(12)));
        assert_eq!(row["device_syncs"][0]["device_name"], "Deck");
        assert_eq!(row["is_current"], false, "this device's entry says it is behind");
        let other = build_file_status(FileStatusInput { filename: "T.zip", local_path: None, local_hash: None, local_mtime: None, local_size: None, server: Some(&s), last_sync_at: None, status: "download", own_device_id: Some("someone else"), uploaded_by_us: None });
        assert_eq!(other["is_current"], true, "no entry for this device defaults to current");
        let none = build_file_status(FileStatusInput { filename: "T.zip", local_path: Some("/p"), local_hash: None, local_mtime: None, local_size: None, server: None, last_sync_at: None, status: "upload", own_device_id: None, uploaded_by_us: None });
        assert_eq!((none["server_save_id"].clone(), none["server_updated_at"].clone(), none["device_syncs"].clone()), (Value::Null, Value::Null, json!([])));
    }
}
