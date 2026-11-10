//! The real binary over the real socket: the protocol contract.

mod common;

use std::time::Duration;

use common::{BIN, Backend, Dirs};
use serde_json::{Value, json};

fn expected_names() -> Vec<String> {
    let raw = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/callables.json")).unwrap();
    let mut names: Vec<String> = serde_json::from_str(&raw).unwrap();
    names.sort();
    names
}

#[test]
fn list_methods_flag_prints_the_surface_without_any_environment() {
    let mut cmd = std::process::Command::new(BIN);
    cmd.arg("--list-methods").env_clear();
    // Keep coverage profiles landing in the target dir under `cargo llvm-cov`.
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        cmd.env("LLVM_PROFILE_FILE", profile);
    }
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    let names: Vec<String> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(names.len(), 116);
    assert_eq!(names, expected_names());
}

#[test]
fn bad_arguments_exit_with_usage_error() {
    let out = std::process::Command::new(BIN).arg("--bogus").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let out = std::process::Command::new(BIN)
        .env_remove("ROMM_BACKEND_SOCKET")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[tokio::test]
async fn list_unknown_unported_arity_and_malformed() {
    let dirs = Dirs::new();
    let mut b = Backend::spawn(&dirs).await;

    let names = b.call("__list_methods__", json!([])).await;
    let names: Vec<String> = serde_json::from_value(names).unwrap();
    assert_eq!(names, expected_names());

    let r = b.request("no_such_method", json!([])).await;
    assert_eq!(r["error"], json!({"message": "unknown method: no_such_method"}));

    // Every callable is ported now (save sync landed in 0.55.0): the "not implemented" answer is kept for the next gap, so it
    // is only exercised while one exists.
    if let Some(unported) = names.iter().find(|name| !romm_backend::app::PORTED_CALLABLES.contains(&name.as_str())) {
        let r = b.request(unported, json!([])).await;
        assert_eq!(r.get("error"), None);
        assert_eq!(
            r["result"],
            json!({
                "success": false,
                "reason": "not_implemented",
                "message": format!("not implemented in the Rust backend yet: {unported}"),
            })
        );
    } else {
        assert_eq!(romm_backend::app::PORTED_CALLABLES.len(), names.len(), "no gaps and no stale entries");
    }

    let r = b.request("get_settings", json!([1])).await;
    assert!(r["error"]["message"].as_str().unwrap().contains("positional arguments"));

    let r = b.request("save_server_url", json!([5])).await;
    assert!(r["error"]["message"].as_str().unwrap().contains("must be a string"));

    b.send_raw("{\"id\": 1, oops\n").await;
    let frame = b.next_frame().await.unwrap();
    assert_eq!(frame["id"], Value::Null);
    assert!(
        frame["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("malformed frame")
    );

    let status = b.shutdown().await;
    assert!(status.success());
    assert!(!dirs.socket().exists(), "socket removed on shutdown");
}

#[tokio::test]
async fn concurrent_requests_on_one_connection() {
    let dirs = Dirs::new();
    let mut b = Backend::spawn(&dirs).await;
    let mut batch = String::new();
    for id in 1..=20 {
        batch.push_str(&format!(
            "{}\n",
            json!({"id": id, "method": "get_whitelist_settings", "args": []})
        ));
    }
    b.send_raw(&batch).await;
    let mut seen = Vec::new();
    for _ in 0..20 {
        let f = b.next_frame().await.unwrap();
        assert_eq!(f["result"], json!({"disabled_defaults": [], "custom_names": []}));
        seen.push(f["id"].as_u64().unwrap());
    }
    seen.sort();
    assert_eq!(seen, (1..=20).collect::<Vec<_>>());
    b.shutdown().await;
}

#[tokio::test]
async fn sigterm_shuts_down_cleanly() {
    let dirs = Dirs::new();
    let b = Backend::spawn(&dirs).await;
    let pid = b.child.id().unwrap() as i32;
    // SAFETY: plain kill(2) on our own child.
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
    let mut child = b.child;
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    assert!(!dirs.socket().exists());
}

#[tokio::test]
async fn log_file_is_written_in_the_decky_log_dir() {
    let dirs = Dirs::new();
    let b = Backend::spawn(&dirs).await;
    b.shutdown().await;
    let log = std::fs::read_to_string(dirs.log()).unwrap();
    assert!(log.contains("backend started"), "{log}");
    assert!(log.contains("backend stopped"), "{log}");
}

#[tokio::test]
async fn missing_decky_environment_fails_startup() {
    let dirs = Dirs::new();
    let mut cmd = dirs.command();
    cmd.env_remove("DECKY_PLUGIN_SETTINGS_DIR");
    let out = cmd.output().await.unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("DECKY_PLUGIN_SETTINGS_DIR"));
}

#[tokio::test]
async fn unusable_database_fails_startup() {
    let dirs = Dirs::new();
    std::fs::write(dirs.db(), b"this is not a sqlite database, just bytes........").unwrap();
    let out = dirs.command().output().await.unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("SQLite"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!dirs.socket().exists());
}
