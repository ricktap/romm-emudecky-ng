//! The seams services depend on: the live settings document and the traits
//! adapters implement. Services never name a concrete adapter.

use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::domain::errors::RommError;
use crate::domain::settings::truthy;

/// The live, shared `settings.json` document.
///
/// Every service reads and mutates the same map (as the Python services shared
/// one dict); the persister writes snapshots of it. The lock is a plain mutex
/// and is never held across an `.await`.
#[derive(Clone, Default)]
pub struct SettingsStore(Arc<Mutex<Map<String, Value>>>);

impl SettingsStore {
    pub fn new(map: Map<String, Value>) -> Self {
        Self(Arc::new(Mutex::new(map)))
    }

    fn lock(&self) -> MutexGuard<'_, Map<String, Value>> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The value at `key` (`None` when absent).
    pub fn get(&self, key: &str) -> Option<Value> {
        self.lock().get(key).cloned()
    }

    /// The value at `key` when it is a string.
    pub fn get_str(&self, key: &str) -> Option<String> {
        self.lock().get(key).and_then(Value::as_str).map(str::to_string)
    }

    /// Python-style truthiness of `key` (`false` when absent).
    pub fn truthy(&self, key: &str) -> bool {
        self.lock().get(key).is_some_and(truthy)
    }

    pub fn set(&self, key: &str, value: Value) {
        self.lock().insert(key.to_string(), value);
    }

    pub fn remove(&self, key: &str) -> Option<Value> {
        self.lock().shift_remove(key)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.lock().contains_key(key)
    }

    /// Run `f` with exclusive access to the whole document.
    pub fn with<R>(&self, f: impl FnOnce(&mut Map<String, Value>) -> R) -> R {
        f(&mut self.lock())
    }

    /// A copy of the whole document.
    pub fn snapshot(&self) -> Map<String, Value> {
        self.lock().clone()
    }
}

/// Persists the live settings document to disk.
#[async_trait]
pub trait SettingsPersister: Send + Sync {
    async fn save_settings(&self) -> anyhow::Result<()>;
}

/// The RomM endpoints the connection flows use.
#[async_trait]
pub trait RommConnectionApi: Send + Sync {
    /// Cache the detected server version (`None` clears it).
    fn set_version(&self, version: Option<String>);
    /// `GET /api/heartbeat`, retried on transient failures.
    async fn heartbeat(&self) -> Result<Value, RommError>;
    /// `GET /api/heartbeat` once, with a short timeout.
    async fn heartbeat_once(&self) -> Result<Value, RommError>;
    async fn list_platforms(&self) -> Result<Value, RommError>;
    /// `GET /api/users/me` — validates the bearer token.
    async fn get_current_user(&self) -> Result<Value, RommError>;
    /// Mint a scoped Client API Token via one-off Basic auth.
    async fn mint_client_token(&self, username: &str, password: &str, token_name: &str) -> Result<Value, RommError>;
    /// Delete a minted token via Basic auth (a 404 counts as success).
    async fn delete_client_token(&self, username: &str, password: &str, token_id: &str) -> Result<(), RommError>;
    /// Exchange a one-time pairing code for a token (public endpoint, never retried).
    async fn exchange_pairing_code(&self, code: &str) -> Result<Value, RommError>;
}

/// The `kv_config` singleton-scalar store.
#[async_trait]
pub trait KvConfigRepo: Send + Sync {
    async fn get(&self, key: &str) -> anyhow::Result<Option<String>>;
    async fn set(&self, key: &str, value: &str) -> anyhow::Result<()>;
    async fn delete(&self, key: &str) -> anyhow::Result<()>;
}

/// The `regions` list of every ROM row in the local library.
#[async_trait]
pub trait RomRegionsReader: Send + Sync {
    async fn all_rom_regions(&self) -> anyhow::Result<Vec<Vec<String>>>;
}

/// RetroArch's `input_driver` setting (a bad `x` value breaks controllers).
#[async_trait]
pub trait RetroArchInputConfig: Send + Sync {
    /// `{warning, current, config_path}` for the first config that sets
    /// `input_driver`, or `None` when no config sets it.
    async fn check_input_driver(&self) -> Option<Value>;
    /// Rewrite a bad `input_driver` to `sdl2`; returns the frontend result dict.
    async fn fix_input_driver(&self) -> Value;
}
