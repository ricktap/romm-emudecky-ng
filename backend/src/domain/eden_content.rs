//! Eden's "External Content" folders in its `qt-config.ini`. Eden scans each folder, subfolders included, for update
//! and DLC files (`.nsp`/`.nsz`/`.xci`/`.xcz`) whenever a game starts, also when the game is started directly
//! (`eden -g <file>`), so updates and DLC work without installing them to its NAND.
//!
//! Eden reads the list inside its `UI` group, in the nested `Paths` group (`QtConfig::ReadUIValues` →
//! `ReadPathValues`). Its config writer turns the first group into the INI section and the rest into a key prefix, so
//! the list lives in `[UI]` as `Paths\external_content_dirs\1\path=/dir`, …, `Paths\external_content_dirs\size=N`, and
//! Eden reads entries `1..=size` only. Values are read with `"` removed (Eden quotes values holding special characters).
//!
//! v0.64.0 wrote the list into a `[Paths]` section, which Eden never reads; those lines are removed again.

const SECTION: &str = "UI";
const PREFIX: &str = "Paths\\external_content_dirs\\";
const SIZE_KEY: &str = "Paths\\external_content_dirs\\size";
/// The section and keys v0.64.0 wrote by mistake.
const LEGACY_SECTION: &str = "Paths";
const LEGACY_PREFIX: &str = "external_content_dirs\\";
/// Characters for which Eden's writer wraps a value in quotes (`Config::special_characters`).
const SPECIAL: &[char] = &['!', '#', '$', '%', '^', '&', '*', '|', ';', '\'', '"', ',', '<', '>', '?', '`', '~', '='];

fn entry_key(index: usize) -> String {
    format!("{PREFIX}{index}\\path")
}

fn section_name(line: &str) -> Option<&str> {
    let line = line.trim();
    line.strip_prefix('[')?.strip_suffix(']')
}

fn key_value(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once('=')?;
    Some((key.trim(), value.trim()))
}

/// A value as Eden reads it: every `"` removed.
fn read_value(raw: &str) -> String {
    raw.replace('"', "")
}

/// A path as Eden writes it: quoted when it holds a special character.
fn written_value(path: &str) -> String {
    if path.contains(SPECIAL) { format!("\"{}\"", path.replace('"', "")) } else { path.to_string() }
}

fn normalize(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() { path } else { trimmed }
}

/// The `size` Eden reads for the list (0 when absent or not a number) and the entries by index.
fn read_list(ini: &str) -> (usize, std::collections::HashMap<usize, String>) {
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
            size = read_value(value).parse().unwrap_or(0);
        } else if let Some(index) =
            key.strip_prefix(PREFIX).and_then(|rest| rest.strip_suffix("\\path")).and_then(|n| n.parse::<usize>().ok())
        {
            entries.insert(index, read_value(value));
        }
    }
    (size, entries)
}

/// The external content folders Eden reads from `ini`: entries `1..=size` with a non-empty path, in order.
pub fn external_content_dirs(ini: &str) -> Vec<String> {
    let (size, entries) = read_list(ini);
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

fn has_legacy_lines(ini: &str) -> bool {
    let mut in_legacy = false;
    ini.lines().any(|line| {
        if let Some(name) = section_name(line) {
            in_legacy = name == LEGACY_SECTION;
            return false;
        }
        in_legacy && key_value(line).is_some_and(|(k, _)| k.starts_with(LEGACY_PREFIX))
    })
}

/// The lines of `ini` without the list v0.64.0 put into `[Paths]` (and without that section when nothing else is in
/// it), so the file only holds what Eden reads.
fn without_legacy_lines(lines: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        if section_name(&lines[i]) == Some(LEGACY_SECTION) {
            let end = lines[i + 1..].iter().position(|l| section_name(l).is_some()).map_or(lines.len(), |p| i + 1 + p);
            let kept: Vec<String> = lines[i + 1..end]
                .iter()
                .filter(|l| key_value(l).is_none_or(|(k, _)| !k.starts_with(LEGACY_PREFIX)))
                .cloned()
                .collect();
            if kept.iter().any(|l| !l.trim().is_empty()) {
                out.push(lines[i].clone());
                out.extend(kept);
            } else {
                // The section held only the stray list: drop it, and the blank line that separated it.
                while out.last().is_some_and(|l| l.trim().is_empty()) {
                    out.pop();
                }
                if end < lines.len() && !out.is_empty() {
                    out.push(String::new());
                }
            }
            i = end;
        } else {
            out.push(lines[i].clone());
            i += 1;
        }
    }
    out
}

/// `ini` with `dir` appended to the external content folders and without stray lines from v0.64.0, or `None` when
/// nothing has to change (Eden already scans `dir` and no stray lines are left). Every other line (other sections,
/// other keys, the user's own folders) is kept as it is; the line ending of the file is kept.
pub fn with_external_content_dir(ini: &str, dir: &str) -> Option<String> {
    let needs_entry = !dir.is_empty() && !covers(ini, dir);
    if !needs_entry && !has_legacy_lines(ini) {
        return None;
    }
    let eol = if ini.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines = without_legacy_lines(ini.lines().map(str::to_string).collect());
    if needs_entry {
        let index = read_list(ini).0 + 1;
        let new_entry = format!("{}={}", entry_key(index), written_value(dir));
        let new_size = format!("{SIZE_KEY}={index}");
        match lines.iter().position(|l| section_name(l) == Some(SECTION)) {
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
    }
    let mut out = lines.join(eol);
    out.push_str(eol);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROMS: &str = "/run/media/deck/SD/Emulation/roms/switch";

    #[test]
    fn adds_the_folder_where_eden_reads_it_and_keeps_everything_else() {
        let ini = "[UI]\ntheme=default\nPaths\\gamedirs\\1\\path=SDMC\nPaths\\gamedirs\\size=1\n\n[Renderer]\nbackend=1\n";
        let out = with_external_content_dir(ini, ROMS).unwrap();
        assert_eq!(
            out,
            format!(
                "[UI]\ntheme=default\nPaths\\gamedirs\\1\\path=SDMC\nPaths\\gamedirs\\size=1\nPaths\\external_content_dirs\\1\\path={ROMS}\nPaths\\external_content_dirs\\size=1\n\n[Renderer]\nbackend=1\n"
            )
        );
        assert_eq!(external_content_dirs(&out), [ROMS]);
        // Running it again changes nothing.
        assert_eq!(with_external_content_dir(&out, ROMS), None);
    }

    #[test]
    fn reads_the_list_as_eden_writes_it() {
        // Eden quotes values with special characters and reads them back without the quotes.
        let ini = "[UI]\nPaths\\external_content_dirs\\1\\path=\"/media/Tom's SD/roms\"\nPaths\\external_content_dirs\\size=1\n";
        assert_eq!(external_content_dirs(ini), ["/media/Tom's SD/roms"]);
        assert_eq!(with_external_content_dir(ini, "/media/Tom's SD/roms/switch"), None);
        // The same keys in another section are not Eden's list.
        assert!(external_content_dirs("[Paths]\nPaths\\external_content_dirs\\1\\path=/a\nPaths\\external_content_dirs\\size=1\n").is_empty());
    }

    #[test]
    fn a_path_with_special_characters_is_written_quoted_like_eden_does() {
        let out = with_external_content_dir("[UI]\n", "/media/Tom's SD/Emulation/roms/switch").unwrap();
        assert!(out.contains("Paths\\external_content_dirs\\1\\path=\"/media/Tom's SD/Emulation/roms/switch\"\n"), "{out}");
        assert_eq!(external_content_dirs(&out), ["/media/Tom's SD/Emulation/roms/switch"]);
        // Spaces alone are not special.
        let out = with_external_content_dir("[UI]\n", "/run/media/deck/SD Card/roms/switch").unwrap();
        assert!(out.contains("path=/run/media/deck/SD Card/roms/switch\n"), "{out}");
    }

    #[test]
    fn keeps_the_users_own_folders_and_appends_after_them() {
        let ini = "[UI]\nPaths\\external_content_dirs\\1\\path=/mine\nPaths\\external_content_dirs\\size=1\n";
        let out = with_external_content_dir(ini, ROMS).unwrap();
        assert_eq!(external_content_dirs(&out), ["/mine", ROMS]);
        assert_eq!(out.matches("Paths\\external_content_dirs\\size=").count(), 1);
    }

    #[test]
    fn a_folder_already_covered_by_itself_or_a_parent_is_left_alone() {
        for existing in [ROMS, "/run/media/deck/SD/Emulation/roms/switch/", "/run/media/deck/SD/Emulation/roms", "/"] {
            let ini = format!("[UI]\nPaths\\external_content_dirs\\1\\path={existing}\nPaths\\external_content_dirs\\size=1\n");
            assert_eq!(with_external_content_dir(&ini, ROMS), None, "{existing}");
        }
        // A sibling whose name merely starts the same is not a parent.
        let ini = "[UI]\nPaths\\external_content_dirs\\1\\path=/run/media/deck/SD/Emulation/roms/swi\nPaths\\external_content_dirs\\size=1\n";
        assert!(with_external_content_dir(ini, ROMS).is_some());
        assert_eq!(with_external_content_dir("", ""), None);
    }

    #[test]
    fn creates_the_ui_section_when_missing_and_keeps_crlf_files_crlf() {
        assert_eq!(
            with_external_content_dir("[Renderer]\r\nbackend=1", ROMS).unwrap(),
            format!("[Renderer]\r\nbackend=1\r\n\r\n[UI]\r\nPaths\\external_content_dirs\\1\\path={ROMS}\r\nPaths\\external_content_dirs\\size=1\r\n")
        );
        assert_eq!(
            with_external_content_dir("", ROMS).unwrap(),
            format!("[UI]\nPaths\\external_content_dirs\\1\\path={ROMS}\nPaths\\external_content_dirs\\size=1\n")
        );
    }

    #[test]
    fn reads_only_what_eden_reads_and_overwrites_stale_entries_beyond_size() {
        let ini = "[UI]\nPaths\\external_content_dirs\\1\\path=/a\nPaths\\external_content_dirs\\2\\path=/stale\nPaths\\external_content_dirs\\size=1\n[Other]\nPaths\\external_content_dirs\\size=9\n";
        assert_eq!(external_content_dirs(ini), ["/a"]);
        let out = with_external_content_dir(ini, ROMS).unwrap();
        assert_eq!(external_content_dirs(&out), ["/a", ROMS]);
        assert!(!out.contains("/stale"));
        assert!(out.contains("[Other]\nPaths\\external_content_dirs\\size=9"));
        assert!(external_content_dirs("[UI]\nPaths\\external_content_dirs\\1\\path=\nPaths\\external_content_dirs\\size=1\n").is_empty());
        assert!(external_content_dirs("[UI]\nPaths\\external_content_dirs\\1\\path=/a\n").is_empty());
        assert_eq!(external_content_dirs("[UI]\nPaths\\external_content_dirs\\1\\path = /a\nPaths\\external_content_dirs\\size = 1\n"), ["/a"]);
    }

    #[test]
    fn removes_the_stray_list_v0_64_0_wrote_into_a_paths_section() {
        // v0.64.0's write: a [Paths] section Eden never reads, appended after the user's sections.
        let broken = format!("[UI]\ntheme=default\n\n[Renderer]\nbackend=1\n\n[Paths]\nexternal_content_dirs\\1\\path={ROMS}\nexternal_content_dirs\\size=1\n");
        let fixed = with_external_content_dir(&broken, ROMS).unwrap();
        assert_eq!(
            fixed,
            format!("[UI]\ntheme=default\nPaths\\external_content_dirs\\1\\path={ROMS}\nPaths\\external_content_dirs\\size=1\n\n[Renderer]\nbackend=1\n")
        );
        assert_eq!(with_external_content_dir(&fixed, ROMS), None);
        // Already scanned, but the stray lines are still there: only they go.
        let both = format!("[UI]\nPaths\\external_content_dirs\\1\\path={ROMS}\nPaths\\external_content_dirs\\size=1\n\n[Paths]\nexternal_content_dirs\\1\\path={ROMS}\nexternal_content_dirs\\size=1\n\n[Renderer]\nbackend=1\n");
        assert_eq!(
            with_external_content_dir(&both, ROMS).unwrap(),
            format!("[UI]\nPaths\\external_content_dirs\\1\\path={ROMS}\nPaths\\external_content_dirs\\size=1\n\n[Renderer]\nbackend=1\n")
        );
        // Anything else in a [Paths] section is left alone.
        let mixed = "[Paths]\nsomething=else\nexternal_content_dirs\\size=1\n";
        let out = with_external_content_dir(mixed, ROMS).unwrap();
        assert!(out.starts_with("[Paths]\nsomething=else\n[UI]\n") || out.starts_with("[Paths]\nsomething=else\n\n[UI]\n"), "{out}");
        assert!(!out.contains("\nexternal_content_dirs\\size"));
    }
}
