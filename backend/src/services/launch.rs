//! LaunchService — the one launch-command resolver every bake site uses.
//!
//! Answers, for one installed ROM: which emulator it launches with (per-game
//! override → per-platform label → first installed table emulator), which
//! file it launches (a folder install's largest `.xci`/`.nsp`, or the
//! persisted `roms.selected_disc` pick), and the composed Steam
//! `launch_options`. Download completion and adoption both go through here.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::domain::disc_selection::{enumerate_size_ranked, resolve_size_ranked_path};
use crate::domain::emulator_table::{EmulatorInvocation, StaleLayer, TABLE_KIND, resolve_by_precedence, table_options};
use crate::domain::launch::{LaunchResolution, build_launch_options, resolve_invocation};
use crate::domain::rom_install::RomInstall;
use crate::services::ports::{DownloadFiles, EmulatorLocator, LibraryRepo, SettingsStore, SystemResolver};
use crate::util::path_safety::basename;

/// Everything [`LaunchService`] depends on.
pub struct LaunchServiceConfig {
    pub library: Arc<dyn LibraryRepo>,
    pub locator: Arc<dyn EmulatorLocator>,
    pub systems: Arc<dyn SystemResolver>,
    pub files: Arc<dyn DownloadFiles>,
    pub settings: SettingsStore,
}

pub struct LaunchService {
    library: Arc<dyn LibraryRepo>,
    locator: Arc<dyn EmulatorLocator>,
    systems: Arc<dyn SystemResolver>,
    files: Arc<dyn DownloadFiles>,
    settings: SettingsStore,
}

impl LaunchService {
    pub fn new(config: LaunchServiceConfig) -> Self {
        Self {
            library: config.library,
            locator: config.locator,
            systems: config.systems,
            files: config.files,
            settings: config.settings,
        }
    }

    /// The emulator ROM `rom_id` launches with, or `None` when its platform
    /// has no installed table emulator. Without a `roms` row the chain starts
    /// at the per-platform layer for `platform_slug` (a first sync bakes
    /// before its rows are written); with neither the plain launch applies.
    pub async fn active_emulator_for_rom(&self, rom_id: i64, platform_slug: Option<&str>) -> Option<EmulatorInvocation> {
        let row = match self.library.rom(rom_id).await {
            Ok(row) => row,
            Err(e) => {
                tracing::warn!("launch: reading rom {rom_id} failed: {e:#}");
                None
            }
        };
        let (slug, game_override) = match (&row, platform_slug.filter(|s| !s.is_empty())) {
            (Some(row), _) => (row.platform_slug.clone(), row.emulator_override.clone()),
            (None, Some(slug)) => (slug.to_string(), None),
            (None, None) => {
                tracing::warn!("active_core_resolver: no ROM for rom_id={rom_id}; resolving to plain launch");
                return None;
            }
        };
        let system = self.systems.resolve_system(&slug, None);
        let options = table_options(&system, &self.locator.installed(&system));
        let platform_label = self
            .settings
            .get("platform_cores")
            .and_then(|m| m.get(&slug).and_then(Value::as_str).map(str::to_string));
        let (invocation, stale) = resolve_by_precedence(&options, game_override.as_deref(), platform_label.as_deref());
        for (layer, label) in stale {
            match layer {
                StaleLayer::Game => tracing::warn!(
                    "active_core_resolver: per-game override '{label}' for rom_id={rom_id} no longer resolves on \
                     {system}; degrading to the per-platform/system default"
                ),
                StaleLayer::Platform => tracing::warn!(
                    "active_core_resolver: per-platform core '{label}' for {slug} (rom_id={rom_id}) no longer \
                     resolves; degrading to the system default"
                ),
            }
        }
        invocation
    }

    /// The path to bake for `install`: for a table system's folder install,
    /// the persisted pick or the largest launch file; otherwise `file_path`.
    pub async fn resolve_bake_path(&self, install: &RomInstall, selected_disc: Option<&str>) -> String {
        let Some(extensions) = crate::domain::emulator_table::launch_file_extensions(Some(&install.system)) else {
            return install.file_path.clone();
        };
        let Some(folder) = install.rom_dir.as_deref() else {
            return install.file_path.clone();
        };
        let files = self.files.scan_files_with_sizes(folder).await;
        let candidates = enumerate_size_ranked(&files, extensions);
        let (path, stale) = resolve_size_ranked_path(&install.file_path, &candidates, selected_disc);
        if stale {
            tracing::warn!(
                "disc_launch_resolver: pinned disc '{}' for rom_id={} is no longer present; degrading to the \
                 default launch target",
                selected_disc.unwrap_or_default(),
                install.rom_id
            );
        }
        path
    }

    /// Resolve `emulator` into a launch-command prefix, re-probing a table
    /// emulator's executable so a removed AppImage reads as `launcher_missing`.
    pub fn resolve_launch(&self, emulator: Option<&EmulatorInvocation>) -> LaunchResolution {
        if let Some(e) = emulator.filter(|e| e.kind == TABLE_KIND)
            && let Some(exe) = e.launcher.as_deref().filter(|l| !l.is_empty())
            && !self.locator.is_executable(exe)
        {
            return LaunchResolution::launcher_missing(basename(exe));
        }
        resolve_invocation(emulator)
    }

    /// The full `launch_options` for `emulator` launching `bake_path`.
    pub fn launch_options(&self, emulator: Option<&EmulatorInvocation>, bake_path: &str) -> String {
        build_launch_options(&self.resolve_launch(emulator).launch_options, bake_path)
    }

    /// `{app_id, launch_options}` of one installed and shortcut-bound ROM,
    /// `None` when it has no install or no shortcut.
    pub async fn relaunch_item_for_rom(&self, rom_id: i64) -> Option<Value> {
        let install = self.library.install(rom_id).await.ok().flatten()?;
        let rom = self.library.rom(rom_id).await.ok().flatten()?;
        let app_id = rom.shortcut_app_id?;
        let emulator = self.active_emulator_for_rom(rom_id, None).await;
        let bake = self.resolve_bake_path(&install, rom.selected_disc.as_deref()).await;
        Some(json!({"app_id": app_id, "launch_options": self.launch_options(emulator.as_ref(), &bake)}))
    }
}

#[cfg(test)]
#[path = "launch_tests.rs"]
mod tests;
