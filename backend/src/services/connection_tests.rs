//! ConnectionService unit tests — the Rust twin of `tests/services/test_connection.py`.

use super::*;
use crate::services::testing::{FakeKv, FakePersister, FakeRomm};
use serde_json::Map;

const MIN: &[u64] = &[4, 9, 0];

struct Fx {
    settings: SettingsStore,
    romm: Arc<FakeRomm>,
    persister: Arc<FakePersister>,
    kv: Arc<FakeKv>,
    svc: ConnectionService,
}

fn fx_with(initial: Value) -> Fx {
    let settings = SettingsStore::new(initial.as_object().cloned().unwrap_or_else(Map::new));
    let romm = Arc::new(FakeRomm::new(settings.clone()));
    let persister = Arc::new(FakePersister::default());
    let kv = Arc::new(FakeKv::default());
    let svc = ConnectionService::new(ConnectionServiceConfig {
        settings: settings.clone(),
        romm_api: romm.clone(),
        persister: persister.clone(),
        kv_config: kv.clone(),
        min_required_version: MIN,
    });
    Fx {
        settings,
        romm,
        persister,
        kv,
        svc,
    }
}

fn fx() -> Fx {
    fx_with(json!({}))
}

fn signed_in() -> Value {
    json!({
        "romm_url": "http://romm.local",
        "romm_api_token": "rmm_tok",
        "romm_api_token_origin": "http://romm.local",
    })
}

/// A settings document already signed in against the OLD server.
fn working() -> Value {
    json!({
        "romm_url": "https://old.server",
        "romm_allow_insecure_ssl": false,
        "romm_api_token": "rmm_old",
        "romm_api_token_id": 7,
        "romm_api_token_origin": "https://old.server",
    })
}

fn assert_old_state_intact(s: &SettingsStore) {
    assert_eq!(s.get("romm_url"), Some(json!("https://old.server")));
    assert_eq!(s.get("romm_api_token"), Some(json!("rmm_old")));
    assert_eq!(s.get("romm_api_token_id"), Some(json!(7)));
    assert_eq!(s.get("romm_api_token_origin"), Some(json!("https://old.server")));
}

fn heartbeat(f: &Fx, v: Result<Value, RommError>) {
    FakeRomm::set(&f.romm.heartbeat, v);
}

fn version(v: &str) -> Result<Value, RommError> {
    Ok(json!({"SYSTEM": {"VERSION": v}}))
}

fn conn_err() -> RommError {
    RommError::Connection("refused".into())
}

// ── test_connection ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_connection_success_with_version() {
    let f = fx_with(signed_in());
    let r = f.svc.test_connection().await;
    assert_eq!(
        r,
        json!({"success": true, "message": "Connected to RomM 4.9.0", "romm_version": "4.9.0"})
    );
}

#[tokio::test]
async fn test_connection_config_errors_without_probing() {
    for initial in [
        json!({"romm_url": "", "romm_api_token": "t"}),
        json!({"romm_api_token": "t"}),
    ] {
        let f = fx_with(initial);
        let r = f.svc.test_connection().await;
        assert_eq!(
            r,
            json!({"success": false, "reason": "config_error", "message": "No server URL configured"})
        );
    }
    let f = fx_with(json!({"romm_url": "http://romm.local"}));
    let r = f.svc.test_connection().await;
    assert_eq!(r["reason"], "config_error");
    assert!(r["message"].as_str().unwrap().contains("Not signed in"));
    assert!(f.romm.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn test_connection_heartbeat_error_clears_version() {
    let f = fx_with(signed_in());
    heartbeat(&f, Err(conn_err()));
    let r = f.svc.test_connection().await;
    assert_eq!(r["reason"], "server_unreachable");
    assert_eq!(f.romm.versions.lock().unwrap().last(), Some(&None));
}

#[tokio::test]
async fn test_connection_list_platforms_errors() {
    let f = fx_with(signed_in());
    FakeRomm::set(
        &f.romm.list_platforms,
        Err(RommError::Server {
            status: 500,
            message: "x".into(),
        }),
    );
    let r = f.svc.test_connection().await;
    assert_eq!(r["reason"], "server_unreachable");
    assert!(
        r["message"]
            .as_str()
            .unwrap()
            .starts_with("Server reachable but API request failed: Server error (500)")
    );

    let f = fx_with(signed_in());
    FakeRomm::set(&f.romm.list_platforms, Err(RommError::Auth("401".into())));
    let r = f.svc.test_connection().await;
    assert_eq!(r["reason"], "auth_failed");
    assert!(!r["message"].as_str().unwrap().contains("Server reachable"));
}

#[tokio::test]
async fn test_connection_version_gate() {
    for (v, ok) in [
        ("4.8.1", false),
        ("4.5.0", false),
        ("4.9.0-beta.3", false),
        ("4.9.1-beta", true),
        ("5.0.0", true),
    ] {
        let f = fx_with(signed_in());
        heartbeat(&f, version(v));
        let r = f.svc.test_connection().await;
        assert_eq!(r["success"], ok, "{v}");
        if !ok {
            assert_eq!(r["reason"], "version_error");
            assert_eq!(r["romm_version"], v);
            assert!(r["message"].as_str().unwrap().contains("requires RomM 4.9.0 or newer"));
            assert!(r["message"].as_str().unwrap().contains(v));
        }
    }
}

#[tokio::test]
async fn test_connection_development_bypasses_gate() {
    let f = fx_with(signed_in());
    heartbeat(&f, version("development"));
    let r = f.svc.test_connection().await;
    assert_eq!(
        r,
        json!({"success": true, "message": "Connected to RomM", "romm_version": "development"})
    );
}

#[tokio::test]
async fn test_connection_malformed_heartbeats_mean_no_version() {
    for hb in [
        json!({}),
        json!(null),
        json!({"SYSTEM": "nope"}),
        json!({"SYSTEM": {"VERSION": 49}}),
        json!([1]),
    ] {
        let f = fx_with(signed_in());
        heartbeat(&f, Ok(hb.clone()));
        let r = f.svc.test_connection().await;
        assert_eq!(r, json!({"success": true, "message": "Connected to RomM"}), "{hb}");
        assert_eq!(f.romm.versions.lock().unwrap().last(), Some(&None));
    }
}

#[tokio::test]
async fn min_required_version_is_injected() {
    let settings = SettingsStore::new(signed_in().as_object().cloned().unwrap());
    let romm = Arc::new(FakeRomm::new(settings.clone()));
    FakeRomm::set(&romm.heartbeat, version("5.0.0"));
    let svc = ConnectionService::new(ConnectionServiceConfig {
        settings,
        romm_api: romm,
        persister: Arc::new(FakePersister::default()),
        kv_config: Arc::new(FakeKv::default()),
        min_required_version: &[6, 0, 0],
    });
    let r = svc.test_connection().await;
    assert_eq!(r["reason"], "version_error");
    assert!(r["message"].as_str().unwrap().contains("requires RomM 6.0.0"));
}

// ── probe_reachability ──────────────────────────────────────────────────

#[tokio::test]
async fn probe_reachability() {
    let f = fx();
    assert_eq!(f.svc.probe_reachability().await, json!({"online": true}));
    assert_eq!(f.romm.calls_to("heartbeat_once").len(), 1);
    assert!(f.romm.calls_to("heartbeat").is_empty(), "never the retrying heartbeat");
    FakeRomm::set(&f.romm.heartbeat_once, Err(RommError::Timeout("t".into())));
    assert_eq!(f.svc.probe_reachability().await, json!({"online": false}));
    FakeRomm::set(&f.romm.heartbeat_once, Err(RommError::Api("weird".into())));
    assert_eq!(f.svc.probe_reachability().await, json!({"online": false}));
}

// ── establish_token (credentials) ───────────────────────────────────────

#[tokio::test]
async fn establish_token_mints_and_stores_in_one_save() {
    let f = fx();
    let r = f
        .svc
        .establish_token("http://romm.local", "alice", "secret", None)
        .await;
    assert_eq!(r["success"], true);
    assert_eq!(r["romm_version"], "4.9.0");
    assert_eq!(f.settings.get("romm_api_token"), Some(json!("rmm_minted")));
    assert_eq!(f.settings.get("romm_api_token_id"), Some(json!(42)));
    assert_eq!(
        f.settings.get("romm_api_token_origin"),
        Some(json!("http://romm.local"))
    );
    assert_eq!(f.settings.get("romm_api_token_source"), Some(json!("minted")));
    assert_eq!(f.persister.saves(), 1);
    assert!(!f.settings.contains("romm_user") && !f.settings.contains("romm_pass"));
    assert_eq!(f.romm.calls_to("mint_client_token").len(), 1);
}

#[tokio::test]
async fn establish_token_keeps_settings_reset_marker() {
    let f = fx_with(json!({"_settings_reset_notice": {"backed_up_to": "settings.json.corrupt-42"}}));
    assert_eq!(
        f.svc.establish_token("http://romm.local", "a", "b", None).await["success"],
        true
    );
    assert_eq!(
        f.settings.get("_settings_reset_notice"),
        Some(json!({"backed_up_to": "settings.json.corrupt-42"}))
    );

    let f = fx_with(json!({"_settings_reset_notice": {"backed_up_to": "settings.json.corrupt-42"}}));
    FakeRomm::set(&f.romm.mint, Err(RommError::Connection("offline".into())));
    assert_eq!(
        f.svc.establish_token("http://romm.local", "a", "b", None).await["success"],
        false
    );
    assert!(f.settings.contains("_settings_reset_notice"));
    assert_eq!(f.persister.saves(), 0);
}

#[tokio::test]
async fn establish_token_wipes_legacy_credentials() {
    let f = fx_with(json!({"romm_user": "old", "romm_pass": "pw"}));
    f.svc
        .establish_token("http://romm.local", "alice", "secret", None)
        .await;
    assert!(!f.settings.contains("romm_user") && !f.settings.contains("romm_pass"));
}

#[tokio::test]
async fn establish_token_token_name() {
    let f = fx_with(json!({"device_name": "Living Room Deck"}));
    f.svc.establish_token("http://romm.local", "a", "b", None).await;
    assert_eq!(
        f.romm.calls_to("mint_client_token")[0].args[2],
        "romm-emudecky-ng (Living Room Deck)"
    );
    let f = fx_with(json!({"device_name": null}));
    f.svc.establish_token("http://romm.local", "a", "b", None).await;
    assert_eq!(
        f.romm.calls_to("mint_client_token")[0].args[2],
        "romm-emudecky-ng (Steam Deck)"
    );
}

#[tokio::test]
async fn establish_token_persists_url_and_ssl_flag() {
    let f = fx();
    f.svc
        .establish_token("  https://romm.local/  ", "a", "b", Some(&json!(true)))
        .await;
    assert_eq!(f.settings.get("romm_url"), Some(json!("https://romm.local/")));
    assert_eq!(f.settings.get("romm_allow_insecure_ssl"), Some(json!(true)));
    assert_eq!(
        f.settings.get("romm_api_token_origin"),
        Some(json!("https://romm.local"))
    );
}

#[tokio::test]
async fn sign_in_clears_playtime_scope_notice_only_on_success() {
    let f = fx();
    f.kv.insert("playtime_scope_notice", "1");
    assert_eq!(
        f.svc.establish_token("http://romm.local", "a", "b", None).await["success"],
        true
    );
    assert!(!f.kv.has("playtime_scope_notice"));

    let f = fx();
    f.kv.insert("playtime_scope_notice", "1");
    FakeRomm::set(&f.romm.mint, Err(RommError::Auth("401".into())));
    f.svc.establish_token("http://romm.local", "a", "b", None).await;
    assert!(f.kv.has("playtime_scope_notice"));
}

#[tokio::test]
async fn sign_in_succeeds_even_if_local_clears_fail() {
    let f = fx();
    f.kv.fail();
    let r = f.svc.establish_token("http://romm.local", "a", "b", None).await;
    assert_eq!(r["success"], true);
    assert_eq!(f.settings.get("romm_api_token"), Some(json!("rmm_minted")));
}

#[tokio::test]
async fn old_token_deleted_only_on_same_origin_and_minted() {
    // Same origin (trailing slash / default port ignored) → DELETE replayed.
    for url in ["https://old.server", "https://old.server:443/"] {
        let f = fx_with(working());
        f.svc.establish_token(url, "u", "p", None).await;
        let deletes = f.romm.calls_to("delete_client_token");
        assert_eq!(deletes.len(), 1, "{url}");
        assert_eq!(deletes[0].args, vec!["u", "p", "7"]);
    }
    // Different origin → skipped.
    let f = fx_with(working());
    f.svc.establish_token("https://new.server", "u", "p", None).await;
    assert!(f.romm.calls_to("delete_client_token").is_empty());
    // Unknown old origin → skipped.
    let mut w = working();
    w["romm_api_token_origin"] = Value::Null;
    let f = fx_with(w);
    f.svc.establish_token("https://old.server", "u", "p", None).await;
    assert!(f.romm.calls_to("delete_client_token").is_empty());
    // No old token → nothing to delete.
    let f = fx();
    f.svc.establish_token("https://old.server", "u", "p", None).await;
    assert!(f.romm.calls_to("delete_client_token").is_empty());
    // A user-supplied token is never deleted.
    let mut w = working();
    w["romm_api_token_source"] = json!("user");
    let f = fx_with(w);
    f.svc.establish_token("https://old.server", "u", "p", None).await;
    assert!(f.romm.calls_to("delete_client_token").is_empty());
    // A minted one still is.
    let mut w = working();
    w["romm_api_token_source"] = json!("minted");
    let f = fx_with(w);
    f.svc.establish_token("https://old.server", "u", "p", None).await;
    assert_eq!(f.romm.calls_to("delete_client_token").len(), 1);
}

#[tokio::test]
async fn delete_failure_is_ignored() {
    let f = fx_with(working());
    FakeRomm::set(
        &f.romm.delete,
        Err(RommError::Server {
            status: 500,
            message: "x".into(),
        }),
    );
    let r = f.svc.establish_token("https://old.server", "u", "p", None).await;
    assert_eq!(r["success"], true);
}

#[tokio::test]
async fn device_forget_on_origin_change() {
    // Different origin → forgotten.
    let f = fx_with(working());
    f.kv.insert("device_id", "dev-1");
    f.svc.establish_token("https://new.server", "u", "p", None).await;
    assert!(!f.kv.has("device_id"));
    // Same origin (normalized) → kept.
    for url in ["https://old.server", "https://OLD.server:443/romm"] {
        let f = fx_with(working());
        f.kv.insert("device_id", "dev-1");
        f.svc.establish_token(url, "u", "p", None).await;
        assert!(f.kv.has("device_id"), "{url}");
    }
    // Unknown old origin / first sign-in → forgotten.
    let mut w = working();
    w["romm_api_token_origin"] = Value::Null;
    for initial in [w, json!({})] {
        let f = fx_with(initial);
        f.kv.insert("device_id", "dev-1");
        f.svc.establish_token("https://old.server", "u", "p", None).await;
        assert!(!f.kv.has("device_id"));
    }
}

#[tokio::test]
async fn device_kept_on_failed_sign_in() {
    let f = fx_with(working());
    f.kv.insert("device_id", "dev-1");
    FakeRomm::set(&f.romm.mint, Err(RommError::Auth("401".into())));
    f.svc.establish_token("https://new.server", "u", "p", None).await;
    assert!(f.kv.has("device_id"));

    let f = fx_with(working());
    f.kv.insert("device_id", "dev-1");
    heartbeat(&f, version("4.5.0"));
    f.svc.establish_token("https://new.server", "u", "p", None).await;
    assert!(f.kv.has("device_id"));
}

#[tokio::test]
async fn establish_token_bad_inputs() {
    let f = fx();
    let r = f.svc.establish_token("", "u", "p", None).await;
    assert_eq!(
        r,
        json!({"success": false, "reason": "config_error", "message": "No server URL configured"})
    );
    for bad in ["romm.local", "ftp://romm.local", "   ", "https://"] {
        let r = f.svc.establish_token(bad, "u", "p", None).await;
        assert_eq!(
            r,
            json!({"success": false, "reason": "config_error", "message": "Enter a valid http(s):// server URL"})
        );
    }
    assert!(f.romm.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn establish_token_failures() {
    let f = fx();
    heartbeat(&f, Err(conn_err()));
    let r = f.svc.establish_token("http://romm.local", "u", "p", None).await;
    assert_eq!(r["reason"], "server_unreachable");
    assert!(f.romm.calls_to("mint_client_token").is_empty());

    let f = fx();
    heartbeat(&f, version("4.5.0"));
    let r = f.svc.establish_token("http://romm.local", "u", "p", None).await;
    assert_eq!(r["reason"], "version_error");
    assert!(f.romm.calls_to("mint_client_token").is_empty());

    let f = fx();
    FakeRomm::set(&f.romm.mint, Err(RommError::Forbidden("403".into())));
    let r = f.svc.establish_token("http://romm.local", "u", "p", None).await;
    assert_eq!(
        r,
        json!({"success": false, "reason": "auth_failed", "message": FORBIDDEN_TOKEN_MESSAGE})
    );

    let f = fx();
    FakeRomm::set(&f.romm.mint, Err(RommError::Auth("401".into())));
    let r = f.svc.establish_token("http://romm.local", "u", "p", None).await;
    assert_eq!(r["reason"], "auth_failed");
    assert!(r["message"].as_str().unwrap().contains("Authentication failed"));

    for minted in [
        json!({"id": 42}),
        json!({"raw_token": "x"}),
        json!({"raw_token": "", "id": 1}),
        json!([]),
    ] {
        let f = fx();
        FakeRomm::set(&f.romm.mint, Ok(minted));
        let r = f.svc.establish_token("http://romm.local", "u", "p", None).await;
        assert_eq!(
            r,
            json!({"success": false, "reason": "server_unreachable", "message": NO_USABLE_TOKEN_MESSAGE})
        );
        assert_eq!(f.persister.saves(), 0);
    }
}

#[tokio::test]
async fn establish_token_snapshot_restore() {
    // Probe failure.
    let f = fx_with(working());
    heartbeat(&f, Err(conn_err()));
    assert_eq!(
        f.svc.establish_token("https://new.server", "u", "p", None).await["success"],
        false
    );
    assert_old_state_intact(&f.settings);
    assert_eq!(f.persister.saves(), 0);
    // Version gate.
    let f = fx_with(working());
    heartbeat(&f, version("4.5.0"));
    assert_eq!(
        f.svc.establish_token("https://new.server", "u", "p", None).await["reason"],
        "version_error"
    );
    assert_old_state_intact(&f.settings);
    assert_eq!(f.persister.saves(), 0);
    // Mint failure.
    let f = fx_with(working());
    FakeRomm::set(&f.romm.mint, Err(RommError::Forbidden("403".into())));
    assert_eq!(
        f.svc.establish_token("https://new.server", "u", "p", None).await["reason"],
        "auth_failed"
    );
    assert_old_state_intact(&f.settings);
    assert_eq!(f.persister.saves(), 0);
    // Persist failure.
    let f = fx_with(working());
    f.persister.fail_with("disk full");
    let r = f.svc.establish_token("https://new.server", "u", "p", None).await;
    assert_eq!(
        r,
        json!({"success": false, "reason": "unknown", "message": "disk full"})
    );
    assert_old_state_intact(&f.settings);
}

#[tokio::test]
async fn establish_token_clears_old_bearer_before_probe() {
    let f = fx_with(working());
    f.svc.establish_token("https://new.server", "u", "p", None).await;
    let hb = &f.romm.calls_to("heartbeat")[0];
    assert_eq!(hb.token, Value::Null);
    assert_eq!(hb.origin, Value::Null);
}

#[tokio::test]
async fn establish_token_new_origin_stamps_and_persists_once() {
    let f = fx_with(working());
    let r = f.svc.establish_token("https://new.server", "u", "p", None).await;
    assert_eq!(r["success"], true);
    assert_eq!(f.settings.get("romm_url"), Some(json!("https://new.server")));
    assert_eq!(f.settings.get("romm_api_token"), Some(json!("rmm_minted")));
    assert_eq!(f.settings.get("romm_api_token_id"), Some(json!(42)));
    assert_eq!(
        f.settings.get("romm_api_token_origin"),
        Some(json!("https://new.server"))
    );
    assert_eq!(f.persister.saves(), 1);
}

// ── establish_user_token (pasted token) ─────────────────────────────────

#[tokio::test]
async fn user_token_happy_path() {
    let f = fx();
    let r = f
        .svc
        .establish_user_token("http://romm.local", "  rmm_pasted \n", Some(&json!(true)))
        .await;
    assert_eq!(
        r,
        json!({"success": true, "message": "Connected to RomM 4.9.0", "romm_version": "4.9.0"})
    );
    assert_eq!(f.settings.get("romm_api_token"), Some(json!("rmm_pasted")));
    assert_eq!(f.settings.get("romm_api_token_id"), Some(Value::Null));
    assert_eq!(
        f.settings.get("romm_api_token_origin"),
        Some(json!("http://romm.local"))
    );
    assert_eq!(f.settings.get("romm_api_token_source"), Some(json!("user")));
    assert_eq!(f.settings.get("romm_allow_insecure_ssl"), Some(json!(true)));
    assert_eq!(f.persister.saves(), 1);
    let me = f.romm.calls_to("get_current_user");
    assert_eq!(me.len(), 1);
    assert_eq!(me[0].token, json!("rmm_pasted"));
    assert!(f.romm.calls_to("mint_client_token").is_empty());
    assert!(f.romm.calls_to("delete_client_token").is_empty());
}

#[tokio::test]
async fn user_token_side_effects() {
    let f = fx();
    f.kv.insert("device_id", "d");
    f.kv.insert("playtime_scope_notice", "1");
    f.svc.establish_user_token("http://romm.local", "rmm", None).await;
    assert!(!f.kv.has("device_id"));
    assert!(!f.kv.has("playtime_scope_notice"));
}

#[tokio::test]
async fn user_token_bad_inputs() {
    let f = fx();
    assert_eq!(
        f.svc.establish_user_token("", "t", None).await["message"],
        "No server URL configured"
    );
    for bad in ["romm.local", "ftp://romm.local", "   ", "https://"] {
        assert_eq!(
            f.svc.establish_user_token(bad, "t", None).await["message"],
            INVALID_URL_MESSAGE
        );
    }
    for blank in ["", "   ", "\t\n"] {
        assert_eq!(
            f.svc.establish_user_token("http://romm.local", blank, None).await,
            json!({"success": false, "reason": "config_error", "message": "Enter your RomM API token"})
        );
    }
    assert!(f.romm.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn user_token_failures() {
    let f = fx();
    heartbeat(&f, Err(conn_err()));
    assert_eq!(
        f.svc.establish_user_token("http://romm.local", "t", None).await["reason"],
        "server_unreachable"
    );
    assert!(f.romm.calls_to("get_current_user").is_empty());

    let f = fx();
    heartbeat(&f, version("4.5.0"));
    assert_eq!(
        f.svc.establish_user_token("http://romm.local", "t", None).await["reason"],
        "version_error"
    );
    assert!(f.romm.calls_to("get_current_user").is_empty());

    let f = fx();
    FakeRomm::set(&f.romm.current_user, Err(RommError::Auth("401".into())));
    let r = f.svc.establish_user_token("http://romm.local", "t", None).await;
    assert_eq!(
        r,
        json!({"success": false, "reason": "auth_failed", "message": USER_TOKEN_INVALID_MESSAGE})
    );
    assert_eq!(f.settings.get("romm_api_token"), Some(Value::Null));
    assert_eq!(f.settings.get("romm_api_token_source"), Some(Value::Null));
    assert_eq!(f.persister.saves(), 0);

    let f = fx();
    FakeRomm::set(&f.romm.current_user, Err(RommError::Forbidden("403".into())));
    let r = f.svc.establish_user_token("http://romm.local", "t", None).await;
    assert_eq!(r["message"], USER_TOKEN_SCOPE_MESSAGE);

    let f = fx();
    FakeRomm::set(
        &f.romm.current_user,
        Err(RommError::Server {
            status: 503,
            message: "x".into(),
        }),
    );
    let r = f.svc.establish_user_token("http://romm.local", "t", None).await;
    assert_eq!(r["reason"], "server_unreachable");

    let f = fx();
    f.persister.fail_with("disk full");
    let r = f.svc.establish_user_token("http://romm.local", "t", None).await;
    assert_eq!(
        r,
        json!({"success": false, "reason": "unknown", "message": "disk full"})
    );
}

#[tokio::test]
async fn user_token_snapshot_restore_and_binding() {
    let f = fx_with(working());
    FakeRomm::set(&f.romm.current_user, Err(RommError::Auth("401".into())));
    assert_eq!(
        f.svc.establish_user_token("https://new.server", "rmm_bad", None).await["success"],
        false
    );
    assert_old_state_intact(&f.settings);
    assert_eq!(f.persister.saves(), 0);

    let f = fx_with(working());
    heartbeat(&f, Err(conn_err()));
    assert_eq!(
        f.svc.establish_user_token("https://new.server", "rmm", None).await["success"],
        false
    );
    assert_old_state_intact(&f.settings);
    assert_eq!(f.persister.saves(), 0);

    let f = fx_with(working());
    f.svc
        .establish_user_token("https://new.server", "rmm_pasted", None)
        .await;
    let me = &f.romm.calls_to("get_current_user")[0];
    assert_eq!(me.token, json!("rmm_pasted"));
    assert_eq!(me.origin, json!("https://new.server"));
}

// ── establish_paired_token (pairing code) ───────────────────────────────

#[tokio::test]
async fn paired_token_happy_path() {
    let f = fx();
    let r = f
        .svc
        .establish_paired_token("http://romm.local", " ab-cd ef23 ", Some(&json!(false)))
        .await;
    assert_eq!(r["success"], true);
    assert_eq!(f.romm.calls_to("exchange_pairing_code")[0].args, vec!["ABCDEF23"]);
    assert_eq!(f.settings.get("romm_api_token"), Some(json!("rmm_paired")));
    assert_eq!(f.settings.get("romm_api_token_id"), Some(Value::Null));
    assert_eq!(f.settings.get("romm_api_token_source"), Some(json!("user")));
    assert_eq!(
        f.settings.get("romm_api_token_origin"),
        Some(json!("http://romm.local"))
    );
    assert_eq!(f.settings.get("romm_allow_insecure_ssl"), Some(json!(false)));
    assert_eq!(f.persister.saves(), 1);
    let me = &f.romm.calls_to("get_current_user")[0];
    assert_eq!(me.token, json!("rmm_paired"));
    assert_eq!(me.origin, json!("http://romm.local"));
}

#[tokio::test]
async fn paired_token_side_effects() {
    let f = fx();
    f.kv.insert("device_id", "d");
    f.kv.insert("playtime_scope_notice", "1");
    f.svc.establish_paired_token("http://romm.local", "ABCD", None).await;
    assert!(!f.kv.has("device_id"));
    assert!(!f.kv.has("playtime_scope_notice"));
}

#[tokio::test]
async fn paired_token_bad_inputs() {
    let f = fx();
    assert_eq!(
        f.svc.establish_paired_token("", "c", None).await["message"],
        "No server URL configured"
    );
    for bad in ["romm.local", "ftp://romm.local", "   ", "https://"] {
        assert_eq!(
            f.svc.establish_paired_token(bad, "c", None).await["message"],
            INVALID_URL_MESSAGE
        );
    }
    for blank in ["", "   ", "\t\n", "-", "- -"] {
        assert_eq!(
            f.svc.establish_paired_token("http://romm.local", blank, None).await,
            json!({"success": false, "reason": "config_error", "message": ENTER_PAIRING_CODE_MESSAGE})
        );
    }
    assert!(f.romm.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn paired_token_failures() {
    let f = fx();
    heartbeat(&f, Err(conn_err()));
    assert_eq!(
        f.svc.establish_paired_token("http://romm.local", "c", None).await["reason"],
        "server_unreachable"
    );
    assert!(f.romm.calls_to("exchange_pairing_code").is_empty());

    let f = fx();
    heartbeat(&f, version("4.5.0"));
    assert_eq!(
        f.svc.establish_paired_token("http://romm.local", "c", None).await["reason"],
        "version_error"
    );
    assert!(f.romm.calls_to("exchange_pairing_code").is_empty());

    for (err, reason, message) in [
        (
            RommError::PairingCodeInvalid,
            "auth_failed",
            PAIRING_CODE_INVALID_MESSAGE,
        ),
        (RommError::PairingTokenGone, "auth_failed", PAIRING_TOKEN_GONE_MESSAGE),
        (
            RommError::PairingOwnerDisabled,
            "auth_failed",
            PAIRING_OWNER_DISABLED_MESSAGE,
        ),
        (
            RommError::PairingRateLimited,
            "rate_limited",
            PAIRING_RATE_LIMITED_MESSAGE,
        ),
    ] {
        let f = fx();
        FakeRomm::set(&f.romm.exchange, Err(err));
        let r = f.svc.establish_paired_token("http://romm.local", "c", None).await;
        assert_eq!(r, json!({"success": false, "reason": reason, "message": message}));
        assert_eq!(f.persister.saves(), 0);
        assert_eq!(f.settings.get("romm_api_token"), Some(Value::Null));
    }

    let f = fx();
    FakeRomm::set(&f.romm.exchange, Err(RommError::Timeout("t".into())));
    let r = f.svc.establish_paired_token("http://romm.local", "c", None).await;
    assert_eq!(r["reason"], "server_unreachable");
    assert!(r["message"].as_str().unwrap().contains("timed out"));

    let f = fx();
    FakeRomm::set(&f.romm.exchange, Ok(json!({"id": 3})));
    let r = f.svc.establish_paired_token("http://romm.local", "c", None).await;
    assert_eq!(r["message"], NO_USABLE_TOKEN_MESSAGE);
    assert_eq!(f.persister.saves(), 0);

    let f = fx();
    FakeRomm::set(&f.romm.current_user, Err(RommError::Auth("401".into())));
    let r = f.svc.establish_paired_token("http://romm.local", "c", None).await;
    assert_eq!(r["message"], USER_TOKEN_INVALID_MESSAGE);
    assert_eq!(f.persister.saves(), 0);
}

#[tokio::test]
async fn paired_token_snapshot_restore() {
    for err in [RommError::PairingCodeInvalid, RommError::PairingRateLimited] {
        let f = fx_with(working());
        FakeRomm::set(&f.romm.exchange, Err(err));
        f.svc.establish_paired_token("https://new.server", "c", None).await;
        assert_old_state_intact(&f.settings);
        assert_eq!(f.persister.saves(), 0);
    }
    let f = fx_with(working());
    heartbeat(&f, Err(conn_err()));
    f.svc.establish_paired_token("https://new.server", "c", None).await;
    assert_old_state_intact(&f.settings);
    // The exchange itself runs with no bearer bound.
    let f = fx_with(working());
    f.svc.establish_paired_token("https://new.server", "c", None).await;
    let ex = &f.romm.calls_to("exchange_pairing_code")[0];
    assert_eq!(ex.token, Value::Null);
}

// ── migrate_legacy_credentials ──────────────────────────────────────────

#[tokio::test]
async fn legacy_credentials_minted_and_wiped() {
    let f = fx_with(json!({"romm_url": "https://romm.local:443/", "romm_user": "alice", "romm_pass": "pw"}));
    f.svc.migrate_legacy_credentials().await;
    assert_eq!(f.settings.get("romm_api_token"), Some(json!("rmm_minted")));
    assert_eq!(f.settings.get("romm_api_token_id"), Some(json!(42)));
    assert_eq!(
        f.settings.get("romm_api_token_origin"),
        Some(json!("https://romm.local"))
    );
    assert_eq!(f.settings.get("romm_api_token_source"), Some(json!("minted")));
    assert!(!f.settings.contains("romm_user") && !f.settings.contains("romm_pass"));
    assert_eq!(f.persister.saves(), 1);
}

#[tokio::test]
async fn legacy_credentials_noops() {
    for initial in [
        json!({"romm_api_token": "t", "romm_user": "a", "romm_pass": "p"}),
        json!({}),
        json!({"romm_user": "a"}),
        json!({"romm_user": "a", "romm_pass": ""}),
    ] {
        let f = fx_with(initial);
        f.svc.migrate_legacy_credentials().await;
        assert!(f.romm.calls_to("mint_client_token").is_empty());
        assert_eq!(f.persister.saves(), 0);
    }
}

#[tokio::test]
async fn legacy_credentials_failures_leave_credentials() {
    let f = fx_with(json!({"romm_user": "a", "romm_pass": "p"}));
    FakeRomm::set(&f.romm.mint, Err(conn_err()));
    f.svc.migrate_legacy_credentials().await;
    assert_eq!(f.settings.get("romm_pass"), Some(json!("p")));

    let f = fx_with(json!({"romm_user": "a", "romm_pass": "p"}));
    FakeRomm::set(&f.romm.mint, Ok(json!({"raw_token": "x"})));
    f.svc.migrate_legacy_credentials().await;
    assert_eq!(f.settings.get("romm_pass"), Some(json!("p")));
    assert_eq!(f.settings.get("romm_api_token"), None);

    let f = fx_with(json!({"romm_user": "a", "romm_pass": "p"}));
    f.persister.fail_with("disk full");
    f.svc.migrate_legacy_credentials().await; // swallowed, never panics
}

// ── sign_out ────────────────────────────────────────────────────────────

#[tokio::test]
async fn sign_out_clears_token_keeps_url_and_ssl() {
    let mut w = working();
    w["romm_api_token_source"] = json!("minted");
    let f = fx_with(w);
    let r = f.svc.sign_out().await;
    assert_eq!(r["success"], true);
    assert!(r["message"].as_str().unwrap().contains("Signed out"));
    for key in [
        "romm_api_token",
        "romm_api_token_id",
        "romm_api_token_origin",
        "romm_api_token_source",
    ] {
        assert_eq!(f.settings.get(key), Some(Value::Null), "{key}");
    }
    assert_eq!(f.settings.get("romm_url"), Some(json!("https://old.server")));
    assert_eq!(f.settings.get("romm_allow_insecure_ssl"), Some(json!(false)));
    assert_eq!(f.persister.saves(), 1);
    assert_eq!(*f.romm.versions.lock().unwrap(), vec![None]);
    assert!(f.romm.calls_to("delete_client_token").is_empty());
}

#[tokio::test]
async fn sign_out_idempotent() {
    let f = fx_with(json!({"romm_url": "https://old.server", "romm_allow_insecure_ssl": true}));
    assert_eq!(f.svc.sign_out().await["success"], true);
    assert_eq!(f.settings.get("romm_api_token"), Some(Value::Null));
    assert_eq!(f.settings.get("romm_allow_insecure_ssl"), Some(json!(true)));
    assert_eq!(f.persister.saves(), 1);
}

#[tokio::test]
async fn sign_out_persist_failure_restores() {
    let mut w = working();
    w["romm_api_token_source"] = json!("minted");
    let f = fx_with(w);
    f.persister.fail_with("disk full");
    let r = f.svc.sign_out().await;
    assert_eq!(
        r,
        json!({"success": false, "reason": "unknown", "message": "disk full"})
    );
    assert_old_state_intact(&f.settings);
    assert_eq!(f.settings.get("romm_api_token_source"), Some(json!("minted")));
    assert!(f.romm.versions.lock().unwrap().is_empty());
}

#[test]
fn pairing_code_normalization() {
    assert_eq!(normalize_pairing_code("ab-cd ef23"), "ABCDEF23");
    assert_eq!(normalize_pairing_code(" \tx-y\n"), "XY");
    assert_eq!(normalize_pairing_code("- -"), "");
}
