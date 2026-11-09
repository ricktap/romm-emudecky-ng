//! RomInstall — where a downloaded (or adopted) ROM lives on disk.
//!
//! Exists only while the ROM is installed. `file_path` is always the launch
//! file; `rom_dir` names the dedicated per-ROM folder of a multi-file ROM and
//! is `None` for a single-file ROM, which owns no folder.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RomInstall {
    pub rom_id: i64,
    pub file_path: String,
    pub rom_dir: Option<String>,
    pub platform_slug: String,
    pub system: String,
    pub installed_at: String,
}

impl RomInstall {
    /// Record a freshly installed ROM. Rejects a non-positive `rom_id`.
    pub fn mark_installed(
        rom_id: i64,
        file_path: String,
        rom_dir: Option<String>,
        platform_slug: String,
        system: String,
        installed_at: String,
    ) -> Result<Self, String> {
        if rom_id <= 0 {
            return Err("rom_id must be positive".into());
        }
        Ok(Self {
            rom_id,
            file_path,
            rom_dir,
            platform_slug,
            system,
            installed_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_installed_validates_id() {
        let ok = RomInstall::mark_installed(1, "/r/a".into(), None, "gba".into(), "gba".into(), "t".into()).unwrap();
        assert_eq!(ok.rom_dir, None);
        assert!(RomInstall::mark_installed(0, "/r/a".into(), None, "gba".into(), "gba".into(), "t".into()).is_err());
    }
}
