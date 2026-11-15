//! LaunchService: emulator precedence, launch-file pick, relaunch items.

use serde_json::json;

use super::*;
use crate::services::testing::{DownloadEnv, EDEN};

const RYU: &str = "/home/deck/Applications/publish/Ryujinx.sh";

#[tokio::test]
async fn precedence_game_then_platform_then_default() {
    let env = DownloadEnv::new().await;
    env.locator.installed.lock().unwrap().insert("ryujinx".into(), RYU.into());
    env.seed_rom(1, "switch", "a.nsp", Some(5), None).await;
    let eden = env.launch.active_emulator_for_rom(1, None).await.unwrap();
    assert_eq!(eden.launcher.as_deref(), Some(EDEN));

    env.settings.set("platform_cores", json!({"switch": "Ryujinx"}));
    let ryu = env.launch.active_emulator_for_rom(1, None).await.unwrap();
    assert_eq!(ryu.command.as_deref(), Some("ryujinx"));

    env.sql("UPDATE roms SET emulator_override = 'Eden' WHERE rom_id = 1").await;
    let game = env.launch.active_emulator_for_rom(1, None).await.unwrap();
    assert_eq!(game.command.as_deref(), Some("eden"));

    // A stale per-game pin degrades to the per-platform layer.
    env.sql("UPDATE roms SET emulator_override = 'yuzu' WHERE rom_id = 1").await;
    let degraded = env.launch.active_emulator_for_rom(1, None).await.unwrap();
    assert_eq!(degraded.command.as_deref(), Some("ryujinx"));
}

#[tokio::test]
async fn first_sync_fallback_and_unknown_rom() {
    let env = DownloadEnv::new().await;
    // No roms row yet: the caller's platform starts the chain.
    let inv = env.launch.active_emulator_for_rom(42, Some("nintendo-switch")).await.unwrap();
    assert_eq!(inv.command.as_deref(), Some("eden"));
    assert!(env.launch.active_emulator_for_rom(42, None).await.is_none());
    assert!(env.launch.active_emulator_for_rom(42, Some("")).await.is_none());
    // Uncovered systems have no table emulator.
    env.seed_rom(2, "gba", "a.gba", None, None).await;
    assert!(env.launch.active_emulator_for_rom(2, None).await.is_none());
    // Nothing installed: no default.
    env.locator.installed.lock().unwrap().clear();
    assert!(env.launch.active_emulator_for_rom(42, Some("switch")).await.is_none());
}

#[tokio::test]
async fn a_dlc_bigger_than_the_base_game_never_becomes_the_default_launch_file() {
    let env = DownloadEnv::new().await;
    let dir = env.roms().join("switch/Small Base");
    std::fs::create_dir_all(dir.join("dlc")).unwrap();
    std::fs::create_dir_all(dir.join("update")).unwrap();
    std::fs::write(dir.join("base.nsp"), vec![0u8; 10]).unwrap();
    std::fs::write(dir.join("dlc/huge.nsp"), vec![0u8; 900]).unwrap();
    std::fs::write(dir.join("update/u.nsp"), vec![0u8; 500]).unwrap();
    let d = dir.to_string_lossy().into_owned();
    let install = RomInstall {
        rom_id: 1,
        file_path: format!("{d}/base.nsp"),
        rom_dir: Some(d.clone()),
        platform_slug: "switch".into(),
        system: "switch".into(),
        installed_at: "t".into(),
    };
    assert_eq!(env.launch.resolve_bake_path(&install, None).await, format!("{d}/base.nsp"));
    // The picker still offers every candidate: the top level first, then the rest by size.
    let names: Vec<(String, usize)> = env.launch.enumerate_launch_files(&install).await.into_iter().map(|c| (c.filename, c.index)).collect();
    assert_eq!(names, [("base.nsp".to_string(), 1), ("huge.nsp".to_string(), 2), ("u.nsp".to_string(), 3)]);
    // Without a candidate at the top, the largest one further down is the default.
    std::fs::remove_file(dir.join("base.nsp")).unwrap();
    assert_eq!(env.launch.resolve_bake_path(&install, None).await, format!("{d}/dlc/huge.nsp"));
}

#[tokio::test]
async fn bake_path_for_folder_and_single_installs() {
    let env = DownloadEnv::new().await;
    let dir = env.roms().join("switch/G");
    std::fs::create_dir_all(dir.join("dlc")).unwrap();
    std::fs::write(dir.join("base.xci"), vec![0u8; 50]).unwrap();
    std::fs::write(dir.join("dlc/d.nsp"), vec![0u8; 5]).unwrap();
    std::fs::write(dir.join("notes.txt"), vec![0u8; 500]).unwrap();
    let d = dir.to_string_lossy().into_owned();
    let folder = RomInstall {
        rom_id: 1,
        file_path: d.clone(),
        rom_dir: Some(d.clone()),
        platform_slug: "switch".into(),
        system: "switch".into(),
        installed_at: "t".into(),
    };
    assert_eq!(env.launch.resolve_bake_path(&folder, None).await, format!("{d}/base.xci"));
    assert_eq!(env.launch.resolve_bake_path(&folder, Some("d.nsp")).await, format!("{d}/dlc/d.nsp"));
    assert_eq!(env.launch.resolve_bake_path(&folder, Some("gone.nsp")).await, format!("{d}/base.xci"));
    let single = RomInstall {
        rom_dir: None,
        file_path: "/r/switch/x.nsp".into(),
        ..folder.clone()
    };
    assert_eq!(env.launch.resolve_bake_path(&single, Some("y")).await, "/r/switch/x.nsp");
    let other = RomInstall {
        system: "psx".into(),
        ..folder
    };
    assert_eq!(env.launch.resolve_bake_path(&other, None).await, d);
}

#[tokio::test]
async fn relaunch_item_needs_install_and_binding() {
    let env = DownloadEnv::new().await;
    env.seed_rom(1, "switch", "a.nsp", None, None).await;
    assert!(env.launch.relaunch_item_for_rom(1).await.is_none());
    let install = RomInstall::mark_installed(
        1,
        "/r/switch/a b.nsp".into(),
        None,
        "switch".into(),
        "switch".into(),
        "t".into(),
    )
    .unwrap();
    env.library.save_install(&install).await.unwrap();
    assert!(env.launch.relaunch_item_for_rom(1).await.is_none(), "unbound");
    env.seed_rom(1, "switch", "a.nsp", Some(99), None).await;
    env.library.save_install(&install).await.unwrap();
    assert_eq!(
        env.launch.relaunch_item_for_rom(1).await.unwrap(),
        json!({"app_id": 99, "launch_options": format!("vblank_mode=0 %command% \"{EDEN}\" -f -g \"/r/switch/a b.nsp\"")})
    );
    let resolution = env.launch.resolve_launch(None);
    assert_eq!(resolution.status, "needs_setup");
}
