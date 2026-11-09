//! Host facts the download and launch flows need: EmuDeck's storage roots
//! (`~/.config/EmuDeck/settings.json`), the RomM platform → system map
//! (`config.json` shipped with the plugin), the located emulator-table
//! executables, and the system clock.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::domain::download_failure::iso_from_epoch;
use crate::domain::emulator_table::table_for;
use crate::services::ports::{Clock, EmuDeckPaths, EmulatorLocator, SystemResolver};

const EMULATION_DIR: &str = "Emulation";
const CONFIG_TTL: Duration = Duration::from_secs(30);

/// Reads `storagePath` from EmuDeck's `settings.json` (cached for 30 s). A
/// missing or unreadable file falls back to `<home>/Emulation`.
pub struct EmuDeckSettingsPaths {
    user_home: PathBuf,
    cache: Mutex<Option<(Instant, Option<String>)>>,
}

impl EmuDeckSettingsPaths {
    pub fn new(user_home: &Path) -> Self {
        Self {
            user_home: user_home.to_path_buf(),
            cache: Mutex::new(None),
        }
    }

    pub fn config_path(&self) -> PathBuf {
        self.user_home.join(".config/EmuDeck/settings.json")
    }

    fn storage_path(&self) -> Option<String> {
        let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, value)) = cache.as_ref()
            && at.elapsed() < CONFIG_TTL
        {
            return value.clone();
        }
        let raw = match std::fs::read(self.config_path()) {
            Ok(bytes) => bytes,
            Err(_) => {
                // A missing file is the pre-install state; re-read next time.
                *cache = None;
                return None;
            }
        };
        let value = match serde_json::from_slice::<Value>(&raw) {
            Ok(config) => config
                .get("storagePath")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(|s| expand(s, &self.user_home)),
            Err(e) => {
                tracing::warn!("Failed to load EmuDeck config at {}: {e}", self.config_path().display());
                None
            }
        };
        *cache = Some((Instant::now(), value.clone()));
        value
    }

    fn under_emulation(&self, sub: &str) -> String {
        let root = match self.storage_path() {
            Some(storage) => PathBuf::from(storage).join(EMULATION_DIR),
            None => self.user_home.join(EMULATION_DIR),
        };
        root.join(sub).to_string_lossy().into_owned()
    }
}

/// Expand `$VAR` / `${VAR}` from the environment, then a leading `~`.
fn expand(raw: &str, home: &Path) -> String {
    let mut out = String::new();
    let mut rest = raw;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let (name, consumed) = if let Some(inner) = after.strip_prefix('{') {
            match inner.find('}') {
                Some(end) => (&inner[..end], end + 2),
                None => ("", 0),
            }
        } else {
            let end = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            (&after[..end], end)
        };
        match std::env::var(name).ok().filter(|_| !name.is_empty()) {
            Some(value) => out.push_str(&value),
            None => out.push_str(&rest[i..i + 1 + consumed]),
        }
        rest = &after[consumed..];
    }
    out.push_str(rest);
    if out == "~" {
        return home.to_string_lossy().into_owned();
    }
    if let Some(tail) = out.strip_prefix("~/") {
        return home.join(tail).to_string_lossy().into_owned();
    }
    out
}

impl EmuDeckPaths for EmuDeckSettingsPaths {
    fn roms_path(&self) -> String {
        self.under_emulation("roms")
    }
    fn bios_path(&self) -> String {
        self.under_emulation("bios")
    }
}

/// `platform_map` from the plugin's `config.json` (plugin root, else
/// `defaults/`); unmapped slugs pass through verbatim.
pub struct PlatformMap {
    map: HashMap<String, String>,
}

impl PlatformMap {
    pub fn load(plugin_dir: &Path) -> Self {
        let root = plugin_dir.join("config.json");
        let path = if root.exists() {
            root
        } else {
            plugin_dir.join("defaults/config.json")
        };
        let map = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v.get("platform_map").and_then(Value::as_object).cloned())
            .map(|m| {
                m.into_iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
                    .collect()
            })
            .unwrap_or_else(|| {
                tracing::warn!("Failed to load platform_map from {}", path.display());
                HashMap::new()
            });
        Self { map }
    }
}

impl SystemResolver for PlatformMap {
    fn resolve_system(&self, platform_slug: &str, platform_fs_slug: Option<&str>) -> String {
        if let Some(s) = self.map.get(platform_slug) {
            return s.clone();
        }
        if let Some(s) = platform_fs_slug.filter(|s| !s.is_empty()).and_then(|f| self.map.get(f)) {
            return s.clone();
        }
        platform_slug.to_string()
    }
}

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Case-insensitive glob over one name: `*` and `?` wildcards.
fn glob_match(pattern: &[char], name: &[char]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some(('*', rest)) => (0..=name.len()).any(|i| glob_match(rest, &name[i..])),
        Some(('?', rest)) => !name.is_empty() && glob_match(rest, &name[1..]),
        Some((c, rest)) => name.first() == Some(c) && glob_match(rest, &name[1..]),
    }
}

/// Finds table emulators under the user's home (never a hardcoded `/home/deck`).
pub struct HomeEmulatorLocator {
    user_home: PathBuf,
}

impl HomeEmulatorLocator {
    pub fn new(user_home: &Path) -> Self {
        Self {
            user_home: user_home.to_path_buf(),
        }
    }

    fn match_component(parent: &Path, component: &str) -> Vec<PathBuf> {
        let Ok(rd) = std::fs::read_dir(parent) else {
            return Vec::new();
        };
        let wanted: Vec<char> = component.to_lowercase().chars().collect();
        let mut names: Vec<String> = rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| glob_match(&wanted, &n.to_lowercase().chars().collect::<Vec<_>>()))
            .collect();
        names.sort();
        names.into_iter().map(|n| parent.join(n)).collect()
    }

    /// The executable path of the emulator matching `glob`, if installed.
    pub fn locate(&self, glob: &str) -> Option<String> {
        if self.user_home.as_os_str().is_empty() {
            return None;
        }
        let mut candidates = vec![self.user_home.clone()];
        for component in glob.split('/').filter(|c| !c.is_empty()) {
            candidates = candidates
                .iter()
                .flat_map(|p| Self::match_component(p, component))
                .collect();
            if candidates.is_empty() {
                return None;
            }
        }
        candidates
            .into_iter()
            .find(|p| is_executable_file(p))
            .map(|p| p.to_string_lossy().into_owned())
    }
}

impl EmulatorLocator for HomeEmulatorLocator {
    fn installed(&self, system: &str) -> HashMap<String, String> {
        table_for(Some(system))
            .iter()
            .filter_map(|e| self.locate(e.locate_glob).map(|p| (e.emulator_id.to_string(), p)))
            .collect()
    }

    fn is_executable(&self, path: &str) -> bool {
        is_executable_file(Path::new(path))
    }
}

/// The real clock.
pub struct SystemClock {
    start: Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        Self { start: Instant::now() }
    }
}

impl Clock for SystemClock {
    fn epoch_seconds(&self)->f64 {std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64()}
    fn now_iso(&self) -> String {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        iso_from_epoch(secs)
    }
    fn monotonic(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn storage_path_and_fallback() {
        let home = tempfile::tempdir().unwrap();
        let p = EmuDeckSettingsPaths::new(home.path());
        assert_eq!(p.roms_path(), format!("{}/Emulation/roms", home.path().display()));
        std::fs::create_dir_all(home.path().join(".config/EmuDeck")).unwrap();
        std::fs::write(p.config_path(), r#"{"storagePath": "/run/media/mmcblk0p1"}"#).unwrap();
        assert_eq!(p.roms_path(), "/run/media/mmcblk0p1/Emulation/roms");
        assert_eq!(p.bios_path(), "/run/media/mmcblk0p1/Emulation/bios");
        // Cached for the TTL even after the file changes.
        std::fs::write(p.config_path(), r#"{"storagePath": "/elsewhere"}"#).unwrap();
        assert_eq!(p.roms_path(), "/run/media/mmcblk0p1/Emulation/roms");
        let fresh = EmuDeckSettingsPaths::new(home.path());
        assert_eq!(fresh.roms_path(), "/elsewhere/Emulation/roms");
        std::fs::write(fresh.config_path(), b"{broken").unwrap();
        let broken = EmuDeckSettingsPaths::new(home.path());
        assert_eq!(broken.roms_path(), format!("{}/Emulation/roms", home.path().display()));
        std::fs::write(fresh.config_path(), r#"{"storagePath": "  "}"#).unwrap();
        let blank = EmuDeckSettingsPaths::new(home.path());
        assert_eq!(blank.bios_path(), format!("{}/Emulation/bios", home.path().display()));
    }

    #[test]
    fn expansion() {
        let home = Path::new("/home/u");
        assert_eq!(expand("~/x", home), "/home/u/x");
        assert_eq!(expand("~", home), "/home/u");
        assert_eq!(expand("/plain", home), "/plain");
        assert_eq!(expand("$NOPE_UNSET_VAR/x", home), "$NOPE_UNSET_VAR/x");
        let path = std::env::var("PATH").unwrap();
        assert_eq!(expand("${PATH}", home), path);
        assert_eq!(expand("$PATH!", home), format!("{path}!"));
        assert_eq!(expand("${unclosed", home), "${unclosed");
    }

    #[test]
    fn platform_map() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("defaults")).unwrap();
        std::fs::write(
            dir.path().join("defaults/config.json"),
            r#"{"platform_map": {"nintendo-switch": "switch", "gba": "gba"}}"#,
        )
        .unwrap();
        let m = PlatformMap::load(dir.path());
        assert_eq!(m.resolve_system("nintendo-switch", None), "switch");
        assert_eq!(m.resolve_system("x", Some("gba")), "gba");
        assert_eq!(m.resolve_system("unmapped", Some("")), "unmapped");
        let empty = PlatformMap::load(&dir.path().join("missing"));
        assert_eq!(empty.resolve_system("switch", None), "switch");
    }

    #[test]
    fn locates_emulators_case_insensitively() {
        let home = tempfile::tempdir().unwrap();
        let loc = HomeEmulatorLocator::new(home.path());
        assert!(loc.installed("switch").is_empty());
        exe(&home.path().join("applications/eden-v0.1.AppImage"));
        exe(&home.path().join("Applications/publish/Ryujinx.sh"));
        std::fs::write(home.path().join("Applications/yuzu.AppImage"), b"not executable").ok();
        let found = loc.installed("switch");
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found["eden"].ends_with("eden-v0.1.AppImage"));
        assert!(found["ryujinx"].ends_with("publish/Ryujinx.sh"));
        assert!(loc.is_executable(&found["eden"]));
        assert!(!loc.is_executable("/nonexistent"));
        assert!(loc.installed("psx").is_empty());
        assert!(HomeEmulatorLocator::new(Path::new("")).locate("Applications/x").is_none());
        let c: Vec<char> = "a*b?".chars().collect();
        assert!(glob_match(&c, &"axxbz".chars().collect::<Vec<_>>()));
        assert!(!glob_match(&c, &"ab".chars().collect::<Vec<_>>()));
    }

    #[test]
    fn clock() {
        let c = SystemClock::default();
        assert!(c.now_iso().ends_with("+00:00"));
        assert!(c.monotonic() >= 0.0);
    }
}

/// Process-unique run/preview identities; generated only at the adapter boundary.
#[derive(Default)]
pub struct SyncRuntime {counter:std::sync::atomic::AtomicU64}
impl crate::services::ports::SyncIds for SyncRuntime {
    fn next_id(&self)->String {
        let n=self.counter.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
        let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        format!("{now:x}-{:x}-{n:x}",std::process::id())
    }
}
#[async_trait::async_trait]
impl crate::services::ports::SyncSleeper for SyncRuntime {
    async fn sleep(&self,seconds:f64) {tokio::time::sleep(std::time::Duration::from_secs_f64(seconds)).await;}
}
