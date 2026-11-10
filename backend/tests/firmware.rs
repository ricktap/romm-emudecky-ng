//! The Switch firmware status through the real backend: RomM's `bios/Switch` list (the shape a real
//! server returned) compared with the keys on this machine.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use common::fake_romm::FakeRomm;
use common::{Backend, Dirs};
use serde_json::{Value, json};

fn home(dirs: &Dirs) -> PathBuf {
    dirs.root.path().join("home")
}

fn install_emulator(dirs: &Dirs, relative: &str) {
    let exe = home(dirs).join(relative);
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn rom_entry(id: i64, name: &str, size: u64, sha1: &str) -> Value {
    json!({
        "id": id, "platform_id": 9, "file_name": name, "file_name_no_ext": name, "file_path": "bios/Switch",
        "file_size_bytes": size, "full_path": format!("bios/Switch/{name}"), "is_verified": false,
        "crc_hash": "00", "md5_hash": "00", "sha1_hash": sha1, "missing_from_fs": false,
    })
}

#[tokio::test]
async fn firmware_status_compares_romms_switch_files_with_the_emulators_keys_and_never_writes() {
    let romm = FakeRomm::start().await;
    let dirs = Dirs::new();
    let storage = dirs.root.path().join("sd");
    let config = home(&dirs).join(".config/EmuDeck");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("settings.json"), json!({"storagePath": storage}).to_string()).unwrap();
    install_emulator(&dirs, "Applications/Eden.AppImage");
    install_emulator(&dirs, "Applications/publish/Ryujinx.sh");
    dirs.write_settings(&json!({"romm_url": romm.url, "romm_api_token": "rmm_tok", "romm_api_token_origin": romm.url}));
    // The user's own keys: Eden has RomM's prod.keys ("abc"), Ryujinx has different ones.
    let eden_keys = home(&dirs).join(".local/share/eden/keys");
    std::fs::create_dir_all(&eden_keys).unwrap();
    std::fs::write(eden_keys.join("prod.keys"), b"abc").unwrap();
    let ryu_keys = home(&dirs).join(".config/Ryujinx/system");
    std::fs::create_dir_all(&ryu_keys).unwrap();
    std::fs::write(ryu_keys.join("prod.keys"), b"mine").unwrap();
    romm.set_firmware(json!([
        rom_entry(1, "console.keys", 451, "c1"),
        rom_entry(5, "Firmware.23.0.1.zip", 340_422_248, "f23"),
        rom_entry(3, "prod.keys", 3, "a9993e364706816aba3e25717850c26c9cd0d89d"),
        rom_entry(4, "title.keys", 1224, "t1"),
    ]));

    let mut b = Backend::spawn(&dirs).await;
    let status = b.call("get_firmware_status", json!([])).await;
    assert_eq!(status["success"], true);
    assert_eq!(status["server_offline"], false);
    let emulators: Vec<&str> = status["emulators"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert_eq!(emulators, ["eden", "ryujinx"], "only the emulators that are installed");
    let prod = status["keys"].as_array().unwrap().iter().find(|k| k["file_name"] == "prod.keys").unwrap();
    assert_eq!(prod["emulators"][0]["state"], "matches");
    assert_eq!(prod["emulators"][1]["state"], "differs");
    assert_eq!(status["firmware"][0]["version"], "23.0.1");
    assert_eq!(status["firmware"][0]["selected"], true);
    assert_eq!(status["firmware"][0]["downloaded"], false);

    // Still unported: the downloads. They answer the not-implemented result, and nothing was written.
    for call in ["download_all_firmware", "download_required_firmware"] {
        assert_eq!(b.call(call, json!(["switch"])).await["reason"], "not_implemented");
    }
    assert_eq!(std::fs::read(eden_keys.join("prod.keys")).unwrap(), b"abc");
    assert_eq!(std::fs::read(ryu_keys.join("prod.keys")).unwrap(), b"mine");
    assert!(!eden_keys.join("title.keys").exists());
    assert!(!storage.join("Emulation/bios").exists());
    b.shutdown().await;
}

#[tokio::test]
async fn firmware_status_says_offline_when_romm_has_no_firmware_endpoint() {
    let romm = FakeRomm::start().await; // no firmware list: 404
    let dirs = Dirs::new();
    dirs.write_settings(&json!({"romm_url": romm.url, "romm_api_token": "rmm_tok", "romm_api_token_origin": romm.url}));
    let mut b = Backend::spawn(&dirs).await;
    let status = b.call("get_firmware_status", json!([])).await;
    assert_eq!(status["success"], true);
    assert_eq!(status["server_offline"], true);
    assert!(status["firmware"].as_array().unwrap().is_empty());
    assert_eq!(status["keys"].as_array().unwrap().len(), 4, "the known key files are still listed");
    b.shutdown().await;
}
