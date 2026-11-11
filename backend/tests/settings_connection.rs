//! Settings + connection callables through the real binary, against a fake
//! RomM server — the Rust replacement of the Python contract tests
//! (`test_connect_with_*`, `test_sign_out`, `test_get_settings_reset_notice`).

mod common;

use std::os::unix::fs::PermissionsExt;

use common::{Backend, Dirs};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const UA: &str = "romm-emudecky-ng/0.40.0-test";

async fn romm(version: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/heartbeat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"SYSTEM": {"VERSION": version}})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/platforms"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"id": 1, "slug": "switch"}])))
        .mount(&server)
        .await;
    server
}

async fn mount_mint(server: &MockServer, id: i64, token: &str) {
    Mock::given(method("POST"))
        .and(path("/api/client-tokens"))
        .and(header("user-agent", UA))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": id, "raw_token": token})))
        .mount(server)
        .await;
}

async fn mount_me(server: &MockServer, status: u16) {
    Mock::given(method("GET"))
        .and(path("/api/users/me"))
        .respond_with(ResponseTemplate::new(status).set_body_json(json!({"id": 1})))
        .mount(server)
        .await;
}

fn auth_headers(reqs: &[wiremock::Request], p: &str) -> Vec<Option<String>> {
    reqs.iter()
        .filter(|r| r.url.path() == p)
        .map(|r| r.headers.get("authorization").map(|v| v.to_str().unwrap().to_string()))
        .collect()
}

#[tokio::test]
async fn fresh_install_defaults_and_settings_file() {
    let dirs = Dirs::new();
    let mut b = Backend::spawn(&dirs).await;
    let s = b.call("get_settings", json!([])).await;
    assert_eq!(
        s,
        json!({
            "romm_url": "", "romm_url_external": "", "has_token": false, "steam_input_mode": "default", "sgdb_api_key_masked": "",
            "log_level": "warn", "romm_allow_insecure_ssl": false,
            "collection_create_platform_groups": false, "preferred_region": "auto", "platform_cores": {}
        })
    );
    assert_eq!(
        b.call("get_settings_reset_notice", json!([])).await,
        json!({"pending": false, "backed_up_to": null})
    );
    assert_eq!(b.call("get_known_regions", json!([])).await, json!([]));
    b.shutdown().await;
    let on_disk = dirs.read_settings();
    assert_eq!(on_disk["version"], 10);
    assert_eq!(on_disk["romm_api_token"], Value::Null);
    let mode = std::fs::metadata(dirs.settings_json()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let conn = rusqlite::Connection::open(dirs.db()).unwrap();
    let v: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    assert_eq!(v, 15);
}

#[tokio::test]
async fn settings_writers_persist_to_disk() {
    let dirs = Dirs::new();
    let mut b = Backend::spawn(&dirs).await;
    assert_eq!(
        b.call("save_server_url", json!(["  https://romm.local/  ", true]))
            .await,
        json!({"success": true, "message": "Settings saved"})
    );
    assert_eq!(
        b.call("save_log_level", json!(["debug"])).await,
        json!({"success": true})
    );
    assert_eq!(
        b.call("save_steam_input_setting", json!(["force_off"])).await,
        json!({"success": true})
    );
    assert_eq!(
        b.call("save_preferred_region", json!(["  Japan "])).await,
        json!({"success": true})
    );
    assert_eq!(
        b.call("save_collection_platform_groups", json!([1])).await,
        json!({"success": true})
    );
    assert_eq!(
        b.call("update_whitelist_settings", json!([["Steam"], ["Mine"]])).await,
        json!({"success": true})
    );
    assert_eq!(
        b.call("save_server_url", json!(["ftp://nope"])).await,
        json!({"success": false, "reason": "config_error", "message": "Enter a valid http(s):// server URL"})
    );
    assert_eq!(
        b.call("save_log_level", json!(["loud"])).await["reason"],
        "invalid_log_level"
    );
    assert_eq!(
        b.call("frontend_log", json!(["info", "hello from the frontend"])).await,
        Value::Null
    );
    assert_eq!(b.call("debug_log", json!(["debug line"])).await, Value::Null);
    let s = b.call("get_settings", json!([])).await;
    assert_eq!(s["romm_url"], "https://romm.local/");
    assert_eq!(s["romm_allow_insecure_ssl"], true);
    assert_eq!(s["log_level"], "debug");
    assert_eq!(s["steam_input_mode"], "force_off");
    assert_eq!(s["preferred_region"], "Japan");
    assert_eq!(s["collection_create_platform_groups"], true);
    b.shutdown().await;

    let on_disk = dirs.read_settings();
    assert_eq!(on_disk["romm_url"], "https://romm.local/");
    assert_eq!(on_disk["whitelist_custom_names"], json!(["Mine"]));
    assert_eq!(on_disk["preferred_region"], "Japan");
    let log = std::fs::read_to_string(dirs.log()).unwrap();
    assert!(log.contains("[FE] hello from the frontend"));
    assert!(
        log.contains("[FE] debug line"),
        "debug level configured → debug lines kept"
    );

    // A restart reads everything back.
    let mut b = Backend::spawn(&dirs).await;
    assert_eq!(b.call("get_settings", json!([])).await, s);
    b.shutdown().await;
}

#[tokio::test]
async fn corrupt_settings_are_backed_up_and_the_notice_persists_until_dismissed() {
    let dirs = Dirs::new();
    std::fs::write(dirs.settings_json(), b"{\"romm_url\": \"http://x\", trunc").unwrap();
    let mut b = Backend::spawn(&dirs).await;
    let notice = b.call("get_settings_reset_notice", json!([])).await;
    assert_eq!(notice["pending"], true);
    let backup = notice["backed_up_to"].as_str().unwrap().to_string();
    assert!(backup.starts_with("settings.json.corrupt-"));
    assert_eq!(
        std::fs::read(dirs.settings().join(&backup)).unwrap(),
        b"{\"romm_url\": \"http://x\", trunc",
        "original bytes survive"
    );
    assert_eq!(
        b.call("get_settings_reset_notice", json!([])).await,
        notice,
        "non-consuming"
    );
    b.shutdown().await;
    assert_eq!(
        dirs.read_settings()["_settings_reset_notice"],
        json!({"backed_up_to": backup})
    );

    // Survives a reload; cleared only by the explicit dismiss.
    let mut b = Backend::spawn(&dirs).await;
    assert_eq!(b.call("get_settings_reset_notice", json!([])).await, notice);
    assert_eq!(
        b.call("dismiss_settings_reset_notice", json!([])).await,
        json!({"success": true})
    );
    assert_eq!(
        b.call("get_settings_reset_notice", json!([])).await,
        json!({"pending": false, "backed_up_to": null})
    );
    b.shutdown().await;
    assert!(dirs.read_settings().get("_settings_reset_notice").is_none());
}

#[tokio::test]
async fn newer_settings_version_is_never_down_stamped() {
    let dirs = Dirs::new();
    dirs.write_settings(&json!({"version": 99, "romm_url": "http://keep", "unknown_future": [1, 2]}));
    let b = Backend::spawn(&dirs).await;
    b.shutdown().await;
    let on_disk = dirs.read_settings();
    assert_eq!(on_disk["version"], 99);
    assert_eq!(on_disk["romm_url"], "http://keep");
    assert_eq!(on_disk["unknown_future"], json!([1, 2]));
}

#[tokio::test]
async fn sign_in_with_credentials_then_test_connection_and_sign_out() {
    let server = romm("4.9.0").await;
    mount_mint(&server, 42, "rmm_minted").await;
    let dirs = Dirs::new();
    let mut b = Backend::spawn(&dirs).await;

    let r = b
        .call(
            "connect_with_credentials",
            json!([server.uri(), "alice", "secret", false]),
        )
        .await;
    assert_eq!(
        r,
        json!({"success": true, "message": "Connected to RomM 4.9.0", "romm_version": "4.9.0"})
    );
    let on_disk = dirs.read_settings();
    assert_eq!(on_disk["romm_api_token"], "rmm_minted");
    assert_eq!(on_disk["romm_api_token_id"], 42);
    assert_eq!(on_disk["romm_api_token_origin"], json!(server.uri()));
    assert_eq!(on_disk["romm_api_token_source"], "minted");
    assert!(on_disk.get("romm_pass").is_none() && !on_disk.to_string().contains("secret"));

    let r = b.call("test_connection", json!([])).await;
    assert_eq!(r["success"], true);
    assert_eq!(b.call("probe_reachability", json!([])).await, json!({"online": true}));
    assert_eq!(b.call("get_settings", json!([])).await["has_token"], true);

    let reqs = server.received_requests().await.unwrap();
    // Sign-in probe carried no bearer; the authenticated calls carried the new one.
    assert_eq!(auth_headers(&reqs, "/api/heartbeat")[0], None);
    assert_eq!(
        auth_headers(&reqs, "/api/platforms"),
        vec![Some("Bearer rmm_minted".to_string())]
    );
    assert_eq!(
        auth_headers(&reqs, "/api/client-tokens"),
        vec![Some("Basic YWxpY2U6c2VjcmV0".to_string())]
    );
    assert!(reqs.iter().all(|r| r.headers.get("user-agent").unwrap() == UA));

    let r = b.call("sign_out", json!([])).await;
    assert_eq!(r["success"], true);
    assert!(r["message"].as_str().unwrap().contains("still valid in RomM"));
    let r = b.call("test_connection", json!([])).await;
    assert_eq!(r["reason"], "config_error");
    b.shutdown().await;
    let on_disk = dirs.read_settings();
    assert_eq!(on_disk["romm_api_token"], Value::Null);
    assert_eq!(on_disk["romm_url"], json!(server.uri()));
}

#[tokio::test]
async fn re_sign_in_same_origin_deletes_old_token_and_keeps_device() {
    let server = romm("4.9.0").await;
    mount_mint(&server, 43, "rmm_second").await;
    Mock::given(method("DELETE"))
        .and(path("/api/client-tokens/7"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let dirs = Dirs::new();
    dirs.write_settings(&json!({
        "version": 10, "romm_url": server.uri(), "romm_api_token": "rmm_first",
        "romm_api_token_id": 7, "romm_api_token_origin": server.uri(), "romm_api_token_source": "minted",
    }));
    let b = Backend::spawn(&dirs).await;
    b.shutdown().await;
    let conn = rusqlite::Connection::open(dirs.db()).unwrap();
    conn.execute("INSERT INTO kv_config (key, value) VALUES ('device_id', 'dev-1')", [])
        .unwrap();
    drop(conn);

    let mut b = Backend::spawn(&dirs).await;
    let r = b
        .call(
            "connect_with_credentials",
            json!([format!("{}/", server.uri()), "u", "p"]),
        )
        .await;
    assert_eq!(r["success"], true);
    b.shutdown().await;
    assert_eq!(
        dirs.kv("device_id").as_deref(),
        Some("dev-1"),
        "same origin keeps the device id"
    );
    assert_eq!(dirs.read_settings()["romm_api_token"], "rmm_second");
}

#[tokio::test]
async fn version_gate_rejects_old_romm_and_keeps_previous_state() {
    let server = romm("4.8.1").await;
    let dirs = Dirs::new();
    dirs.write_settings(
        &json!({"version": 10, "romm_url": "https://old.server", "romm_api_token": "rmm_old",
        "romm_api_token_origin": "https://old.server", "romm_api_token_id": 7}),
    );
    let mut b = Backend::spawn(&dirs).await;
    let r = b
        .call("connect_with_credentials", json!([server.uri(), "u", "p", null]))
        .await;
    assert_eq!(r["success"], false);
    assert_eq!(r["reason"], "version_error");
    assert_eq!(r["romm_version"], "4.8.1");
    let s = b.call("get_settings", json!([])).await;
    assert_eq!(s["romm_url"], "https://old.server");
    assert_eq!(s["has_token"], true);
    b.shutdown().await;
    assert_eq!(dirs.read_settings()["romm_api_token"], "rmm_old");
    assert_eq!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/api/client-tokens")
            .count(),
        0
    );
}

#[tokio::test]
async fn connect_with_token_happy_and_invalid() {
    let server = romm("4.9.0").await;
    mount_me(&server, 200).await;
    let dirs = Dirs::new();
    let mut b = Backend::spawn(&dirs).await;
    let r = b
        .call("connect_with_token", json!([server.uri(), " rmm_pasted ", false]))
        .await;
    assert_eq!(r["success"], true);
    assert_eq!(r["romm_version"], "4.9.0");
    let on_disk = dirs.read_settings();
    assert_eq!(on_disk["romm_api_token"], "rmm_pasted");
    assert_eq!(on_disk["romm_api_token_id"], Value::Null);
    assert_eq!(on_disk["romm_api_token_source"], "user");
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(
        auth_headers(&reqs, "/api/users/me"),
        vec![Some("Bearer rmm_pasted".to_string())]
    );
    assert!(reqs.iter().all(|r| r.url.path() != "/api/client-tokens"));

    let r = b.call("connect_with_token", json!([server.uri(), "   ", false])).await;
    assert_eq!(
        r,
        json!({"success": false, "reason": "config_error", "message": "Enter your RomM API token"})
    );
    b.shutdown().await;

    let bad = romm("4.9.0").await;
    mount_me(&bad, 401).await;
    let dirs = Dirs::new();
    let mut b = Backend::spawn(&dirs).await;
    let r = b.call("connect_with_token", json!([bad.uri(), "rmm_bad", false])).await;
    assert_eq!(r["success"], false);
    assert_eq!(r["reason"], "auth_failed");
    assert!(r["message"].as_str().unwrap().contains("invalid or has been revoked"));
    assert_eq!(b.call("get_settings", json!([])).await["has_token"], false);
    b.shutdown().await;
    assert_eq!(dirs.read_settings()["romm_api_token"], Value::Null);
}

#[tokio::test]
async fn connect_with_pairing_code_flows() {
    let server = romm("4.9.0").await;
    mount_me(&server, 200).await;
    Mock::given(method("POST"))
        .and(path("/api/client-tokens/exchange"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 5, "raw_token": "rmm_paired"})))
        .mount(&server)
        .await;
    let dirs = Dirs::new();
    let mut b = Backend::spawn(&dirs).await;
    let r = b
        .call("connect_with_pairing_code", json!([server.uri(), "ab-cd 12", false]))
        .await;
    assert_eq!(r["success"], true);
    let reqs = server.received_requests().await.unwrap();
    let exchange = reqs
        .iter()
        .find(|r| r.url.path() == "/api/client-tokens/exchange")
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&exchange.body).unwrap(),
        json!({"code": "ABCD12"})
    );
    assert!(exchange.headers.get("authorization").is_none());
    b.shutdown().await;
    assert_eq!(dirs.read_settings()["romm_api_token"], "rmm_paired");

    let limited = romm("4.9.0").await;
    Mock::given(path("/api/client-tokens/exchange"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&limited)
        .await;
    let dirs = Dirs::new();
    let mut b = Backend::spawn(&dirs).await;
    let r = b
        .call("connect_with_pairing_code", json!([limited.uri(), "CODE", false]))
        .await;
    assert_eq!(r["reason"], "rate_limited");
    let r = b
        .call("connect_with_pairing_code", json!([limited.uri(), "- -", false]))
        .await;
    assert_eq!(
        r,
        json!({"success": false, "reason": "config_error", "message": "Enter the pairing code from RomM"})
    );
    b.shutdown().await;
}

#[tokio::test]
async fn token_host_binding_blocks_a_foreign_origin() {
    let server = romm("4.9.0").await;
    let dirs = Dirs::new();
    dirs.write_settings(
        &json!({"version": 10, "romm_url": "https://old.server", "romm_api_token": "rmm_secret",
        "romm_api_token_origin": "https://old.server", "romm_api_token_source": "minted"}),
    );
    let mut b = Backend::spawn(&dirs).await;
    // Pointing the URL elsewhere without signing in leaves the token bound to the old origin.
    assert_eq!(b.call("save_server_url", json!([server.uri()])).await["success"], true);
    let r = b.call("test_connection", json!([])).await;
    assert_eq!(r["reason"], "config_error");
    assert!(r["message"].as_str().unwrap().contains("Sign in again"));
    b.shutdown().await;
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "the bearer never reached the foreign host"
    );
}

#[tokio::test]
async fn retries_emit_progress_events_before_the_response() {
    let server = MockServer::start().await;
    Mock::given(path("/api/heartbeat"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(path("/api/heartbeat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"SYSTEM": {"VERSION": "4.9.0"}})))
        .mount(&server)
        .await;
    Mock::given(path("/api/platforms"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    let dirs = Dirs::new();
    dirs.write_settings(&json!({"version": 10, "romm_url": server.uri(), "romm_api_token": "t",
        "romm_api_token_origin": server.uri()}));
    let mut b = Backend::spawn(&dirs).await;
    let r = b.call("test_connection", json!([])).await;
    assert_eq!(r["success"], true);
    let events: Vec<Value> = b.events.drain(..).collect();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["event"], "server_retry_progress");
    assert_eq!(events[0]["args"][0]["attempt"], 2);
    assert_eq!(events[0]["args"][0]["max_attempts"], 3);
    assert!(events[0]["args"][0]["delay_s"].is_f64());
    assert_eq!(events[1]["args"][0]["attempt"], 3);
    b.shutdown().await;
}

#[tokio::test]
async fn unreachable_server_maps_to_server_unreachable() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    drop(listener);
    let dirs = Dirs::new();
    let mut b = Backend::spawn(&dirs).await;
    let r = b.call("connect_with_credentials", json!([url, "u", "p", false])).await;
    assert_eq!(r["success"], false);
    assert_eq!(r["reason"], "server_unreachable");
    assert!(r["message"].as_str().unwrap().contains("Server unreachable"));
    assert_eq!(b.call("probe_reachability", json!([])).await, json!({"online": false}));
    b.shutdown().await;
}

#[tokio::test]
async fn legacy_password_install_is_upgraded_at_startup() {
    let server = romm("4.9.0").await;
    mount_mint(&server, 11, "rmm_from_legacy").await;
    let dirs = Dirs::new();
    dirs.write_settings(&json!({"version": 8, "romm_url": server.uri(), "romm_user": "alice", "romm_pass": "pw"}));
    let mut b = Backend::spawn(&dirs).await;
    assert_eq!(b.call("get_settings", json!([])).await["has_token"], true);
    b.shutdown().await;
    let on_disk = dirs.read_settings();
    assert_eq!(on_disk["romm_api_token"], "rmm_from_legacy");
    assert_eq!(on_disk["romm_api_token_source"], "minted");
    assert!(on_disk.get("romm_user").is_none() && on_disk.get("romm_pass").is_none());
}

/// The Python plugin's own database and settings file, continued in place.
#[tokio::test]
async fn continues_a_python_created_v12_database_and_settings() {
    let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    let dirs = Dirs::new();
    std::fs::copy(format!("{fixtures}/python_v12.db"), dirs.db()).unwrap();
    std::fs::copy(
        format!("{fixtures}/python_settings/settings.json"),
        dirs.settings_json(),
    )
    .unwrap();

    let mut b = Backend::spawn(&dirs).await;
    let s = b.call("get_settings", json!([])).await;
    assert_eq!(s["romm_url"], "http://romm.example:8080");
    assert_eq!(s["has_token"], true);
    assert_eq!(s["sgdb_api_key_masked"], "\u{2022}\u{2022}\u{2022}\u{2022}");
    assert_eq!(
        b.call("get_known_regions", json!([])).await,
        json!(["Europe", "Japan", "USA"])
    );
    b.shutdown().await;

    // Nothing lost or down-stamped; the DB stays at v12 with its rows.
    let on_disk = dirs.read_settings();
    assert_eq!(on_disk["version"], 11);
    assert_eq!(on_disk["device_name"], "Deck \u{e9}t\u{e9}");
    assert_eq!(on_disk["some_future_key"], json!({"kept": true}));
    assert_eq!(on_disk["romm_api_token"], "rmm_python_token");
    assert_eq!(dirs.kv("device_id").as_deref(), Some("dev-from-python"));
    let conn = rusqlite::Connection::open(dirs.db()).unwrap();
    let v: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    assert_eq!(v, 15);
    drop(conn);

    // Signing in to a different server forgets the Python-registered device id,
    // clears the playtime notice, and never replays the old token's DELETE.
    let server = romm("4.9.2").await;
    mount_mint(&server, 50, "rmm_new").await;
    let mut b = Backend::spawn(&dirs).await;
    let r = b
        .call("connect_with_credentials", json!([server.uri(), "u", "p", false]))
        .await;
    assert_eq!(r["success"], true);
    b.shutdown().await;
    assert_eq!(dirs.kv("device_id"), None);
    assert_eq!(dirs.kv("playtime_scope_notice"), None);
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method.as_str() != "DELETE")
    );
    assert_eq!(dirs.read_settings()["romm_api_token_origin"], json!(server.uri()));
}
