//! ROM file layout decisions over a RomM ROM payload: single vs multi-file
//! download, the on-disk names, and the launch-file pick in an extracted
//! folder. Pure functions over JSON and path lists.

use serde_json::Value;

/// Python `os.path.splitext` on the base name: `("stem", ".ext")`; a leading
/// dot does not start an extension.
pub fn splitext(path: &str) -> (&str, &str) {
    let base_start = path.rfind('/').map_or(0, |i| i + 1);
    let base = &path[base_start..];
    let leading = base.len() - base.trim_start_matches('.').len();
    match base[leading..].rfind('.') {
        Some(i) => {
            let dot = base_start + leading + i;
            (&path[..dot], &path[dot..])
        }
        None => (path, ""),
    }
}

/// Whether RomM serves this ROM as a ZIP: more than one file in total, or the
/// `has_multiple_files` flag (a Switch folder with one top-level file plus
/// `update/` and `dlc/` subfolders is multi-file).
pub fn is_multi_file_download(rom: &Value) -> bool {
    let files = rom.get("files").and_then(Value::as_array).map_or(0, Vec::len);
    files > 1 || rom.get("has_multiple_files").is_some_and(crate::domain::settings::truthy)
}

/// `rom_<id>` (or `rom_unknown`): the identity name for a ROM without a usable name.
pub fn synthetic_rom_name(rom: &Value) -> String {
    match rom.get("id") {
        Some(Value::Number(n)) => format!("rom_{n}"),
        Some(Value::String(s)) => format!("rom_{s}"),
        _ => "rom_unknown".into(),
    }
}

fn str_field<'a>(rom: &'a Value, key: &str) -> Option<&'a str> {
    rom.get(key).and_then(Value::as_str)
}

/// The on-disk file name: `fs_name`, or `files[0].file_name` for a
/// nested-single-file ROM. Returns `(name, inconsistent)` where
/// `inconsistent` flags `has_nested_single_file` with an empty `files` list.
pub fn resolve_local_file_name(rom: &Value) -> (String, bool) {
    let fs_name = str_field(rom, "fs_name").map_or_else(|| synthetic_rom_name(rom), str::to_string);
    if !rom.get("has_nested_single_file").is_some_and(crate::domain::settings::truthy) {
        return (fs_name, false);
    }
    let first = rom.get("files").and_then(Value::as_array).and_then(|f| f.first());
    match first {
        None => (fs_name, true),
        Some(entry) => match entry.get("file_name").and_then(Value::as_str) {
            Some(name) if !name.is_empty() => (name.to_string(), false),
            _ => (fs_name, false),
        },
    }
}

/// The extract directory name of a multi-file ROM: `fs_name_no_ext`, else
/// `fs_name` without its extension, else the synthetic name. Never `files[0]`.
pub fn resolve_extract_dir_name(rom: &Value) -> String {
    if let Some(no_ext) = str_field(rom, "fs_name_no_ext").filter(|s| !s.is_empty()) {
        return no_ext.to_string();
    }
    let fs_name = str_field(rom, "fs_name").map_or_else(|| synthetic_rom_name(rom), str::to_string);
    splitext(&fs_name).0.to_string()
}

/// The best launch file among `(path, size)` pairs: `.m3u` (only when
/// `m3u_supported`), `.cue`, `.rpx`, `.wud`/`.wux`/`.wua`, `EBOOT.BIN`,
/// `.3ds`/`.cia`/`.cxi`, else the largest file. `None` for an empty list.
pub fn detect_launch_file(files: &[(String, u64)], m3u_supported: bool) -> Option<String> {
    if files.is_empty() {
        return None;
    }
    let first_with = |ext: &str| {
        files
            .iter()
            .find(|(p, _)| p.to_lowercase().ends_with(ext))
            .map(|(p, _)| p.clone())
    };
    let preferred: &[&str] = if m3u_supported { &[".m3u", ".cue"] } else { &[".cue"] };
    for ext in preferred.iter().chain([".rpx", ".wud", ".wux", ".wua"].iter()) {
        if let Some(p) = first_with(ext) {
            return Some(p);
        }
    }
    if let Some((p, _)) = files.iter().find(|(p, _)| p.ends_with("EBOOT.BIN")) {
        return Some(p.clone());
    }
    for ext in [".3ds", ".cia", ".cxi"] {
        if let Some(p) = first_with(ext) {
            return Some(p);
        }
    }
    // Python's max() keeps the first of equal sizes.
    let mut best = &files[0];
    for f in &files[1..] {
        if f.1 > best.1 {
            best = f;
        }
    }
    Some(best.0.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn f(list: &[(&str, u64)]) -> Vec<(String, u64)> {
        list.iter().map(|(p, s)| (p.to_string(), *s)).collect()
    }

    #[test]
    fn splitext_matches_python() {
        assert_eq!(splitext("a.b.c"), ("a.b", ".c"));
        assert_eq!(splitext(".bashrc"), (".bashrc", ""));
        assert_eq!(splitext("dir.x/file"), ("dir.x/file", ""));
        assert_eq!(splitext("Zelda (USA).z64"), ("Zelda (USA)", ".z64"));
        assert_eq!(splitext("..a"), ("..a", ""));
    }

    #[test]
    fn multi_file_gate() {
        assert!(!is_multi_file_download(&json!({"files": [{}]})));
        assert!(is_multi_file_download(&json!({"files": [{}, {}]})));
        assert!(is_multi_file_download(&json!({"has_multiple_files": true})));
        assert!(!is_multi_file_download(&json!({})));
        // Canonical Switch folder: one top-level file, update/ + dlc/ nested.
        assert!(is_multi_file_download(
            &json!({"has_multiple_files": false, "has_nested_single_file": true, "files": [{}, {}, {}]})
        ));
    }

    #[test]
    fn names() {
        assert_eq!(synthetic_rom_name(&json!({"id": 5})), "rom_5");
        assert_eq!(synthetic_rom_name(&json!({})), "rom_unknown");
        assert_eq!(resolve_local_file_name(&json!({"fs_name": "a.gba"})), ("a.gba".into(), false));
        assert_eq!(resolve_local_file_name(&json!({"id": 3})), ("rom_3".into(), false));
        let nested = json!({"fs_name": "Zelda", "has_nested_single_file": true, "files": [{"file_name": "Zelda.nsp"}]});
        assert_eq!(resolve_local_file_name(&nested), ("Zelda.nsp".into(), false));
        let empty = json!({"fs_name": "Zelda", "has_nested_single_file": true, "files": []});
        assert_eq!(resolve_local_file_name(&empty), ("Zelda".into(), true));
        let missing = json!({"fs_name": "Zelda", "has_nested_single_file": true});
        assert_eq!(resolve_local_file_name(&missing), ("Zelda".into(), true));
        let blank = json!({"fs_name": "Zelda", "has_nested_single_file": true, "files": [{"file_name": ""}]});
        assert_eq!(resolve_local_file_name(&blank), ("Zelda".into(), false));
        assert_eq!(resolve_extract_dir_name(&json!({"fs_name_no_ext": "FF7", "fs_name": "x.zip"})), "FF7");
        assert_eq!(resolve_extract_dir_name(&json!({"fs_name": "FF7 (USA).zip"})), "FF7 (USA)");
        assert_eq!(resolve_extract_dir_name(&json!({"id": 9})), "rom_9");
        assert_eq!(resolve_extract_dir_name(&json!({"fs_name_no_ext": "", "fs_name": "a.b"})), "a");
    }

    #[test]
    fn launch_file_priority() {
        assert_eq!(detect_launch_file(&[], true), None);
        let files = f(&[("/g/a.bin", 9), ("/g/a.cue", 1), ("/g/a.m3u", 1)]);
        assert_eq!(detect_launch_file(&files, true).unwrap(), "/g/a.m3u");
        assert_eq!(detect_launch_file(&files, false).unwrap(), "/g/a.cue");
        assert_eq!(detect_launch_file(&f(&[("/g/a", 1), ("/g/b", 5)]), true).unwrap(), "/g/b");
        assert_eq!(
            detect_launch_file(&f(&[("/g/meta/x.xml", 1), ("/g/code/x.rpx", 1)]), true).unwrap(),
            "/g/code/x.rpx"
        );
        assert_eq!(detect_launch_file(&f(&[("/g/x.wux", 1), ("/g/y", 9)]), true).unwrap(), "/g/x.wux");
        assert_eq!(
            detect_launch_file(&f(&[("/g/PS3_GAME/USRDIR/EBOOT.BIN", 1), ("/g/big", 9)]), true).unwrap(),
            "/g/PS3_GAME/USRDIR/EBOOT.BIN"
        );
        assert_eq!(detect_launch_file(&f(&[("/g/x.cia", 1), ("/g/x.3ds", 1)]), true).unwrap(), "/g/x.3ds");
        assert_eq!(detect_launch_file(&f(&[("/g/x.cia", 1), ("/g/y", 9)]), true).unwrap(), "/g/x.cia");
        // Switch folder: largest container wins; ties keep the first.
        let sw = f(&[("/s/G/base.nsp", 100), ("/s/G/update/u.nsp", 10), ("/s/G/dlc/d.nsp", 100)]);
        assert_eq!(detect_launch_file(&sw, false).unwrap(), "/s/G/base.nsp");
    }
}
