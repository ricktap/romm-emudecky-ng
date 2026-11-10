//! Where a ROM's files land on disk, and whether files already there match RomM.
//!
//! The single source of truth for the on-disk names the download path writes
//! (the target file name of a single-file ROM, the extract-directory name of a
//! multi-file ROM) and for the "is this ROM already on disk?" decision that
//! adoption makes. Both derive their paths from [`resolve_download_names`] so
//! the two can never diverge. Pure compute: the caller joins the returned
//! single safe components onto the platform directory and reports sizes back.

use std::collections::HashMap;

use serde_json::Value;

use crate::domain::rom_files::{
    is_multi_file_download, resolve_extract_dir_name, resolve_local_file_name, synthetic_rom_name,
};
use crate::domain::rom_install::RomInstall;
use crate::util::path_safety::{coerce_safe_component, safe_path_component, unquote};

/// The sanitized on-disk names the download path uses for one ROM.
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadNames {
    pub file_name: String,
    pub raw_file_name: String,
    pub files_missing: bool,
    pub is_multi: bool,
    pub extract_dir_name: String,
    pub raw_extract_dir_name: String,
}

impl DownloadNames {
    pub fn file_name_changed(&self) -> bool {
        self.file_name != self.raw_file_name
    }
    pub fn extract_dir_name_changed(&self) -> bool {
        self.extract_dir_name != self.raw_extract_dir_name
    }
}

/// Resolve the sanitized target names for `rom` (download + adoption).
pub fn resolve_download_names(rom: &Value) -> DownloadNames {
    let fallback = synthetic_rom_name(rom);
    let (raw_file_name, files_missing) = resolve_local_file_name(rom);
    let (file_name, _) = coerce_safe_component(&raw_file_name, &fallback);
    let raw_extract = resolve_extract_dir_name(rom);
    let (extract_dir_name, _) = coerce_safe_component(&raw_extract, &fallback);
    DownloadNames {
        file_name,
        raw_file_name,
        files_missing,
        is_multi: is_multi_file_download(rom),
        extract_dir_name,
        raw_extract_dir_name: raw_extract,
    }
}

/// One file of a multi-file ROM: `rel_path` (`/`-separated) inside the install dir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedFile {
    pub rel_path: String,
    pub size: u64,
}

impl ExpectedFile {
    pub fn parts(&self) -> Vec<&str> {
        self.rel_path.split('/').collect()
    }
}

/// Where the downloader would put a ROM's files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallLayout {
    /// One file named `file_name` of `size` bytes in the platform dir.
    Single { file_name: String, size: u64 },
    /// Every expected file inside one of `dir_candidates` (the extract dir
    /// name first, then the ES-DE collapse names a download may rename to).
    Multi {
        dir_candidates: Vec<String>,
        files: Vec<ExpectedFile>,
    },
}

fn non_negative_int(v: Option<&Value>) -> Option<u64> {
    match v {
        Some(Value::Number(n)) => n.as_u64(),
        _ => None,
    }
}

fn rom_root(rom: &Value) -> Option<String> {
    let fs_path = rom.get("fs_path").and_then(Value::as_str).filter(|s| !s.is_empty())?;
    let fs_name = rom.get("fs_name").and_then(Value::as_str).filter(|s| !s.is_empty())?;
    Some(crate::util::path_safety::join(fs_path, &[fs_name]))
}

/// Path of one RomM file entry relative to the ROM's own folder, URL-decoded
/// per component like the extraction; `None` when outside the folder or unsafe.
fn relative_file_path(entry: &Value, rom_root: &str) -> Option<String> {
    let file_path = entry.get("file_path")?.as_str()?;
    let file_name = entry.get("file_name")?.as_str().filter(|s| !s.is_empty())?;
    let full = if file_path.is_empty() {
        file_name.to_string()
    } else {
        crate::util::path_safety::join(file_path, &[file_name])
    };
    let prefix = format!("{}/", rom_root.trim_end_matches('/'));
    let rel = full.strip_prefix(&prefix)?;
    let mut decoded = Vec::new();
    for part in rel.split('/') {
        let d = unquote(part);
        safe_path_component(&d).ok()?;
        decoded.push(d);
    }
    Some(decoded.join("/"))
}

fn multi_file_layout(rom: &Value, names: &DownloadNames) -> Option<InstallLayout> {
    let files = rom.get("files").and_then(Value::as_array).filter(|f| !f.is_empty())?;
    let root = rom_root(rom)?;
    let mut expected = Vec::new();
    for entry in files {
        if !entry.is_object() {
            return None;
        }
        let rel = relative_file_path(entry, &root)?;
        let size = non_negative_int(entry.get("file_size_bytes"))?;
        expected.push(ExpectedFile { rel_path: rel, size });
    }
    let mut candidates = vec![names.extract_dir_name.clone()];
    let mut top_level: Vec<String> = expected
        .iter()
        .filter(|f| !f.rel_path.contains('/'))
        .map(|f| f.rel_path.clone())
        .collect();
    let stem = match rom.get("fs_name_no_ext") {
        Some(v) => v.as_str(),
        None => match rom.get("name") {
            Some(v) => v.as_str(),
            None => Some("playlist"),
        },
    };
    if let Some(stem) = stem.filter(|s| !s.is_empty()) {
        top_level.push(format!("{stem}.m3u"));
    }
    for name in top_level {
        let (safe, changed) = coerce_safe_component(&name, "");
        if !changed && !safe.is_empty() && !candidates.contains(&safe) {
            candidates.push(safe);
        }
    }
    Some(InstallLayout::Multi {
        dir_candidates: candidates,
        files: expected,
    })
}

/// Where the downloader would put `rom`'s files, or `None` if not determinable
/// (single-file without a positive RomM size; multi-file with an entry lacking
/// a size, outside the ROM folder, or carrying an unsafe name).
pub fn expected_install_layout(rom: &Value) -> Option<InstallLayout> {
    let names = resolve_download_names(rom);
    if names.is_multi {
        return multi_file_layout(rom, &names);
    }
    let size = non_negative_int(rom.get("fs_size_bytes")).filter(|s| *s > 0)?;
    Some(InstallLayout::Single {
        file_name: names.file_name,
        size,
    })
}

/// True when a regular file was observed with exactly RomM's size.
pub fn single_file_matches(size: u64, observed: Option<u64>) -> bool {
    observed == Some(size)
}

/// True when every expected file was observed (by `rel_path`) with its exact size.
pub fn multi_file_matches(files: &[ExpectedFile], observed: &HashMap<String, Option<u64>>) -> bool {
    files
        .iter()
        .all(|f| observed.get(&f.rel_path).copied().flatten() == Some(f.size))
}

/// A ROM whose files were found on disk where a download would put them.
#[derive(Debug, Clone, PartialEq)]
pub struct AdoptionCandidate {
    pub rom_id: i64,
    pub install: RomInstall,
    pub bake_path: String,
    pub group_key: Option<String>,
}

/// Keep at most one candidate per sibling group (lowest `rom_id` wins);
/// candidates without a group key are all kept. Sorted by `rom_id`.
pub fn pick_one_per_group(mut candidates: Vec<AdoptionCandidate>) -> Vec<AdoptionCandidate> {
    candidates.sort_by_key(|c| c.rom_id);
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|c| match &c.group_key {
        None => true,
        Some(key) => seen.insert(key.clone()),
    });
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn single(over: Value) -> Value {
        let mut rom = json!({
            "id": 7, "fs_name": "Pokemon (USA).gba", "fs_size_bytes": 1024,
            "files": [{"file_name": "Pokemon (USA).gba"}],
        });
        for (k, v) in over.as_object().unwrap() {
            rom[k] = v.clone();
        }
        rom
    }

    fn multi(files: Vec<Value>, over: Value) -> Value {
        let mut rom = json!({
            "id": 9, "name": "Final Fantasy VII", "fs_path": "psx",
            "fs_name": "Final Fantasy VII (USA)", "fs_name_no_ext": "Final Fantasy VII (USA)",
            "has_multiple_files": true, "files": files,
        });
        for (k, v) in over.as_object().unwrap() {
            rom[k] = v.clone();
        }
        rom
    }

    fn f(rel: &str, size: u64) -> Value {
        let full = format!("psx/Final Fantasy VII (USA)/{rel}");
        let (path, name) = full.rsplit_once('/').unwrap();
        json!({"file_path": path, "file_name": name, "file_size_bytes": size})
    }

    fn observed(pairs: &[(&str, Option<u64>)]) -> HashMap<String, Option<u64>> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn download_names() {
        let n = resolve_download_names(&single(json!({})));
        assert_eq!(n.file_name, "Pokemon (USA).gba");
        assert!(!n.file_name_changed());
        assert!(!n.is_multi);
        let nested = json!({"id": 1, "fs_name": "Zelda (USA)", "has_nested_single_file": true,
                            "files": [{"file_name": "Zelda.nsp"}]});
        assert_eq!(resolve_download_names(&nested).file_name, "Zelda.nsp");
        let t = resolve_download_names(&single(json!({"fs_name": "../../etc/passwd"})));
        assert_eq!(t.file_name, "passwd");
        assert!(t.file_name_changed());
        assert_eq!(resolve_download_names(&single(json!({"fs_name": ".."}))).file_name, "rom_7");
        let m = resolve_download_names(&multi(vec![f("a.cue", 1), f("a.bin", 2)], json!({})));
        assert!(m.is_multi);
        assert_eq!(m.extract_dir_name, "Final Fantasy VII (USA)");
        assert!(!m.extract_dir_name_changed());
    }

    #[test]
    fn single_layout() {
        let layout = expected_install_layout(&single(json!({}))).unwrap();
        assert_eq!(
            layout,
            InstallLayout::Single {
                file_name: "Pokemon (USA).gba".into(),
                size: 1024
            }
        );
        assert!(single_file_matches(1024, Some(1024)));
        assert!(!single_file_matches(1024, Some(1023)));
        assert!(!single_file_matches(1024, None));
        assert_eq!(expected_install_layout(&single(json!({"fs_size_bytes": null}))), None);
        let mut rom = single(json!({}));
        rom.as_object_mut().unwrap().remove("fs_size_bytes");
        assert_eq!(expected_install_layout(&rom), None);
        for bogus in [json!(0), json!("1024"), json!(true), json!(10.5)] {
            assert_eq!(expected_install_layout(&single(json!({"fs_size_bytes": bogus}))), None);
        }
        assert_eq!(
            expected_install_layout(&single(json!({"fs_name": "../evil.gba"}))),
            Some(InstallLayout::Single {
                file_name: "evil.gba".into(),
                size: 1024
            })
        );
    }

    fn multi_layout(rom: &Value) -> (Vec<String>, Vec<ExpectedFile>) {
        match expected_install_layout(rom) {
            Some(InstallLayout::Multi { dir_candidates, files }) => (dir_candidates, files),
            other => panic!("expected multi, got {other:?}"),
        }
    }

    #[test]
    fn multi_layout_matching() {
        let (dirs, files) = multi_layout(&multi(vec![f("FF7.cue", 10), f("FF7.bin", 20)], json!({})));
        assert_eq!(dirs[0], "Final Fantasy VII (USA)");
        assert!(multi_file_matches(&files, &observed(&[("FF7.cue", Some(10)), ("FF7.bin", Some(20))])));
        assert!(!multi_file_matches(&files, &observed(&[("FF7.cue", Some(10)), ("FF7.bin", None)])));
        assert!(!multi_file_matches(&files, &observed(&[("FF7.cue", Some(10))])));
        assert!(!multi_file_matches(&files, &observed(&[("FF7.cue", Some(10)), ("FF7.bin", Some(21))])));
    }

    #[test]
    fn multi_layout_paths() {
        let (_, files) = multi_layout(&multi(vec![f("base.nsp", 5), f("update/patch.nsp", 6)], json!({})));
        let rels: Vec<_> = files.iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(rels, ["base.nsp", "update/patch.nsp"]);
        assert_eq!(files[1].parts(), ["update", "patch.nsp"]);
        let (_, files) = multi_layout(&multi(vec![f("Disc%201.cue", 1), f("Disc 1.bin", 2)], json!({})));
        let rels: Vec<_> = files.iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(rels, ["Disc 1.cue", "Disc 1.bin"]);
        let (dirs, _) = multi_layout(&multi(vec![f("FF7.cue", 10), f("sub/x.bin", 20)], json!({})));
        assert!(dirs.contains(&"FF7.cue".to_string()));
        assert!(dirs.contains(&"Final Fantasy VII (USA).m3u".to_string()));
        assert!(!dirs.contains(&"x.bin".to_string()));
    }

    #[test]
    fn multi_layout_undeterminable() {
        let evil = json!({"file_path": "psx/Final Fantasy VII (USA)", "file_name": "../../evil", "file_size_bytes": 1});
        assert_eq!(expected_install_layout(&multi(vec![f("ok.cue", 1), evil], json!({}))), None);
        assert_eq!(expected_install_layout(&multi(vec![f("ok.cue", 1), f("%2e%2e", 1)], json!({}))), None);
        let outside = json!({"file_path": "psx/Other Game", "file_name": "x.bin", "file_size_bytes": 1});
        assert_eq!(expected_install_layout(&multi(vec![f("ok.cue", 1), outside], json!({}))), None);
        let no_size = json!({"file_path": "psx/Final Fantasy VII (USA)", "file_name": "x.bin"});
        assert_eq!(expected_install_layout(&multi(vec![f("ok.cue", 1), no_size], json!({}))), None);
        let mut rom = multi(vec![f("a.cue", 1), f("a.bin", 2)], json!({}));
        rom.as_object_mut().unwrap().remove("fs_path");
        assert_eq!(expected_install_layout(&rom), None);
        assert_eq!(expected_install_layout(&multi(vec![], json!({}))), None);
        assert_eq!(expected_install_layout(&multi(vec![json!(1), f("a", 1)], json!({}))), None);
    }

    fn cand(rom_id: i64, group: Option<&str>) -> AdoptionCandidate {
        let install = RomInstall::mark_installed(
            rom_id,
            format!("/r/{rom_id}"),
            None,
            "gba".into(),
            "gba".into(),
            "t".into(),
        )
        .unwrap();
        AdoptionCandidate {
            rom_id,
            install,
            bake_path: format!("/r/{rom_id}"),
            group_key: group.map(str::to_string),
        }
    }

    #[test]
    fn one_per_group() {
        let picked = pick_one_per_group(vec![cand(5, Some("g")), cand(3, Some("g")), cand(9, Some("h"))]);
        assert_eq!(picked.iter().map(|c| c.rom_id).collect::<Vec<_>>(), [3, 9]);
        let picked = pick_one_per_group(vec![cand(2, None), cand(1, None)]);
        assert_eq!(picked.iter().map(|c| c.rom_id).collect::<Vec<_>>(), [1, 2]);
    }
}
