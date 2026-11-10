//! Python library read/preference contracts through the real socket server,
//! HTTP adapter, settings file, and SQLite actor.

mod common;

use common::{Backend, Dirs};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn configured(server: &MockServer) -> Dirs {
    let dirs = Dirs::new();
    dirs.write_settings(&json!({
        "version": 10,
        "romm_url": server.uri(),
        "romm_api_token": "test-library-token",
        "romm_api_token_origin": server.uri(),
    }));
    dirs
}

async fn mount(server: &MockServer, endpoint: &str, body: Value) {
    Mock::given(method("GET"))
        .and(path(endpoint))
        .and(header("user-agent", "romm-emudecky-ng/0.40.0-test"))
        .and(header("authorization", "Bearer test-library-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn platform_preferences_materialize_toggle_and_survive_restart() {
    let server = MockServer::start().await;
    mount(&server, "/api/platforms", json!([
        {"id": 1, "name": "Switch", "slug": "switch", "rom_count": 4},
        {"id": 2, "name": "Another library", "slug": "switch", "rom_count": 1},
        {"id": 3, "name": "Empty", "slug": "switch", "rom_count": 0},
    ])).await;
    let dirs = configured(&server);
    let mut b = Backend::spawn(&dirs).await;
    assert_eq!(b.call("get_platforms", json!([])).await, json!({
        "success": true,
        "platforms": [
            {"id": 1, "name": "Switch", "slug": "switch", "rom_count": 4, "sync_enabled": true},
            {"id": 2, "name": "Another library", "slug": "switch", "rom_count": 1, "sync_enabled": true},
        ],
    }));
    assert_eq!(dirs.read_settings()["enabled_platforms"], json!({"1": true, "2": true}));
    assert_eq!(b.call("save_platform_sync", json!(["1", false])).await, json!({"success": true}));
    assert_eq!(dirs.read_settings()["enabled_platforms"], json!({"1": false, "2": true}));
    b.shutdown().await;

    let mut b = Backend::spawn(&dirs).await;
    let rows = b.call("get_platforms", json!([])).await;
    assert_eq!(rows["platforms"][0]["sync_enabled"], false);
    assert_eq!(rows["platforms"][1]["sync_enabled"], true);
    assert_eq!(b.call("set_all_platforms_sync", json!([false])).await, json!({"success": true}));
    // Bulk selection also records platforms currently empty on RomM.
    assert_eq!(dirs.read_settings()["enabled_platforms"], json!({"1": false, "2": false, "3": false}));
    b.shutdown().await;
}

#[tokio::test]
async fn collection_kinds_scopes_and_favorites_keep_the_python_contract() {
    let server = MockServer::start().await;
    mount(&server, "/api/collections", json!([
        {"id": 7, "name": "Zelda", "rom_ids": [10, 11]},
        {"id": 8, "name": "Favorites", "rom_count": 1, "is_favorite": true},
    ])).await;
    mount(&server, "/api/collections/smart", json!([
        {"id": 7, "name": "Recently added", "rom_count": 3},
    ])).await;
    Mock::given(method("GET"))
        .and(path("/api/collections/virtual"))
        .and(query_param("type", "franchise"))
        .and(header("authorization", "Bearer test-library-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": "zelda:series", "name": "Zelda series", "rom_count": 5},
        ])))
        .mount(&server).await;
    let dirs = configured(&server);
    let mut b = Backend::spawn(&dirs).await;
    let out = b.call("get_collections", json!([])).await;
    assert_eq!(out, json!({"success": true, "collections": [
        {"id": "8", "name": "Favorites", "rom_count": 1, "sync_enabled": false, "kind": "user", "is_favorite": true},
        {"id": "7", "name": "Zelda", "rom_count": 2, "sync_enabled": false, "kind": "user", "is_favorite": false},
        {"id": "7", "name": "Recently added", "rom_count": 3, "sync_enabled": false, "kind": "smart", "is_favorite": false},
        {"id": "zelda:series", "name": "Zelda series", "rom_count": 5, "sync_enabled": false, "kind": "franchise", "is_favorite": false},
    ]}));
    assert_eq!(b.call("save_collection_sync", json!([8, "user", true])).await, json!({"success": true}));
    assert_eq!(b.call("set_all_collections_sync", json!([true, "my"])).await, json!({"success": true}));
    assert_eq!(b.call("set_all_collections_sync", json!([false, "my"])).await, json!({"success": true}));
    assert_eq!(dirs.read_settings()["enabled_collections"], json!({
        "user": {"7": false, "8": true}, "smart": {}, "franchise": {},
    }));
    assert_eq!(b.call("set_all_collections_sync", json!([true, "smart"])).await, json!({"success": true}));
    assert_eq!(b.call("set_all_collections_sync", json!([true, "franchise"])).await, json!({"success": true}));
    assert_eq!(b.call("get_sync_stats", json!([])).await["collections"], 3);
    b.shutdown().await;

    let mut b = Backend::spawn(&dirs).await;
    let out = b.call("get_collections", json!([])).await;
    assert_eq!(out["collections"][1]["sync_enabled"], false);
    assert_eq!(out["collections"][2]["sync_enabled"], true);
    // Omitted and explicit-null scope both mean every bucket, including Favorites.
    assert_eq!(b.call("set_all_collections_sync", json!([true])).await, json!({"success": true}));
    assert_eq!(b.call("get_sync_stats", json!([])).await["collections"], 4);
    assert_eq!(b.call("set_all_collections_sync", json!([false, null])).await, json!({"success": true}));
    assert_eq!(b.call("get_sync_stats", json!([])).await["collections"], 0);
    b.shutdown().await;
}

#[tokio::test]
async fn optional_collection_failures_are_tolerated_but_explicit_scopes_fail() {
    let server = MockServer::start().await;
    mount(&server, "/api/collections", json!([{"id": 1, "name": "Mine"}])).await;
    // Optional endpoints return 404 on older RomM servers.
    let dirs = configured(&server);
    let mut b = Backend::spawn(&dirs).await;
    assert_eq!(b.call("get_collections", json!([])).await["collections"].as_array().unwrap().len(), 1);
    assert_eq!(b.call("set_all_collections_sync", json!([true])).await, json!({"success": true}));
    let saved = dirs.read_settings();
    for scope in ["smart", "franchise"] {
        let out = b.call("set_all_collections_sync", json!([false, scope])).await;
        assert_eq!(out["success"], false);
        assert_eq!(out["reason"], "not_found");
        assert_eq!(out.as_object().unwrap().len(), 3);
        assert_eq!(dirs.read_settings(), saved);
    }
    b.shutdown().await;
}

#[tokio::test]
async fn invalid_collection_arguments_do_not_fetch_or_write() {
    let server = MockServer::start().await;
    let dirs = configured(&server);
    let mut b = Backend::spawn(&dirs).await;
    let saved = dirs.read_settings();
    for scope in [json!("favorites"), json!("bogus"), json!(false), json!(42), json!([]), json!({})] {
        let out = b.call("set_all_collections_sync", json!([true, scope])).await;
        assert_eq!(out["reason"], "invalid_scope");
    }
    for kind in [json!("my"), json!(null), json!(7)] {
        assert_eq!(b.call("save_collection_sync", json!([1, kind, true])).await["reason"], "invalid_kind");
    }
    for (name, args) in [
        ("get_platforms", json!([true])),
        ("save_platform_sync", json!([1])),
        ("save_collection_sync", json!([1, "user"])),
        ("set_all_collections_sync", json!([])),
    ] {
        assert!(b.request(name, args).await["error"]["message"].as_str().unwrap().contains("positional arguments"));
    }
    assert_eq!(dirs.read_settings(), saved);
    assert!(server.received_requests().await.unwrap().is_empty());
    b.shutdown().await;
}

#[tokio::test]
async fn malformed_lists_and_auth_failures_follow_python_adapter_behavior() {
    let server = MockServer::start().await;
    for endpoint in ["/api/platforms", "/api/collections", "/api/collections/smart", "/api/collections/virtual"] {
        mount(&server, endpoint, json!({"unexpected": "object"})).await;
    }
    let dirs = configured(&server);
    let mut b = Backend::spawn(&dirs).await;
    for method in ["get_platforms", "set_all_platforms_sync"] {
        let args = if method == "get_platforms" { json!([]) } else { json!([true]) };
        assert_eq!(b.call(method, args).await, json!({
            "success": false, "reason": "server_unreachable", "message": "Invalid server response",
        }));
    }
    assert_eq!(b.call("get_collections", json!([])).await, json!({"success": true, "collections": []}));
    server.reset().await;
    for endpoint in ["/api/platforms", "/api/collections"] {
        Mock::given(path(endpoint)).respond_with(ResponseTemplate::new(401)).mount(&server).await;
    }
    let saved = dirs.read_settings();
    for (name, args) in [
        ("get_platforms", json!([])), ("get_collections", json!([])),
        ("set_all_platforms_sync", json!([true])), ("set_all_collections_sync", json!([true])),
    ] {
        let out = b.call(name, args).await;
        assert_eq!(out["success"], false);
        assert_eq!(out["reason"], "auth_failed");
        assert_eq!(out.as_object().unwrap().len(), 3);
    }
    assert_eq!(dirs.read_settings(), saved);
    b.shutdown().await;
}

#[tokio::test]
async fn offline_registry_stats_and_cache_reset_preserve_roms_and_unfinished_runs() {
    let dirs = Dirs::new();
    let mut b = Backend::spawn(&dirs).await;
    assert_eq!(b.call("get_registry_platforms", json!([])).await, json!({"platforms": []}));
    assert_eq!(b.call("get_app_id_rom_id_map", json!([])).await, json!({}));
    assert_eq!(b.call("get_sync_stats", json!([])).await, json!({
        "last_sync": null, "platforms": 0, "collections": 0, "roms": 0, "total_shortcuts": 0,
    }));
    let conn = rusqlite::Connection::open(dirs.db()).unwrap();
    conn.execute_batch("
        INSERT INTO roms (rom_id, platform_slug, name, fs_name, shortcut_app_id, last_synced_at)
        VALUES (11, 'switch', 'Game', 'game.xci', 4000000001, '2026-01-01'),
               (12, 'switch', 'Sibling', 'sibling.xci', NULL, '2026-01-01'),
               (13, 'switch', 'Another game', 'another.nsp', 4000000002, '2026-01-01');
        INSERT INTO kv_config (key, value) VALUES ('platform_names', '{\"switch\":\"Nintendo Switch\"}');
        INSERT INTO sync_runs (id, started_at, status, platforms_planned, roms_planned, finished_at)
        VALUES ('older', '2026-01-01', 'completed', 1, 2, '2026-01-05'),
               ('latest', '2026-01-02', 'completed', 1, 2, '2026-01-03'),
               ('interrupted', '2026-01-06', 'running', 1, 2, NULL),
               ('cancelled', '2026-01-07', 'cancelled', 1, 2, '2026-01-07');
    ").unwrap();
    assert_eq!(b.call("get_app_id_rom_id_map", json!([])).await, json!({"4000000001": 11, "4000000002": 13}));
    assert_eq!(b.call("get_registry_platforms", json!([])).await, json!({
        "platforms": [{"name": "Nintendo Switch", "slug": "switch", "count": 2}],
    }));
    let stats = b.call("get_sync_stats", json!([])).await;
    assert_eq!(stats["last_sync"], "2026-01-03"); // latest by started_at, not finished_at
    assert_eq!(stats["roms"], 2);
    assert_eq!(stats["total_shortcuts"], 2);
    for cache in ["not json", "[]", "null", "{}"] {
        conn.execute("UPDATE kv_config SET value = ?1 WHERE key = 'platform_names'", [cache]).unwrap();
        assert_eq!(b.call("get_registry_platforms", json!([])).await["platforms"][0]["name"], "switch");
    }
    assert_eq!(b.call("clear_sync_cache", json!([])).await, json!({
        "success": true, "message": "Next sync will do a full fetch",
    }));
    assert_eq!(b.call("get_sync_stats", json!([])).await["last_sync"], Value::Null);
    assert_eq!(b.call("get_sync_stats", json!([])).await["roms"], 2);
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM roms", [], |r| r.get::<_, i64>(0)).unwrap(), 3);
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM sync_runs", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
    b.shutdown().await;
    let mut b = Backend::spawn(&dirs).await;
    assert_eq!(b.call("get_sync_stats", json!([])).await["last_sync"], Value::Null);
    assert_eq!(b.call("get_sync_stats", json!([])).await["roms"], 2);
    b.shutdown().await;
}

#[tokio::test]
async fn persistence_and_database_failures_use_canonical_callable_results() {
    let dirs = Dirs::new();
    let mut b = Backend::spawn(&dirs).await;
    // A directory at the settings-file destination makes the atomic replace
    // fail deterministically, even when the tests run with elevated privileges.
    std::fs::rename(dirs.settings_json(), dirs.settings().join("saved.json")).unwrap();
    std::fs::create_dir(dirs.settings_json()).unwrap();
    let conn = rusqlite::Connection::open(dirs.db()).unwrap();
    conn.execute_batch("DROP TABLE roms; DROP TABLE sync_runs;").unwrap();
    for (name, args) in [
        ("save_platform_sync", json!([1, true])),
        ("save_collection_sync", json!([1, "user", true])),
        ("get_registry_platforms", json!([])),
        ("get_app_id_rom_id_map", json!([])),
        ("get_sync_stats", json!([])),
        ("clear_sync_cache", json!([])),
    ] {
        let out = b.call(name, args).await;
        assert_eq!(out["success"], false, "{name}: {out}");
        assert_eq!(out["reason"], "unknown", "{name}: {out}");
        assert!(!out["message"].as_str().unwrap().is_empty());
        assert_eq!(out.as_object().unwrap().len(), 3);
    }
    // Restore the file before shutdown persists settings.
    std::fs::remove_dir(dirs.settings_json()).unwrap();
    std::fs::rename(dirs.settings().join("saved.json"), dirs.settings_json()).unwrap();
    b.shutdown().await;
}

#[tokio::test]
async fn restart_heals_interrupted_runs_without_changing_the_successful_baseline() {
    let dirs = Dirs::new();
    let mut b = Backend::spawn(&dirs).await;
    let conn = rusqlite::Connection::open(dirs.db()).unwrap();
    conn.execute_batch("
        INSERT INTO sync_runs (id, started_at, status, platforms_planned, roms_planned, finished_at, error)
        VALUES ('completed', '2026-01-01', 'completed', 1, 2, '2026-01-02', NULL),
               ('running', '2026-01-03', 'running', 1, 3, NULL, NULL),
               ('cancelled', '2026-01-03', 'cancelled', 1, 3, '2026-01-04', 'user cancelled'),
               ('errored', '2026-01-03', 'errored', 1, 3, '2026-01-04', 'server unreachable');
    ").unwrap();
    b.child.kill().await.unwrap();
    drop(b);

    let mut b = Backend::spawn(&dirs).await;
    let healed: (String, String, String, i64) = conn.query_row(
        "SELECT status, finished_at, error, roms_planned FROM sync_runs WHERE id = 'running'",
        [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    ).unwrap();
    assert_eq!(healed.0, "errored");
    assert!(!healed.1.is_empty());
    assert_eq!(healed.2, "interrupted by restart");
    assert_eq!(healed.3, 3);
    assert_eq!(b.call("get_sync_stats", json!([])).await["last_sync"], "2026-01-02");
    for (id, status, error) in [
        ("cancelled", "cancelled", "user cancelled"),
        ("errored", "errored", "server unreachable"),
    ] {
        let row: (String, String, String) = conn.query_row(
            "SELECT status, finished_at, error FROM sync_runs WHERE id = ?1", [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).unwrap();
        assert_eq!(row, (status.into(), "2026-01-04".into(), error.into()));
    }
    b.shutdown().await;

    let b = Backend::spawn(&dirs).await;
    let stamp: String = conn.query_row(
        "SELECT finished_at FROM sync_runs WHERE id = 'running'", [], |r| r.get(0),
    ).unwrap();
    assert_eq!(stamp, healed.1); // Healing is idempotent on later restarts.
    b.shutdown().await;
}

#[tokio::test]
async fn live_sync_dispatches_commits_all_siblings_and_restarts_with_incremental_skip() {
    let server=MockServer::start().await;
    mount(&server,"/api/platforms",json!([{"id":1,"name":"Switch","slug":"switch","rom_count":2}])).await;
    Mock::given(method("GET")).and(path("/api/roms")).and(query_param("limit","50")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"items":[
        {"id":1,"name":"Game USA","fs_name":"a.nsp","platform_id":1,"igdb_id":7,"platform_slug":"switch","regions":["USA"],"sibling_roms":[{"id":2}],"metadatum":{"genres":["Action"]}},
        {"id":2,"name":"Game Japan","fs_name":"b.nsp","platform_id":1,"ss_id":9,"platform_slug":"switch","regions":["Japan"]}
    ],"total":2}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/api/roms")).and(query_param("limit","1")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"items":[],"total":0}))).mount(&server).await;
    let dirs=configured(&server);let mut b=Backend::spawn(&dirs).await;
    assert_eq!(b.call("get_sync_status",json!([])).await["running"],false);
    assert_eq!(b.call("start_sync",json!([])).await["success"],true);
    let unit=b.wait_event("sync_apply_unit", |_|true).await;
    assert_eq!(unit["shortcuts"].as_array().unwrap().len(),1);assert_eq!(unit["shortcuts"][0]["rom_id"],1);assert_eq!(unit["shortcuts"][0]["launch_options"],"");
    assert_eq!(b.call("start_sync",json!([])).await["reason"],"sync_in_progress");
    assert_eq!(b.call("report_unit_results",json!([{"1":123},"old",1])).await["ignored"],true);
    assert_eq!(b.call("sync_heartbeat",json!([])).await["success"],true);
    assert_eq!(b.call("report_unit_results",json!([{"1":123},unit["run_id"],1])).await["count"],1);
    let complete=b.wait_event("sync_complete", |_|true).await;assert_eq!(complete["platform_app_ids"]["Switch"],json!([123]));
    b.wait_event("sync_progress",|p|p["stage"]=="done").await;
    assert_eq!(b.call("get_sync_stats",json!([])).await["roms"],1);
    let db=rusqlite::Connection::open(dirs.db()).unwrap();
    assert_eq!(db.query_row("SELECT COUNT(*) FROM roms",[],|r|r.get::<_,i64>(0)).unwrap(),2);
    assert_eq!(db.query_row("SELECT shortcut_app_id FROM roms WHERE rom_id=2",[],|r|r.get::<_,Option<i64>>(0)).unwrap(),None);
    assert!(db.query_row("SELECT cached_at FROM rom_metadata WHERE rom_id=1",[],|r|r.get::<_,f64>(0)).unwrap()>1_000_000.0);
    b.shutdown().await;
    let mut b=Backend::spawn(&dirs).await;b.call("start_sync",json!([])).await;
    b.wait_event("sync_complete", |_|true).await;
    assert!(b.events.iter().all(|e|e["event"]!="sync_apply_unit"));
    b.shutdown().await;
}

#[tokio::test]
async fn user_cancel_discards_inflight_unit_and_zero_queue_does_not_advance_history() {
    let server=MockServer::start().await;mount(&server,"/api/platforms",json!([{"id":1,"name":"Switch","slug":"switch","rom_count":1}])).await;
    mount(&server,"/api/roms",json!({"items":[{"id":1,"name":"Game","platform_id":1}],"total":1})).await;
    let dirs=configured(&server);let mut b=Backend::spawn(&dirs).await;b.call("start_sync",json!([])).await;
    let unit=b.wait_event("sync_apply_unit", |_|true).await;
    assert_eq!(b.call("cancel_sync",json!(["old"])).await["message"],"Cancel ignored (stale run)");
    assert_eq!(b.call("cancel_sync",json!([1])).await["message"],"Cancel ignored (stale run)");
    b.call("cancel_sync",json!([unit["run_id"]])).await;
    assert_eq!(b.wait_event("sync_complete", |_|true).await["cancelled"],true);
    assert_eq!(b.call("report_unit_results",json!([{"1":123},unit["run_id"],1])).await["ignored"],true);
    b.call("save_platform_sync",json!([1,false])).await;b.call("start_sync",json!([])).await;
    b.wait_event("sync_progress",|p|p["message"]=="Nothing to sync").await;
    assert_eq!(b.call("get_sync_stats",json!([])).await["last_sync"],Value::Null);b.shutdown().await;
}

#[tokio::test]
async fn preview_is_read_only_apply_refetches_and_snapshot_is_single_use() {
    let server=MockServer::start().await;
    mount(&server,"/api/platforms",json!([{"id":1,"name":"Switch","slug":"switch","rom_count":1}])).await;
    mount(&server,"/api/roms",json!({"items":[{"id":1,"name":"Game","platform_id":1,"fs_name":"game.nsp","metadatum":{"genres":["Action"]}}],"total":1})).await;
    let dirs=configured(&server);let mut b=Backend::spawn(&dirs).await;
    let preview=b.call("sync_preview",json!([])).await;assert_eq!(preview["success"],true);assert_eq!(preview["summary"]["new_count"],1);
    let db=rusqlite::Connection::open(dirs.db()).unwrap();assert_eq!(db.query_row("SELECT COUNT(*) FROM roms",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    assert_eq!(b.call("sync_apply_delta",json!(["wrong"])).await["reason"],"stale_preview");
    assert_eq!(b.call("sync_apply_delta",json!([preview["preview_id"]])).await["success"],true);
    let unit=b.wait_event("sync_apply_unit", |_|true).await;
    assert_eq!(b.call("sync_apply_delta",json!([preview["preview_id"]])).await["reason"],"stale_preview");
    b.call("report_unit_results",json!([{"1":123},unit["run_id"],1])).await;
    b.wait_event("sync_progress",|p|p["stage"]=="done"&&p["message"]!="Preview ready").await;
    let preview=b.call("sync_preview",json!([])).await;assert_eq!(preview["summary"]["unchanged_count"],1);
    b.call("sync_cancel_preview",json!([])).await;assert_eq!(b.call("sync_apply_delta",json!([preview["preview_id"]])).await["reason"],"stale_preview");
    b.shutdown().await;
}

#[tokio::test]
async fn failed_page_aborts_before_stale_cleanup_and_preserves_existing_bindings() {
    let server=MockServer::start().await;
    mount(&server,"/api/platforms",json!([{"id":1,"name":"Switch","slug":"switch","rom_count":51}])).await;
    Mock::given(method("GET")).and(path("/api/roms")).and(query_param("offset","0")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"items":(1..=50).map(|id|json!({"id":id,"name":"Game","platform_id":1})).collect::<Vec<_>>(),"total":51}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/api/roms")).and(query_param("offset","50")).respond_with(ResponseTemplate::new(401)).mount(&server).await;
    let dirs=configured(&server);let mut b=Backend::spawn(&dirs).await;
    let db=rusqlite::Connection::open(dirs.db()).unwrap();db.execute("INSERT INTO roms(rom_id,platform_slug,name,fs_name,last_synced_at,shortcut_app_id) VALUES(99,'switch','Existing','e.nsp','old',789)",[]).unwrap();
    b.call("start_sync",json!([])).await;b.wait_event("sync_progress",|p|p["stage"]=="error").await;
    assert_eq!(db.query_row("SELECT shortcut_app_id FROM roms WHERE rom_id=99",[],|r|r.get::<_,i64>(0)).unwrap(),789);
    assert!(b.events.iter().all(|e|e["event"]!="sync_stale"));
    assert_eq!(db.query_row("SELECT status FROM sync_runs",[],|r|r.get::<_,String>(0)).unwrap(),"errored");b.shutdown().await;
}

#[tokio::test]
async fn sync_downloads_cover_publishes_after_ack_and_applies_steam_input_without_touching_shortcuts_vdf(){
    let server=MockServer::start().await;mount(&server,"/api/platforms",json!([{"id":1,"name":"Switch","slug":"switch","rom_count":1}])).await;
    mount(&server,"/api/roms",json!({"items":[{"id":1,"name":"Game","platform_id":1,"path_cover_large":"/covers/a b.png"}]})).await;
    Mock::given(method("GET")).and(path("/covers/a%20b.png")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"cover-png")).expect(1).mount(&server).await;
    let dirs=configured(&server);let mut settings=dirs.read_settings();settings["steam_input_mode"]=json!("force_off");dirs.write_settings(&settings);
    let config=dirs.root.path().join("home/.local/share/Steam/userdata/123/config");std::fs::create_dir_all(&config).unwrap();std::fs::write(config.join("localconfig.vdf"),b"\"UserLocalConfigStore\" { \"Apps\" { \"999\" { \"Unrelated\" \"keep\" } } }").unwrap();std::fs::write(config.join("shortcuts.vdf"),b"untouched").unwrap();
    let mut b=Backend::spawn(&dirs).await;b.call("start_sync",json!([])).await;let unit=b.wait_event("sync_apply_unit", |_|true).await;
    assert_eq!(b.call("get_artwork_base64",json!([1])).await["base64"],"Y292ZXItcG5n");
    let cache=dirs.runtime().join("covers/1.png");assert_eq!(std::fs::read(&cache).unwrap(),b"cover-png");assert!(!config.join("grid/123p.png").exists());
    b.call("report_unit_results",json!([{"1":123},unit["run_id"],1])).await;b.wait_event("sync_progress",|p|p["stage"]=="done").await;
    assert_eq!(std::fs::read(config.join("grid/123p.png")).unwrap(),b"cover-png");assert!(cache.exists());assert!(config.join("localconfig.vdf.romm-backup").exists());
    let vdf=std::fs::read_to_string(config.join("localconfig.vdf")).unwrap();assert!(vdf.contains("UseSteamControllerConfig"));assert!(vdf.contains("Unrelated"));assert_eq!(std::fs::read(config.join("shortcuts.vdf")).unwrap(),b"untouched");
    assert_eq!(b.call("save_shortcut_icon",json!([123,"aWNvbg=="])).await["success"],true);assert_eq!(std::fs::read(config.join("grid/123_icon.png")).unwrap(),b"icon");
    assert_eq!(b.call("save_shortcut_icon",json!([123,"!bad"])).await["reason"],"invalid_payload");
    // Seed the per-ROM cache from a single-version game's existing grid cover.
    std::fs::remove_file(&cache).unwrap();b.call("clear_sync_cache",json!([])).await;b.call("start_sync",json!([])).await;
    let unit=b.wait_event("sync_apply_unit", |_|true).await;assert_eq!(std::fs::read(&cache).unwrap(),b"cover-png");
    b.call("report_unit_results",json!([{"1":123},unit["run_id"],1])).await;b.wait_event("sync_progress",|p|p["stage"]=="done").await;
    b.shutdown().await;
}

#[tokio::test]
async fn removal_queries_and_reconcile_unbind_only_and_preserve_installs_history_and_cache_baseline(){
    let server=MockServer::start().await;let dirs=configured(&server);let mut b=Backend::spawn(&dirs).await;let db=rusqlite::Connection::open(dirs.db()).unwrap();
    db.execute_batch(r#"INSERT INTO roms(rom_id,platform_slug,name,fs_name,last_synced_at,shortcut_app_id) VALUES(1,'switch','A','a','t',123),(2,'switch','B','b','t',456),(3,'other','C','c','t',NULL);
        INSERT INTO rom_installs(rom_id,file_path,platform_slug,system,installed_at) VALUES(1,'/roms/a','switch','switch','t');
        INSERT INTO rom_playtime(rom_id,total_seconds,session_count) VALUES(1,42,1);
        INSERT INTO kv_config(key,value) VALUES('platform_names','{"switch":"Nintendo Switch"}');"#).unwrap();
    assert_eq!(b.call("remove_all_shortcuts",json!([])).await,json!({"success":true,"app_ids":[123,456],"rom_ids":["1","2","3"]}));
    assert_eq!(b.call("remove_platform_shortcuts",json!(["switch"])).await["platform_name"],"Nintendo Switch");
    assert_eq!(b.call("reconcile_shortcuts",json!([["123","bad"]])).await["unbound_count"],1);
    assert_eq!(b.call("get_app_id_rom_id_map",json!([])).await,json!({"123":1}));
    assert_eq!(b.call("report_removal_results",json!([["1",2]])).await["success"],true);
    assert_eq!(db.query_row("SELECT COUNT(*) FROM roms",[],|r|r.get::<_,i64>(0)).unwrap(),3);assert_eq!(db.query_row("SELECT COUNT(*) FROM rom_installs",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    assert_eq!(db.query_row("SELECT total_seconds FROM rom_playtime WHERE rom_id=1",[],|r|r.get::<_,i64>(0)).unwrap(),42);assert_eq!(b.call("get_app_id_rom_id_map",json!([])).await,json!({}));b.shutdown().await;
}

#[tokio::test]
async fn missing_cover_is_nonfatal_and_atomic_sidecar_is_removed(){
    let server=MockServer::start().await;mount(&server,"/api/platforms",json!([{"id":1,"name":"Switch","slug":"switch","rom_count":1}])).await;
    mount(&server,"/api/roms",json!({"items":[{"id":1,"name":"Game","platform_id":1,"path_cover_small":"/missing.png"}]})).await;
    let dirs=configured(&server);std::fs::create_dir_all(dirs.root.path().join("home/.steam/steam/userdata/1")).unwrap();
    let mut b=Backend::spawn(&dirs).await;b.call("start_sync",json!([])).await;let unit=b.wait_event("sync_apply_unit", |_|true).await;
    assert_eq!(unit["shortcuts"][0]["cover_path"],"");assert!(!dirs.runtime().join("covers/1.png.tmp").exists());
    b.call("report_unit_results",json!([{"1":123},unit["run_id"],1])).await;b.wait_event("sync_progress",|p|p["stage"]=="done").await;b.shutdown().await;
}

#[tokio::test]
async fn reconcile_forces_refetch_for_a_group_whose_only_shortcut_was_deleted(){
    let server=MockServer::start().await;mount(&server,"/api/platforms",json!([{"id":1,"name":"Switch","slug":"switch","rom_count":1}])).await;
    mount(&server,"/api/roms",json!({"items":[{"id":1,"name":"Game","platform_id":1}],"total":0})).await;
    let dirs=configured(&server);let mut b=Backend::spawn(&dirs).await;b.call("start_sync",json!([])).await;let unit=b.wait_event("sync_apply_unit", |_|true).await;
    b.call("report_unit_results",json!([{"1":123},unit["run_id"],1])).await;b.wait_event("sync_progress",|p|p["stage"]=="done").await;
    b.call("reconcile_shortcuts",json!([[]])).await;b.call("start_sync",json!([])).await;
    let unit=b.wait_event("sync_apply_unit", |_|true).await;assert_eq!(unit["shortcuts"][0]["rom_id"],1);
    b.call("report_unit_results",json!([{"1":123},unit["run_id"],1])).await;b.wait_event("sync_progress",|p|p["stage"]=="done").await;b.shutdown().await;
}

#[tokio::test]
async fn preview_auth_errors_use_canonical_reason_and_a_retry_can_run(){
    let server=MockServer::start().await;Mock::given(method("GET")).and(path("/api/platforms")).respond_with(ResponseTemplate::new(401)).mount(&server).await;
    let dirs=configured(&server);let mut b=Backend::spawn(&dirs).await;
    assert_eq!(b.call("sync_preview",json!([])).await["reason"],"auth_failed");
    assert_eq!(b.call("sync_preview",json!([])).await["reason"],"auth_failed");b.shutdown().await;
}

#[tokio::test]
async fn same_sync_adoption_bakes_eden_before_dispatch_and_commits_install_only_after_ack(){
    use std::os::unix::fs::PermissionsExt;
    let server=MockServer::start().await;let raw=json!({"id":1,"name":"Game","fs_name":"Game.nsp","fs_name_no_ext":"Game","fs_size_bytes":5,"platform_id":1,"platform_slug":"switch","files":[{"file_name":"Game.nsp","file_size_bytes":5}]});
    mount(&server,"/api/platforms",json!([{"id":1,"name":"Switch","slug":"switch","rom_count":1}])).await;mount(&server,"/api/roms",json!({"items":[raw.clone()]})).await;mount(&server,"/api/roms/1",raw).await;
    let dirs=configured(&server);let home=dirs.root.path().join("home");let storage=dirs.root.path().join("sd");let emudeck=home.join(".config/EmuDeck");std::fs::create_dir_all(&emudeck).unwrap();std::fs::write(emudeck.join("settings.json"),json!({"storagePath":storage}).to_string()).unwrap();
    let exe=home.join("Applications/Eden.AppImage");std::fs::create_dir_all(exe.parent().unwrap()).unwrap();std::fs::write(&exe,b"#!/bin/sh\n").unwrap();std::fs::set_permissions(&exe,std::fs::Permissions::from_mode(0o755)).unwrap();
    let rom=storage.join("Emulation/roms/switch/Game.nsp");std::fs::create_dir_all(rom.parent().unwrap()).unwrap();std::fs::write(&rom,b"game!").unwrap();
    let mut b=Backend::spawn(&dirs).await;b.call("start_sync",json!([])).await;let unit=b.wait_event("sync_apply_unit", |_|true).await;let options=unit["shortcuts"][0]["launch_options"].as_str().unwrap();
    assert!(options.contains("Eden.AppImage"),"{options}");assert!(options.contains(" -f -g "),"{options}");assert!(options.contains(rom.to_str().unwrap()),"{options}");
    let db=rusqlite::Connection::open(dirs.db()).unwrap();assert_eq!(db.query_row("SELECT COUNT(*) FROM rom_installs",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    b.call("report_unit_results",json!([{"1":123},unit["run_id"],1])).await;b.wait_event("sync_progress",|p|p["stage"]=="done").await;
    assert_eq!(db.query_row("SELECT file_path FROM rom_installs WHERE rom_id=1",[],|r|r.get::<_,String>(0)).unwrap(),rom.to_string_lossy());assert_eq!(std::fs::read(rom).unwrap(),b"game!");b.shutdown().await;
}

#[tokio::test]
async fn collection_only_partial_view_keeps_bound_sibling_and_resolves_membership_without_minting_a_duplicate(){
    let server=MockServer::start().await;mount(&server,"/api/platforms",json!([{"id":1,"name":"Switch","slug":"switch","rom_count":2}])).await;
    mount(&server,"/api/collections",json!([{"id":7,"name":"Favorites","rom_count":1}])).await;
    Mock::given(method("GET")).and(path("/api/roms")).and(query_param("collection_id","7")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"items":[{"id":2,"name":"Japan","platform_id":1,"platform_slug":"switch","igdb_id":7,"sibling_roms":[{"id":1}]}]}))).mount(&server).await;
    let dirs=configured(&server);let mut settings=dirs.read_settings();settings["enabled_platforms"]=json!({"1":false});settings["enabled_collections"]=json!({"user":{"7":true}});dirs.write_settings(&settings);
    let mut b=Backend::spawn(&dirs).await;let db=rusqlite::Connection::open(dirs.db()).unwrap();db.execute("INSERT INTO roms(rom_id,platform_slug,name,fs_name,last_synced_at,shortcut_app_id,sibling_group_key) VALUES(1,'switch','Sticky','old.nsp','t',123,'igdb:7:1')",[]).unwrap();
    b.call("start_sync",json!([])).await;let unit=b.wait_event("sync_apply_unit", |_|true).await;assert_eq!(unit["unit_type"],"collection");assert!(unit["shortcuts"].as_array().unwrap().is_empty());
    b.call("report_unit_results",json!([{},unit["run_id"],"7"])).await;
    let stale=b.wait_event("sync_stale", |_|true).await;assert!(stale["remove"].as_array().unwrap().is_empty());
    let done=b.wait_event("sync_complete", |_|true).await;assert_eq!(done["romm_collection_app_ids"]["Favorites"],json!([123]));assert_eq!(done["platform_app_ids"],json!({}));
    assert_eq!(db.query_row("SELECT shortcut_app_id FROM roms WHERE rom_id=1",[],|r|r.get::<_,i64>(0)).unwrap(),123);assert_eq!(db.query_row("SELECT shortcut_app_id FROM roms WHERE rom_id=2",[],|r|r.get::<_,Option<i64>>(0)).unwrap(),None);b.shutdown().await;
}

#[tokio::test]
async fn startup_artwork_cleanup_preserves_known_siblings_and_unpublished_staging(){
    let server=MockServer::start().await;let dirs=configured(&server);let b=Backend::spawn(&dirs).await;b.shutdown().await;
    let db=rusqlite::Connection::open(dirs.db()).unwrap();db.execute_batch("INSERT INTO roms(rom_id,platform_slug,name,fs_name,last_synced_at,shortcut_app_id) VALUES(1,'switch','A','a','t',123),(2,'switch','B','b','t',NULL),(3,'switch','C','c','t',456);").unwrap();
    let cache=dirs.runtime().join("covers");let grid=dirs.root.path().join("home/.steam/steam/userdata/1/config/grid");std::fs::create_dir_all(&cache).unwrap();std::fs::create_dir_all(&grid).unwrap();
    for name in ["1.png","2.png","999.png","1.png.tmp","note.txt"]{std::fs::write(cache.join(name),b"art").unwrap();}
    for name in ["123p.png","romm_1_cover.png","romm_2_cover.png","romm_3_cover.png","unrelated.png"]{std::fs::write(grid.join(name),b"art").unwrap();}
    let mut b=Backend::spawn(&dirs).await;
    assert!(cache.join("1.png").exists());assert!(cache.join("2.png").exists());assert!(!cache.join("999.png").exists());assert!(!cache.join("1.png.tmp").exists());assert!(cache.join("note.txt").exists());
    assert!(!grid.join("romm_1_cover.png").exists());assert!(!grid.join("romm_2_cover.png").exists());assert!(grid.join("romm_3_cover.png").exists());assert!(grid.join("unrelated.png").exists());
    assert_eq!(b.call("get_artwork_base64",json!([3])).await["base64"],"YXJ0");assert_eq!(b.call("get_artwork_base64",json!([999])).await["base64"],Value::Null);b.shutdown().await;
}

#[tokio::test]
async fn library_overview_lists_bound_roms_by_name_with_install_state_and_platform_counts() {
    let dirs = Dirs::new();
    let mut b = Backend::spawn(&dirs).await;
    assert_eq!(
        b.call("get_library_overview", json!([])).await,
        json!({"success": true, "roms": [], "platforms": []})
    );
    let conn = rusqlite::Connection::open(dirs.db()).unwrap();
    conn.execute_batch("
        INSERT INTO roms (rom_id, platform_slug, name, fs_name, shortcut_app_id, last_synced_at, cover_path)
        VALUES (11, 'switch', 'zelda', 'z.xci', 4000000001, '2026-01-01', '/c/11.png'),
               (12, 'switch', 'Unbound', 'u.xci', NULL, '2026-01-01', NULL),
               (13, 'switch', 'Another game', 'a.nsp', 4000000002, '2026-01-01', NULL);
        INSERT INTO rom_installs (rom_id, file_path, rom_dir, platform_slug, system, installed_at)
        VALUES (11, '/r/z.xci', NULL, 'switch', 'switch', '2026-01-02');
    ").unwrap();
    let r = b.call("get_library_overview", json!([])).await;
    assert_eq!(r["success"], true);
    let roms = r["roms"].as_array().unwrap();
    assert_eq!(roms.len(), 2, "an unbound ROM has no shortcut to open and is left out");
    assert_eq!(roms[0]["name"], "Another game", "case-insensitive name order");
    assert_eq!(roms[0]["installed"], false);
    assert_eq!(roms[1], json!({
        "rom_id": 11, "app_id": 4000000001_i64, "name": "zelda", "platform_slug": "switch",
        "cover_path": "/c/11.png", "installed": true, "download": null, "issues": []
    }));
    assert_eq!(r["platforms"], json!([{"platform_slug": "switch", "count": 2, "installed": 1}]));
    b.shutdown().await;
}
