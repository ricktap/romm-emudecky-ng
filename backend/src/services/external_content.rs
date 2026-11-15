//! Keeps Eden scanning the Switch ROM folder for updates and DLC. Switch game folders hold their update and DLC files
//! in subfolders; with `<roms>/switch` in Eden's External Content folders (scanned recursively whenever a game starts,
//! also from a Steam shortcut) they work without installing anything to Eden's NAND.
use std::sync::Arc;
use serde_json::{Value, json};
use crate::{domain::errors::failure, services::ports::{EmuDeckPaths, ExternalContentDirs, ExternalContentOutcome}, util::path_safety::safe_join_lexical};

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
        self.check().await.ok().map(|(_, outcome)| outcome)
    }

    /// The same check on demand (Danger zone), as a callable result: `{success: true, outcome, folder, message}` when
    /// Eden scans the folder now, otherwise `{success: false, reason, message}` saying what to do.
    pub async fn ensure_switch_content_result(&self) -> Value {
        let (folder, outcome) = match self.check().await {
            Ok(done) => done,
            Err(why) => return failure("emudeck_unavailable", &format!("EmuDeck's storage folder is not available ({why})")),
        };
        let done = |outcome: &str, message: &str| json!({"success": true, "outcome": outcome, "folder": folder, "message": message});
        match outcome {
            ExternalContentOutcome::Added => done("added", "Eden now loads updates and DLC from roms/switch"),
            ExternalContentOutcome::AlreadyPresent => done("already_present", "Eden already loads updates and DLC from roms/switch"),
            ExternalContentOutcome::NotConfigured => failure("eden_not_configured", "Start Eden once so it creates its settings, then try again"),
            ExternalContentOutcome::EmulatorRunning => failure("emulator_running", "Close Eden, then try again"),
            ExternalContentOutcome::Failed(e) => failure("write_failed", &e),
        }
    }

    /// `(folder, outcome)`, or why nothing was checked.
    async fn check(&self) -> Result<(String, ExternalContentOutcome), String> {
        let health = self.paths.health();
        if !health.is_ok() {
            tracing::info!("Eden external content not checked: EmuDeck paths are {}", health.as_str());
            return Err(health.as_str().to_string());
        }
        let dir = match safe_join_lexical(&self.paths.roms_path(), &[SWITCH_SYSTEM]) {
            Ok(dir) if dir.starts_with('/') => dir,
            Ok(dir) => {
                tracing::warn!("Eden external content not checked: ROM folder {dir:?} is not an absolute path");
                return Err("ROM folder is not an absolute path".into());
            }
            Err(e) => {
                tracing::warn!("Eden external content not checked: {e}");
                return Err(e.to_string());
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
        Ok((dir, outcome))
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
    async fn the_on_demand_result_says_what_happened_and_what_to_do() {
        let result = |outcome: ExternalContentOutcome| async move {
            let dirs = Arc::new(RecordingContentDirs::answering(outcome));
            ExternalContentService::new(paths("/sd/Emulation/roms", EmuDeckHealth::Ok), dirs).ensure_switch_content_result().await
        };
        let added = result(ExternalContentOutcome::Added).await;
        assert_eq!(added["success"], true);
        assert_eq!(added["outcome"], "added");
        assert_eq!(added["folder"], "/sd/Emulation/roms/switch");
        assert_eq!(result(ExternalContentOutcome::AlreadyPresent).await["outcome"], "already_present");
        for (outcome, reason, message) in [
            (ExternalContentOutcome::NotConfigured, "eden_not_configured", "Start Eden once so it creates its settings, then try again"),
            (ExternalContentOutcome::EmulatorRunning, "emulator_running", "Close Eden, then try again"),
            (ExternalContentOutcome::Failed("Cannot write x: disk full".into()), "write_failed", "Cannot write x: disk full"),
        ] {
            let r = result(outcome).await;
            assert_eq!((r["success"].clone(), r["reason"].clone(), r["message"].clone()), (json!(false), json!(reason), json!(message)));
            assert!(r.get("error").is_none() && r.get("error_code").is_none());
        }
        let dirs = Arc::new(RecordingContentDirs::answering(ExternalContentOutcome::Added));
        let r = ExternalContentService::new(paths("/sd", EmuDeckHealth::Absent), dirs.clone()).ensure_switch_content_result().await;
        assert_eq!(r["reason"], "emudeck_unavailable");
        assert!(dirs.calls().is_empty());
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
