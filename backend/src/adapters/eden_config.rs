//! Eden's `~/.config/eden/qt-config.ini`: keeps a folder in its External Content list (see `domain::eden_content`).
//! Written only while Eden is not running (Eden saves its config when it closes and would drop the change), atomically
//! (tmp → fsync → rename → fsync dir), with a one-time backup of the file as it was before the plugin first changed it.
use std::{fs, io::Write, path::{Path, PathBuf}};
use async_trait::async_trait;
use crate::{domain::eden_content::with_external_content_dir, services::ports::{ExternalContentDirs, ExternalContentOutcome}};

pub struct EdenConfig {
    config: PathBuf,
    proc_dir: PathBuf,
}

impl EdenConfig {
    pub fn new(home: &Path) -> Self {
        Self::with_proc(home, Path::new("/proc"))
    }
    /// `proc_dir` stands in for `/proc` in tests.
    pub fn with_proc(home: &Path, proc_dir: &Path) -> Self {
        Self { config: home.join(".config/eden/qt-config.ini"), proc_dir: proc_dir.to_path_buf() }
    }
}

/// Whether an Eden process is running: a process whose name (`comm`) starts with "eden", case-insensitively. That is
/// the emulator binary inside the AppImage as well as the AppImage runtime (`Eden-…AppImage`).
fn eden_running(proc_dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(proc_dir) else { return false };
    entries.flatten().filter(|e| e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit())).any(|e| {
        fs::read_to_string(e.path().join("comm")).is_ok_and(|comm| comm.trim().to_ascii_lowercase().starts_with("eden"))
    })
}

fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let tmp = dir.join(".qt-config.ini.romm-tmp");
    let result = (|| {
        let mut file = fs::File::create(&tmp)?;
        if let Ok(meta) = fs::metadata(path) {
            file.set_permissions(meta.permissions())?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
        fs::File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn ensure_blocking(config: &Path, proc_dir: &Path, dir: &str) -> ExternalContentOutcome {
    let Ok(ini) = fs::read_to_string(config) else { return ExternalContentOutcome::NotConfigured };
    let Some(updated) = with_external_content_dir(&ini, dir) else { return ExternalContentOutcome::AlreadyPresent };
    if eden_running(proc_dir) {
        return ExternalContentOutcome::EmulatorRunning;
    }
    let backup = config.with_extension("ini.romm-backup");
    if !backup.exists() {
        if let Err(e) = fs::copy(config, &backup) {
            return ExternalContentOutcome::Failed(format!("Cannot back up {}: {e}", config.display()));
        }
    }
    match write_atomically(config, updated.as_bytes()) {
        Ok(()) => ExternalContentOutcome::Added,
        Err(e) => ExternalContentOutcome::Failed(format!("Cannot write {}: {e}", config.display())),
    }
}

#[async_trait]
impl ExternalContentDirs for EdenConfig {
    async fn ensure(&self, dir: &str) -> ExternalContentOutcome {
        let (config, proc_dir, dir) = (self.config.clone(), self.proc_dir.clone(), dir.to_string());
        tokio::task::spawn_blocking(move || ensure_blocking(&config, &proc_dir, &dir))
            .await
            .unwrap_or_else(|e| ExternalContentOutcome::Failed(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Env {
        _dir: tempfile::TempDir,
        home: PathBuf,
        proc_dir: PathBuf,
    }
    impl Env {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let (home, proc_dir) = (dir.path().join("home"), dir.path().join("proc"));
            fs::create_dir_all(home.join(".config/eden")).unwrap();
            fs::create_dir_all(&proc_dir).unwrap();
            Self { _dir: dir, home, proc_dir }
        }
        fn config(&self) -> PathBuf { self.home.join(".config/eden/qt-config.ini") }
        fn process(&self, pid: &str, comm: &str) {
            fs::create_dir_all(self.proc_dir.join(pid)).unwrap();
            fs::write(self.proc_dir.join(pid).join("comm"), format!("{comm}\n")).unwrap();
        }
        async fn ensure(&self, dir: &str) -> ExternalContentOutcome {
            EdenConfig::with_proc(&self.home, &self.proc_dir).ensure(dir).await
        }
    }

    #[tokio::test]
    async fn adds_the_folder_once_keeps_a_backup_of_the_original_and_the_file_mode() {
        use std::os::unix::fs::PermissionsExt;
        let env = Env::new();
        fs::write(env.config(), "[UI]\ntheme=dark\n").unwrap();
        fs::set_permissions(env.config(), fs::Permissions::from_mode(0o640)).unwrap();
        env.process("1", "systemd");
        env.process("42", "steam");
        assert_eq!(env.ensure("/roms/switch").await, ExternalContentOutcome::Added);
        let ini = fs::read_to_string(env.config()).unwrap();
        assert_eq!(crate::domain::eden_content::external_content_dirs(&ini), ["/roms/switch"]);
        assert_eq!(fs::metadata(env.config()).unwrap().permissions().mode() & 0o777, 0o640);
        let backup = env.config().with_extension("ini.romm-backup");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "[UI]\ntheme=dark\n");
        assert_eq!(env.ensure("/roms/switch").await, ExternalContentOutcome::AlreadyPresent);
        // A later change keeps the first backup (the user's original), not an already-changed file.
        assert_eq!(env.ensure("/other").await, ExternalContentOutcome::Added);
        assert_eq!(fs::read_to_string(&backup).unwrap(), "[UI]\ntheme=dark\n");
        assert!(!env.home.join(".config/eden/.qt-config.ini.romm-tmp").exists());
    }

    #[tokio::test]
    async fn never_writes_while_eden_runs_or_before_eden_made_its_config() {
        let env = Env::new();
        assert_eq!(env.ensure("/roms/switch").await, ExternalContentOutcome::NotConfigured);
        assert!(!env.config().exists());
        fs::write(env.config(), "[UI]\n").unwrap();
        for comm in ["eden", "Eden-Linux-v0.2", "EDEN"] {
            let _ = fs::remove_dir_all(&env.proc_dir);
            env.process("77", comm);
            assert_eq!(env.ensure("/roms/switch").await, ExternalContentOutcome::EmulatorRunning, "{comm}");
            assert_eq!(fs::read_to_string(env.config()).unwrap(), "[UI]\n");
        }
        // Names that only contain "eden" elsewhere, and non-process entries, do not count.
        let _ = fs::remove_dir_all(&env.proc_dir);
        env.process("78", "sweden-helper");
        fs::create_dir_all(env.proc_dir.join("self")).unwrap();
        fs::write(env.proc_dir.join("self/comm"), "eden\n").unwrap();
        assert_eq!(env.ensure("/roms/switch").await, ExternalContentOutcome::Added);
    }

    #[tokio::test]
    async fn a_failed_write_reports_it_and_leaves_the_file() {
        let env = Env::new();
        fs::write(env.config(), "[UI]\n").unwrap();
        // The temp file's place is taken by a directory: creating it fails (also for root).
        let blocker = env.home.join(".config/eden/.qt-config.ini.romm-tmp");
        fs::create_dir_all(&blocker).unwrap();
        let outcome = env.ensure("/roms/switch").await;
        assert!(matches!(outcome, ExternalContentOutcome::Failed(_)), "{outcome:?}");
        assert_eq!(fs::read_to_string(env.config()).unwrap(), "[UI]\n");
        assert!(blocker.is_dir(), "a directory in the way is not removed");
    }
}
