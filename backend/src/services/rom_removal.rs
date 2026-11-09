//! RomRemovalService — delete an installed ROM's files and its install record.
//!
//! An uninstall is not a purge: the `roms` row, playtime, saves and metadata
//! survive; only the on-disk files and the `rom_installs` row go. Every
//! delete is guarded by the roms-root containment check, and a single-file
//! ROM never takes its shared system folder with it.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::domain::errors::{failure, reason};
use crate::domain::rom_install::RomInstall;
use crate::services::ports::{DownloadFiles, EmuDeckPaths, LibraryRepo};
use crate::util::path_safety::is_safe_rom_path;

/// Everything [`RomRemovalService`] depends on.
pub struct RomRemovalServiceConfig {
    pub library: Arc<dyn LibraryRepo>,
    pub files: Arc<dyn DownloadFiles>,
    pub paths: Arc<dyn EmuDeckPaths>,
}

pub struct RomRemovalService {
    library: Arc<dyn LibraryRepo>,
    files: Arc<dyn DownloadFiles>,
    paths: Arc<dyn EmuDeckPaths>,
}

impl RomRemovalService {
    pub fn new(config: RomRemovalServiceConfig) -> Self {
        Self {
            library: config.library,
            files: config.files,
            paths: config.paths,
        }
    }

    async fn delete_rom_files(&self, install: &RomInstall) -> std::io::Result<()> {
        let roms_base = self.paths.roms_path();
        if let Some(dir) = install.rom_dir.as_deref()
            && is_safe_rom_path(dir, &roms_base)
            && self.files.is_dir(dir).await
        {
            return self.files.remove_tree(dir).await;
        }
        let path = install.file_path.as_str();
        if path.is_empty() {
            return Ok(());
        }
        if !is_safe_rom_path(path, &roms_base) {
            tracing::error!("Refusing to delete path outside roms directory: {path}");
            return Ok(());
        }
        if self.files.is_dir(path).await {
            self.files.remove_tree(path).await
        } else if self.files.exists(path).await {
            self.files.remove_file(path).await
        } else {
            Ok(())
        }
    }

    /// Delete one installed ROM's files, then its install record.
    /// `not_installed` when there is no record.
    pub async fn remove_rom(&self, rom_id: i64) -> Value {
        let install = match self.library.install(rom_id).await {
            Ok(Some(install)) => install,
            Ok(None) => return failure("not_installed", "ROM not installed"),
            Err(e) => {
                tracing::error!("Failed to read the install record of ROM {rom_id}: {e:#}");
                return failure(reason::UNKNOWN, "Failed to delete ROM files");
            }
        };
        let deleted = match self.delete_rom_files(&install).await {
            Ok(()) => self.library.delete_install(rom_id).await,
            Err(e) => Err(e.into()),
        };
        if let Err(e) = deleted {
            tracing::error!("Failed to delete ROM files: {e:#}");
            return failure(reason::UNKNOWN, "Failed to delete ROM files");
        }
        json!({"success": true, "message": "ROM removed"})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::testing::{DownloadEnv, FixedPaths};

    fn service(env: &DownloadEnv) -> RomRemovalService {
        RomRemovalService::new(RomRemovalServiceConfig {
            library: env.library.clone(),
            files: env.files.clone(),
            paths: Arc::new(FixedPaths {
                roms: env.roms_s(),
                bios: String::new(),
            }),
        })
    }

    async fn installed(env: &DownloadEnv, id: i64, file: &str, dir: Option<&str>) {
        env.seed_rom(id, "switch", "x", None, None).await;
        let i = RomInstall::mark_installed(id, file.into(), dir.map(str::to_string), "switch".into(), "switch".into(), "t".into())
            .unwrap();
        env.library.save_install(&i).await.unwrap();
    }

    #[tokio::test]
    async fn removes_folder_file_and_refuses_outside_paths() {
        let env = DownloadEnv::new().await;
        let svc = service(&env);
        assert_eq!(svc.remove_rom(1).await["reason"], "not_installed");

        let dir = env.roms().join("switch/G");
        std::fs::create_dir_all(dir.join("update")).unwrap();
        std::fs::write(dir.join("base.nsp"), b"x").unwrap();
        let d = dir.to_string_lossy().into_owned();
        installed(&env, 1, &format!("{d}/base.nsp"), Some(&d)).await;
        assert_eq!(svc.remove_rom(1).await["success"], true);
        assert!(!dir.exists());
        assert!(env.library.install(1).await.unwrap().is_none());
        assert!(env.library.rom(1).await.unwrap().is_some(), "the roms row survives");

        let file = env.roms().join("switch/a.nsp");
        std::fs::write(&file, b"x").unwrap();
        installed(&env, 2, &file.to_string_lossy(), None).await;
        assert_eq!(svc.remove_rom(2).await["success"], true);
        assert!(!file.exists());
        assert!(env.roms().join("switch").exists(), "the shared system folder stays");

        let outside = env.dir.path().join("outside.nsp");
        std::fs::write(&outside, b"x").unwrap();
        installed(&env, 3, &outside.to_string_lossy(), None).await;
        assert_eq!(svc.remove_rom(3).await["success"], true);
        assert!(outside.exists(), "never deletes outside the roms root");

        // A file path that is a folder, and one already gone.
        let as_dir = env.roms().join("switch/folder.nsp");
        std::fs::create_dir_all(&as_dir).unwrap();
        installed(&env, 4, &as_dir.to_string_lossy(), None).await;
        assert_eq!(svc.remove_rom(4).await["success"], true);
        assert!(!as_dir.exists());
        installed(&env, 5, &env.roms().join("switch/gone.nsp").to_string_lossy(), None).await;
        assert_eq!(svc.remove_rom(5).await["success"], true);
        installed(&env, 6, "", None).await;
        assert_eq!(svc.remove_rom(6).await["success"], true);

        env.sql("DROP TABLE rom_installs").await;
        assert_eq!(svc.remove_rom(7).await["reason"], "unknown");
    }
}
