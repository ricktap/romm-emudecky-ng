//! Integration-test harness: spawns the real backend binary on a temp socket
//! with temp Decky directories and drives it with NDJSON, as the shim does.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::process::{Child, Command};

pub const BIN: &str = env!("CARGO_BIN_EXE_romm-emudecky-ng-backend");

/// The Decky directories of one plugin instance.
pub struct Dirs {
    pub root: tempfile::TempDir,
}

impl Dirs {
    pub fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        for d in ["settings", "runtime", "plugin", "home", "logs"] {
            std::fs::create_dir_all(root.path().join(d)).unwrap();
        }
        std::fs::write(root.path().join("plugin/package.json"), r#"{"version": "0.40.0-test"}"#).unwrap();
        Self { root }
    }
    pub fn settings(&self) -> PathBuf {
        self.root.path().join("settings")
    }
    pub fn runtime(&self) -> PathBuf {
        self.root.path().join("runtime")
    }
    pub fn settings_json(&self) -> PathBuf {
        self.settings().join("settings.json")
    }
    pub fn db(&self) -> PathBuf {
        self.runtime().join("romm_sync.db")
    }
    pub fn socket(&self) -> PathBuf {
        self.root.path().join("b.sock")
    }
    pub fn log(&self) -> PathBuf {
        self.root.path().join("logs/backend.log")
    }
    pub fn read_settings(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.settings_json()).unwrap()).unwrap()
    }
    pub fn write_settings(&self, v: &Value) {
        std::fs::write(self.settings_json(), serde_json::to_vec_pretty(v).unwrap()).unwrap();
    }
    pub fn kv(&self, key: &str) -> Option<String> {
        let conn = rusqlite::Connection::open(self.db()).unwrap();
        conn.query_row("SELECT value FROM kv_config WHERE key = ?1", [key], |r| r.get(0))
            .ok()
    }

    pub fn command(&self) -> Command {
        let mut cmd = Command::new(BIN);
        cmd.arg("--socket")
            .arg(self.socket())
            .env("DECKY_PLUGIN_SETTINGS_DIR", self.settings())
            .env("DECKY_PLUGIN_RUNTIME_DIR", self.runtime())
            .env("DECKY_PLUGIN_DIR", self.root.path().join("plugin"))
            .env("DECKY_USER_HOME", self.root.path().join("home"))
            .env("DECKY_PLUGIN_LOG_DIR", self.root.path().join("logs"))
            .env("ROMM_BACKEND_RETRY_BASE_MS", "1")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        cmd
    }
}

/// A running backend process plus one shim-like connection.
pub struct Backend {
    pub child: Child,
    read: tokio::io::Lines<BufReader<OwnedReadHalf>>,
    write: OwnedWriteHalf,
    next_id: u64,
    /// Event frames seen while waiting for responses, in arrival order.
    pub events: VecDeque<Value>,
    /// Every frame received, in arrival order.
    pub frames: Vec<Value>,
}

impl Backend {
    pub async fn spawn(dirs: &Dirs) -> Self {
        let child = dirs.command().spawn().expect("spawn backend");
        Self::attach(child, &dirs.socket()).await
    }

    pub async fn attach(mut child: Child, socket: &Path) -> Self {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        let stream = loop {
            if let Ok(s) = UnixStream::connect(socket).await {
                break s;
            }
            if let Ok(Some(status)) = child.try_wait() {
                panic!("backend exited during startup: {status}");
            }
            assert!(tokio::time::Instant::now() < deadline, "backend socket never came up");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        let (r, w) = stream.into_split();
        Self {
            child,
            read: BufReader::new(r).lines(),
            write: w,
            next_id: 0,
            events: VecDeque::new(),
            frames: Vec::new(),
        }
    }

    pub async fn send_raw(&mut self, raw: &str) {
        self.write.write_all(raw.as_bytes()).await.unwrap();
    }

    pub async fn next_frame(&mut self) -> Option<Value> {
        let line = tokio::time::timeout(Duration::from_secs(20), self.read.next_line())
            .await
            .expect("timed out waiting for a frame")
            .ok()??;
        let v: Value = serde_json::from_str(&line).unwrap();
        self.frames.push(v.clone());
        Some(v)
    }

    /// Send a request and return its response frame (events are queued).
    pub async fn request(&mut self, method: &str, args: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let frame = json!({"id": id, "method": method, "args": args});
        self.send_raw(&format!("{frame}\n")).await;
        loop {
            let v = self.next_frame().await.expect("connection closed before the response");
            if v.get("event").is_some() {
                self.events.push_back(v);
                continue;
            }
            assert_eq!(v["id"], json!(id), "unexpected frame {v}");
            return v;
        }
    }

    /// Call a callable and unwrap its `result`.
    pub async fn call(&mut self, method: &str, args: Value) -> Value {
        let resp = self.request(method, args).await;
        assert!(resp.get("error").is_none(), "{method} failed: {resp}");
        resp["result"].clone()
    }

    /// `__shutdown__`, then wait for the process to exit.
    pub async fn shutdown(mut self) -> std::process::ExitStatus {
        let resp = self.request("__shutdown__", json!([])).await;
        assert_eq!(resp["result"], Value::Null, "{resp}");
        tokio::time::timeout(Duration::from_secs(10), self.child.wait())
            .await
            .expect("backend did not exit")
            .unwrap()
    }
}
