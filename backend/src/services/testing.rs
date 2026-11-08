//! In-memory fakes of the service ports, for unit tests.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::domain::errors::RommError;
use crate::services::ports::{
    KvConfigRepo, RetroArchInputConfig, RomRegionsReader, RommConnectionApi, SettingsPersister, SettingsStore,
};

/// Counts saves; can be told to fail. Optionally records the settings
/// snapshot each save would have written.
#[derive(Default)]
pub struct FakePersister {
    saves: AtomicUsize,
    fail: Mutex<Option<String>>,
}

impl FakePersister {
    pub fn saves(&self) -> usize {
        self.saves.load(Ordering::SeqCst)
    }
    pub fn fail_with(&self, message: &str) {
        *self.fail.lock().unwrap() = Some(message.to_string());
    }
}

#[async_trait]
impl SettingsPersister for FakePersister {
    async fn save_settings(&self) -> anyhow::Result<()> {
        if let Some(msg) = self.fail.lock().unwrap().clone() {
            anyhow::bail!(msg);
        }
        self.saves.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Default)]
pub struct FakeRegions(Mutex<Vec<Vec<String>>>);

impl FakeRegions {
    pub fn set(&self, regions: Vec<Vec<String>>) {
        *self.0.lock().unwrap() = regions;
    }
}

#[async_trait]
impl RomRegionsReader for FakeRegions {
    async fn all_rom_regions(&self) -> anyhow::Result<Vec<Vec<String>>> {
        Ok(self.0.lock().unwrap().clone())
    }
}

#[derive(Default)]
pub struct FakeRetroArch {
    check: Mutex<Option<Value>>,
    fix: Mutex<Option<Value>>,
}

impl FakeRetroArch {
    pub fn set_check(&self, v: Option<Value>) {
        *self.check.lock().unwrap() = v;
    }
    pub fn set_fix(&self, v: Value) {
        *self.fix.lock().unwrap() = Some(v);
    }
}

#[async_trait]
impl RetroArchInputConfig for FakeRetroArch {
    async fn check_input_driver(&self) -> Option<Value> {
        self.check.lock().unwrap().clone()
    }
    async fn fix_input_driver(&self) -> Value {
        self.fix
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(json!({"success": false, "message": "No fix needed"}))
    }
}

#[derive(Default)]
pub struct FakeKv {
    pub rows: Mutex<HashMap<String, String>>,
    fail: Mutex<bool>,
}

impl FakeKv {
    pub fn insert(&self, k: &str, v: &str) {
        self.rows.lock().unwrap().insert(k.into(), v.into());
    }
    pub fn has(&self, k: &str) -> bool {
        self.rows.lock().unwrap().contains_key(k)
    }
    pub fn fail(&self) {
        *self.fail.lock().unwrap() = true;
    }
    fn check(&self) -> anyhow::Result<()> {
        if *self.fail.lock().unwrap() {
            anyhow::bail!("database is locked");
        }
        Ok(())
    }
}

#[async_trait]
impl KvConfigRepo for FakeKv {
    async fn get(&self, key: &str) -> anyhow::Result<Option<String>> {
        self.check()?;
        Ok(self.rows.lock().unwrap().get(key).cloned())
    }
    async fn set(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.check()?;
        self.insert(key, value);
        Ok(())
    }
    async fn delete(&self, key: &str) -> anyhow::Result<()> {
        self.check()?;
        self.rows.lock().unwrap().remove(key);
        Ok(())
    }
}

/// One recorded call: method name, arguments, and the auth-relevant settings
/// (token, origin) visible at call time.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub method: &'static str,
    pub args: Vec<String>,
    pub token: Value,
    pub origin: Value,
}

/// Scriptable RomM API. Responses default to a healthy 4.9.0 server.
pub struct FakeRomm {
    settings: SettingsStore,
    pub calls: Mutex<Vec<Call>>,
    pub versions: Mutex<Vec<Option<String>>>,
    pub heartbeat: Mutex<Result<Value, RommError>>,
    pub heartbeat_once: Mutex<Result<Value, RommError>>,
    pub list_platforms: Mutex<Result<Value, RommError>>,
    pub current_user: Mutex<Result<Value, RommError>>,
    pub mint: Mutex<Result<Value, RommError>>,
    pub delete: Mutex<Result<(), RommError>>,
    pub exchange: Mutex<Result<Value, RommError>>,
}

impl FakeRomm {
    pub fn new(settings: SettingsStore) -> Self {
        Self {
            settings,
            calls: Mutex::new(Vec::new()),
            versions: Mutex::new(Vec::new()),
            heartbeat: Mutex::new(Ok(json!({"SYSTEM": {"VERSION": "4.9.0"}}))),
            heartbeat_once: Mutex::new(Ok(json!({"SYSTEM": {"VERSION": "4.9.0"}}))),
            list_platforms: Mutex::new(Ok(json!([{"id": 1, "slug": "n64"}]))),
            current_user: Mutex::new(Ok(json!({"id": 1, "username": "tester"}))),
            mint: Mutex::new(Ok(json!({"id": 42, "raw_token": "rmm_minted"}))),
            delete: Mutex::new(Ok(())),
            exchange: Mutex::new(Ok(json!({"id": 99, "raw_token": "rmm_paired"}))),
        }
    }

    fn record(&self, method: &'static str, args: &[&str]) {
        self.calls.lock().unwrap().push(Call {
            method,
            args: args.iter().map(|s| s.to_string()).collect(),
            token: self.settings.get("romm_api_token").unwrap_or(Value::Null),
            origin: self.settings.get("romm_api_token_origin").unwrap_or(Value::Null),
        });
    }

    pub fn calls_to(&self, method: &str) -> Vec<Call> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.method == method)
            .cloned()
            .collect()
    }

    pub fn set<T>(slot: &Mutex<T>, value: T) {
        *slot.lock().unwrap() = value;
    }
}

#[async_trait]
impl RommConnectionApi for FakeRomm {
    fn set_version(&self, version: Option<String>) {
        self.versions.lock().unwrap().push(version);
    }
    async fn heartbeat(&self) -> Result<Value, RommError> {
        self.record("heartbeat", &[]);
        self.heartbeat.lock().unwrap().clone()
    }
    async fn heartbeat_once(&self) -> Result<Value, RommError> {
        self.record("heartbeat_once", &[]);
        self.heartbeat_once.lock().unwrap().clone()
    }
    async fn list_platforms(&self) -> Result<Value, RommError> {
        self.record("list_platforms", &[]);
        self.list_platforms.lock().unwrap().clone()
    }
    async fn get_current_user(&self) -> Result<Value, RommError> {
        self.record("get_current_user", &[]);
        self.current_user.lock().unwrap().clone()
    }
    async fn mint_client_token(&self, username: &str, password: &str, token_name: &str) -> Result<Value, RommError> {
        self.record("mint_client_token", &[username, password, token_name]);
        self.mint.lock().unwrap().clone()
    }
    async fn delete_client_token(&self, username: &str, password: &str, token_id: &str) -> Result<(), RommError> {
        self.record("delete_client_token", &[username, password, token_id]);
        self.delete.lock().unwrap().clone()
    }
    async fn exchange_pairing_code(&self, code: &str) -> Result<Value, RommError> {
        self.record("exchange_pairing_code", &[code]);
        self.exchange.lock().unwrap().clone()
    }
}
