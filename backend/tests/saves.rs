//! Save sync through the real socket: a Switch game's save goes up to a fake RomM and the status reads it back.
mod common;
use common::{Backend, Dirs};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use wiremock::{matchers::{method, path, query_param}, Mock, MockServer, ResponseTemplate};

const TITLE: &str = "01008CF01BAAC000";
const USER: &str = "7EBDCC305B4640571756A9D7A0BCDB3C";

fn write(path: &std::path::Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A machine with Eden installed, configured and holding a save for `TITLE`.
fn machine(d: &Dirs) -> std::path::PathBuf {
    let home = d.root.path().join("home");
    let nand = d.root.path().join("eden-nand");
    write(&home.join(".config/eden/qt-config.ini"), &format!("nand_directory\\default=false\nnand_directory={}\n", nand.display()));
    let save = nand.join("user/save/0000000000000000").join(USER).join(TITLE);
    write(&save.join("0/data.bin"), "level 3");
    let eden = home.join("Applications/Eden.AppImage");
    write(&eden, "#!/bin/sh\n");
    std::fs::set_permissions(&eden, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cfg = home.join(".config/EmuDeck");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("settings.json"), json!({"storagePath": d.root.path().join("storage")}).to_string()).unwrap();
    nand.join("user/save/0000000000000000").join(USER).join(TITLE)
}

fn seed(d: &Dirs) {
    let c = rusqlite::Connection::open(d.db()).unwrap();
    c.execute_batch(&format!(
        "INSERT INTO roms(rom_id,platform_slug,name,fs_name,last_synced_at,shortcut_app_id) VALUES(1,'switch','Zelda','Zelda [{TITLE}][v0].nsp','t',99);
         INSERT INTO rom_installs(rom_id,file_path,platform_slug,system,installed_at) VALUES(1,'/roms/switch/Zelda [{TITLE}][v0].nsp','switch','switch','t');"
    ))
    .unwrap();
}

#[tokio::test]
async fn a_switch_save_goes_up_as_a_zip_with_the_device_and_slot_and_the_status_reads_it_back() {
    let server = MockServer::start().await;
    Mock::given(path("/api/heartbeat")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"SYSTEM": {"VERSION": "4.9.0"}}))).mount(&server).await;
    Mock::given(method("POST")).and(path("/api/devices")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "dev-1"}))).expect(1).mount(&server).await;
    Mock::given(method("PUT")).and(path("/api/devices/dev-1")).respond_with(ResponseTemplate::new(200).set_body_json(json!({}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/api/saves")).and(query_param("rom_id", "1")).respond_with(ResponseTemplate::new(200).set_body_json(json!([]))).up_to_n_times(1).mount(&server).await;
    Mock::given(method("POST")).and(path("/api/saves")).and(query_param("emulator", "eden")).and(query_param("slot", "default")).and(query_param("device_id", "dev-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 5, "updated_at": "2026-01-01T00:00:00+00:00", "file_size_bytes": 200}))).expect(1).mount(&server).await;

    let d = Dirs::new();
    d.write_settings(&json!({"version": 10, "romm_url": server.uri(), "romm_api_token": "t", "romm_api_token_origin": server.uri(), "save_sync_enabled": true}));
    let save_dir = machine(&d);
    let mut b = Backend::spawn(&d).await;
    seed(&d);

    let r = b.call("sync_rom_saves", json!([1])).await;
    assert_eq!((r["success"].clone(), r["synced"].clone()), (json!(true), json!(1)), "{r}");
    let upload = server.received_requests().await.unwrap().into_iter().find(|q| q.method.as_str() == "POST" && q.url.path() == "/api/saves").unwrap();
    let body = String::from_utf8_lossy(&upload.body).to_string();
    assert!(body.contains("name=\"saveFile\"") && body.contains(&format!("filename=\"{TITLE}.zip\"")), "{body}");
    assert!(!upload.url.query().unwrap().contains("overwrite"));
    assert!(body.contains(&format!("user/{TITLE}/0/data.bin")), "the zip carries the role-rooted entry");
    assert_eq!(std::fs::read_to_string(save_dir.join("0/data.bin")).unwrap(), "level 3", "the local save was not touched");

    // the device registered once and its id is remembered
    assert_eq!(d.kv("device_id").as_deref(), Some("dev-1"));
    // the status now compares against what RomM holds
    let hash = {
        let c = rusqlite::Connection::open(d.db()).unwrap();
        c.query_row("SELECT last_sync_hash FROM rom_save_files WHERE rom_id = 1", [], |r| r.get::<_, String>(0)).unwrap()
    };
    server.reset().await;
    Mock::given(path("/api/heartbeat")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"SYSTEM": {"VERSION": "4.9.0"}}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/api/saves")).respond_with(ResponseTemplate::new(200).set_body_json(json!([{
        "id": 5, "rom_id": 1, "file_name": format!("{TITLE}.zip"), "slot": "default", "emulator": "eden", "updated_at": "2026-01-01T00:00:00+00:00",
        "content_hash": hash, "file_size_bytes": 200, "device_syncs": [{"device_id": "dev-1", "device_name": "d", "is_current": true, "last_synced_at": "t"}]
    }]))).mount(&server).await;
    let status = b.call("get_save_status", json!([1])).await;
    assert_eq!(status["files"][0]["status"], "synced", "{status}");
    assert_eq!(status["save_sync_display"]["status"], "synced");
    b.shutdown().await;
}

#[tokio::test]
async fn a_game_without_a_title_is_unsupported_and_a_launch_is_not_warned_or_sent_to_the_network() {
    let server = MockServer::start().await;
    let d = Dirs::new();
    d.write_settings(&json!({"version": 10, "romm_url": server.uri(), "romm_api_token": "t", "romm_api_token_origin": server.uri(), "save_sync_enabled": true}));
    machine(&d);
    let mut b = Backend::spawn(&d).await;
    let c = rusqlite::Connection::open(d.db()).unwrap();
    c.execute_batch(
        "INSERT INTO roms(rom_id,platform_slug,name,fs_name,last_synced_at,shortcut_app_id) VALUES(1,'switch','Metroid','Metroid Dread NSW-VENOM.xci','t',99);
         INSERT INTO rom_installs(rom_id,file_path,platform_slug,system,installed_at) VALUES(1,'/roms/switch/Metroid','switch','switch','t');",
    )
    .unwrap();
    let launch = b.call("pre_launch_sync", json!([1])).await;
    assert_eq!((launch["success"].clone(), launch["unsupported_reason"].clone()), (json!(true), json!("needs_title_id")), "{launch}");
    let status = b.call("get_save_status", json!([1])).await;
    assert_eq!(status["save_sync_display"]["label"], "Choose save folder");
    assert!(server.received_requests().await.unwrap().is_empty(), "no request was made for an unsupported game");
    // the user binds the save folder; it is remembered and validated
    let info = b.call("get_save_title_info", json!([1])).await;
    assert_eq!(info["candidates"].as_array().unwrap().len(), 1, "the Eden save folder is offered");
    assert_eq!(b.call("set_save_title_id", json!([1, "bogus"])).await["reason"], "invalid_title_id");
    assert_eq!(b.call("set_save_title_id", json!([1, TITLE])).await["success"], true);
    assert_eq!(b.call("get_save_title_info", json!([1])).await["title_id"], TITLE);
    b.shutdown().await;
}

#[tokio::test]
async fn the_settings_callables_round_trip_through_the_socket() {
    let d = Dirs::new();
    let mut b = Backend::spawn(&d).await;
    assert_eq!(b.call("get_save_sync_settings", json!([])).await["save_sync_enabled"], false);
    assert_eq!(b.call("update_save_sync_settings", json!([{"save_sync_enabled": true, "default_slot": " Mine ", "autocleanup_limit": 4}])).await, json!({"success": true}));
    let v: Value = b.call("get_save_sync_settings", json!([])).await;
    assert_eq!((v["save_sync_enabled"].clone(), v["default_slot"].clone(), v["autocleanup_limit"].clone()), (json!(true), json!("Mine"), json!(4)));
    assert_eq!(d.read_settings()["default_slot"], "Mine", "persisted to settings.json");
    b.shutdown().await;
}
