//! The plugin's own emulator table — which standalone emulators launch a
//! system, and how.
//!
//! Each covered system lists its emulators in preference order; each emulator
//! says where EmuDeck installs it (a glob relative to the user's home) and how
//! to call it (an args template whose last token is the ROM). Anything that is
//! pure data or pure decision-making over this table belongs here: the table,
//! the per-system launch-file extensions, picker options, the per-game →
//! per-platform → default precedence, and rendering the launch-command
//! prefix. Finding the emulators on disk is the adapter's job.

use std::collections::HashMap;

use crate::domain::launch::quote_launch_arg;

/// The ROM placeholder in an args template; it must be the last token.
pub const ROM_PLACEHOLDER: &str = "{rom}";
/// Steam replaces `%command%` with the shortcut's exe (`bin/rom-launcher-ng`).
pub const STEAM_COMMAND_TOKEN: &str = "%command%";
/// Mesa: disable vsync throttling for standalone emulators.
pub const STANDALONE_ENV: &[&str] = &["vblank_mode=0"];

/// One emulator entry of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableEmulator {
    pub emulator_id: &'static str,
    /// Display label and the per-game override pick key.
    pub label: &'static str,
    /// `/`-separated glob relative to the user's home, matched case-insensitively per component.
    pub locate_glob: &'static str,
    pub args: &'static [&'static str],
    pub env: &'static [&'static str],
}

const SWITCH: &[TableEmulator] = &[
    TableEmulator {
        emulator_id: "eden",
        label: "Eden",
        locate_glob: "Applications/Eden*.AppImage",
        args: &["-f", "-g", ROM_PLACEHOLDER],
        env: STANDALONE_ENV,
    },
    TableEmulator {
        emulator_id: "ryujinx",
        label: "Ryujinx",
        locate_glob: "Applications/publish/Ryujinx.sh",
        args: &[ROM_PLACEHOLDER],
        env: STANDALONE_ENV,
    },
    TableEmulator {
        emulator_id: "yuzu",
        label: "yuzu",
        locate_glob: "Applications/yuzu*.AppImage",
        args: &["-f", "-g", ROM_PLACEHOLDER],
        env: STANDALONE_ENV,
    },
];

/// Every covered system and its emulators in preference order.
pub const EMULATOR_TABLE: &[(&str, &[TableEmulator])] = &[("switch", SWITCH)];

/// Folder installs of these systems launch one file chosen by extension.
const LAUNCH_FILE_EXTENSIONS: &[(&str, &[&str])] = &[("switch", &[".xci", ".nsp"])];

/// The table entries for `system` in preference order (empty when uncovered).
pub fn table_for(system: Option<&str>) -> &'static [TableEmulator] {
    let Some(system) = system.filter(|s| !s.is_empty()) else {
        return &[];
    };
    EMULATOR_TABLE
        .iter()
        .find(|(s, _)| *s == system)
        .map_or(&[], |(_, entries)| entries)
}

/// Whether `system` resolves through the emulator table.
pub fn covers(system: Option<&str>) -> bool {
    !table_for(system).is_empty()
}

/// A table entry by id, across every system.
pub fn find_emulator(emulator_id: Option<&str>) -> Option<&'static TableEmulator> {
    let id = emulator_id.filter(|s| !s.is_empty())?;
    EMULATOR_TABLE
        .iter()
        .flat_map(|(_, entries)| entries.iter())
        .find(|e| e.emulator_id == id)
}

/// The launch-file extensions of `system`'s folder installs (`None` when uncovered).
pub fn launch_file_extensions(system: Option<&str>) -> Option<&'static [&'static str]> {
    let system = system.filter(|s| !s.is_empty())?;
    LAUNCH_FILE_EXTENSIONS
        .iter()
        .find(|(s, _)| *s == system)
        .map(|(_, exts)| *exts)
}

/// What a ROM launches with. Only table emulators exist in this backend:
/// `command` is the table emulator id, `launcher` its located executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmulatorInvocation {
    pub kind: &'static str,
    pub label: Option<String>,
    pub command: Option<String>,
    pub launcher: Option<String>,
}

impl EmulatorInvocation {
    pub fn table(emulator_id: &str, launcher: &str, label: &str) -> Self {
        Self {
            kind: TABLE_KIND,
            label: Some(label.into()),
            command: Some(emulator_id.into()),
            launcher: Some(launcher.into()),
        }
    }
}

/// `kind` of a table emulator option / invocation.
pub const TABLE_KIND: &str = "table";

/// One picker option for a system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmulatorOption {
    pub label: String,
    pub kind: &'static str,
    pub command: String,
    /// `bakeable` or `needs_setup`.
    pub status: &'static str,
    pub reason: Option<&'static str>,
    pub launcher: Option<String>,
}

/// Project `system`'s table into picker options; `installed` maps emulator id
/// to its located executable. Missing emulators are `needs_setup` /
/// `not_installed`.
pub fn table_options(system: &str, installed: &HashMap<String, String>) -> Vec<EmulatorOption> {
    table_for(Some(system))
        .iter()
        .map(|entry| {
            let path = installed.get(entry.emulator_id).filter(|p| !p.is_empty());
            EmulatorOption {
                label: entry.label.into(),
                kind: TABLE_KIND,
                command: entry.emulator_id.into(),
                status: if path.is_some() { "bakeable" } else { "needs_setup" },
                reason: if path.is_some() { None } else { Some("not_installed") },
                launcher: path.cloned(),
            }
        })
        .collect()
}

/// The first bakeable option (the system default).
pub fn select_default_option(options: &[EmulatorOption]) -> Option<&EmulatorOption> {
    options.iter().find(|o| o.status == "bakeable")
}

/// A bakeable option as an invocation (`None` otherwise).
pub fn option_to_invocation(option: Option<&EmulatorOption>) -> Option<EmulatorInvocation> {
    let option = option.filter(|o| o.status == "bakeable")?;
    let launcher = option.launcher.as_deref()?;
    (option.kind == TABLE_KIND && !option.command.is_empty())
        .then(|| EmulatorInvocation::table(&option.command, launcher, &option.label))
}

/// The option carrying `label`, as an invocation (`None` when missing or not bakeable).
pub fn label_to_invocation(options: &[EmulatorOption], label: &str) -> Option<EmulatorInvocation> {
    options
        .iter()
        .find(|o| o.label == label)
        .and_then(|o| option_to_invocation(Some(o)))
}

/// Which precedence layer a pin came from (for the stale-label warning).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleLayer {
    Game,
    Platform,
}

/// Per-game override → per-platform label → first installed emulator.
/// Returns the invocation and every pin that no longer resolves.
pub fn resolve_by_precedence(
    options: &[EmulatorOption],
    game_override: Option<&str>,
    platform_label: Option<&str>,
) -> (Option<EmulatorInvocation>, Vec<(StaleLayer, String)>) {
    let mut stale = Vec::new();
    if let Some(label) = game_override {
        if let Some(inv) = label_to_invocation(options, label) {
            return (Some(inv), stale);
        }
        stale.push((StaleLayer::Game, label.to_string()));
    }
    if let Some(label) = platform_label {
        if let Some(inv) = label_to_invocation(options, label) {
            return (Some(inv), stale);
        }
        stale.push((StaleLayer::Platform, label.to_string()));
    }
    (option_to_invocation(select_default_option(options)), stale)
}

/// The launch-command prefix of `entry` at `executable`:
/// `<env> %command% "<executable>" <args before {rom}>`. Errors when the
/// template does not end with exactly one `{rom}`.
pub fn render_invocation(entry: &TableEmulator, executable: &str) -> Result<String, String> {
    let ends_with_rom = entry.args.last() == Some(&ROM_PLACEHOLDER);
    if !ends_with_rom || entry.args[..entry.args.len() - 1].contains(&ROM_PLACEHOLDER) {
        return Err(format!(
            "args template for {} must end with exactly one {ROM_PLACEHOLDER}",
            entry.emulator_id
        ));
    }
    let quoted = quote_launch_arg(executable);
    let mut parts: Vec<&str> = entry.env.to_vec();
    parts.push(STEAM_COMMAND_TOKEN);
    parts.push(&quoted);
    parts.extend_from_slice(&entry.args[..entry.args.len() - 1]);
    Ok(parts.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::launch::build_launch_options;

    const EDEN: &str = "/home/deck/Applications/Eden.AppImage";
    const RYU: &str = "/home/deck/Applications/publish/Ryujinx.sh";
    const YUZU: &str = "/home/deck/Applications/yuzu.AppImage";

    fn installed(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn table_data() {
        let ids: Vec<_> = table_for(Some("switch")).iter().map(|e| e.emulator_id).collect();
        assert_eq!(ids, ["eden", "ryujinx", "yuzu"]);
        for (_, entries) in EMULATOR_TABLE {
            for e in *entries {
                assert_eq!(e.args.last(), Some(&ROM_PLACEHOLDER));
                assert!(!e.args[..e.args.len() - 1].contains(&ROM_PLACEHOLDER));
                assert!(!e.locate_glob.starts_with('/'));
                assert!(!e.locate_glob.contains("/home/deck"));
            }
        }
        assert!(table_for(Some("psx")).is_empty());
        assert!(table_for(None).is_empty());
        assert!(!covers(Some("psx")) && covers(Some("switch")));
        assert_eq!(find_emulator(Some("ryujinx")).unwrap().label, "Ryujinx");
        assert!(find_emulator(Some("nope")).is_none() && find_emulator(None).is_none());
        assert_eq!(launch_file_extensions(Some("switch")), Some(&[".xci", ".nsp"][..]));
        assert!(launch_file_extensions(Some("psx")).is_none() && launch_file_extensions(None).is_none());
    }

    #[test]
    fn options_and_selection() {
        let all = table_options("switch", &installed(&[("eden", EDEN), ("ryujinx", RYU), ("yuzu", YUZU)]));
        assert!(all.iter().all(|o| o.status == "bakeable"));
        let default = select_default_option(&all).unwrap();
        assert_eq!(default.label, "Eden");
        assert_eq!(
            option_to_invocation(Some(default)),
            Some(EmulatorInvocation::table("eden", EDEN, "Eden"))
        );
        let some = table_options("switch", &installed(&[("yuzu", YUZU), ("ryujinx", RYU)]));
        assert_eq!(select_default_option(&some).unwrap().label, "Ryujinx");
        let only_yuzu = table_options("switch", &installed(&[("yuzu", YUZU)]));
        let eden = &only_yuzu[0];
        assert_eq!((eden.status, eden.reason, eden.launcher.clone()), ("needs_setup", Some("not_installed"), None));
        let none = table_options("switch", &HashMap::new());
        assert!(select_default_option(&none).is_none());
        assert!(option_to_invocation(Some(&none[0])).is_none());
        let two = table_options("switch", &installed(&[("eden", EDEN), ("ryujinx", RYU)]));
        assert_eq!(
            label_to_invocation(&two, "Ryujinx"),
            Some(EmulatorInvocation::table("ryujinx", RYU, "Ryujinx"))
        );
        assert!(label_to_invocation(&table_options("switch", &installed(&[("eden", EDEN)])), "yuzu").is_none());
    }

    #[test]
    fn precedence() {
        let opts = table_options("switch", &installed(&[("eden", EDEN), ("ryujinx", RYU)]));
        let (inv, stale) = resolve_by_precedence(&opts, Some("Ryujinx"), Some("Eden"));
        assert_eq!(inv.unwrap().command.as_deref(), Some("ryujinx"));
        assert!(stale.is_empty());
        let (inv, stale) = resolve_by_precedence(&opts, Some("yuzu"), Some("Ryujinx"));
        assert_eq!(inv.unwrap().command.as_deref(), Some("ryujinx"));
        assert_eq!(stale, vec![(StaleLayer::Game, "yuzu".to_string())]);
        let (inv, stale) = resolve_by_precedence(&opts, Some("Bogus"), Some("yuzu"));
        assert_eq!(inv.unwrap().command.as_deref(), Some("eden"));
        assert_eq!(stale.len(), 2);
        let (inv, _) = resolve_by_precedence(&table_options("switch", &HashMap::new()), None, None);
        assert!(inv.is_none());
    }

    #[test]
    fn rendering() {
        let eden = find_emulator(Some("eden")).unwrap();
        assert_eq!(
            render_invocation(eden, EDEN).unwrap(),
            format!("vblank_mode=0 %command% \"{EDEN}\" -f -g")
        );
        assert_eq!(
            render_invocation(find_emulator(Some("ryujinx")).unwrap(), RYU).unwrap(),
            format!("vblank_mode=0 %command% \"{RYU}\"")
        );
        assert_eq!(
            render_invocation(find_emulator(Some("yuzu")).unwrap(), YUZU).unwrap(),
            format!("vblank_mode=0 %command% \"{YUZU}\" -f -g")
        );
        let bad = TableEmulator {
            args: &[ROM_PLACEHOLDER, "--fullscreen"],
            ..eden.clone()
        };
        assert!(render_invocation(&bad, "/x").is_err());
        let empty = TableEmulator { args: &[], ..eden.clone() };
        assert!(render_invocation(&empty, "/x").is_err());
        let twice = TableEmulator {
            args: &[ROM_PLACEHOLDER, ROM_PLACEHOLDER],
            ..eden.clone()
        };
        assert!(render_invocation(&twice, "/x").is_err());
    }

    /// A POSIX shell (what Steam runs launch options through) must yield
    /// exactly: env, %command%, exe, -f, -g, rom.
    #[test]
    fn full_command_tokenizes_to_exact_argv_in_sh() {
        let exe = "/home/deck/My Apps/Eden v2.AppImage";
        for rom in [
            "/run/media/mmcblk0p1/Emulation/roms/switch/Pokemon Shining Pearl [010018E011D92000][v0].nsp",
            "/roms/switch/Zelda's Echoes (v1.0) [DLC].xci",
            "/roms/switch/odd \"quoted\" name.nsp",
            "/roms/switch/back\\slash.nsp",
            "/roms/switch/$(touch pwned) `id` $HOME.nsp",
        ] {
            let options = build_launch_options(&render_invocation(find_emulator(Some("eden")).unwrap(), exe).unwrap(), rom);
            let out = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(format!("for a in {options}; do printf '%s\\0' \"$a\"; done"))
                .output()
                .unwrap();
            let argv: Vec<String> = String::from_utf8(out.stdout)
                .unwrap()
                .split_terminator('\0')
                .map(str::to_string)
                .collect();
            assert_eq!(argv, ["vblank_mode=0", "%command%", exe, "-f", "-g", rom], "{options}");
        }
    }
}
