//! How trustworthy the resolved EmuDeck roots are right now.
//!
//! Port of the Python reference `lib/emudeck_health.py`, with one deliberate change (CLAUDE.md, "Environment facts"):
//! a missing config is a **visible** error here, not a quiet pre-install state, and no state falls back to
//! `~/Emulation`. Anything but [`EmuDeckHealth::Ok`] stops downloads and installs.

use serde_json::{Value, json};

/// The five states of EmuDeck's configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmuDeckHealth {
    /// `settings.json` was read, `storagePath` is set and `<storagePath>/Emulation` exists.
    Ok,
    /// `~/.config/EmuDeck/settings.json` does not exist: EmuDeck is not installed or not set up yet.
    Absent,
    /// The file exists but cannot be read or parsed.
    Unreadable,
    /// The file was read but `storagePath` is missing or empty.
    Invalid,
    /// `storagePath` is set but `<storagePath>/Emulation` is not there (an ejected SD card, an unmounted drive).
    RootMissing,
}

/// What reading EmuDeck's `settings.json` found, before the disk is looked at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigRead {
    Absent,
    Unreadable,
    /// Read fine, but no usable `storagePath`.
    Invalid,
    /// The expanded `storagePath`.
    Storage(String),
}

/// Combine the config read with whether `<storagePath>/Emulation` exists.
pub fn classify(read: &ConfigRead, root_exists: bool) -> EmuDeckHealth {
    match read {
        ConfigRead::Absent => EmuDeckHealth::Absent,
        ConfigRead::Unreadable => EmuDeckHealth::Unreadable,
        ConfigRead::Invalid => EmuDeckHealth::Invalid,
        ConfigRead::Storage(_) if root_exists => EmuDeckHealth::Ok,
        ConfigRead::Storage(_) => EmuDeckHealth::RootMissing,
    }
}

impl EmuDeckHealth {
    pub fn is_ok(self) -> bool {
        self == Self::Ok
    }

    /// The slug the frontend routes on.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Absent => "absent",
            Self::Unreadable => "unreadable",
            Self::Invalid => "invalid",
            Self::RootMissing => "root_missing",
        }
    }

    /// One short sentence for the banner (the Deck UI cannot scroll, so no more than that).
    pub fn message(self) -> &'static str {
        match self {
            Self::Ok => "EmuDeck is set up",
            Self::Absent => "EmuDeck is not set up on this device. Install it and run its setup first.",
            Self::Unreadable => "EmuDeck's settings file cannot be read. Check ~/.config/EmuDeck/settings.json.",
            Self::Invalid => "EmuDeck's settings have no storage location. Run EmuDeck's setup again.",
            Self::RootMissing => "EmuDeck's storage is not available. Insert the SD card or mount the drive.",
        }
    }

    /// The `get_emudeck_health` answer.
    pub fn to_value(self, storage_path: Option<&str>) -> Value {
        json!({
            "health": self.as_str(),
            "ok": self.is_ok(),
            "message": self.message(),
            "storage_path": storage_path,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_every_state() {
        assert_eq!(classify(&ConfigRead::Absent, true), EmuDeckHealth::Absent);
        assert_eq!(classify(&ConfigRead::Unreadable, true), EmuDeckHealth::Unreadable);
        assert_eq!(classify(&ConfigRead::Invalid, true), EmuDeckHealth::Invalid);
        assert_eq!(classify(&ConfigRead::Storage("/sd".into()), true), EmuDeckHealth::Ok);
        assert_eq!(classify(&ConfigRead::Storage("/sd".into()), false), EmuDeckHealth::RootMissing);
    }

    #[test]
    fn only_ok_is_ok_and_every_state_has_a_distinct_slug_and_message() {
        let all = [
            EmuDeckHealth::Ok,
            EmuDeckHealth::Absent,
            EmuDeckHealth::Unreadable,
            EmuDeckHealth::Invalid,
            EmuDeckHealth::RootMissing,
        ];
        assert_eq!(all.iter().filter(|h| h.is_ok()).count(), 1);
        let slugs: std::collections::HashSet<_> = all.iter().map(|h| h.as_str()).collect();
        let messages: std::collections::HashSet<_> = all.iter().map(|h| h.message()).collect();
        assert_eq!(slugs.len(), 5);
        assert_eq!(messages.len(), 5);
    }

    #[test]
    fn the_answer_carries_slug_flag_message_and_path() {
        let v = EmuDeckHealth::RootMissing.to_value(Some("/run/media/sd"));
        assert_eq!(v["health"], "root_missing");
        assert_eq!(v["ok"], false);
        assert_eq!(v["storage_path"], "/run/media/sd");
        assert_eq!(EmuDeckHealth::Ok.to_value(None)["storage_path"], Value::Null);
    }
}
