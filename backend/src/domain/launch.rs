//! The Steam shortcut launch command: quoting, composition, and the
//! structured resolution of a ROM's emulator into a launch-command prefix.
//!
//! Every path that writes `launch_options` composes it here, so download
//! completion, adoption, and later bake sites never diverge.

use crate::domain::emulator_table::{EmulatorInvocation, TABLE_KIND, find_emulator, render_invocation};

/// Resolution states surfaced to the UI.
pub const RESOLUTION_OK: &str = "ok";
pub const RESOLUTION_NEEDS_SETUP: &str = "needs_setup";
pub const RESOLUTION_LAUNCHER_MISSING: &str = "launcher_missing";

/// Outcome of resolving a ROM's launch command. `launch_options` is empty
/// unless `status` is `ok`; `reason` explains the other states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchResolution {
    pub status: &'static str,
    pub launch_options: String,
    pub launcher_path: String,
    pub reason: String,
}

impl LaunchResolution {
    pub fn needs_setup(reason: &str) -> Self {
        Self {
            status: RESOLUTION_NEEDS_SETUP,
            launch_options: String::new(),
            launcher_path: String::new(),
            reason: reason.into(),
        }
    }

    pub fn launcher_missing(reason: &str) -> Self {
        Self {
            status: RESOLUTION_LAUNCHER_MISSING,
            reason: reason.into(),
            ..Self::needs_setup("")
        }
    }
}

/// Double-quote `value` as one launch-options argument. Steam runs launch
/// options through a shell, so `\` is escaped first, then `"`, `$` and the
/// backtick: a value can neither break out of the token nor trigger
/// expansion or command substitution.
pub fn quote_launch_arg(value: &str) -> String {
    let mut escaped = value.replace('\\', "\\\\");
    for ch in ['"', '$', '`'] {
        escaped = escaped.replace(ch, &format!("\\{ch}"));
    }
    format!("\"{escaped}\"")
}

/// Compose the launch command: the trusted `invocation` verbatim, then the
/// quoted ROM `path`.
pub fn build_launch_options(invocation: &str, path: &str) -> String {
    format!("{invocation} {}", quote_launch_arg(path))
}

/// Resolve `emulator` into a launch-command prefix. Only table emulators
/// render (EmuDeck plan D4: invoked directly through `rom-launcher-ng`);
/// `None` or any other kind is `needs_setup`. The adapter re-probes the
/// executable before calling this so a removed AppImage reads as
/// `launcher_missing`.
pub fn resolve_invocation(emulator: Option<&EmulatorInvocation>) -> LaunchResolution {
    let Some(emulator) = emulator else {
        return LaunchResolution::needs_setup("no_emulator_resolved");
    };
    if emulator.kind != TABLE_KIND {
        return LaunchResolution::needs_setup("unrenderable_emulator_kind");
    }
    let Some(entry) = find_emulator(emulator.command.as_deref()) else {
        return LaunchResolution::needs_setup("unknown_table_emulator");
    };
    let Some(launcher) = emulator.launcher.as_deref().filter(|l| !l.is_empty()) else {
        return LaunchResolution::needs_setup("emulator_not_installed");
    };
    match render_invocation(entry, launcher) {
        Ok(options) => LaunchResolution {
            status: RESOLUTION_OK,
            launch_options: options,
            launcher_path: launcher.into(),
            reason: String::new(),
        },
        Err(e) => LaunchResolution::needs_setup(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh_argv(words: &str) -> Vec<String> {
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("for a in {words}; do printf '%s\\0' \"$a\"; done"))
            .output()
            .unwrap();
        String::from_utf8(out.stdout)
            .unwrap()
            .split_terminator('\0')
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn quoting() {
        assert_eq!(quote_launch_arg("/roms/a.nsp"), "\"/roms/a.nsp\"");
        assert_eq!(quote_launch_arg("a b"), "\"a b\"");
        assert_eq!(quote_launch_arg(""), "\"\"");
        assert_eq!(quote_launch_arg("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(quote_launch_arg("a\\"), "\"a\\\\\"");
        assert_eq!(quote_launch_arg("$x`y`"), "\"\\$x\\`y\\`\"");
        assert_eq!(build_launch_options("inv", "/r/x y.nsp"), "inv \"/r/x y.nsp\"");
    }

    /// Every value survives a real `/bin/sh` round trip as exactly one argument.
    #[test]
    fn sh_round_trip() {
        for value in [
            "a b",
            "it's [v0] (USA)",
            "say \"hi\"",
            "back\\slash",
            "trailing\\",
            "\"",
            "$(rm -rf ~).nsp",
            "`id`",
            "$HOME",
            "quote \" and back\\slash \\\" mix",
            "Pokémon (Disc 1) ünïcödé.nsp",
        ] {
            assert_eq!(sh_argv(&quote_launch_arg(value)), vec![value.to_string()], "{value:?}");
        }
    }

    #[test]
    fn resolution() {
        assert_eq!(resolve_invocation(None).reason, "no_emulator_resolved");
        let ok = resolve_invocation(Some(&EmulatorInvocation::table("eden", "/h/Eden.AppImage", "Eden")));
        assert_eq!(ok.status, RESOLUTION_OK);
        assert_eq!(ok.launch_options, "vblank_mode=0 %command% \"/h/Eden.AppImage\" -f -g");
        assert_eq!(ok.launcher_path, "/h/Eden.AppImage");
        let unknown = resolve_invocation(Some(&EmulatorInvocation::table("nope", "/x", "Nope")));
        assert_eq!((unknown.status, unknown.reason.as_str()), (RESOLUTION_NEEDS_SETUP, "unknown_table_emulator"));
        let mut no_launcher = EmulatorInvocation::table("eden", "", "Eden");
        assert_eq!(resolve_invocation(Some(&no_launcher)).reason, "emulator_not_installed");
        no_launcher.kind = "libretro";
        assert_eq!(resolve_invocation(Some(&no_launcher)).reason, "unrenderable_emulator_kind");
        let missing = LaunchResolution::launcher_missing("Eden.AppImage");
        assert_eq!((missing.status, missing.launch_options.as_str()), (RESOLUTION_LAUNCHER_MISSING, ""));
    }
}
