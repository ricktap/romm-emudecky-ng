//! Game page, metadata and artwork callables through the real binary/socket.
mod common;
use common::{Backend, Dirs};
use serde_json::{Value,json};
use base64::{Engine,engine::general_purpose::STANDARD};
use wiremock::{Mock,MockServer,ResponseTemplate,matchers::{path,header}};

#[tokio::test]
async fn cached_reads_and_explicit_cover_refresh_survive_restart() {
    let server=MockServer::start().await;
    Mock::given(path("/api/roms/1")).and(header("authorization","Bearer romm-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":1,"path_cover_large":"/covers/game cover.png"}))).expect(1).mount(&server).await;
    Mock::given(path("/covers/game%20cover.png")).and(header("authorization","Bearer romm-test"))
        .and(header("user-agent","romm-emudecky-ng/0.40.0-test"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"cover")).expect(1).mount(&server).await;
    let dirs=Dirs::new();
    dirs.write_settings(&json!({"version":10,"romm_url":server.uri(),"romm_api_token":"romm-test","romm_api_token_origin":server.uri()}));
    let grid=dirs.root.path().join("home/.local/share/Steam/userdata/1/config/grid");
    std::fs::create_dir_all(&grid).unwrap();
    let mut b=Backend::spawn(&dirs).await;
    let c=rusqlite::Connection::open(dirs.db()).unwrap();
    c.execute_batch("INSERT INTO roms (rom_id,platform_slug,name,fs_name,last_synced_at,shortcut_app_id) VALUES (1,'switch','Game','game.nsp','t',99);
        INSERT INTO rom_installs (rom_id,file_path,rom_dir,platform_slug,system,installed_at) VALUES (1,'/storage/game.nsp','/storage','switch','switch','t');
        INSERT INTO rom_metadata (rom_id,summary,player_count,cached_at) VALUES (1,'Offline summary','1',123);").unwrap();
    assert_eq!(b.call("get_cached_game_detail",json!([999])).await,json!({"found":false}));
    let detail=b.call("get_cached_game_detail",json!(["99"])).await;
    assert_eq!(detail["installed"],true);
    assert_eq!(detail["metadata"]["summary"],"Offline summary");
    assert_eq!(detail["stale_fields"],json!(["metadata"]));
    assert_eq!(b.call("get_rom_metadata",json!([1])).await,detail["metadata"]);
    assert_eq!(b.call("get_all_metadata_cache",json!([])).await,json!({"1":detail["metadata"]}));
    assert_eq!(b.call("get_artwork_base64",json!([1])).await,json!({"base64":null}));
    assert!(server.received_requests().await.unwrap().is_empty());
    let refreshed=b.call("refresh_cover_artwork",json!([1])).await;
    assert_eq!(refreshed["success"],true);
    assert_eq!(std::fs::read(grid.join("99p.png")).unwrap(),b"cover");
    for method in ["fetch_cover_base64","get_artwork_base64"] {
        assert_eq!(b.call(method,json!([1])).await,json!({"base64":STANDARD.encode(b"cover")}));
    }
    let art=dirs.runtime().join("artwork");std::fs::create_dir_all(&art).unwrap();
    std::fs::write(art.join("1_hero.png"),b"hero").unwrap();
    assert_eq!(b.call("get_sgdb_artwork_base64",json!([1,1])).await,json!({"base64":STANDARD.encode(b"hero"),"no_api_key":false}));
    assert_eq!(b.call("get_sgdb_artwork_base64",json!([1,2])).await,json!({"base64":null,"no_api_key":true}));
    assert_eq!(b.call("get_sgdb_resolution",json!([1])).await,json!({"decision":"no_api_key"}));
    assert_eq!(b.call("search_sgdb_games",json!(["test"])).await["reason"],"no_api_key");
    assert_eq!(b.call("verify_sgdb_api_key",json!([])).await["reason"],"no_api_key");
    assert_eq!(b.call("verify_sgdb_api_key",json!([null])).await["reason"],"no_api_key");
    assert_eq!(b.call("save_sgdb_api_key",json!(["••••"])).await["success"],true);
    assert_eq!(b.call("save_sgdb_api_key",json!(["fake-saved-key"])).await["success"],true);
    assert_eq!(dirs.read_settings()["steamgriddb_api_key"],"fake-saved-key");
    assert_eq!(b.call("save_sgdb_api_key",json!([""])).await["success"],true);
    assert!(!dirs.runtime().join("shortcuts.vdf").exists());
    b.shutdown().await;
    let mut b=Backend::spawn(&dirs).await;
    assert_eq!(b.call("get_artwork_base64",json!([1])).await["base64"],STANDARD.encode(b"cover"));
    assert_eq!(b.call("get_sgdb_artwork_base64",json!([1,1])).await["base64"],STANDARD.encode(b"hero"));
    assert_eq!(b.call("get_rom_metadata",json!([1])).await["summary"],"Offline summary");
    b.shutdown().await;
}

#[tokio::test]
async fn game_page_triggers_background_adoption_without_waiting_for_server() {
    let server=MockServer::start().await;
    let dirs=Dirs::new();
    dirs.write_settings(&json!({"version":10,"romm_url":server.uri(),"romm_api_token":"romm-test","romm_api_token_origin":server.uri()}));
    let storage=dirs.root.path().join("storage");let home=dirs.root.path().join("home");
    let config=home.join(".config/EmuDeck");std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("settings.json"),json!({"storagePath":storage}).to_string()).unwrap();
    let roms=storage.join("Emulation/roms/switch");std::fs::create_dir_all(&roms).unwrap();
    std::fs::write(roms.join("game.nsp"),b"game").unwrap();
    Mock::given(path("/api/roms/1"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_millis(300)).set_body_json(json!({
            "id":1,"name":"Game","fs_name":"game.nsp","fs_size_bytes":4,"platform_slug":"switch","platform_fs_slug":"switch",
            "files":[{"file_name":"game.nsp","file_size_bytes":4}]
        }))).mount(&server).await;
    let mut b=Backend::spawn(&dirs).await;
    let c=rusqlite::Connection::open(dirs.db()).unwrap();
    c.execute_batch("INSERT INTO roms (rom_id,platform_slug,name,fs_name,last_synced_at,shortcut_app_id) VALUES (1,'switch','Game','game.nsp','t',99);").unwrap();
    let d=b.call("get_cached_game_detail",json!([99])).await;
    assert_eq!(d["found"],true);
    assert_eq!(d["installed"],false); // response precedes background adoption
    let adopted=b.wait_event("download_complete",|e|e["rom_id"]==1).await;
    assert_eq!(adopted["rom_id"],1);
    assert_eq!(b.call("get_cached_game_detail",json!([99])).await["installed"],true);
    assert!(roms.join("game.nsp").exists());
    b.shutdown().await;
}

#[tokio::test]
async fn game_artwork_callable_arity_types_and_failure_shapes() {
    let dirs=Dirs::new();let mut b=Backend::spawn(&dirs).await;
    for (name,args) in [
        ("get_cached_game_detail",json!([])),("get_rom_metadata",json!([])),("get_all_metadata_cache",json!([1])),
        ("fetch_cover_base64",json!([])),("refresh_cover_artwork",json!([])),("get_sgdb_artwork_base64",json!([1])),
        ("verify_sgdb_api_key",json!([null,null])),("save_sgdb_api_key",json!([])),("get_sgdb_resolution",json!([])),
        ("search_sgdb_games",json!([])),("apply_sgdb_game_id",json!([1])),
    ] {assert!(b.request(name,args).await["error"]["message"].as_str().unwrap().contains("positional arguments"));}
    for (name,args) in [("get_cached_game_detail",json!([{}])),("get_rom_metadata",json!([false])),
        ("fetch_cover_base64",json!([[]])),("refresh_cover_artwork",json!(["bad"])),("get_sgdb_artwork_base64",json!([1,{}])),
        ("verify_sgdb_api_key",json!([42])),("save_sgdb_api_key",json!([false])),("get_sgdb_resolution",json!([[]])),
        ("search_sgdb_games",json!([null])),("apply_sgdb_game_id",json!([1,{}]))] {
        assert!(b.request(name,args).await.get("error").is_some(),"{name}");
    }
    assert_eq!(b.call("refresh_cover_artwork",json!([999])).await["reason"],"not_synced");
    assert_eq!(b.call("fetch_cover_base64",json!([999])).await,json!({"base64":null}));
    assert_eq!(b.call("apply_sgdb_game_id",json!([999,5])).await,json!({"success":true}));
    assert_eq!(b.call("get_sgdb_artwork_base64",json!([1,i64::MIN])).await,json!({"base64":null,"no_api_key":false}));
    assert_eq!(b.call("get_rom_metadata",json!([999])).await["summary"],Value::String(String::new()));
    b.shutdown().await;
}
