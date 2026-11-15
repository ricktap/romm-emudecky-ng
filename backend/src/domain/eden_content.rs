//! Eden's "External Content" folders: `[Paths] external_content_dirs` in its `qt-config.ini`. Eden scans each folder,
//! subfolders included, for update and DLC files (`.nsp`/`.nsz`/`.xci`/`.xcz`) whenever a game starts, also when the
//! game is started directly (`eden -g <file>`), so updates and DLC work without installing them to its NAND.
//!
//! The array is written the way Eden's own config writer does it:
//! `external_content_dirs\1\path=/dir`, …, `external_content_dirs\size=N`. Eden reads entries `1..=size` only.

const SECTION: &str = "Paths";
const SIZE_KEY: &str = "external_content_dirs\\size";

fn entry_key(index: usize) -> String {
    format!("external_content_dirs\\{index}\\path")
}

fn section_name(line: &str) -> Option<&str> {
    let line = line.trim();
    line.strip_prefix('[')?.strip_suffix(']')
}

fn key_value(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once('=')?;
    Some((key.trim(), value.trim()))
}

fn normalize(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() { path } else { trimmed }
}

/// The external content folders Eden reads from `ini`: entries `1..=size` with a non-empty path, in order.
pub fn external_content_dirs(ini: &str) -> Vec<String> {
    let mut in_section = false;
    let mut size = 0usize;
    let mut entries = std::collections::HashMap::new();
    for line in ini.lines() {
        if let Some(name) = section_name(line) {
            in_section = name == SECTION;
            continue;
        }
        if !in_section {
            continue;
        }
        let Some((key, value)) = key_value(line) else { continue };
        if key == SIZE_KEY {
            size = value.parse().unwrap_or(0);
        } else if let Some(index) = key
            .strip_prefix("external_content_dirs\\")
            .and_then(|rest| rest.strip_suffix("\\path"))
            .and_then(|n| n.parse::<usize>().ok())
        {
            entries.insert(index, value.to_string());
        }
    }
    (1..=size).filter_map(|i| entries.get(&i).filter(|p| !p.is_empty()).cloned()).collect()
}

/// Whether Eden already scans `dir`: it, or a folder above it, is an external content folder.
pub fn covers(ini: &str, dir: &str) -> bool {
    let dir = normalize(dir);
    external_content_dirs(ini).iter().any(|existing| {
        let parent = format!("{}/", existing.trim_end_matches('/'));
        dir == normalize(existing) || dir.starts_with(&parent)
    })
}

/// `ini` with `dir` appended to the external content folders, or `None` when Eden already scans it. Every other line
/// (other sections, other keys, the user's own folders) is kept as it is; the line ending of the file is kept.
pub fn with_external_content_dir(ini: &str, dir: &str) -> Option<String> {
    if dir.is_empty() || covers(ini, dir) {
        return None;
    }
    let eol = if ini.contains("\r\n") { "\r\n" } else { "\n" };
    let index = external_content_dirs_size(ini) + 1;
    let new_entry = format!("{}={}", entry_key(index), dir);
    let new_size = format!("{SIZE_KEY}={index}");

    let mut lines: Vec<String> = ini.lines().map(str::to_string).collect();
    let start = lines.iter().position(|l| section_name(l) == Some(SECTION));
    match start {
        Some(start) => {
            let end = lines[start + 1..].iter().position(|l| section_name(l).is_some()).map_or(lines.len(), |i| start + 1 + i);
            // Drop the size line and any stale line for the index about to be used; both are written again below.
            let stale = entry_key(index);
            let mut section: Vec<String> = lines[start + 1..end]
                .iter()
                .filter(|l| key_value(l).is_none_or(|(k, _)| k != SIZE_KEY && k != stale))
                .cloned()
                .collect();
            let insert_at = section.iter().rposition(|l| !l.trim().is_empty()).map_or(0, |i| i + 1);
            section.splice(insert_at..insert_at, [new_entry, new_size]);
            lines.splice(start + 1..end, section);
        }
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.extend([format!("[{SECTION}]"), new_entry, new_size]);
        }
    }
    let mut out = lines.join(eol);
    out.push_str(eol);
    Some(out)
}

/// The `size` Eden reads for the array (0 when absent or not a number).
fn external_content_dirs_size(ini: &str) -> usize {
    let mut in_section = false;
    let mut size = 0;
    for line in ini.lines() {
        if let Some(name) = section_name(line) {
            in_section = name == SECTION;
        } else if in_section {
            if let Some((SIZE_KEY, value)) = key_value(line) {
                size = value.parse().unwrap_or(0);
            }
        }
    }
    size
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROMS: &str = "/run/media/deck/SD/Emulation/roms/switch";

    #[test]
    fn adds_the_folder_to_an_existing_paths_section_and_keeps_everything_else() {
        let ini = "[UI]\ntheme=dark\n\n[Paths]\ngamedirs\\1\\path=/games\ngamedirs\\size=1\n\n[Renderer]\nbackend=1\n";
        let out = with_external_content_dir(ini, ROMS).unwrap();
        assert_eq!(
            out,
            format!(
                "[UI]\ntheme=dark\n\n[Paths]\ngamedirs\\1\\path=/games\ngamedirs\\size=1\nexternal_content_dirs\\1\\path={ROMS}\nexternal_content_dirs\\size=1\n\n[Renderer]\nbackend=1\n"
            )
        );
        assert_eq!(external_content_dirs(&out), [ROMS]);
        // Running it again changes nothing.
        assert_eq!(with_external_content_dir(&out, ROMS), None);
    }

    #[test]
    fn keeps_the_users_own_folders_and_appends_after_them() {
        let ini = "[Paths]\nexternal_content_dirs\\1\\path=/mine\nexternal_content_dirs\\size=1\n";
        let out = with_external_content_dir(ini, ROMS).unwrap();
        assert_eq!(external_content_dirs(&out), ["/mine", ROMS]);
        assert_eq!(out.matches("external_content_dirs\\size=").count(), 1);
    }

    #[test]
    fn a_folder_already_covered_by_itself_or_a_parent_is_left_alone() {
        for existing in [ROMS, "/run/media/deck/SD/Emulation/roms/switch/", "/run/media/deck/SD/Emulation/roms", "/"] {
            let ini = format!("[Paths]\nexternal_content_dirs\\1\\path={existing}\nexternal_content_dirs\\size=1\n");
            assert_eq!(with_external_content_dir(&ini, ROMS), None, "{existing}");
        }
        // A sibling whose name merely starts the same is not a parent.
        let ini = "[Paths]\nexternal_content_dirs\\1\\path=/run/media/deck/SD/Emulation/roms/swi\nexternal_content_dirs\\size=1\n";
        assert!(with_external_content_dir(ini, ROMS).is_some());
        assert_eq!(with_external_content_dir("", ""), None);
    }

    #[test]
    fn creates_the_paths_section_when_missing_and_keeps_crlf_files_crlf() {
        assert_eq!(
            with_external_content_dir("[UI]\r\ntheme=dark", ROMS).unwrap(),
            format!("[UI]\r\ntheme=dark\r\n\r\n[Paths]\r\nexternal_content_dirs\\1\\path={ROMS}\r\nexternal_content_dirs\\size=1\r\n")
        );
        assert_eq!(
            with_external_content_dir("", ROMS).unwrap(),
            format!("[Paths]\nexternal_content_dirs\\1\\path={ROMS}\nexternal_content_dirs\\size=1\n")
        );
    }

    #[test]
    fn reads_only_what_eden_reads_and_overwrites_stale_entries_beyond_size() {
        // Entry 2 exists but size says 1: Eden ignores it, and the next folder takes index 2 (replacing the stale line).
        let ini = "[Paths]\nexternal_content_dirs\\1\\path=/a\nexternal_content_dirs\\2\\path=/stale\nexternal_content_dirs\\size=1\n[Other]\nexternal_content_dirs\\size=9\n";
        assert_eq!(external_content_dirs(ini), ["/a"]);
        let out = with_external_content_dir(ini, ROMS).unwrap();
        assert_eq!(external_content_dirs(&out), ["/a", ROMS]);
        assert!(!out.contains("/stale"));
        assert!(out.contains("[Other]\nexternal_content_dirs\\size=9"));
        // Empty paths and a missing or broken size count as nothing.
        assert!(external_content_dirs("[Paths]\nexternal_content_dirs\\1\\path=\nexternal_content_dirs\\size=1\n").is_empty());
        assert!(external_content_dirs("[Paths]\nexternal_content_dirs\\1\\path=/a\n").is_empty());
        assert!(external_content_dirs("[Paths]\nexternal_content_dirs\\1\\path=/a\nexternal_content_dirs\\size=x\n").is_empty());
        // Spaces around `=` are tolerated.
        assert_eq!(external_content_dirs("[Paths]\nexternal_content_dirs\\1\\path = /a\nexternal_content_dirs\\size = 1\n"), ["/a"]);
    }
}
