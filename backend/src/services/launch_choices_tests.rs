//! Python core/disc/relaunch contracts with real SQLite and local ROM files.
use super::*;
use crate::domain::rom_install::RomInstall;
use crate::services::ports::LibraryRepo;
use crate::services::testing::{DownloadEnv, EDEN};

const RYU: &str = "/test-home/Applications/publish/Ryujinx.sh";

fn service(env: &DownloadEnv) -> LaunchChoicesService {
    LaunchChoicesService::new(LaunchChoicesServiceConfig { preferences: env.library.clone(), launch: env.launch.clone() })
}

async fn folder(env: &DownloadEnv, id: i64, app: Option<i64>) -> String {
    env.seed_rom(id, "switch", "G", app, Some("igdb:7:1")).await;
    let dir = env.roms().join(format!("switch/G{id}"));
    std::fs::create_dir_all(dir.join("dlc")).unwrap();
    std::fs::write(dir.join("base.XCI"), [0; 50]).unwrap();
    std::fs::write(dir.join("dlc/update.nsp"), [0; 5]).unwrap();
    std::fs::write(dir.join("ignored.txt"), [0; 100]).unwrap();
    let path = dir.to_string_lossy().into_owned();
    env.library.save_install(&RomInstall { rom_id: id, file_path: format!("{path}/base.XCI"),
        rom_dir: Some(path.clone()), system: "switch".into(), platform_slug: "switch".into(), installed_at: "t".into() }).await.unwrap();
    path
}

#[tokio::test]
async fn picker_contract_installed_default_precedence_and_stale_pin() {
    let env = DownloadEnv::new().await;
    env.locator.installed.lock().unwrap().insert("ryujinx".into(), RYU.into());
    env.seed_rom(1, "nintendo-switch", "a.nsp", None, None).await;
    let s = service(&env);
    assert_eq!(s.get_platform_core_info(999).await.unwrap(), json!({
        "emulators": [], "emulator_data_available": true, "active_core": null,
        "active_core_label": null, "platform_core_label": null, "has_game_override": false, "game_override_label": null,
    }));
    let info = s.get_platform_core_info(1).await.unwrap();
    assert_eq!(info, json!({
        "emulators": [
            {"label": "Eden", "kind": "standalone", "core_so": null, "is_default": true, "bakeable": true, "reason": null},
            {"label": "Ryujinx", "kind": "standalone", "core_so": null, "is_default": false, "bakeable": true, "reason": null},
            {"label": "yuzu", "kind": "standalone", "core_so": null, "is_default": false, "bakeable": false, "reason": "not_installed"},
        ], "emulator_data_available": true, "active_core": null, "active_core_label": "Eden",
        "platform_core_label": null, "has_game_override": false, "game_override_label": null,
    }));
    env.settings.set("platform_cores", json!({"nintendo-switch": "Ryujinx"}));
    assert_eq!(s.get_platform_core_info(1).await.unwrap()["active_core_label"], "Ryujinx");
    assert_eq!(s.set_game_core(1, "Eden").await.unwrap(), json!({"success": true, "launch_options": null, "app_id": null}));
    let pinned = s.get_platform_core_info(1).await.unwrap();
    assert_eq!(pinned["active_core_label"], "Eden");
    assert_eq!(pinned["platform_core_label"], "Ryujinx");
    assert_eq!(pinned["has_game_override"], true);
    env.locator.installed.lock().unwrap().remove("eden");
    let stale = s.get_platform_core_info(1).await.unwrap();
    assert_eq!(stale["active_core_label"], "Ryujinx");
    assert_eq!(stale["game_override_label"], "Eden");
    assert_eq!(stale["emulators"][0]["bakeable"], false);
    assert_eq!(stale["emulators"][1]["is_default"], true);
    assert_eq!(s.clear_game_core(1).await.unwrap()["success"], true);
    assert_eq!(s.get_platform_core_info(1).await.unwrap()["has_game_override"], false);
    env.locator.installed.lock().unwrap().clear();
    let missing = s.get_platform_core_info(1).await.unwrap();
    assert_eq!(missing["active_core_label"], Value::Null);
    assert!(missing["emulators"].as_array().unwrap().iter().all(|o| o["is_default"] == false));
    env.seed_rom(2, "gba", "g.gba", None, None).await;
    assert_eq!(s.get_platform_core_info(2).await.unwrap()["emulators"], json!([]));
    assert_eq!(s.set_game_core(2, "Eden").await.unwrap()["reason"], "core_unavailable");
}

#[tokio::test]
async fn core_changes_preserve_launch_file_install_binding_and_metadata() {
    let env = DownloadEnv::new().await;
    let dir = folder(&env, 1, Some(99)).await;
    env.locator.installed.lock().unwrap().insert("ryujinx".into(), RYU.into());
    let s = service(&env);
    s.select_disc(1, Some("update.nsp")).await.unwrap();
    let before = env.library.launch_snapshot(1).await.unwrap().unwrap();
    let changed = s.set_game_core(1, "Ryujinx").await.unwrap();
    assert_eq!(changed, json!({"success": true, "app_id": 99,
        "launch_options": format!("vblank_mode=0 %command% \"{RYU}\" \"{dir}/dlc/update.nsp\"")}));
    let after = env.library.launch_snapshot(1).await.unwrap().unwrap();
    assert_eq!(after.install, before.install);
    assert_eq!(after.rom.selected_disc.as_deref(), Some("update.nsp"));
    assert_eq!(after.rom.sibling_group_key, before.rom.sibling_group_key);
    assert_eq!(after.rom.name, before.rom.name);
    assert_eq!(after.rom.shortcut_app_id, Some(99));
    // Clear resolves AFTER the committed write, including the platform default.
    env.settings.set("platform_cores", json!({"switch": "Ryujinx"}));
    assert_eq!(s.clear_game_core(1).await.unwrap(), changed);
    env.settings.set("platform_cores", json!({}));
    let cleared = s.clear_game_core(1).await.unwrap();
    assert_eq!(cleared["launch_options"], format!("vblank_mode=0 %command% \"{EDEN}\" -f -g \"{dir}/dlc/update.nsp\""));
    // An emulator removed before a re-bake produces the empty placeholder.
    env.locator.missing.lock().unwrap().insert(EDEN.into());
    assert_eq!(s.clear_game_core(1).await.unwrap()["launch_options"], "");
    env.locator.installed.lock().unwrap().clear();
    assert_eq!(s.clear_game_core(1).await.unwrap()["launch_options"], "");
}

#[tokio::test]
async fn invalid_core_labels_never_write_and_unbound_pins_apply_on_next_install() {
    let env = DownloadEnv::new().await;
    let s = service(&env);
    folder(&env, 1, None).await;
    env.seed_rom(2, "switch", "a.nsp", Some(101), None).await;
    for id in [1, 2] {
        let before = env.library.launch_snapshot(id).await.unwrap();
        for label in ["yuzu", "unknown", "", "  ", " Eden "] {
            let result = s.set_game_core(id, label).await.unwrap();
            assert_eq!(result["reason"], "core_unavailable");
            assert_eq!(result.as_object().unwrap().len(), 3);
            assert_eq!(env.library.launch_snapshot(id).await.unwrap(), before);
        }
        assert_eq!(s.set_game_core(id, "Eden").await.unwrap(), json!({"success": true, "launch_options": null, "app_id": null}));
        assert_eq!(env.library.rom(id).await.unwrap().unwrap().emulator_override.as_deref(), Some("Eden"));
        assert_eq!(s.clear_game_core(id).await.unwrap(), json!({"success": true, "launch_options": null, "app_id": null}));
        assert_eq!(env.library.rom(id).await.unwrap().unwrap().emulator_override, None);
    }
    assert_eq!(s.set_game_core(999, "Eden").await.unwrap(), not_found(999));
    assert_eq!(s.clear_game_core(999).await.unwrap(), not_found(999));
}

#[tokio::test]
async fn launch_file_picker_ranks_pins_clears_and_down_validates_without_mutation() {
    let env = DownloadEnv::new().await;
    let dir = folder(&env, 1, Some(99)).await;
    let s = service(&env);
    let base = s.get_disc_selection(1).await.unwrap();
    assert_eq!(base, json!({"multi_disc": true,
        "discs": [{"filename": "base.XCI", "label": "base", "index": 1}, {"filename": "update.nsp", "label": "update", "index": 2}],
        "selected": null, "default": {"kind": "file", "label": "base", "filename": "base.XCI"}}));
    assert_eq!(s.select_disc(1, Some("update.nsp")).await.unwrap(), json!({"success": true, "selected": "update.nsp",
        "launch_options": format!("vblank_mode=0 %command% \"{EDEN}\" -f -g \"{dir}/dlc/update.nsp\"")}));
    assert_eq!(s.get_disc_selection(1).await.unwrap()["selected"], "update.nsp");
    let before = env.library.launch_snapshot(1).await.unwrap();
    for name in ["gone.nsp", "../base.XCI", "", "BASE.XCI", "/arbitrary/game.nsp"] {
        assert_eq!(s.select_disc(1, Some(name)).await.unwrap()["reason"], "not_found");
        assert_eq!(env.library.launch_snapshot(1).await.unwrap(), before);
    }
    let reset = s.select_disc(1, None).await.unwrap();
    assert_eq!(reset["selected"], Value::Null);
    assert!(reset["launch_options"].as_str().unwrap().ends_with("/base.XCI\""));
    // A vanished pick is displayed as default without destroying the stored pin.
    env.sql("UPDATE roms SET selected_disc='gone.nsp' WHERE rom_id=1").await;
    assert_eq!(s.get_disc_selection(1).await.unwrap(), base);
    assert_eq!(env.library.rom(1).await.unwrap().unwrap().selected_disc.as_deref(), Some("gone.nsp"));
    assert!(s.get_rom_relaunch_options(1).await.unwrap()["launch_options"].as_str().unwrap().ends_with("/base.XCI\""));
}

#[tokio::test]
async fn non_folder_and_fewer_than_two_candidates_have_no_picker_or_writes() {
    let env = DownloadEnv::new().await;
    let s = service(&env);
    env.seed_rom(1, "switch", "single.nsp", Some(1), None).await;
    for id in [1, 999] {
        assert_eq!(s.get_disc_selection(id).await.unwrap(), json!({"multi_disc": false}));
        assert_eq!(s.select_disc(id, None).await.unwrap()["reason"], "not_installed");
    }
    let dir = folder(&env, 2, Some(2)).await;
    std::fs::remove_file(format!("{dir}/dlc/update.nsp")).unwrap();
    for zero in [false, true] {
        if zero { std::fs::remove_file(format!("{dir}/base.XCI")).unwrap(); }
        assert_eq!(s.get_disc_selection(2).await.unwrap(), json!({"multi_disc": false}));
        assert_eq!(s.select_disc(2, None).await.unwrap()["reason"], "unsupported");
        assert_eq!(env.library.rom(2).await.unwrap().unwrap().selected_disc, None);
    }
    let mut install = env.library.install(2).await.unwrap().unwrap();
    install.rom_dir = None;
    env.library.save_install(&install).await.unwrap();
    assert_eq!(s.get_disc_selection(2).await.unwrap(), json!({"multi_disc": false}));
    assert_eq!(s.select_disc(2, None).await.unwrap()["reason"], "not_installed");
}

#[tokio::test]
async fn relaunch_batch_matches_single_and_excludes_unbound_uninstalled() {
    let env = DownloadEnv::new().await;
    let s = service(&env);
    assert_eq!(s.get_installed_relaunch_options().await.unwrap(), json!([]));
    let dir = folder(&env, 1, Some(99)).await;
    folder(&env, 2, None).await;
    env.seed_rom(3, "switch", "no_install.nsp", Some(103), None).await;
    assert_eq!(s.get_rom_relaunch_options(2).await.unwrap(), Value::Null);
    assert_eq!(s.get_rom_relaunch_options(3).await.unwrap(), Value::Null);
    assert_eq!(s.get_rom_relaunch_options(999).await.unwrap(), Value::Null);
    s.select_disc(1, Some("update.nsp")).await.unwrap();
    let item = s.get_rom_relaunch_options(1).await.unwrap();
    assert_eq!(s.get_installed_relaunch_options().await.unwrap(), json!([item]));
    assert!(item["launch_options"].as_str().unwrap().ends_with(&format!("{dir}/dlc/update.nsp\"")));
    env.locator.installed.lock().unwrap().clear();
    assert_eq!(s.get_installed_relaunch_options().await.unwrap(), json!([{"app_id": 99, "launch_options": ""}]));
}

#[tokio::test]
async fn conditional_writes_refuse_concurrent_pin_or_install_changes() {
    let env = DownloadEnv::new().await;
    folder(&env, 1, Some(99)).await;
    let original = env.library.launch_snapshot(1).await.unwrap().unwrap();
    assert!(env.library.change_emulator_override(&original, Some("Eden")).await.unwrap());
    assert!(!env.library.change_selected_disc(&original, Some("update.nsp")).await.unwrap());
    assert_eq!(env.library.rom(1).await.unwrap().unwrap().selected_disc, None);
    let current = env.library.launch_snapshot(1).await.unwrap().unwrap();
    env.library.delete_install(1).await.unwrap();
    assert!(!env.library.change_emulator_override(&current, None).await.unwrap());
    assert!(!env.library.change_selected_disc(&current, Some("update.nsp")).await.unwrap());
    assert_eq!(env.library.rom(1).await.unwrap().unwrap().emulator_override.as_deref(), Some("Eden"));
    assert!(env.library.launch_snapshot(999).await.unwrap().is_none());
}

#[tokio::test]
async fn database_write_failure_does_not_claim_success_or_lose_the_pin() {
    let env = DownloadEnv::new().await;
    folder(&env, 1, Some(99)).await;
    let s = service(&env);
    s.set_game_core(1, "Eden").await.unwrap();
    env.sql("CREATE TRIGGER reject_prefs BEFORE UPDATE ON roms BEGIN SELECT RAISE(ABORT, 'disk write failed'); END;").await;
    assert!(s.clear_game_core(1).await.unwrap_err().to_string().contains("disk write failed"));
    assert!(s.set_game_core(1, "Eden").await.is_err());
    assert!(s.select_disc(1, Some("update.nsp")).await.is_err());
    let rom = env.library.rom(1).await.unwrap().unwrap();
    assert_eq!(rom.emulator_override.as_deref(), Some("Eden"));
    assert_eq!(rom.selected_disc, None);
}

#[tokio::test]
async fn single_file_yuzu_launch_and_exact_whitespace_basename() {
    let env = DownloadEnv::new().await;
    let dir = folder(&env, 1, Some(99)).await;
    let s = service(&env);
    let name = " update.nsp";
    std::fs::write(format!("{dir}/{name}"), [0; 2]).unwrap();
    let result = s.select_disc(1, Some(name)).await.unwrap();
    assert_eq!(result["selected"], name);
    assert!(result["launch_options"].as_str().unwrap().ends_with("/ update.nsp\""));
    assert_eq!(s.get_disc_selection(1).await.unwrap()["selected"], name);
    let yuzu = "/test-home/Applications/yuzu.AppImage";
    env.locator.installed.lock().unwrap().insert("yuzu".into(), yuzu.into());
    let mut install = env.library.install(1).await.unwrap().unwrap();
    install.rom_dir = None;
    install.file_path = "/storage/switch/single.nsp".into();
    env.library.save_install(&install).await.unwrap();
    let changed = s.set_game_core(1, "yuzu").await.unwrap();
    assert_eq!(changed["launch_options"], format!("vblank_mode=0 %command% \"{yuzu}\" -f -g \"/storage/switch/single.nsp\""));
    assert_eq!(s.get_disc_selection(1).await.unwrap(), json!({"multi_disc":false}));
}

/// A concurrent uninstall or sync wins between probe and conditional commit.
struct ChangedDuringProbe(Arc<dyn LaunchPreferencesRepo>);

#[async_trait::async_trait]
impl LaunchPreferencesRepo for ChangedDuringProbe {
    async fn launch_snapshot(&self, id: i64) -> anyhow::Result<Option<LaunchSnapshot>> { self.0.launch_snapshot(id).await }
    async fn installed_launch_snapshots(&self) -> anyhow::Result<Vec<LaunchSnapshot>> { self.0.installed_launch_snapshots().await }
    async fn change_emulator_override(&self, _: &LaunchSnapshot, _: Option<&str>) -> anyhow::Result<bool> { Ok(false) }
    async fn change_selected_disc(&self, _: &LaunchSnapshot, _: Option<&str>) -> anyhow::Result<bool> { Ok(false) }
}

#[tokio::test]
async fn concurrent_changes_are_reported_without_claiming_a_rebake() {
    let env = DownloadEnv::new().await;
    folder(&env, 1, Some(99)).await;
    let s = LaunchChoicesService::new(LaunchChoicesServiceConfig {
        preferences: Arc::new(ChangedDuringProbe(env.library.clone())), launch: env.launch.clone(),
    });
    let before = env.library.launch_snapshot(1).await.unwrap();
    for result in [s.set_game_core(1, "Eden").await.unwrap(), s.clear_game_core(1).await.unwrap(), s.select_disc(1, Some("update.nsp")).await.unwrap()] {
        assert_eq!(result, state_changed());
        assert!(result.get("launch_options").is_none());
    }
    assert_eq!(env.library.launch_snapshot(1).await.unwrap(), before);
}
