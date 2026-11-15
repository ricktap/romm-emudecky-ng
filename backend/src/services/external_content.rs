//! Keeps Eden scanning the Switch ROM folder for updates and DLC. Switch game folders hold their update and DLC files
//! in subfolders; with `<roms>/switch` in Eden's External Content folders (scanned recursively whenever a game starts,
//! also from a Steam shortcut) they work without installing anything to Eden's NAND.
use std::sync::Arc;
use crate::{services::ports::{EmuDeckPaths, ExternalContentDirs, ExternalContentOutcome}, util::path_safety::safe_join_lexical};

/// EmuDeck's ROM folder for the Switch.
pub const SWITCH_SYSTEM: &str = "switch";

pub struct ExternalContentService {
    paths: Arc<dyn EmuDeckPaths>,
    dirs: Arc<dyn ExternalContentDirs>,
}

impl ExternalContentService {
    pub fn new(paths: Arc<dyn EmuDeckPaths>, dirs: Arc<dyn ExternalContentDirs>) -> Self {
        Self { paths, dirs }
    }

    /// Make sure Eden scans `<roms>/switch`. `None` when EmuDeck's paths cannot be trusted right now (nothing is
    /// written then); otherwise what happened. Called at startup and after every Switch download, so a change skipped
    /// because Eden was running is made the next time.
    pub async fn ensure_switch_content(&self) -> Option<ExternalContentOutcome> {
        let health = self.paths.health();
        if !health.is_ok() {
            tracing::info!("Eden external content not checked: EmuDeck paths are {}", health.as_str());
            return None;
        }
        let dir = match safe_join_lexical(&self.paths.roms_path(), &[SWITCH_SYSTEM]) {
            Ok(dir) if dir.starts_with('/') => dir,
            Ok(dir) => {
                tracing::warn!("Eden external content not checked: ROM folder {dir:?} is not an absolute path");
                return None;
            }
            Err(e) => {
                tracing::warn!("Eden external content not checked: {e}");
                return None;
            }
        };
        let outcome = self.dirs.ensure(&dir).await;
        match &outcome {
            ExternalContentOutcome::Added => tracing::info!("Eden now loads updates and DLC from {dir}"),
            ExternalContentOutcome::AlreadyPresent => tracing::debug!("Eden already loads updates and DLC from {dir}"),
            ExternalContentOutcome::NotConfigured => tracing::info!("Eden has no config yet; external content folder not set"),
            ExternalContentOutcome::EmulatorRunning => tracing::info!("Eden is running; external content folder will be set later"),
            ExternalContentOutcome::Failed(e) => tracing::warn!("Cannot set Eden's external content folder: {e}"),
        }
        Some(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{domain::emudeck_health::EmuDeckHealth, services::testing::{FixedPaths, GatedPaths, RecordingContentDirs}};

    fn paths(roms: &str, health: EmuDeckHealth) -> Arc<GatedPaths> {
        Arc::new(GatedPaths {
            inner: FixedPaths { roms: roms.into(), bios: "/bios".into() },
            health: Arc::new(std::sync::Mutex::new(health)),
            storage: Arc::new(std::sync::Mutex::new(None)),
        })
    }

    #[tokio::test]
    async fn points_eden_at_the_switch_rom_folder_and_reports_the_outcome() {
        let dirs = Arc::new(RecordingContentDirs::answering(ExternalContentOutcome::Added));
        let svc = ExternalContentService::new(paths("/sd/Emulation/roms", EmuDeckHealth::Ok), dirs.clone());
        assert_eq!(svc.ensure_switch_content().await, Some(ExternalContentOutcome::Added));
        assert_eq!(dirs.calls(), ["/sd/Emulation/roms/switch"]);
    }

    #[tokio::test]
    async fn untrusted_emudeck_paths_write_nothing() {
        let dirs = Arc::new(RecordingContentDirs::answering(ExternalContentOutcome::Added));
        for health in [EmuDeckHealth::Absent, EmuDeckHealth::Unreadable] {
            let svc = ExternalContentService::new(paths("/sd/Emulation/roms", health), dirs.clone());
            assert_eq!(svc.ensure_switch_content().await, None);
        }
        // A relative ROM root is not a usable base either.
        for roms in ["", "Emulation/roms"] {
            let svc = ExternalContentService::new(paths(roms, EmuDeckHealth::Ok), dirs.clone());
            assert_eq!(svc.ensure_switch_content().await, None, "{roms:?}");
        }
        assert!(dirs.calls().is_empty(), "{:?}", dirs.calls());
    }

    #[tokio::test]
    async fn every_outcome_is_passed_through() {
        for outcome in [
            ExternalContentOutcome::AlreadyPresent,
            ExternalContentOutcome::NotConfigured,
            ExternalContentOutcome::EmulatorRunning,
            ExternalContentOutcome::Failed("disk full".into()),
        ] {
            let dirs = Arc::new(RecordingContentDirs::answering(outcome.clone()));
            let svc = ExternalContentService::new(paths("/roms", EmuDeckHealth::Ok), dirs);
            assert_eq!(svc.ensure_switch_content().await, Some(outcome));
        }
    }
}
