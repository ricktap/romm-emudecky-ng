//! The save-sync decision kernel: one action per save file (ADR-0017). A faithful port of the Python reference
//! (`domain/sync_action.py` at commit 5806808); it is the **only** authority on what to do, RomM's `negotiate` is transport.
//!
//! Inputs are the local file (if any) and its hash, the server saves of the slot, the recorded baseline of the file and
//! this device's id. Output is `Skip`, `Upload`, `Download` or `Conflict`. A present local of unknown provenance that
//! collides with a server head we never synced is always a `Conflict` (the user decides); a corrupt-looking local (0 byte or
//! shrunk below half its baseline) is never pushed over the only good copy. No I/O.
use serde_json::Value;

use crate::domain::iso_time;

/// A local save file as the engine saw it.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalFile {
    pub size: u64,
    /// Modification time, seconds since the epoch.
    pub mtime: f64,
}

/// What this device last synced for a file (the baseline).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileState {
    pub last_sync_hash: Option<String>,
    pub last_sync_local_size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSync {
    pub device_id: String,
    pub is_current: bool,
}

/// A save version on the server (the fields the decision needs, plus the raw JSON for the executor).
#[derive(Debug, Clone, PartialEq)]
pub struct ServerSave {
    pub id: Option<i64>,
    pub updated_at: Option<String>,
    pub content_hash: Option<String>,
    pub device_syncs: Vec<DeviceSync>,
    pub raw: Value,
}

impl ServerSave {
    pub fn from_json(raw: &Value) -> ServerSave {
        let device_syncs = raw["device_syncs"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|d| Some(DeviceSync { device_id: d["device_id"].as_str()?.to_string(), is_current: d["is_current"].as_bool().unwrap_or(false) }))
                    .collect()
            })
            .unwrap_or_default();
        ServerSave {
            id: raw["id"].as_i64(),
            updated_at: raw["updated_at"].as_str().map(str::to_string),
            content_hash: raw["content_hash"].as_str().map(str::to_string),
            device_syncs,
            raw: raw.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    Synced,
    NothingToSync,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SyncAction {
    /// Nothing to do. `adopt_baseline`: persist the current local hash as the file's baseline (state change only).
    Skip { reason: SkipReason, adopt_baseline: bool },
    /// Push local to the server as a new save version; `target_save_id` only echoes the head being superseded.
    Upload { target_save_id: Option<i64> },
    /// Adopt this server save.
    Download(ServerSave),
    /// Both sides changed (or provenance is unknown): the user decides.
    Conflict(ServerSave),
}

const SHRINK_RATIO: f64 = 0.5;

/// A local save that looks corrupt: exactly 0 bytes, or below half of a positive baseline size.
pub fn is_implausibly_shrunken(new_size: Option<u64>, baseline_size: Option<u64>) -> bool {
    match (new_size, baseline_size) {
        (None, _) => false,
        (Some(0), _) => true,
        (Some(_), None) | (Some(_), Some(0)) => false,
        (Some(new), Some(base)) => (new as f64) < (base as f64) * SHRINK_RATIO,
    }
}

fn epoch(text: Option<&str>) -> Option<f64> {
    iso_time::parse(text?).map(|t| t.seconds)
}

/// Local mtime at or after the server save's `updated_at`; any missing or garbled timestamp is `false` (the server wins).
fn local_not_older(local: &LocalFile, server: &ServerSave) -> bool {
    epoch(server.updated_at.as_deref()).is_some_and(|server_epoch| local.mtime >= server_epoch)
}

fn when_current(server: &ServerSave, local: Option<&LocalFile>, local_hash: Option<&str>, state: &FileState) -> SyncAction {
    let Some(local) = local else {
        // Recovery: the server still tracks our last version and the local copy is gone.
        return SyncAction::Download(server.clone());
    };
    let Some(baseline) = state.last_sync_hash.as_deref().filter(|h| !h.is_empty()) else {
        return SyncAction::Skip { reason: SkipReason::Synced, adopt_baseline: true };
    };
    if local_hash.is_some_and(|h| !h.is_empty() && h != baseline) {
        if is_implausibly_shrunken(Some(local.size), state.last_sync_local_size) {
            return SyncAction::Conflict(server.clone());
        }
        return SyncAction::Upload { target_save_id: server.id };
    }
    SyncAction::Skip { reason: SkipReason::Synced, adopt_baseline: false }
}

fn when_not_current(server: &ServerSave, local: Option<&LocalFile>, local_hash: Option<&str>, state: &FileState) -> SyncAction {
    if local.is_none() {
        return SyncAction::Download(server.clone());
    }
    let Some(baseline) = state.last_sync_hash.as_deref().filter(|h| !h.is_empty()) else {
        // No baseline: only a byte-identical local may be replaced silently.
        let identical = matches!((server.content_hash.as_deref(), local_hash), (Some(s), Some(l)) if !s.is_empty() && !l.is_empty() && s == l);
        return if identical { SyncAction::Download(server.clone()) } else { SyncAction::Conflict(server.clone()) };
    };
    if local_hash.is_some_and(|h| !h.is_empty() && h != baseline) {
        return SyncAction::Conflict(server.clone());
    }
    SyncAction::Download(server.clone())
}

fn when_no_entry(server: &ServerSave, local: Option<&LocalFile>, local_hash: Option<&str>, state: &FileState) -> SyncAction {
    let Some(local) = local else {
        return SyncAction::Download(server.clone());
    };
    let baseline = state.last_sync_hash.as_deref().filter(|h| !h.is_empty());
    let local_hash = local_hash.filter(|h| !h.is_empty());
    if let (Some(s), Some(l)) = (server.content_hash.as_deref().filter(|h| !h.is_empty()), local_hash) {
        if s == l {
            // Byte-identical to a head we never synced: adopt it instead of posting a duplicate.
            return SyncAction::Skip { reason: SkipReason::Synced, adopt_baseline: true };
        }
    }
    if let (Some(b), Some(l)) = (baseline, local_hash) {
        if l != b {
            return SyncAction::Conflict(server.clone());
        }
    }
    if baseline.is_none() && local_hash.is_some() {
        return SyncAction::Conflict(server.clone());
    }
    if local_not_older(local, server) {
        SyncAction::Upload { target_save_id: None }
    } else {
        SyncAction::Download(server.clone())
    }
}

/// The action for one `(rom, filename, slot)`. `server_saves` are the saves of that slot only.
pub fn compute_sync_action(
    local: Option<&LocalFile>,
    server_saves: &[ServerSave],
    state: &FileState,
    device_id: &str,
    local_hash: Option<&str>,
) -> SyncAction {
    // 1. Nothing on the server.
    if server_saves.is_empty() {
        return if local.is_some() { SyncAction::Upload { target_save_id: None } } else { SyncAction::Skip { reason: SkipReason::NothingToSync, adopt_baseline: false } };
    }
    // 2. The newest save by `updated_at` (unparseable sorts to the bottom; the first of equals wins).
    let mut head = &server_saves[0];
    let mut head_epoch = epoch(head.updated_at.as_deref()).unwrap_or(0.0);
    for save in &server_saves[1..] {
        let e = epoch(save.updated_at.as_deref()).unwrap_or(0.0);
        if e > head_epoch {
            head = save;
            head_epoch = e;
        }
    }
    // 3. Branch on this device's entry on the head.
    match head.device_syncs.iter().find(|d| d.device_id == device_id) {
        Some(entry) if entry.is_current => when_current(head, local, local_hash, state),
        Some(_) => when_not_current(head, local, local_hash, state),
        None => when_no_entry(head, local, local_hash, state),
    }
}

/// What to do after RomM answers an upload with `409`: adopt the server's save when local is provably unchanged (equal to
/// our baseline) or already byte-identical to the server, otherwise a conflict. Missing or empty information never
/// downloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadConflict {
    Download,
    Conflict,
}

pub fn resolve_upload_conflict(local_hash: Option<&str>, last_sync_hash: Option<&str>, server_content_hash: Option<&str>) -> UploadConflict {
    let local = local_hash.filter(|h| !h.is_empty());
    let eq = |other: Option<&str>| matches!((local, other.filter(|h| !h.is_empty())), (Some(l), Some(o)) if l == o);
    if eq(last_sync_hash) || eq(server_content_hash) {
        UploadConflict::Download
    } else {
        UploadConflict::Conflict
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ME: &str = "device-abc";
    const OTHER: &str = "device-xyz";

    fn local(size: u64, mtime: f64) -> LocalFile {
        LocalFile { size, mtime }
    }
    fn server(id: i64, updated: &str, devices: &[(&str, bool)], hash: Option<&str>) -> ServerSave {
        let mut v = json!({"id": id, "slot": 0, "updated_at": updated,
            "device_syncs": devices.iter().map(|(d, c)| json!({"device_id": d, "is_current": c})).collect::<Vec<_>>()});
        if let Some(h) = hash {
            v["content_hash"] = json!(h);
        }
        ServerSave::from_json(&v)
    }
    fn state(hash: Option<&str>, size: Option<u64>) -> FileState {
        FileState { last_sync_hash: hash.map(str::to_string), last_sync_local_size: size }
    }
    const T0: &str = "2024-01-01T12:00:00+00:00";
    const OLD: f64 = 1_600_000_000.0; // 2020
    const NEW: f64 = 1_800_000_000.0; // 2027
    fn skip(adopt: bool) -> SyncAction {
        SyncAction::Skip { reason: SkipReason::Synced, adopt_baseline: adopt }
    }

    #[test]
    fn row_1_an_empty_slot_uploads_a_local_file_or_has_nothing_to_do() {
        assert_eq!(compute_sync_action(None, &[], &state(None, None), ME, None), SyncAction::Skip { reason: SkipReason::NothingToSync, adopt_baseline: false });
        assert_eq!(compute_sync_action(Some(&local(8, NEW)), &[], &state(None, None), ME, Some("abc")), SyncAction::Upload { target_save_id: None });
    }

    #[test]
    fn the_newest_save_by_updated_at_is_the_head_and_garbled_timestamps_sort_last() {
        let old = server(1, "2024-01-01T00:00:00+00:00", &[(ME, true)], None);
        let newer = server(2, "2025-01-01T00:00:00+00:00", &[(ME, false)], None);
        let garbled = server(3, "not a date", &[(ME, true)], None);
        // head = #2 (not current) -> with no local: download it
        assert_eq!(compute_sync_action(None, &[old.clone(), garbled, newer.clone()], &state(None, None), ME, None), SyncAction::Download(newer));
        // ties: the first wins
        let a = server(10, T0, &[(ME, false)], None);
        let b = server(11, T0, &[(ME, false)], None);
        assert_eq!(compute_sync_action(None, &[a.clone(), b], &state(None, None), ME, None), SyncAction::Download(a));
    }

    #[test]
    fn current_and_unchanged_is_synced() {
        let s = server(1, T0, &[(ME, true)], None);
        assert_eq!(compute_sync_action(Some(&local(8192, OLD)), &[s], &state(Some("abc"), None), ME, Some("abc")), skip(false));
    }

    #[test]
    fn current_with_no_local_recovers_by_downloading() {
        let s = server(1, T0, &[(ME, true)], None);
        assert_eq!(compute_sync_action(None, &[s.clone()], &state(Some("abc"), None), ME, None), SyncAction::Download(s));
    }

    #[test]
    fn current_without_a_baseline_adopts_one() {
        let s = server(1, T0, &[(ME, true)], None);
        assert_eq!(compute_sync_action(Some(&local(8, OLD)), &[s.clone()], &state(None, None), ME, Some("abc")), skip(true));
        assert_eq!(compute_sync_action(Some(&local(8, OLD)), &[s], &state(Some(""), None), ME, Some("abc")), skip(true), "an empty baseline is no baseline");
    }

    #[test]
    fn current_and_diverged_uploads_the_offline_edit_echoing_the_head() {
        let s = server(42, T0, &[(ME, true)], None);
        assert_eq!(compute_sync_action(Some(&local(8192, NEW)), &[s], &state(Some("abc"), Some(8192)), ME, Some("DIFFERENT")), SyncAction::Upload { target_save_id: Some(42) });
    }

    #[test]
    fn current_and_diverged_but_corrupt_looking_is_a_conflict_never_an_upload() {
        let s = server(42, T0, &[(ME, true)], None);
        // 0 bytes: unconditional, even without a baseline size
        assert_eq!(compute_sync_action(Some(&local(0, NEW)), &[s.clone()], &state(Some("abc"), None), ME, Some("EMPTY")), SyncAction::Conflict(s.clone()));
        // below half of the baseline size
        assert_eq!(compute_sync_action(Some(&local(100, NEW)), &[s.clone()], &state(Some("abc"), Some(8192)), ME, Some("X")), SyncAction::Conflict(s.clone()));
        // exactly half is plausible
        assert_eq!(compute_sync_action(Some(&local(4096, NEW)), &[s], &state(Some("abc"), Some(8192)), ME, Some("X")), SyncAction::Upload { target_save_id: Some(42) });
    }

    #[test]
    fn not_current_without_local_downloads() {
        let s = server(1, T0, &[(ME, false)], None);
        assert_eq!(compute_sync_action(None, &[s.clone()], &state(Some("abc"), None), ME, None), SyncAction::Download(s));
    }

    #[test]
    fn not_current_with_local_unchanged_since_baseline_downloads() {
        let s = server(1, T0, &[(ME, false)], None);
        assert_eq!(compute_sync_action(Some(&local(8, NEW)), &[s.clone()], &state(Some("abc"), None), ME, Some("abc")), SyncAction::Download(s.clone()));
        assert_eq!(compute_sync_action(Some(&local(8, NEW)), &[s.clone()], &state(Some("abc"), None), ME, None), SyncAction::Download(s), "unknown local hash cannot prove a change");
    }

    #[test]
    fn not_current_and_local_changed_is_the_true_conflict() {
        let s = server(1, T0, &[(ME, false)], None);
        assert_eq!(compute_sync_action(Some(&local(8, NEW)), &[s.clone()], &state(Some("abc"), None), ME, Some("changed")), SyncAction::Conflict(s));
    }

    #[test]
    fn not_current_without_baseline_downloads_only_a_byte_identical_local() {
        let same = server(1, T0, &[(ME, false)], Some("H"));
        assert_eq!(compute_sync_action(Some(&local(8, NEW)), &[same.clone()], &state(None, None), ME, Some("H")), SyncAction::Download(same));
        let different = server(1, T0, &[(ME, false)], Some("H"));
        assert_eq!(compute_sync_action(Some(&local(8, NEW)), &[different.clone()], &state(None, None), ME, Some("OTHER")), SyncAction::Conflict(different));
        let no_server_hash = server(1, T0, &[(ME, false)], None);
        assert_eq!(compute_sync_action(Some(&local(8, NEW)), &[no_server_hash.clone()], &state(None, None), ME, Some("H")), SyncAction::Conflict(no_server_hash));
    }

    #[test]
    fn no_entry_and_no_local_downloads() {
        let s = server(1, T0, &[(OTHER, true)], None);
        assert_eq!(compute_sync_action(None, &[s.clone()], &state(None, None), ME, None), SyncAction::Download(s));
    }

    #[test]
    fn no_entry_byte_identical_local_adopts_the_head_instead_of_a_duplicate() {
        let s = server(1, T0, &[(OTHER, true)], Some("H"));
        assert_eq!(compute_sync_action(Some(&local(8, NEW)), &[s], &state(None, None), ME, Some("H")), skip(true));
    }

    #[test]
    fn no_entry_with_a_baseline_that_local_diverged_from_is_a_conflict() {
        let s = server(1, T0, &[(OTHER, true)], Some("H"));
        assert_eq!(compute_sync_action(Some(&local(8, NEW)), &[s.clone()], &state(Some("abc"), None), ME, Some("changed")), SyncAction::Conflict(s));
    }

    #[test]
    fn no_entry_no_baseline_and_a_differing_local_is_a_conflict_in_both_mtime_directions() {
        let s = server(1, T0, &[(OTHER, true)], Some("H"));
        assert_eq!(compute_sync_action(Some(&local(8, NEW)), &[s.clone()], &state(None, None), ME, Some("local")), SyncAction::Conflict(s.clone()), "local newer");
        assert_eq!(compute_sync_action(Some(&local(8, OLD)), &[s.clone()], &state(None, None), ME, Some("local")), SyncAction::Conflict(s), "local older");
    }

    #[test]
    fn the_mtime_path_runs_only_when_the_local_hash_is_unknown_or_a_baseline_proves_local_unchanged() {
        let s = server(1, T0, &[(OTHER, true)], None);
        // unknown hash, local newer or equal -> upload; older -> download
        assert_eq!(compute_sync_action(Some(&local(8, NEW)), &[s.clone()], &state(None, None), ME, None), SyncAction::Upload { target_save_id: None });
        assert_eq!(compute_sync_action(Some(&local(8, OLD)), &[s.clone()], &state(None, None), ME, None), SyncAction::Download(s.clone()));
        // a baseline that equals the local hash: unchanged since our baseline, mtime decides
        assert_eq!(compute_sync_action(Some(&local(8, NEW)), &[s.clone()], &state(Some("abc"), None), ME, Some("abc")), SyncAction::Upload { target_save_id: None });
        let equal = iso_time::parse(T0).unwrap().seconds;
        assert_eq!(compute_sync_action(Some(&local(8, equal)), &[s], &state(None, None), ME, None), SyncAction::Upload { target_save_id: None }, "equal mtime counts as newer-or-equal");
    }

    #[test]
    fn garbled_server_timestamps_make_the_server_win_on_the_mtime_path() {
        let s = server(1, "garbage", &[(OTHER, true)], None);
        assert_eq!(compute_sync_action(Some(&local(8, NEW)), &[s.clone()], &state(None, None), ME, None), SyncAction::Download(s));
    }

    #[test]
    fn shrink_guard_values() {
        assert!(!is_implausibly_shrunken(None, Some(100)));
        assert!(is_implausibly_shrunken(Some(0), None));
        assert!(!is_implausibly_shrunken(Some(5), None));
        assert!(!is_implausibly_shrunken(Some(5), Some(0)));
        assert!(is_implausibly_shrunken(Some(49), Some(100)));
        assert!(!is_implausibly_shrunken(Some(50), Some(100)));
    }

    #[test]
    fn a_409_downloads_only_when_local_is_provably_unchanged() {
        use UploadConflict::*;
        assert_eq!(resolve_upload_conflict(Some("a"), Some("a"), None), Download, "unchanged since baseline");
        assert_eq!(resolve_upload_conflict(Some("a"), Some("b"), Some("a")), Download, "already byte-identical to the server");
        assert_eq!(resolve_upload_conflict(Some("a"), Some("b"), Some("c")), Conflict);
        for (l, b, s) in [(None, Some("a"), Some("a")), (Some(""), Some(""), Some("")), (Some("a"), None, None), (None, None, None), (Some("a"), Some(""), Some(""))] {
            assert_eq!(resolve_upload_conflict(l, b, s), Conflict, "{l:?} {b:?} {s:?}: missing information never downloads");
        }
    }

    #[test]
    fn a_server_save_is_read_from_romm_json_tolerantly() {
        let s = ServerSave::from_json(&json!({"id": 7, "updated_at": T0, "content_hash": "H", "device_syncs": [{"device_id": "x", "is_current": true}, {"is_current": true}, {"device_id": "y"}]}));
        assert_eq!(s.id, Some(7));
        assert_eq!(s.device_syncs, vec![DeviceSync { device_id: "x".into(), is_current: true }, DeviceSync { device_id: "y".into(), is_current: false }]);
        let bare = ServerSave::from_json(&json!({}));
        assert_eq!((bare.id, bare.updated_at, bare.content_hash, bare.device_syncs.len()), (None, None, None, 0));
    }

    /// A timestamp without a zone counts as UTC (the Python reference used the host's local zone there; RomM always sends a
    /// zone, so the sample below has none without one).
    #[test]
    fn a_timestamp_without_a_zone_is_utc_not_the_hosts_zone() {
        let naive = server(1, "2024-01-01T12:00:00", &[(OTHER, true)], None);
        let aware = server(2, "2024-01-01T12:00:00+00:00", &[(OTHER, true)], None);
        let epoch = iso_time::parse(T0).unwrap().seconds;
        assert_eq!(compute_sync_action(Some(&local(8, epoch)), &[naive], &state(None, None), ME, None), SyncAction::Upload { target_save_id: None });
        assert_eq!(compute_sync_action(Some(&local(8, epoch - 1.0)), &[aware.clone()], &state(None, None), ME, None), SyncAction::Download(aware));
    }

    /// The Python reference (commit 5806808) run on 1,500 random scenarios and 125 upload-conflict combinations: the port
    /// must decide exactly the same, case by case.
    #[test]
    fn it_decides_exactly_like_the_python_reference_on_random_scenarios() {
        let fixture: Value = serde_json::from_str(include_str!("../../tests/fixtures/sync_action_reference.json")).unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for (n, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
            let local = case["local"].as_object().map(|l| LocalFile { size: l["size"].as_u64().unwrap(), mtime: l["mtime"].as_f64().unwrap() });
            let saves: Vec<ServerSave> = case["saves"].as_array().unwrap().iter().map(ServerSave::from_json).collect();
            let state = FileState {
                last_sync_hash: case["state"]["last_sync_hash"].as_str().map(str::to_string),
                last_sync_local_size: case["state"]["last_sync_local_size"].as_u64(),
            };
            let got = compute_sync_action(local.as_ref(), &saves, &state, "me", case["hash"].as_str());
            let want = &case["expect"];
            let ok = match (&got, want["kind"].as_str().unwrap()) {
                (SyncAction::Skip { reason, adopt_baseline }, "skip") => {
                    let name = if *reason == SkipReason::Synced { "synced" } else { "nothing_to_sync" };
                    name == want["reason"].as_str().unwrap() && *adopt_baseline == want["adopt"].as_bool().unwrap()
                }
                (SyncAction::Upload { target_save_id }, "upload") => *target_save_id == want["target"].as_i64(),
                (SyncAction::Download(s), "download") => s.id == want["id"].as_i64(),
                (SyncAction::Conflict(s), "conflict") => s.id == want["id"].as_i64(),
                _ => false,
            };
            assert!(ok, "case {n}: got {got:?}, the reference says {want}\n{case}");
            seen.insert(want["kind"].as_str().unwrap().to_string());
        }
        assert_eq!(seen.len(), 4, "the sample must exercise every kind of action");
        for case in fixture["upload_conflict"].as_array().unwrap() {
            let got = resolve_upload_conflict(case["local"].as_str(), case["baseline"].as_str(), case["server"].as_str());
            let want = if case["expect"] == "download" { UploadConflict::Download } else { UploadConflict::Conflict };
            assert_eq!(got, want, "{case}");
        }
    }
}
