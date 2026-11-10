//! Version, launch, playtime and uninstall callables through the real socket.
mod common;
use common::{Backend,Dirs};
use serde_json::{Value,json};
use wiremock::{Mock,MockServer,ResponseTemplate,matchers::{path,method,header}};
use std::os::unix::fs::PermissionsExt;

fn storage(d:&Dirs)->std::path::PathBuf{
    let home=d.root.path().join("home");let cfg=home.join(".config/EmuDeck");std::fs::create_dir_all(&cfg).unwrap();let storage=d.root.path().join("storage");std::fs::write(cfg.join("settings.json"),json!({"storagePath":storage}).to_string()).unwrap();let roms=storage.join("Emulation/roms/switch");std::fs::create_dir_all(&roms).unwrap();roms
}
fn seed(d:&Dirs,roms:&std::path::Path){
    let c=rusqlite::Connection::open(d.db()).unwrap();c.execute_batch("INSERT INTO roms(rom_id,platform_slug,name,fs_name,last_synced_at,shortcut_app_id,sibling_group_key) VALUES(1,'switch','US','Game US.nsp','t',99,'igdb:7:5'),(2,'switch','EU','Game $EU.nsp','t',NULL,'igdb:7:5'),(3,'switch','Other','Other.nsp','t',101,NULL);").unwrap();
    for (id,name) in [(1,"Game US.nsp"),(2,"Game $EU.nsp")]{let p=roms.join(name);std::fs::write(&p,b"rom").unwrap();c.execute("INSERT INTO rom_installs(rom_id,file_path,platform_slug,system,installed_at) VALUES(?1,?2,'switch','switch','t')",rusqlite::params![id,p.to_str().unwrap()]).unwrap();}
}
#[tokio::test]async fn versions_launch_and_uninstall_preserve_shortcut_identity_and_local_history(){
    let server=MockServer::start().await;Mock::given(path("/api/roms/1")).respond_with(ResponseTemplate::new(503)).mount(&server).await;
    let d=Dirs::new();d.write_settings(&json!({"version":10,"romm_url":server.uri(),"save_sync_enabled":true,"steam_input_mode":"force_off"}));let roms=storage(&d);
    let eden=d.root.path().join("home/Applications/Eden.AppImage");std::fs::create_dir_all(eden.parent().unwrap()).unwrap();std::fs::write(&eden,"#!/bin/sh\n").unwrap();std::fs::set_permissions(&eden,std::fs::Permissions::from_mode(0o755)).unwrap();
    let steam=d.root.path().join("home/.local/share/Steam/userdata/1/config");std::fs::create_dir_all(&steam).unwrap();std::fs::write(steam.join("localconfig.vdf"),"\"UserLocalConfigStore\" { \"Apps\" {} }").unwrap();
    let mut b=Backend::spawn(&d).await;seed(&d,&roms);let c=rusqlite::Connection::open(d.db()).unwrap();c.execute_batch("INSERT INTO rom_playtime(rom_id,total_seconds,session_count) VALUES(1,42,1); INSERT INTO rom_save_states(rom_id,slots) VALUES(1,'{\"a\":{}}');").unwrap();
    let menu=b.call("get_version_list",json!([99])).await;assert_eq!(menu["server_query_failed"],true);assert_eq!(menu["versions"].as_array().unwrap().len(),2);
    assert_eq!(b.call("evaluate_launch",json!([101])).await["reason"],"not_installed");assert_eq!(b.call("evaluate_launch",json!([99])).await["action"],"warn");assert_eq!(b.call("check_local_drift",json!([1])).await,json!({"rom_id":1,"drifted":false}));assert_eq!(b.call("check_core_change",json!([1])).await,json!({"changed":false}));
    let r=b.call("switch_version",json!([99,2,false])).await;assert_eq!(r["success"],true);assert_eq!(r["app_id"],99);assert!(r["launch_options"].as_str().unwrap().contains("Game \\$EU.nsp"));assert_eq!(b.call("evaluate_launch",json!([99])).await["action"],"allow");
    assert_eq!(b.call("apply_steam_input_setting",json!([])).await["message"],"Steam Input set to 'force_off' for 2 shortcuts");assert!(steam.join("localconfig.vdf.romm-backup").exists());assert!(!steam.join("shortcuts.vdf").exists());b.shutdown().await;
    let mut b=Backend::spawn(&d).await;assert_eq!(b.call("get_app_id_rom_id_map",json!([])).await["99"],2);assert_eq!(b.call("remove_rom",json!([2])).await["success"],true);assert!(!roms.join("Game $EU.nsp").exists());assert_eq!(b.call("evaluate_launch",json!([99])).await["reason"],"not_installed");
    let r=b.call("uninstall_all_roms",json!([])).await;assert_eq!(r["success"],true);assert_eq!(r["removed_count"],1);assert_eq!(r["app_ids"],json!([]));assert_eq!(b.call("get_all_playtime",json!([])).await["playtime"]["1"]["total_seconds"],42);assert!(roms.is_dir());assert_eq!(c.query_row("SELECT count(*) FROM rom_save_states WHERE rom_id=1",[],|r|r.get::<_,i64>(0)).unwrap(),1);assert_eq!(b.call("get_app_id_rom_id_map",json!([])).await["99"],2);b.shutdown().await;
}
#[tokio::test]async fn persisted_session_finalization_ingest_and_reconcile_scope_notice(){
    let server=MockServer::start().await;
    Mock::given(method("POST")).and(path("/api/play-sessions")).and(header("authorization","Bearer test-token")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"results":[{"index":0,"status":"created"}]}))).expect(1).mount(&server).await;
    let d=Dirs::new();d.write_settings(&json!({"version":10,"romm_url":server.uri(),"romm_api_token":"test-token","romm_api_token_origin":server.uri(),"save_sync_enabled":true}));let roms=storage(&d);let mut b=Backend::spawn(&d).await;seed(&d,&roms);
    let c=rusqlite::Connection::open(d.db()).unwrap();c.execute_batch("INSERT INTO kv_config(key,value) VALUES('device_id','registered-device');").unwrap();assert_eq!(b.call("record_session_start",json!([1])).await,json!({"success":true}));b.shutdown().await;
    c.execute_batch("UPDATE rom_playtime SET last_session_start='2026-01-01T00:00:00+00:00',last_session_start_monotonic=NULL WHERE rom_id=1;").unwrap();
    let mut b=Backend::spawn(&d).await;let r=b.call("finalize_game_session",json!([1])).await;assert_eq!(r["total_seconds"],86400);assert_eq!(r["sync"]["toast_body"],Value::Null);assert_eq!(r["sync"]["synced"],0);assert!(r["migration"].is_null());assert!(b.call("finalize_game_session",json!([1])).await["total_seconds"].is_null());
    assert_eq!(c.query_row("SELECT count(*) FROM rom_playtime_sessions",[],|r|r.get::<_,i64>(0)).unwrap(),0);let post=server.received_requests().await.unwrap().into_iter().find(|r|r.method.as_str()=="POST").unwrap().body_json::<Value>().unwrap();assert_eq!(post["device_id"],"registered-device");assert_eq!(post["sessions"][0]["duration_ms"],86400000);
    Mock::given(method("GET")).and(path("/api/play-sessions")).respond_with(ResponseTemplate::new(403)).mount(&server).await;let r=b.call("reconcile_playtime",json!([1])).await;assert_eq!(r["server_query_failed"],true);assert_eq!(r["total_seconds"],86400);assert_eq!(b.call("get_playtime_scope_notice",json!([])).await,json!({"pending":true}));b.shutdown().await;
    server.reset().await;Mock::given(method("GET")).and(path("/api/play-sessions")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"items":[{"duration_ms":90000000,"end_time":"2026-12-01T00:00:00Z"}]}))).mount(&server).await;
    let mut b=Backend::spawn(&d).await;assert_eq!(b.call("get_playtime_scope_notice",json!([])).await["pending"],true);let r=b.call("reconcile_playtime",json!([1])).await;assert_eq!(r["server_query_failed"],false);assert_eq!(r["total_seconds"],90000);assert_eq!(r["last_played"],"2026-12-01T00:00:00Z");assert_eq!(b.call("get_playtime_scope_notice",json!([])).await["pending"],false);b.shutdown().await;
}
#[tokio::test]async fn lifecycle_callable_arity_types_and_missing_id_contracts(){
    let d=Dirs::new();let mut b=Backend::spawn(&d).await;
    for (name,args) in [("get_version_list",json!([])),("switch_version",json!([1,2])),("evaluate_launch",json!([])),("check_local_drift",json!([])),("check_core_change",json!([])),("remove_rom",json!([])),("uninstall_all_roms",json!([1])),("apply_steam_input_setting",json!([1])),("check_for_update",json!([])),("record_session_start",json!([])),("finalize_game_session",json!([])),("get_all_playtime",json!([1])),("get_playtime_scope_notice",json!([1])),("reconcile_playtime",json!([]))]{assert!(b.request(name,args).await["error"]["message"].as_str().unwrap().contains("positional arguments"),"{name}");}
    for name in ["get_version_list","evaluate_launch","check_local_drift","check_core_change","remove_rom","record_session_start","finalize_game_session","reconcile_playtime"]{assert!(b.request(name,json!([{}])).await.get("error").is_some(),"{name}");}
    assert_eq!(b.call("get_version_list",json!([1])).await,json!({"multi_version":false}));assert_eq!(b.call("switch_version",json!([99,2,false])).await["reason"],"not_found");assert_eq!(b.call("evaluate_launch",json!([1])).await["action"],"allow");assert_eq!(b.call("remove_rom",json!([1])).await["reason"],"not_installed");assert_eq!(b.call("record_session_start",json!([1])).await["reason"],"unknown_rom");assert_eq!(b.call("get_all_playtime",json!([])).await,json!({"playtime":{}}));b.shutdown().await;
}
