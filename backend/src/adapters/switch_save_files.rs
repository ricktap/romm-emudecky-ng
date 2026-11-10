//! Eden/yuzu save directories on disk (ADR-0023): find them from the emulator's own config, list and stamp the title
//! folders, read a title's save as archive entries and put one back. Synchronous `std::fs`; callers run it on a blocking
//! thread. Never follows a symlink out of the save directory, never writes outside it, never replaces a save without
//! keeping the old one next to it as `<TITLE>.romm-backup[-n]`.
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use crate::domain::{
    save_archive::ArchiveEntry,
    switch_save::{parse_nand_directory, FolderStamp, NandSetting, TitleId, ZERO_PROFILE},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NandEmulator {
    Eden,
    Yuzu,
}

impl NandEmulator {
    fn dir_name(self) -> &'static str {
        match self {
            NandEmulator::Eden => "eden",
            NandEmulator::Yuzu => "yuzu",
        }
    }
}

/// `<nand>/user/save/0000000000000000`: the directory holding the profile folders. `None` when the emulator was never
/// configured (no `qt-config.ini`), the configured path is not absolute, or the directory does not exist: unsupported,
/// never a guessed fallback.
pub fn locate_save_dir(home: &Path, emulator: NandEmulator) -> Option<PathBuf> {
    let config = fs::read_to_string(home.join(".config").join(emulator.dir_name()).join("qt-config.ini")).ok()?;
    let nand = match parse_nand_directory(&config) {
        NandSetting::Custom(path) => {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return None;
            }
            path
        }
        NandSetting::Default => home.join(".local/share").join(emulator.dir_name()).join("nand"),
    };
    let dir = nand.join("user/save/0000000000000000");
    dir.is_dir().then(|| fs::canonicalize(&dir).ok()).flatten()
}

fn is_profile(name: &str) -> bool {
    name.len() == 32 && name.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The profile folders (32 hex digits) under the save directory, names as found on disk (not symlinks).
pub fn list_profiles(save_dir: &Path) -> Vec<String> {
    let Ok(read) = fs::read_dir(save_dir) else { return vec![] };
    let mut names: Vec<String> = read
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| is_profile(n))
        .collect();
    names.sort();
    names
}

fn title_dirs(profile_dir: &Path) -> Vec<(TitleId, PathBuf)> {
    let Ok(read) = fs::read_dir(profile_dir) else { return vec![] };
    read.flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let id = TitleId::parse(e.file_name().to_str()?)?;
            Some((id, e.path()))
        })
        .collect()
}

fn stamp_of(dir: &Path) -> FolderStamp {
    let mut stamp = FolderStamp { files: 0, bytes: 0, newest: 0 };
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(read) = fs::read_dir(&d) else { continue };
        for entry in read.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                stack.push(entry.path());
            } else if kind.is_file() {
                if let Ok(meta) = entry.metadata() {
                    stamp.files += 1;
                    stamp.bytes += meta.len();
                    let secs = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
                    stamp.newest = stamp.newest.max(secs);
                }
            }
        }
    }
    stamp
}

/// Every title folder under every profile, merged per title (a title can have a folder in several profiles).
pub fn snapshot(save_dir: &Path) -> BTreeMap<TitleId, FolderStamp> {
    let mut out: BTreeMap<TitleId, FolderStamp> = BTreeMap::new();
    for profile in list_profiles(save_dir) {
        for (id, dir) in title_dirs(&save_dir.join(&profile)) {
            let s = stamp_of(&dir);
            let merged = out.entry(id).or_insert(FolderStamp { files: 0, bytes: 0, newest: 0 });
            merged.files += s.files;
            merged.bytes += s.bytes;
            merged.newest = merged.newest.max(s.newest);
        }
    }
    out
}

fn collect_dir(base: &Path, role: &str, title: &str, out: &mut Vec<ArchiveEntry>) -> io::Result<()> {
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("symbolic link in a save: {}", entry.path().display())));
            }
            if kind.is_dir() {
                stack.push(entry.path());
            } else if kind.is_file() {
                let rel = entry.path().strip_prefix(base).map_err(io::Error::other)?.to_path_buf();
                let rel = rel.to_str().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 file name in a save"))?.replace('\\', "/");
                out.push(ArchiveEntry { path: format!("{role}/{title}/{rel}"), data: fs::read(entry.path())? });
            }
        }
    }
    Ok(())
}

/// The title's save as archive entries: the all-zero profile's folder as `zero/…`, the chosen user profile's as `user/…`.
/// A symbolic link anywhere inside fails the whole read (nothing is followed). No folder at all gives no entries.
pub fn collect(save_dir: &Path, title: TitleId, user_profile: Option<&str>) -> io::Result<Vec<ArchiveEntry>> {
    let hex = title.to_hex();
    let mut out = Vec::new();
    let zero = save_dir.join(ZERO_PROFILE).join(&hex);
    if zero.is_dir() {
        collect_dir(&zero, "zero", &hex, &mut out)?;
    }
    if let Some(profile) = user_profile {
        let user = save_dir.join(profile).join(&hex);
        if user.is_dir() {
            collect_dir(&user, "user", &hex, &mut out)?;
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn role_of<'a>(entry: &'a ArchiveEntry, hex: &str) -> io::Result<(&'a str, &'a str)> {
    let bad = || io::Error::new(io::ErrorKind::InvalidData, format!("entry does not belong to title {hex}: {}", entry.path));
    let mut parts = entry.path.splitn(3, '/');
    let (role, title, rel) = (parts.next().ok_or_else(bad)?, parts.next().ok_or_else(bad)?, parts.next().ok_or_else(bad)?);
    if title != hex || !matches!(role, "zero" | "user") {
        return Err(bad());
    }
    Ok((role, rel))
}

fn free_backup_name(dir: &Path, hex: &str) -> PathBuf {
    let first = dir.join(format!("{hex}.romm-backup"));
    if !first.exists() {
        return first;
    }
    (1u32..).map(|n| dir.join(format!("{hex}.romm-backup-{n}"))).find(|p| !p.exists()).unwrap_or(first)
}

fn replace_dir(profile_dir: &Path, hex: &str, files: &[(&str, &[u8])]) -> io::Result<Option<PathBuf>> {
    replace_dir_with(profile_dir, profile_dir, hex, files, &|from, to| fs::rename(from, to))
}

/// `replace_dir` with the rename injected, so the rollback of a failed final rename can be tested.
fn replace_dir_with(profile_dir: &Path, backup_dir: &Path, hex: &str, files: &[(&str, &[u8])], rename: &dyn Fn(&Path, &Path) -> io::Result<()>) -> io::Result<Option<PathBuf>> {
    fs::create_dir_all(profile_dir)?;
    let staging = profile_dir.join(format!(".{hex}.romm-tmp-{}", std::process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    let write = || -> io::Result<()> {
        for (rel, data) in files {
            let target = staging.join(rel);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&target, data)?;
        }
        if files.is_empty() {
            fs::create_dir_all(&staging)?;
        }
        Ok(())
    };
    if let Err(e) = write() {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    let target = profile_dir.join(hex);
    let backup = if target.exists() {
        fs::create_dir_all(backup_dir)?;
        let backup = free_backup_name(backup_dir, hex);
        rename(&target, &backup)?;
        Some(backup)
    } else {
        None
    };
    if let Err(e) = rename(&staging, &target) {
        if let Some(b) = &backup {
            let _ = fs::rename(b, &target); // put the old save back
        }
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    Ok(backup)
}

/// Put an archive's entries into the title's folders. Each role's folder is written to a temporary sibling, the current
/// folder is renamed to `<TITLE>.romm-backup[-n]` (never deleted), then the new one is renamed into place; if that last
/// step fails the old save is put back. `user` entries need `user_profile`; an entry for another title, role or an unsafe
/// relative path fails before anything is touched. Returns the backups made.
pub fn restore(save_dir: &Path, title: TitleId, user_profile: Option<&str>, entries: &[ArchiveEntry]) -> io::Result<Vec<PathBuf>> {
    let hex = title.to_hex();
    let mut zero: Vec<(&str, &[u8])> = Vec::new();
    let mut user: Vec<(&str, &[u8])> = Vec::new();
    for entry in entries {
        let (role, rel) = role_of(entry, &hex)?;
        if rel.is_empty() || rel.split('/').any(|c| c.is_empty() || c == "." || c == "..") || rel.contains('\\') || rel.contains('\0') {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("unsafe path in a save: {}", entry.path)));
        }
        if role == "zero" { zero.push((rel, &entry.data)) } else { user.push((rel, &entry.data)) }
    }
    if !user.is_empty() && user_profile.is_none_or(|p| !is_profile(p)) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "a user profile is needed to restore this save"));
    }
    let mut backups = Vec::new();
    if !zero.is_empty() {
        backups.extend(replace_dir(&save_dir.join(ZERO_PROFILE), &hex, &zero)?);
    }
    if !user.is_empty() {
        let profile = user_profile.unwrap_or_default();
        backups.extend(replace_dir(&save_dir.join(profile), &hex, &user)?);
    }
    Ok(backups)
}

/// Newest modification time (seconds since the epoch) of the title's files in the zero profile and the chosen user profile.
pub fn newest_mtime(save_dir: &Path, title: TitleId, user_profile: Option<&str>) -> f64 {
    let hex = title.to_hex();
    let mut dirs = vec![save_dir.join(ZERO_PROFILE).join(&hex)];
    if let Some(p) = user_profile {
        dirs.push(save_dir.join(p).join(&hex));
    }
    dirs.iter().filter(|d| d.is_dir()).map(|d| stamp_of(d).newest).max().unwrap_or(0) as f64
}

fn move_aside(profile_dir: &Path, hex: &str) -> io::Result<Option<PathBuf>> {
    let target = profile_dir.join(hex);
    if !target.exists() {
        return Ok(None);
    }
    let backup = free_backup_name(profile_dir, hex);
    fs::rename(&target, &backup)?;
    Ok(Some(backup))
}

/// Move the title's folders (zero profile and the chosen user profile) aside as `<TITLE>.romm-backup[-n]`; never deletes.
pub fn quarantine(save_dir: &Path, title: TitleId, user_profile: Option<&str>) -> io::Result<Vec<PathBuf>> {
    let hex = title.to_hex();
    let mut out = Vec::new();
    out.extend(move_aside(&save_dir.join(ZERO_PROFILE), &hex)?);
    if let Some(p) = user_profile.filter(|p| is_profile(p)) {
        out.extend(move_aside(&save_dir.join(p), &hex)?);
    }
    Ok(out)
}

// ── Ryujinx: a save is an index directory, found through its own metadata ──

use crate::domain::switch_save::{parse_ryujinx_extra_data, RYUJINX_ACCOUNT_SAVE};

/// `<config>/bis/user/save` of Ryujinx, when it exists.
pub fn ryujinx_save_root(home: &Path) -> Option<PathBuf> {
    let dir = home.join(".config/Ryujinx/bis/user/save");
    dir.is_dir().then(|| fs::canonicalize(&dir).ok()).flatten()
}

/// Every account-save index directory with the title it belongs to (read from `ExtraData0`; unreadable ones are skipped).
pub fn ryujinx_indices(root: &Path) -> Vec<(TitleId, PathBuf)> {
    let Ok(read) = fs::read_dir(root) else { return vec![] };
    let mut out: Vec<(TitleId, PathBuf)> = read
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        // Only real save indices (16 hex digits); a backup or stray folder is never mistaken for one.
        .filter(|e| e.file_name().to_str().is_some_and(|n| n.len() == 16 && n.bytes().all(|b| b.is_ascii_hexdigit())))
        .filter_map(|e| {
            let bytes = fs::read(e.path().join("ExtraData0")).ok()?;
            let save = parse_ryujinx_extra_data(&bytes)?;
            (save.save_type == RYUJINX_ACCOUNT_SAVE).then_some((save.title_id.base(), e.path()))
        })
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// The index directory of `title`, only when exactly one account save exists for it (several are ambiguous: no guess).
pub fn ryujinx_index_of(root: &Path, title: TitleId) -> Result<Option<PathBuf>, String> {
    let mut found: Vec<PathBuf> = ryujinx_indices(root).into_iter().filter(|(t, _)| *t == title).map(|(_, p)| p).collect();
    match found.len() {
        0 => Ok(None),
        1 => Ok(found.pop()),
        _ => Err("Ryujinx has several saves for this game".into()),
    }
}

/// The whole index directory as `index/...` entries (everything but the `.lock` file); symlinks fail the read.
pub fn ryujinx_collect(index: &Path) -> io::Result<Vec<ArchiveEntry>> {
    let mut out = Vec::new();
    let mut stack = vec![index.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("symbolic link in a save: {}", entry.path().display())));
            }
            if kind.is_dir() {
                stack.push(entry.path());
            } else if kind.is_file() && entry.file_name() != ".lock" {
                let rel = entry.path().strip_prefix(index).map_err(io::Error::other)?.to_str().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 file name"))?.replace('\\', "/");
                out.push(ArchiveEntry { path: format!("index/{rel}"), data: fs::read(entry.path())? });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// Replace an existing index directory's content with `entries` (the old one is kept as `<index>.romm-backup[-n]`, the
/// `.lock` file is carried over). Never creates an index: that is Ryujinx's own bookkeeping.
pub fn ryujinx_restore(index: &Path, entries: &[ArchiveEntry]) -> io::Result<Option<PathBuf>> {
    let parent = index.parent().ok_or_else(|| io::Error::other("no parent"))?;
    let name = index.file_name().and_then(|n| n.to_str()).ok_or_else(|| io::Error::other("bad index name"))?.to_string();
    let mut files: Vec<(&str, &[u8])> = Vec::new();
    for entry in entries {
        let rel = entry.path.strip_prefix("index/").ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("entry is not part of an index: {}", entry.path)))?;
        if rel.is_empty() || rel.split('/').any(|c| c.is_empty() || c == "." || c == "..") || rel.contains('\\') || rel.contains('\0') || rel == ".lock" {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("unsafe path in a save: {}", entry.path)));
        }
        files.push((rel, &entry.data));
    }
    let had_lock = index.join(".lock").exists();
    let backup = replace_dir_with_lock(parent, &name, &files, had_lock)?;
    Ok(backup)
}

/// Ryujinx's backups live beside the `save` directory (`bis/user/save.romm-backups`), never inside it.
fn replace_dir_with_lock(parent: &Path, name: &str, files: &[(&str, &[u8])], lock: bool) -> io::Result<Option<PathBuf>> {
    let mut with_lock: Vec<(&str, &[u8])> = files.to_vec();
    if lock {
        with_lock.push((".lock", &[]));
    }
    let backups = parent.parent().unwrap_or(parent).join("save.romm-backups");
    replace_dir_with(parent, &backups, name, &with_lock, &|from, to| fs::rename(from, to))
}

fn ryujinx_snapshot(root: &Path) -> BTreeMap<TitleId, FolderStamp> {
    ryujinx_indices(root).into_iter().map(|(t, dir)| (t, stamp_of(&dir))).collect()
}

/// The save root and the files of one emulator, for the service (all blocking).
#[derive(Clone)]
pub struct SwitchFiles {
    home: PathBuf,
}

impl SwitchFiles {
    pub fn new(home: PathBuf) -> Self {
        Self { home }
    }

    fn nand_emulator(emulator: &str) -> Option<NandEmulator> {
        match emulator {
            "eden" => Some(NandEmulator::Eden),
            "yuzu" => Some(NandEmulator::Yuzu),
            _ => None,
        }
    }

    /// `<nand>/system/save/8000000000000010/su/avators/profiles.dat` from the save dir (`<nand>/user/save/<zero>`).
    fn profiles_dat(save_dir: &Path) -> Option<PathBuf> {
        Some(save_dir.parent()?.parent()?.parent()?.join("system/save/8000000000000010/su/avators/profiles.dat"))
    }

    pub fn profiles(&self, emulator: &str) -> Result<Vec<crate::domain::switch_save::ProfileInfo>, String> {
        use crate::domain::switch_save::{parse_profiles_dat, ProfileInfo};
        let Some(kind) = Self::nand_emulator(emulator) else { return Ok(vec![]) };
        let dir = locate_save_dir(&self.home, kind).ok_or_else(|| format!("{emulator}: save folder not found"))?;
        let known = Self::profiles_dat(&dir).and_then(|p| fs::read(p).ok()).and_then(|b| parse_profiles_dat(&b)).unwrap_or_default();
        Ok(list_profiles(&dir)
            .into_iter()
            .map(|id| ProfileInfo { name: known.iter().find(|p| p.id.eq_ignore_ascii_case(&id)).map(|p| p.name.clone()).unwrap_or_default(), id })
            .collect())
    }

    pub fn read(&self, emulator: &str, title: TitleId, profile: Option<&str>) -> Result<(Vec<ArchiveEntry>, f64, String), String> {
        if let Some(kind) = Self::nand_emulator(emulator) {
            let dir = locate_save_dir(&self.home, kind).ok_or_else(|| format!("{emulator}: save folder not found"))?;
            let entries = collect(&dir, title, profile).map_err(|e| e.to_string())?;
            return Ok((entries, newest_mtime(&dir, title, profile), dir.join("..").to_string_lossy().into_owned()));
        }
        if emulator == "ryujinx" {
            let root = ryujinx_save_root(&self.home).ok_or("Ryujinx: save folder not found")?;
            return match ryujinx_index_of(&root, title)? {
                None => Ok((vec![], 0.0, root.to_string_lossy().into_owned())),
                Some(index) => {
                    let entries = ryujinx_collect(&index).map_err(|e| e.to_string())?;
                    Ok((entries, stamp_of(&index).newest as f64, index.to_string_lossy().into_owned()))
                }
            };
        }
        Err(format!("{emulator}: saves not supported"))
    }

    pub fn write(&self, emulator: &str, title: TitleId, profile: Option<&str>, entries: &[ArchiveEntry]) -> Result<Vec<String>, String> {
        if let Some(kind) = Self::nand_emulator(emulator) {
            let dir = locate_save_dir(&self.home, kind).ok_or_else(|| format!("{emulator}: save folder not found"))?;
            let backups = restore(&dir, title, profile, entries).map_err(|e| e.to_string())?;
            return Ok(backups.iter().map(|p| p.to_string_lossy().into_owned()).collect());
        }
        if emulator == "ryujinx" {
            let root = ryujinx_save_root(&self.home).ok_or("Ryujinx: save folder not found")?;
            let index = ryujinx_index_of(&root, title)?.ok_or("Start the game once in Ryujinx on this machine, then sync again: Ryujinx has no save for it yet")?;
            return Ok(ryujinx_restore(&index, entries).map_err(|e| e.to_string())?.iter().map(|p| p.to_string_lossy().into_owned()).collect());
        }
        Err(format!("{emulator}: saves not supported"))
    }

    pub fn quarantine(&self, emulator: &str, title: TitleId, profile: Option<&str>) -> Result<Vec<String>, String> {
        if let Some(kind) = Self::nand_emulator(emulator) {
            let dir = locate_save_dir(&self.home, kind).ok_or_else(|| format!("{emulator}: save folder not found"))?;
            return Ok(quarantine(&dir, title, profile).map_err(|e| e.to_string())?.iter().map(|p| p.to_string_lossy().into_owned()).collect());
        }
        Err("Ryujinx saves cannot be moved aside safely: remove the save in Ryujinx itself".into())
    }

    pub fn snapshot(&self, emulator: &str) -> Result<BTreeMap<TitleId, FolderStamp>, String> {
        if let Some(kind) = Self::nand_emulator(emulator) {
            let dir = locate_save_dir(&self.home, kind).ok_or_else(|| format!("{emulator}: save folder not found"))?;
            return Ok(snapshot(&dir));
        }
        if emulator == "ryujinx" {
            let root = ryujinx_save_root(&self.home).ok_or("Ryujinx: save folder not found")?;
            return Ok(ryujinx_snapshot(&root));
        }
        Err(format!("{emulator}: saves not supported"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    const USER: &str = "7EBDCC305B4640571756A9D7A0BCDB3C";
    const TITLE: &str = "01008CF01BAAC000";
    const OTHER: &str = "0100152000022000";

    fn tid(s: &str) -> TitleId {
        TitleId::parse(s).unwrap()
    }

    fn write(path: &Path, data: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, data).unwrap();
    }

    /// A save directory like the Deck's: a user profile and the all-zero profile.
    fn save_dir() -> (tempfile::TempDir, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("nand/user/save/0000000000000000");
        write(&dir.join(USER).join(TITLE).join("0/data.bin"), "user save");
        write(&dir.join(USER).join(TITLE).join("meta"), "m");
        write(&dir.join(USER).join(OTHER).join("x"), "other game");
        write(&dir.join(ZERO_PROFILE).join(OTHER).join("dev"), "device save");
        (t, dir)
    }

    fn home_with_config(config: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        write(&home.path().join(".config/eden/qt-config.ini"), config);
        home
    }

    #[test]
    fn the_save_dir_comes_from_the_configured_nand_directory() {
        let (t, dir) = save_dir();
        let nand = t.path().join("nand");
        let home = home_with_config(&format!("nand_directory\\default=false\nnand_directory={}\n", nand.display()));
        assert_eq!(locate_save_dir(home.path(), NandEmulator::Eden), Some(fs::canonicalize(&dir).unwrap()));
    }

    #[test]
    fn a_default_config_uses_the_emulators_own_data_directory() {
        let home = home_with_config("nand_directory\\default=true\n");
        let dir = home.path().join(".local/share/eden/nand/user/save/0000000000000000");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(locate_save_dir(home.path(), NandEmulator::Eden), Some(fs::canonicalize(&dir).unwrap()));
    }

    #[test]
    fn nothing_is_guessed_when_the_config_or_the_directory_is_missing_or_odd() {
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(locate_save_dir(empty.path(), NandEmulator::Eden), None, "never configured");
        let relative = home_with_config("nand_directory\\default=false\nnand_directory=relative/nand\n");
        assert_eq!(locate_save_dir(relative.path(), NandEmulator::Eden), None, "not absolute");
        let missing = home_with_config("nand_directory\\default=false\nnand_directory=/definitely/not/here\n");
        assert_eq!(locate_save_dir(missing.path(), NandEmulator::Eden), None, "directory missing");
        assert_eq!(locate_save_dir(missing.path(), NandEmulator::Yuzu), None, "a different emulator has its own config");
    }

    #[test]
    fn profiles_and_titles_are_listed_and_stamped_per_title_across_profiles() {
        let (_t, dir) = save_dir();
        write(&dir.join("not-a-profile/x"), "x");
        assert_eq!(list_profiles(&dir), vec![ZERO_PROFILE.to_string(), USER.to_string()]);
        let snap = snapshot(&dir);
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[&tid(TITLE)].files, 2);
        assert_eq!(snap[&tid(TITLE)].bytes, "user save".len() as u64 + 1);
        assert_eq!(snap[&tid(OTHER)].files, 2, "the other game has a save in both profiles");
    }

    #[test]
    fn collect_reads_zero_and_user_folders_with_role_rooted_paths() {
        let (_t, dir) = save_dir();
        let entries = collect(&dir, tid(OTHER), Some(USER)).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec![format!("user/{OTHER}/x"), format!("zero/{OTHER}/dev")].iter().map(String::as_str).collect::<Vec<_>>());
        let only_zero = collect(&dir, tid(OTHER), None).unwrap();
        assert_eq!(only_zero.len(), 1, "without a user profile only the zero folder is read");
        assert!(collect(&dir, tid("0100ABCD00000000"), Some(USER)).unwrap().is_empty());
    }

    #[test]
    fn a_symbolic_link_inside_a_save_fails_the_read() {
        let (_t, dir) = save_dir();
        symlink("/etc/passwd", dir.join(USER).join(TITLE).join("link")).unwrap();
        assert!(collect(&dir, tid(TITLE), Some(USER)).is_err());
    }

    #[test]
    fn restore_replaces_the_save_and_keeps_the_old_one_as_a_backup() {
        let (_t, dir) = save_dir();
        let entries = vec![
            ArchiveEntry { path: format!("user/{TITLE}/0/data.bin"), data: b"restored".to_vec() },
            ArchiveEntry { path: format!("user/{TITLE}/new"), data: b"n".to_vec() },
        ];
        let backups = restore(&dir, tid(TITLE), Some(USER), &entries).unwrap();
        assert_eq!(backups, vec![dir.join(USER).join(format!("{TITLE}.romm-backup"))]);
        let live = dir.join(USER).join(TITLE);
        assert_eq!(fs::read_to_string(live.join("0/data.bin")).unwrap(), "restored");
        assert!(!live.join("meta").exists(), "the old tree is replaced, not merged");
        assert_eq!(fs::read_to_string(backups[0].join("0/data.bin")).unwrap(), "user save", "the old save is kept");
        // a second restore never overwrites the first backup
        let again = restore(&dir, tid(TITLE), Some(USER), &entries).unwrap();
        assert_eq!(again, vec![dir.join(USER).join(format!("{TITLE}.romm-backup-1"))]);
        assert!(backups[0].exists());
    }

    #[test]
    fn restore_creates_a_save_that_does_not_exist_yet_without_a_backup() {
        let (_t, dir) = save_dir();
        let entries = vec![ArchiveEntry { path: format!("zero/0100ABCD00000000/f"), data: b"1".to_vec() }];
        assert_eq!(restore(&dir, tid("0100ABCD00000000"), None, &entries).unwrap(), Vec::<PathBuf>::new());
        assert_eq!(fs::read_to_string(dir.join(ZERO_PROFILE).join("0100ABCD00000000/f")).unwrap(), "1");
        assert!(fs::read_dir(dir.join(ZERO_PROFILE)).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().contains("romm-tmp")), "no staging left behind");
    }

    #[test]
    fn restore_refuses_bad_input_before_touching_anything() {
        let (_t, dir) = save_dir();
        let before = snapshot(&dir);
        let user_entry = |p: &str| vec![ArchiveEntry { path: p.into(), data: b"x".to_vec() }];
        let wrong_title = user_entry(&format!("user/{OTHER}/f"));
        assert!(restore(&dir, tid(TITLE), Some(USER), &wrong_title).is_err(), "another title");
        for bad in ["user/T/f", "other/{T}/f", "user/{T}/../x", "user/{T}//x", "user/{T}/", "user/{T}"] {
            let path = bad.replace("{T}", TITLE);
            assert!(restore(&dir, tid(TITLE), Some(USER), &user_entry(&path)).is_err(), "{path}");
        }
        let ok_entry = user_entry(&format!("user/{TITLE}/f"));
        assert!(restore(&dir, tid(TITLE), None, &ok_entry).is_err(), "user entries need a profile");
        assert!(restore(&dir, tid(TITLE), Some("../escape"), &ok_entry).is_err(), "the profile must be a profile folder name");
        assert_eq!(snapshot(&dir), before, "nothing changed");
    }

    #[test]
    fn a_failed_final_rename_puts_the_old_save_back_and_cleans_up() {
        let (_t, dir) = save_dir();
        let profile = dir.join(USER);
        let calls = std::cell::Cell::new(0);
        // 1st rename (target -> backup) works, 2nd (staging -> target) fails.
        let flaky = |from: &Path, to: &Path| {
            calls.set(calls.get() + 1);
            if calls.get() == 2 { Err(io::Error::other("injected")) } else { fs::rename(from, to) }
        };
        let result = replace_dir_with(&profile, &profile, TITLE, &[("f", b"new")], &flaky);
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(profile.join(TITLE).join("0/data.bin")).unwrap(), "user save", "the old save is back in place");
        assert!(fs::read_dir(&profile).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().contains("romm-")), "no backup or staging left");
    }

    #[test]
    fn a_failed_write_leaves_the_live_save_alone_and_no_staging_behind() {
        let (_t, dir) = save_dir();
        // A file where a directory is needed makes the staged write fail.
        let entries = vec![
            ArchiveEntry { path: format!("user/{TITLE}/a"), data: b"1".to_vec() },
            ArchiveEntry { path: format!("user/{TITLE}/a/b"), data: b"2".to_vec() },
        ];
        assert!(restore(&dir, tid(TITLE), Some(USER), &entries).is_err());
        assert_eq!(fs::read_to_string(dir.join(USER).join(TITLE).join("0/data.bin")).unwrap(), "user save");
        assert!(fs::read_dir(dir.join(USER)).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().contains("romm-")), "no backup or staging left");
    }

    // ── Ryujinx ──

    fn extra_data(title: &str, save_type: u8) -> Vec<u8> {
        let id = u64::from_str_radix(title, 16).unwrap();
        let mut bytes = vec![0u8; 512];
        bytes[..8].copy_from_slice(&id.to_le_bytes());
        bytes[8] = 1;
        bytes[0x20] = save_type;
        bytes
    }

    /// `bis/user/save` with one account save index for `TITLE` and a device-save index.
    fn ryujinx_root() -> (tempfile::TempDir, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join(".config/Ryujinx/bis/user/save");
        let idx = root.join("0000000000000001");
        fs::create_dir_all(&idx).unwrap();
        fs::write(idx.join("ExtraData0"), extra_data(TITLE, 1)).unwrap();
        fs::write(idx.join("ExtraData1"), extra_data(TITLE, 1)).unwrap();
        fs::write(idx.join(".lock"), b"").unwrap();
        write(&idx.join("0/save.meta"), "meta");
        write(&idx.join("0/slot00/data"), "progress");
        write(&idx.join("1/save.meta"), "meta1");
        let device = root.join("0000000000000002");
        fs::create_dir_all(&device).unwrap();
        fs::write(device.join("ExtraData0"), extra_data(TITLE, 3)).unwrap();
        (t, root)
    }

    #[test]
    fn ryujinx_finds_account_saves_by_the_title_in_their_own_metadata() {
        let (_t, root) = ryujinx_root();
        let all = ryujinx_indices(&root);
        assert_eq!(all.len(), 1, "the device save (type 3) is not synced");
        assert_eq!(ryujinx_index_of(&root, tid(TITLE)).unwrap(), Some(root.join("0000000000000001")));
        assert_eq!(ryujinx_index_of(&root, tid(OTHER)).unwrap(), None);
        // the title of an update still names its base game
        fs::write(root.join("0000000000000001/ExtraData0"), extra_data("01008CF01BAAC800", 1)).unwrap();
        assert_eq!(ryujinx_index_of(&root, tid(TITLE)).unwrap(), Some(root.join("0000000000000001")));
    }

    #[test]
    fn two_account_saves_for_one_title_are_ambiguous_and_not_guessed() {
        let (_t, root) = ryujinx_root();
        let second = root.join("0000000000000003");
        fs::create_dir_all(&second).unwrap();
        fs::write(second.join("ExtraData0"), extra_data(TITLE, 1)).unwrap();
        assert!(ryujinx_index_of(&root, tid(TITLE)).is_err());
    }

    #[test]
    fn a_folder_that_is_not_a_save_index_is_never_mistaken_for_one() {
        let (_t, root) = ryujinx_root();
        let stray = root.join("0000000000000001.romm-backup");
        fs::create_dir_all(&stray).unwrap();
        fs::write(stray.join("ExtraData0"), extra_data(TITLE, 1)).unwrap();
        assert_eq!(ryujinx_indices(&root).len(), 1);
        assert!(ryujinx_index_of(&root, tid(TITLE)).is_ok());
    }

    #[test]
    fn an_unreadable_extra_data_is_skipped() {
        let (_t, root) = ryujinx_root();
        let broken = root.join("0000000000000009");
        fs::create_dir_all(&broken).unwrap();
        fs::write(broken.join("ExtraData0"), b"short").unwrap();
        assert_eq!(ryujinx_indices(&root).len(), 1);
    }

    #[test]
    fn ryujinx_collects_the_whole_index_but_not_the_lock_and_restores_it_with_a_backup() {
        let (_t, root) = ryujinx_root();
        let index = root.join("0000000000000001");
        let entries = ryujinx_collect(&index).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["index/0/save.meta", "index/0/slot00/data", "index/1/save.meta", "index/ExtraData0", "index/ExtraData1"]);
        let new = vec![
            ArchiveEntry { path: "index/ExtraData0".into(), data: extra_data(TITLE, 1) },
            ArchiveEntry { path: "index/0/slot00/data".into(), data: b"restored".to_vec() },
        ];
        let backup = ryujinx_restore(&index, &new).unwrap().unwrap();
        assert_eq!(fs::read_to_string(index.join("0/slot00/data")).unwrap(), "restored");
        assert!(!index.join("1/save.meta").exists(), "replaced, not merged");
        assert!(index.join(".lock").exists(), "Ryujinx's lock file is carried over");
        assert_eq!(fs::read_to_string(backup.join("0/slot00/data")).unwrap(), "progress", "the old index is kept");
        assert_eq!(ryujinx_index_of(&root, tid(TITLE)).unwrap(), Some(index), "the restored index is still found");
        assert!(backup.starts_with(root.parent().unwrap().join("save.romm-backups")), "the backup is kept outside Ryujinx's save directory");
        assert!(fs::read_dir(&root).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().contains("romm-")));
    }

    #[test]
    fn ryujinx_restore_refuses_unsafe_or_foreign_entries_before_touching_anything() {
        let (_t, root) = ryujinx_root();
        let index = root.join("0000000000000001");
        for bad in ["user/x", "index/", "index/../x", "index/.lock", "index//x", "zero/T/x"] {
            let e = vec![ArchiveEntry { path: bad.into(), data: vec![1] }];
            assert!(ryujinx_restore(&index, &e).is_err(), "{bad}");
        }
        assert_eq!(fs::read_to_string(index.join("0/slot00/data")).unwrap(), "progress");
        assert!(fs::read_dir(&root).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().contains("romm-")));
    }

    // ── SwitchFiles end to end ──

    fn eden_home() -> (tempfile::TempDir, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let nand = t.path().join("storage/eden/nand");
        write(&t.path().join(".config/eden/qt-config.ini"), &format!("nand_directory\\default=false\nnand_directory={}\n", nand.display()));
        let dir = nand.join("user/save/0000000000000000");
        write(&dir.join(USER).join(TITLE).join("0/data.bin"), "user");
        write(&dir.join(ZERO_PROFILE).join(TITLE).join("dev"), "device");
        // profiles.dat with one named profile (the Deck's record)
        let mut dat = vec![0u8; 0x10 + 8 * 0xC8];
        let uuid: Vec<u8> = (0..16).map(|i| u8::from_str_radix(&USER[30 - i * 2..32 - i * 2], 16).unwrap()).collect();
        dat[0x10..0x20].copy_from_slice(&uuid);
        dat[0x10 + 0x28..0x10 + 0x28 + 4].copy_from_slice(b"Eden");
        let dat_path = nand.join("system/save/8000000000000010/su/avators/profiles.dat");
        fs::create_dir_all(dat_path.parent().unwrap()).unwrap();
        fs::write(dat_path, dat).unwrap();
        (t, dir)
    }

    #[test]
    fn eden_files_round_trip_through_the_service_adapter_with_names_from_profiles_dat() {
        let (t, _dir) = eden_home();
        let files = SwitchFiles::new(t.path().to_path_buf());
        let profiles = files.profiles("eden").unwrap();
        assert_eq!(profiles.iter().map(|p| (p.id.as_str(), p.name.as_str())).collect::<Vec<_>>(), vec![(ZERO_PROFILE, ""), (USER, "Eden")]);
        let (entries, mtime, _path) = files.read("eden", tid(TITLE), Some(USER)).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(mtime > 0.0);
        let changed = vec![ArchiveEntry { path: format!("user/{TITLE}/0/data.bin"), data: b"new".to_vec() }, ArchiveEntry { path: format!("zero/{TITLE}/dev"), data: b"dev2".to_vec() }];
        let backups = files.write("eden", tid(TITLE), Some(USER), &changed).unwrap();
        assert_eq!(backups.len(), 2, "both the user and the zero folder were kept as backups");
        assert_eq!(files.read("eden", tid(TITLE), Some(USER)).unwrap().0.iter().find(|e| e.path.ends_with("data.bin")).unwrap().data, b"new");
        let aside = files.quarantine("eden", tid(TITLE), Some(USER)).unwrap();
        assert_eq!(aside.len(), 2);
        assert!(files.read("eden", tid(TITLE), Some(USER)).unwrap().0.is_empty());
        assert!(files.snapshot("eden").unwrap().is_empty());
    }

    #[test]
    fn an_unconfigured_emulator_is_not_found_and_an_unknown_one_not_supported() {
        let t = tempfile::tempdir().unwrap();
        let files = SwitchFiles::new(t.path().to_path_buf());
        assert!(files.read("eden", tid(TITLE), None).unwrap_err().contains("not found"));
        assert!(files.read("ryujinx", tid(TITLE), None).unwrap_err().contains("not found"));
        assert!(files.read("dolphin", tid(TITLE), None).unwrap_err().contains("not supported"));
        assert_eq!(files.profiles("ryujinx").unwrap(), vec![], "Ryujinx has no profile folders");
        assert!(files.quarantine("ryujinx", tid(TITLE), None).is_err(), "Ryujinx saves are never moved aside");
    }

    #[test]
    fn ryujinx_through_the_service_adapter_never_creates_a_save_that_does_not_exist() {
        let (t, root) = ryujinx_root();
        let files = SwitchFiles::new(t.path().to_path_buf());
        let (entries, _, path) = files.read("ryujinx", tid(TITLE), None).unwrap();
        assert_eq!(entries.len(), 5);
        assert!(path.ends_with("0000000000000001"));
        assert!(files.write("ryujinx", tid(OTHER), None, &[]).unwrap_err().contains("Start the game once"));
        assert!(files.read("ryujinx", tid(OTHER), None).unwrap().0.is_empty());
        assert_eq!(files.snapshot("ryujinx").unwrap().keys().copied().collect::<Vec<_>>(), vec![tid(TITLE)]);
        let _ = root;
    }
}
