//! Switch emulator and folder launch-file preferences. All commands are baked
//! by LaunchService; all probes run outside the DB actor's transactions.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::domain::disc_selection::resolve_size_ranked_path;
use crate::domain::emulator_table::{label_to_invocation, options_to_payload};
use crate::domain::errors::failure;
use crate::services::launch::LaunchService;
use crate::services::ports::{LaunchPreferencesRepo, LaunchSnapshot, SettingsPersister, SettingsStore};

pub struct LaunchChoicesServiceConfig {
    pub preferences: Arc<dyn LaunchPreferencesRepo>,
    pub launch: Arc<LaunchService>,
    pub settings: SettingsStore,
    pub persister: Arc<dyn SettingsPersister>,
}

pub struct LaunchChoicesService {
    preferences: Arc<dyn LaunchPreferencesRepo>,
    launch: Arc<LaunchService>,
    settings: SettingsStore,
    persister: Arc<dyn SettingsPersister>,
}

fn not_found(rom_id: i64) -> Value {
    failure("not_found", &format!("ROM {rom_id} is not tracked"))
}

fn state_changed() -> Value {
    failure("state_changed", "Game state changed while updating launch preferences. Try again.")
}

impl LaunchChoicesService {
    pub fn new(config: LaunchChoicesServiceConfig) -> Self {
        Self { preferences: config.preferences, launch: config.launch, settings: config.settings, persister: config.persister }
    }

    pub async fn get_platform_core_info(&self, rom_id: i64) -> anyhow::Result<Value> {
        let Some(snapshot) = self.preferences.launch_snapshot(rom_id).await? else {
            return Ok(json!({"emulators": [], "emulator_data_available": true,
                "active_core": null, "active_core_label": null, "platform_core_label": null,
                "has_game_override": false, "game_override_label": null}));
        };
        let rom = &snapshot.rom;
        let options = self.launch.emulator_options(&rom.platform_slug);
        let active = self.launch.active_emulator_for_row(rom);
        Ok(json!({"emulators": options_to_payload(&options), "emulator_data_available": true,
            "active_core": null, "active_core_label": active.and_then(|e| e.label),
            "platform_core_label": self.launch.platform_label(&rom.platform_slug),
            "has_game_override": rom.emulator_override.is_some(), "game_override_label": rom.emulator_override}))
    }

    pub async fn set_game_core(&self, rom_id: i64, label: &str) -> anyhow::Result<Value> {
        let Some(mut snapshot) = self.preferences.launch_snapshot(rom_id).await? else { return Ok(not_found(rom_id)); };
        let options = self.launch.emulator_options(&snapshot.rom.platform_slug);
        if label_to_invocation(&options, label).is_none() {
            return Ok(failure("core_unavailable", &format!("Emulator '{label}' is not available for {}", snapshot.rom.platform_slug)));
        }
        if !self.preferences.change_emulator_override(&snapshot, Some(label)).await? { return Ok(state_changed()); }
        snapshot.rom.emulator_override = Some(label.to_string());
        self.core_change_response(&snapshot).await
    }

    /// Choose (or, with `None`, clear) the emulator every game of `platform_slug` uses unless it has its own
    /// override. Only an emulator detected as installed can be chosen, by the same detection as the per-game
    /// choice. The caller re-bakes the shortcuts afterwards (`get_installed_relaunch_options`).
    pub async fn set_platform_core(&self, platform_slug: &str, label: Option<&str>) -> anyhow::Result<Value> {
        if let Some(label) = label {
            let options = self.launch.emulator_options(platform_slug);
            if label_to_invocation(&options, label).is_none() {
                return Ok(failure("core_unavailable", &format!("Emulator '{label}' is not available for {platform_slug}")));
            }
        }
        let previous = self.settings.get("platform_cores");
        self.settings.with(|m| {
            let entry = m.entry("platform_cores").or_insert_with(|| json!({}));
            if !entry.is_object() {
                *entry = json!({});
            }
            if let Some(map) = entry.as_object_mut() {
                match label {
                    Some(label) => {
                        map.insert(platform_slug.to_string(), json!(label));
                    }
                    None => {
                        map.shift_remove(platform_slug);
                    }
                }
            }
        });
        if let Err(e) = self.persister.save_settings().await {
            // Put the in-memory choice back: what is shown must be what is saved.
            self.settings.with(|m| match &previous {
                Some(old) => {
                    m.insert("platform_cores".into(), old.clone());
                }
                None => {
                    m.shift_remove("platform_cores");
                }
            });
            return Err(e);
        }
        Ok(json!({"success": true, "platform_core_label": label}))
    }

    pub async fn clear_game_core(&self, rom_id: i64) -> anyhow::Result<Value> {
        let Some(mut snapshot) = self.preferences.launch_snapshot(rom_id).await? else { return Ok(not_found(rom_id)); };
        if !self.preferences.change_emulator_override(&snapshot, None).await? { return Ok(state_changed()); }
        snapshot.rom.emulator_override = None;
        self.core_change_response(&snapshot).await
    }

    async fn core_change_response(&self, snapshot: &LaunchSnapshot) -> anyhow::Result<Value> {
        let item = self.launch.relaunch_item_for_snapshot(snapshot).await;
        Ok(json!({"success": true, "launch_options": item.as_ref().map(|v| &v["launch_options"]),
            "app_id": item.as_ref().map(|v| &v["app_id"])}))
    }

    pub async fn get_disc_selection(&self, rom_id: i64) -> anyhow::Result<Value> {
        let Some(snapshot) = self.preferences.launch_snapshot(rom_id).await? else { return Ok(json!({"multi_disc": false})); };
        let Some(install) = snapshot.install else { return Ok(json!({"multi_disc": false})); };
        let discs = self.launch.enumerate_launch_files(&install).await;
        if discs.len() < 2 { return Ok(json!({"multi_disc": false})); }
        let selected = snapshot.rom.selected_disc.filter(|s| discs.iter().any(|d| d.filename == *s));
        Ok(json!({"multi_disc": true,
            "discs": discs.iter().map(|d| json!({"filename": d.filename, "label": d.label, "index": d.index})).collect::<Vec<_>>(),
            "selected": selected,
            "default": {"kind": "file", "label": discs[0].label, "filename": discs[0].filename}}))
    }

    pub async fn select_disc(&self, rom_id: i64, filename: Option<&str>) -> anyhow::Result<Value> {
        let snapshot = self.preferences.launch_snapshot(rom_id).await?;
        let Some(snapshot) = snapshot.filter(|s| s.install.as_ref().is_some_and(|i| i.rom_dir.is_some())) else {
            return Ok(failure("not_installed", &format!("ROM {rom_id} is not installed as a multi-disc ROM")));
        };
        let install = snapshot.install.as_ref().unwrap();
        let discs = self.launch.enumerate_launch_files(install).await;
        if discs.len() < 2 {
            return Ok(failure("unsupported", &format!("ROM {rom_id} is not a multi-disc ROM")));
        }
        if let Some(filename) = filename && !discs.iter().any(|d| d.filename == filename) {
            return Ok(failure("not_found", &format!("'{filename}' is not a disc of ROM {rom_id}")));
        }
        // Store the exact enumerated basename, including significant whitespace.
        if !self.preferences.change_selected_disc(&snapshot, filename).await? { return Ok(state_changed()); }
        let (path, _) = resolve_size_ranked_path(&install.file_path, &discs, filename);
        let emulator = self.launch.active_emulator_for_row(&snapshot.rom);
        Ok(json!({"success": true, "launch_options": self.launch.launch_options(emulator.as_ref(), &path), "selected": filename}))
    }

    pub async fn get_rom_relaunch_options(&self, rom_id: i64) -> anyhow::Result<Value> {
        let Some(snapshot) = self.preferences.launch_snapshot(rom_id).await? else { return Ok(Value::Null); };
        Ok(self.launch.relaunch_item_for_snapshot(&snapshot).await.unwrap_or(Value::Null))
    }

    pub async fn get_installed_relaunch_options(&self) -> anyhow::Result<Value> {
        let snapshots = self.preferences.installed_launch_snapshots().await?;
        let mut items = Vec::new();
        for snapshot in snapshots {
            if let Some(item) = self.launch.relaunch_item_for_snapshot(&snapshot).await { items.push(item); }
        }
        Ok(Value::Array(items))
    }
}

#[cfg(test)]
#[path = "launch_choices_tests.rs"]
mod tests;
