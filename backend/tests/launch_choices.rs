//! Core/disc/relaunch callable parity through the real binary and Unix socket.
mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use common::{Backend, Dirs};
use serde_json::{Value, json};

fn executable(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn seed(dirs: &Dirs, folder: &str) {
    let c = rusqlite::Connection::open(dirs.db()).unwrap();
    c.execute_batch("INSERT INTO roms (rom_id,platform_slug,name,fs_name,last_synced_at,shortcut_app_id,sibling_group_key)
        VALUES (1,'switch','Game','Game','t',99,'igdb:7:1'),(2,'switch','Uninstalled','single.nsp','t',101,NULL),
        (3,'switch','Unbound','Game','t',NULL,NULL);").unwrap();
    for id in [1, 3] {
        c.execute("INSERT INTO rom_installs (rom_id,file_path,rom_dir,platform_slug,system,installed_at) VALUES (?1,?2,?3,'switch','switch','t')",
            rusqlite::params![id,format!("{folder}/base.xci"),folder]).unwrap();
    }
}

#[tokio::test]
async fn selections_rebake_shared_command_and_survive_backend_restart() {
    let dirs = Dirs::new();
    let eden = dirs.root.path().join("home/Applications/Eden.AppImage");
    let ryu = dirs.root.path().join("home/Applications/publish/Ryujinx.sh");
    executable(&eden);
    executable(&ryu);
    let folder = dirs.root.path().join("storage/Emulation/roms/switch/Game");
    std::fs::create_dir_all(folder.join("dlc")).unwrap();
    std::fs::write(folder.join("base.xci"), [0; 50]).unwrap();
    let selected = "update $thing `id` (EU).nsp";
    std::fs::write(folder.join("dlc").join(selected), [0; 5]).unwrap();
    let mut b = Backend::spawn(&dirs).await;
    seed(&dirs, folder.to_str().unwrap());
    let menu = b.call("get_platform_core_info", json!([1])).await;
    assert_eq!(menu["active_core_label"], "Eden");
    assert_eq!(menu["emulators"][2]["reason"], "not_installed");
    assert_eq!(b.call("set_game_core", json!([1,"yuzu"])).await["reason"], "core_unavailable");
    assert_eq!(b.call("set_game_core", json!([1,"Ryujinx"])).await["app_id"], 99);
    let pick = b.call("get_disc_selection", json!([1])).await;
    assert_eq!(pick["default"], json!({"kind":"file","label":"base","filename":"base.xci"}));
    assert_eq!(pick["discs"][1]["filename"], selected);
    let set = b.call("select_disc", json!([1,selected])).await;
    let expected = format!("vblank_mode=0 %command% \"{}\" \"{}/dlc/update \\$thing \\`id\\` (EU).nsp\"", ryu.display(), folder.display());
    assert_eq!(set, json!({"success":true,"selected":selected,"launch_options":expected}));
    assert_eq!(b.call("get_rom_relaunch_options", json!(["1"])).await, json!({"app_id":99,"launch_options":expected}));
    assert_eq!(b.call("get_installed_relaunch_options", json!([])).await, json!([{"app_id":99,"launch_options":expected}]));
    assert_eq!(b.call("get_rom_relaunch_options", json!([2])).await, Value::Null);
    assert_eq!(b.call("get_rom_relaunch_options", json!([3])).await, Value::Null);
    assert_eq!(b.call("set_game_core", json!([2,"Ryujinx"])).await, json!({"success":true,"launch_options":null,"app_id":null}));
    let c = rusqlite::Connection::open(dirs.db()).unwrap();
    c.execute_batch("CREATE TRIGGER reject_prefs BEFORE UPDATE ON roms BEGIN SELECT RAISE(ABORT, 'disk write failed'); END;").unwrap();
    for (name, args) in [("set_game_core",json!([1,"Eden"])),("clear_game_core",json!([1])),("select_disc",json!([1,null]))] {
        assert_eq!(b.call(name,args).await, json!({"success":false,"reason":"unknown","message":"disk write failed"}));
    }
    c.execute_batch("DROP TRIGGER reject_prefs;").unwrap();
    drop(c);
    b.shutdown().await;

    let mut b = Backend::spawn(&dirs).await;
    assert_eq!(b.call("get_platform_core_info", json!([1])).await["game_override_label"], "Ryujinx");
    assert_eq!(b.call("get_disc_selection", json!([1])).await["selected"], selected);
    assert_eq!(b.call("get_installed_relaunch_options", json!([])).await, json!([{"app_id":99,"launch_options":expected}]));
    let clear = b.call("clear_game_core", json!([1])).await;
    assert!(clear["launch_options"].as_str().unwrap().contains(&format!("\"{}\" -f -g", eden.display())));
    assert!(clear["launch_options"].as_str().unwrap().contains("/dlc/update \\$thing"));
    let reset = b.call("select_disc", json!([1,null])).await;
    assert_eq!(reset["selected"], Value::Null);
    assert!(reset["launch_options"].as_str().unwrap().ends_with("/base.xci\""));
    // Missing executables are re-probed; no file-only invocation escapes.
    std::fs::remove_file(eden).unwrap();
    std::fs::remove_file(ryu).unwrap();
    assert_eq!(b.call("get_installed_relaunch_options", json!([])).await, json!([{"app_id":99,"launch_options":""}]));
    b.shutdown().await;
}

#[tokio::test]
async fn callable_arity_types_and_missing_rom_contracts() {
    let dirs = Dirs::new();
    let mut b = Backend::spawn(&dirs).await;
    for (name, args) in [
        ("get_platform_core_info", json!([])), ("set_game_core", json!([1])),
        ("clear_game_core", json!([])), ("get_disc_selection", json!([1,2])),
        ("select_disc", json!([1])), ("get_rom_relaunch_options", json!([])),
        ("get_installed_relaunch_options", json!([1])),
    ] {
        assert!(b.request(name,args).await["error"]["message"].as_str().unwrap().contains("positional arguments"));
    }
    for (name, args) in [
        ("get_platform_core_info", json!([{}])), ("set_game_core", json!([1,null])),
        ("clear_game_core", json!(["bad"])), ("get_disc_selection", json!([false])),
        ("select_disc", json!([1,4])), ("get_rom_relaunch_options", json!([[]])),
    ] { assert!(b.request(name,args).await.get("error").is_some()); }
    for name in ["set_game_core", "clear_game_core"] {
        let args = if name == "set_game_core" { json!([999,"Eden"]) } else { json!([999]) };
        assert_eq!(b.call(name,args).await, json!({"success":false,"reason":"not_found","message":"ROM 999 is not tracked"}));
    }
    assert_eq!(b.call("get_disc_selection", json!([999])).await, json!({"multi_disc":false}));
    assert_eq!(b.call("select_disc", json!([999,null])).await["reason"], "not_installed");
    assert_eq!(b.call("get_rom_relaunch_options", json!([999])).await, Value::Null);
    assert_eq!(b.call("get_installed_relaunch_options", json!([])).await, json!([]));
    b.shutdown().await;
}
