//! The files of an installed multi-file ROM as RomM lists them, compared with what is on disk: which ones are
//! missing (an update or DLC added on RomM after the install), which differ in size (replaced on RomM), and which are
//! already there. Pure compute; the caller scans the install folder and downloads the chosen files one by one.
use std::collections::HashMap;
use serde_json::Value;
use crate::domain::install_layout::{ExpectedFile, InstallLayout, relative_file_path, rom_root};

/// One file of a multi-file ROM on RomM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    /// RomM's file id (what `?file_ids=` selects).
    pub file_id: i64,
    /// The name RomM serves it under.
    pub file_name: String,
    /// `/`-separated path inside the ROM's folder (URL-decoded, every component safe).
    pub rel_path: String,
    pub size: u64,
    /// RomM's category (`update`, `dlc`, `game`, …), else the first folder's name, else `game`.
    pub category: String,
}

impl RemoteFile {
    pub fn parts(&self) -> Vec<&str> {
        self.rel_path.split('/').collect()
    }
}

/// The state of a RomM file in the install folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    /// Not on disk.
    Missing,
    /// On disk with a different size than RomM reports.
    Changed,
    /// On disk with RomM's size.
    Present,
}

impl FileState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Changed => "changed",
            Self::Present => "present",
        }
    }
}

/// Every usable file of `rom` (a RomM ROM detail), sorted by path. Entries without an id, a size, a name, or with a
/// path outside the ROM's folder or an unsafe component are left out; `None` when the ROM has no folder to compare.
pub fn remote_files(rom: &Value) -> Option<Vec<RemoteFile>> {
    let root = rom_root(rom)?;
    let entries = rom.get("files").and_then(Value::as_array)?;
    let mut out: Vec<RemoteFile> = entries
        .iter()
        .filter_map(|entry| {
            let file_id = entry.get("id").and_then(Value::as_i64).filter(|id| *id > 0)?;
            let file_name = entry.get("file_name").and_then(Value::as_str).filter(|s| !s.is_empty())?.to_string();
            let size = entry.get("file_size_bytes").and_then(Value::as_u64)?;
            let rel_path = relative_file_path(entry, &root)?;
            let category = entry
                .get("category")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty())
                .map(str::to_string)
                .or_else(|| rel_path.split_once('/').map(|(dir, _)| dir.to_ascii_lowercase()))
                .unwrap_or_else(|| "game".into());
            Some(RemoteFile { file_id, file_name, rel_path, size, category })
        })
        .collect();
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Some(out)
}

/// The state of `file` given the sizes found in the install folder (keyed by the same relative path).
pub fn file_state(file: &RemoteFile, on_disk: &HashMap<String, u64>) -> FileState {
    match on_disk.get(&file.rel_path) {
        None => FileState::Missing,
        Some(size) if *size == file.size => FileState::Present,
        Some(_) => FileState::Changed,
    }
}

/// The files that are game content for `system` (what its emulators load). Everything else in a game folder (`.nfo`,
/// `.sfv`, scans, …) is never downloaded or offered. `None` for a system whose folders are fetched whole.
pub fn content_extensions(system: &str) -> Option<&'static [&'static str]> {
    match system {
        "switch" => Some(&[".xci", ".nsp", ".xcz", ".nsz"]),
        _ => None,
    }
}

pub fn is_content(path: &str, extensions: &[&str]) -> bool {
    let lower = path.to_ascii_lowercase();
    extensions.iter().any(|ext| lower.ends_with(ext))
}

/// The content files of `files` (all of them when the system has no content list).
pub fn content_files(files: Vec<RemoteFile>, system: &str) -> Vec<RemoteFile> {
    match content_extensions(system) {
        Some(extensions) => files.into_iter().filter(|f| is_content(&f.rel_path, extensions)).collect(),
        None => files,
    }
}

/// The base game file of a game folder: the largest content file at the top of the folder that RomM does not mark as
/// an update or DLC (those sit in subfolders), ties broken by name. `None` when the folder has none.
pub fn base_game_file<'a>(files: &'a [RemoteFile], extensions: &[&str]) -> Option<&'a RemoteFile> {
    files
        .iter()
        .filter(|f| !f.rel_path.contains('/') && is_content(&f.rel_path, extensions))
        .filter(|f| f.category != "update" && f.category != "dlc")
        .min_by(|a, b| b.size.cmp(&a.size).then_with(|| a.rel_path.cmp(&b.rel_path)))
}

/// The base game file `rom` installs as for `system`, when the system installs game folders file by file.
pub fn base_game_file_of(rom: &Value, system: &str) -> Option<RemoteFile> {
    let extensions = content_extensions(system)?;
    let files = remote_files(rom)?;
    base_game_file(&files, extensions).cloned()
}

/// A game folder installed file by file holds its base game file and whatever else the user chose, so "already on disk"
/// means the base game file is there: the expected layout is narrowed to it. Other layouts are returned unchanged.
pub fn narrow_to_base(layout: InstallLayout, rom: &Value, system: &str) -> InstallLayout {
    match (layout, base_game_file_of(rom, system)) {
        (InstallLayout::Multi { dir_candidates, .. }, Some(base)) => {
            InstallLayout::Multi { dir_candidates, files: vec![ExpectedFile { rel_path: base.rel_path, size: base.size }] }
        }
        (layout, _) => layout,
    }
}

/// How the queue labels a file download.
pub fn category_label(category: &str) -> String {
    match category {
        "update" => "Update".into(),
        "dlc" => "DLC".into(),
        "game" => "Game file".into(),
        other => {
            let mut chars = other.chars();
            chars.next().map_or_else(|| "File".into(), |c| c.to_uppercase().chain(chars).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rom(files: Vec<Value>) -> Value {
        json!({"fs_path": "switch", "fs_name": "Mario Kart", "files": files})
    }
    fn f(id: i64, path: &str, name: &str, size: u64, category: Value) -> Value {
        json!({"id": id, "file_path": path, "file_name": name, "file_size_bytes": size, "category": category})
    }

    #[test]
    fn lists_files_inside_the_rom_folder_with_their_category_sorted_by_path() {
        let files = remote_files(&rom(vec![
            f(3, "switch/Mario Kart/update", "MK8 v3.0.nsp", 300, json!("update")),
            f(1, "switch/Mario Kart", "MK8.xci", 100, json!("game")),
            f(2, "switch/Mario Kart/dlc", "Booster%20Pass.nsp", 200, Value::Null),
            f(4, "switch/Mario Kart", "readme.txt", 1, Value::Null),
        ]))
        .unwrap();
        let summary: Vec<(i64, &str, &str)> = files.iter().map(|f| (f.file_id, f.rel_path.as_str(), f.category.as_str())).collect();
        assert_eq!(
            summary,
            [(1, "MK8.xci", "game"), (2, "dlc/Booster Pass.nsp", "dlc"), (4, "readme.txt", "game"), (3, "update/MK8 v3.0.nsp", "update")]
        );
        assert_eq!(files[3].file_name, "MK8 v3.0.nsp");
        assert_eq!(files[1].parts(), ["dlc", "Booster Pass.nsp"]);
    }

    #[test]
    fn unusable_entries_are_left_out_and_a_rom_without_a_folder_has_no_list() {
        let files = remote_files(&rom(vec![
            f(0, "switch/Mario Kart", "no-id.nsp", 1, Value::Null),
            json!({"id": 5, "file_path": "switch/Mario Kart", "file_name": "no-size.nsp"}),
            f(6, "switch/Other", "elsewhere.nsp", 1, Value::Null),
            f(7, "switch/Mario Kart/..", "escape.nsp", 1, Value::Null),
            f(8, "switch/Mario Kart", "", 1, Value::Null),
            f(9, "switch/Mario Kart", "ok.nsp", 1, json!("")),
            json!("not an object"),
        ]))
        .unwrap();
        assert_eq!(files.iter().map(|f| f.file_id).collect::<Vec<_>>(), [9]);
        assert_eq!(files[0].category, "game");
        assert_eq!(remote_files(&json!({"files": []})), None);
        assert_eq!(remote_files(&json!({"fs_path": "switch", "fs_name": "X"})), None);
    }

    #[test]
    fn states_compare_sizes_by_relative_path() {
        let file = RemoteFile { file_id: 1, file_name: "u.nsp".into(), rel_path: "update/u.nsp".into(), size: 10, category: "update".into() };
        let disk = |pairs: &[(&str, u64)]| pairs.iter().map(|(p, s)| (p.to_string(), *s)).collect::<HashMap<_, _>>();
        assert_eq!(file_state(&file, &disk(&[])), FileState::Missing);
        assert_eq!(file_state(&file, &disk(&[("u.nsp", 10)])), FileState::Missing);
        assert_eq!(file_state(&file, &disk(&[("update/u.nsp", 9)])), FileState::Changed);
        assert_eq!(file_state(&file, &disk(&[("update/u.nsp", 10)])), FileState::Present);
        assert_eq!([FileState::Missing, FileState::Changed, FileState::Present].map(FileState::as_str), ["missing", "changed", "present"]);
    }

    fn remote(id: i64, rel_path: &str, size: u64, category: &str) -> RemoteFile {
        RemoteFile { file_id: id, file_name: rel_path.rsplit('/').next().unwrap().into(), rel_path: rel_path.into(), size, category: category.into() }
    }

    #[test]
    fn only_switch_content_counts_and_other_systems_keep_everything() {
        let files = vec![remote(1, "Game.nsp", 9, "game"), remote(2, "Game.nfo", 1, "game"), remote(3, "update/U.NSZ", 2, "update"), remote(4, "game.sfv", 1, "game"), remote(5, "dlc/D.xcz", 3, "dlc")];
        let ids = |v: Vec<RemoteFile>| v.iter().map(|f| f.file_id).collect::<Vec<_>>();
        assert_eq!(ids(content_files(files.clone(), "switch")), [1, 3, 5]);
        assert_eq!(ids(content_files(files, "psx")), [1, 2, 3, 4, 5]);
        assert!(content_extensions("psx").is_none());
    }

    #[test]
    fn the_base_game_is_the_largest_top_level_content_file_that_is_not_an_update_or_dlc() {
        let exts = content_extensions("switch").unwrap();
        let files = vec![
            remote(1, "Game.nfo", 999, "game"),
            remote(2, "Game [base].nsp", 100, "game"),
            remote(3, "Game.xci", 100, "game"),
            remote(4, "Game update.nsp", 500, "update"),
            remote(5, "dlc/Huge DLC.nsp", 900, "dlc"),
        ];
        assert_eq!(base_game_file(&files, exts).map(|f| f.file_id), Some(2), "ties go to the first name");
        let only_nested = vec![remote(5, "dlc/Huge DLC.nsp", 900, "dlc"), remote(6, "base/Game.nsp", 50, "game")];
        assert_eq!(base_game_file(&only_nested, exts), None);
        assert_eq!(base_game_file(&[], exts), None);
    }

    #[test]
    fn adoption_of_a_switch_folder_expects_only_its_base_game_file() {
        let rom = rom(vec![
            f(1, "switch/Mario Kart", "MK8.xci", 100, json!("game")),
            f(2, "switch/Mario Kart", "MK8.nfo", 1, json!("game")),
            f(3, "switch/Mario Kart/update", "U.nsp", 30, json!("update")),
        ]);
        let all = InstallLayout::Multi {
            dir_candidates: vec!["Mario Kart".into()],
            files: vec![
                ExpectedFile { rel_path: "MK8.xci".into(), size: 100 },
                ExpectedFile { rel_path: "MK8.nfo".into(), size: 1 },
                ExpectedFile { rel_path: "update/U.nsp".into(), size: 30 },
            ],
        };
        assert_eq!(
            narrow_to_base(all.clone(), &rom, "switch"),
            InstallLayout::Multi { dir_candidates: vec!["Mario Kart".into()], files: vec![ExpectedFile { rel_path: "MK8.xci".into(), size: 100 }] }
        );
        assert_eq!(narrow_to_base(all.clone(), &rom, "psx"), all);
        let single = InstallLayout::Single { file_name: "x.nsp".into(), size: 1 };
        assert_eq!(narrow_to_base(single.clone(), &rom, "switch"), single);
        assert_eq!(base_game_file_of(&rom, "switch").map(|f| f.file_id), Some(1));
    }

    #[test]
    fn labels() {
        assert_eq!(["update", "dlc", "game", "manual", ""].map(category_label), ["Update", "DLC", "Game file", "Manual", "File"]);
    }
}
