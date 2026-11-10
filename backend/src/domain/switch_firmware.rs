//! Switch firmware and keys: what RomM hosts for the platform, which emulators need which file, where each
//! emulator reads its keys, and how a local copy compares with RomM's. Pure; the service does the I/O.
//!
//! Facts come from a real Steam Deck + RomM (docs/plans/0.45.0-switch-firmware.md §6): RomM lists the files under
//! `bios/Switch` (capital S), keys are read from each emulator's own data directory (not from `Emulation/bios`),
//! and firmware archives are named `Firmware.<x.y.z>.zip`.

/// What a RomM firmware entry is, judged by its file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileKind {
    Key(KeyFile),
    Firmware(Version),
    Other,
}

/// A firmware version, compared numerically (`23.0.1` > `9.2.0`).
pub type Version = Vec<u64>;

/// A key file an emulator reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyFile {
    pub file_name: &'static str,
    pub required: bool,
    pub description: &'static str,
    /// The emulators that read this file.
    pub emulators: &'static [&'static str],
}

const ALL: &[&str] = &["eden", "yuzu", "ryujinx"];
const RYUJINX: &[&str] = &["ryujinx"];

/// The key files, in display order. `prod.keys` and `title.keys` are required (decision 3 of the 0.45.0 plan);
/// Ryujinx also reads `console.keys` and `key_retail.bin`.
pub const KEY_FILES: &[KeyFile] = &[
    KeyFile { file_name: "prod.keys", required: true, description: "Console keys", emulators: ALL },
    KeyFile { file_name: "title.keys", required: true, description: "Title keys", emulators: ALL },
    KeyFile { file_name: "console.keys", required: false, description: "Console keys (Ryujinx)", emulators: RYUJINX },
    KeyFile { file_name: "key_retail.bin", required: false, description: "Retail key (Ryujinx)", emulators: RYUJINX },
];

/// The emulators with a keys directory, in display order.
pub const EMULATORS: &[&str] = ALL;

/// The key file named `file_name` (exact, case-sensitive: the emulators read these exact names).
pub fn key_file(file_name: &str) -> Option<&'static KeyFile> {
    KEY_FILES.iter().find(|k| k.file_name == file_name)
}

/// `Firmware.23.0.1.zip` / `Firmware 23.0.1.zip` / `firmware-23.0.1.zip` → `[23, 0, 1]`.
pub fn parse_firmware_version(file_name: &str) -> Option<Version> {
    let lower = file_name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".zip")?;
    let rest = stem.strip_prefix("firmware")?;
    let rest = rest.trim_start_matches(['.', ' ', '-', '_', 'v']);
    let parts: Vec<&str> = rest.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    parts
        .iter()
        .map(|p| (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())).then(|| p.parse().ok()).flatten())
        .collect()
}

/// `[23, 0, 1]` → `23.0.1`.
pub fn version_text(v: &[u64]) -> String {
    v.iter().map(u64::to_string).collect::<Vec<_>>().join(".")
}

/// Classify a RomM firmware entry by its file name.
pub fn classify(file_name: &str) -> FileKind {
    if let Some(key) = key_file(file_name) {
        return FileKind::Key(*key);
    }
    match parse_firmware_version(file_name) {
        Some(version) => FileKind::Firmware(version),
        None => FileKind::Other,
    }
}

/// Whether a RomM firmware `file_path` (`bios/Switch`) belongs to the Switch platform. The folder's case is RomM's
/// (`Switch`), so it is compared case-insensitively.
pub fn is_switch_path(file_path: &str) -> bool {
    let parts: Vec<&str> = file_path.trim_matches('/').split('/').collect();
    match parts.as_slice() {
        ["bios", slug, ..] => slug.eq_ignore_ascii_case("switch"),
        [slug, ..] => slug.eq_ignore_ascii_case("switch"),
        [] => false,
    }
}

/// Where `emulator` reads its keys, under `home`. Verified on a Steam Deck for Eden, yuzu and Ryujinx.
pub fn keys_dir(emulator: &str, home: &str) -> Option<String> {
    let home = home.trim_end_matches('/');
    match emulator {
        "eden" => Some(format!("{home}/.local/share/eden/keys")),
        "yuzu" => Some(format!("{home}/.local/share/yuzu/keys")),
        "ryujinx" => Some(format!("{home}/.config/Ryujinx/system")),
        _ => None,
    }
}

/// How a local copy of a key file compares with RomM's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    /// No file there.
    Missing,
    /// The same bytes as RomM's.
    Matches,
    /// A working file of the user's own that is not RomM's: never overwritten without an explicit action.
    Differs,
    /// A file exists but there is nothing to compare it with (offline, or RomM reports no hash).
    Present,
}

impl KeyState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Matches => "matches",
            Self::Differs => "differs",
            Self::Present => "present",
        }
    }
}

/// Compare a local file's SHA-1 with RomM's (both lower-cased hex). `local_sha1` is `None` when no file exists.
pub fn key_state(local_sha1: Option<&str>, romm_sha1: Option<&str>) -> KeyState {
    match (local_sha1, romm_sha1.filter(|h| !h.is_empty())) {
        (None, _) => KeyState::Missing,
        (Some(_), None) => KeyState::Present,
        (Some(local), Some(romm)) if local.eq_ignore_ascii_case(romm) => KeyState::Matches,
        (Some(_), Some(_)) => KeyState::Differs,
    }
}

/// The newest of `versions` (numeric comparison), if any.
pub fn newest<'a>(versions: impl IntoIterator<Item = &'a Version>) -> Option<&'a Version> {
    versions.into_iter().max()
}

/// `"23.0.1"` → `[23, 0, 1]` (the text a version is saved and shown as); `None` for anything else.
pub fn parse_version_text(text: &str) -> Option<Version> {
    let parts: Vec<&str> = text.trim().split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    parts
        .iter()
        .map(|p| (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())).then(|| p.parse().ok()).flatten())
        .collect()
}

/// The firmware version to use: the saved choice when RomM still has it, else the newest. An empty or unknown
/// setting means "newest", so a version RomM dropped never leaves the user with nothing selected.
pub fn selected_version<'a>(available: &'a [Version], setting: &str) -> Option<&'a Version> {
    parse_version_text(setting)
        .and_then(|wanted| available.iter().find(|v| **v == wanted))
        .or_else(|| newest(available.iter()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_file_name() {
        assert_eq!(classify("prod.keys"), FileKind::Key(KEY_FILES[0]));
        assert_eq!(classify("title.keys"), FileKind::Key(KEY_FILES[1]));
        assert!(matches!(classify("console.keys"), FileKind::Key(k) if !k.required));
        assert!(matches!(classify("key_retail.bin"), FileKind::Key(k) if k.emulators == ["ryujinx"]));
        assert_eq!(classify("Firmware.23.0.1.zip"), FileKind::Firmware(vec![23, 0, 1]));
        // names are exact: the emulators read these exact files
        assert_eq!(classify("PROD.KEYS"), FileKind::Other);
        assert_eq!(classify("title.keys_autogenerated"), FileKind::Other);
        assert_eq!(classify("readme.txt"), FileKind::Other);
    }

    #[test]
    fn required_keys_are_prod_and_title() {
        let required: Vec<&str> = KEY_FILES.iter().filter(|k| k.required).map(|k| k.file_name).collect();
        assert_eq!(required, ["prod.keys", "title.keys"]);
        // Eden and yuzu read the two required files; Ryujinx reads all four
        for emu in ["eden", "yuzu"] {
            assert_eq!(KEY_FILES.iter().filter(|k| k.emulators.contains(&emu)).count(), 2);
        }
        assert_eq!(KEY_FILES.iter().filter(|k| k.emulators.contains(&"ryujinx")).count(), 4);
    }

    #[test]
    fn firmware_versions_parse_and_compare_numerically() {
        assert_eq!(parse_firmware_version("Firmware.23.0.1.zip"), Some(vec![23, 0, 1]));
        assert_eq!(parse_firmware_version("Firmware 9.2.0.zip"), Some(vec![9, 2, 0]));
        assert_eq!(parse_firmware_version("firmware-18.1.0.ZIP"), Some(vec![18, 1, 0]));
        assert_eq!(parse_firmware_version("Firmware.20.5.zip"), Some(vec![20, 5]));
        for bad in ["Firmware.zip", "Firmware.x.y.z.zip", "Firmware.1.2.3.4.5.zip", "Firmware.23.0.1.rar", "prod.keys", "Firmware..zip"] {
            assert_eq!(parse_firmware_version(bad), None, "{bad}");
        }
        let versions = [vec![9, 2, 0], vec![23, 0, 1], vec![18, 1, 0]];
        assert_eq!(newest(versions.iter()), Some(&vec![23, 0, 1]), "9 < 18 < 23, not lexicographic");
        assert_eq!(newest(std::iter::empty()), None);
        assert_eq!(version_text(&[23, 0, 1]), "23.0.1");
    }

    #[test]
    fn the_saved_version_is_used_when_romm_has_it_else_the_newest() {
        let available = vec![vec![23, 0, 1], vec![18, 1, 0], vec![9, 2, 0]];
        assert_eq!(selected_version(&available, "18.1.0"), Some(&vec![18, 1, 0]));
        assert_eq!(selected_version(&available, ""), Some(&vec![23, 0, 1]), "empty means newest");
        assert_eq!(selected_version(&available, "20.0.0"), Some(&vec![23, 0, 1]), "a version RomM dropped falls back");
        assert_eq!(selected_version(&available, "garbage"), Some(&vec![23, 0, 1]));
        assert_eq!(selected_version(&available, " 9.2.0 "), Some(&vec![9, 2, 0]));
        assert_eq!(selected_version(&[], "9.2.0"), None);
        assert_eq!(parse_version_text("23.0.1"), Some(vec![23, 0, 1]));
        for bad in ["", "1..2", "a.b", "1.2.3.4.5", "v23.0.1", "-1"] {
            assert_eq!(parse_version_text(bad), None, "{bad}");
        }
    }

    #[test]
    fn switch_paths_match_romms_capitalised_folder() {
        for yes in ["bios/Switch", "bios/switch", "bios/Switch/", "/bios/SWITCH/prod.keys", "Switch"] {
            assert!(is_switch_path(yes), "{yes}");
        }
        for no in ["bios/ps", "bios/Switch2", "bios", "", "bios/gba/Switch"] {
            assert!(!is_switch_path(no), "{no}");
        }
    }

    #[test]
    fn keys_directories_are_the_ones_verified_on_the_deck() {
        assert_eq!(keys_dir("eden", "/home/deck").unwrap(), "/home/deck/.local/share/eden/keys");
        assert_eq!(keys_dir("yuzu", "/home/deck/").unwrap(), "/home/deck/.local/share/yuzu/keys");
        assert_eq!(keys_dir("ryujinx", "/home/deck").unwrap(), "/home/deck/.config/Ryujinx/system");
        assert_eq!(keys_dir("pcsx2", "/home/deck"), None);
        for e in EMULATORS {
            assert!(keys_dir(e, "/h").is_some());
        }
    }

    #[test]
    fn local_state_never_calls_a_user_file_a_match_without_proof() {
        assert_eq!(key_state(None, Some("aa")), KeyState::Missing);
        assert_eq!(key_state(None, None), KeyState::Missing);
        assert_eq!(key_state(Some("aa"), Some("AA")), KeyState::Matches);
        assert_eq!(key_state(Some("aa"), Some("bb")), KeyState::Differs);
        assert_eq!(key_state(Some("aa"), None), KeyState::Present, "offline: nothing to compare with");
        assert_eq!(key_state(Some("aa"), Some("")), KeyState::Present, "RomM reported no hash");
        assert_eq!(
            [KeyState::Missing, KeyState::Matches, KeyState::Differs, KeyState::Present].map(KeyState::as_str),
            ["missing", "matches", "differs", "present"]
        );
    }
}
