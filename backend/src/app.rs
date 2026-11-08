//! Composition root: the only place concrete adapters meet services, plus the
//! startup work (database migrations, settings load/upgrade, legacy credential
//! migration) and the callable table the protocol server serves.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use serde_json::{Value, json};

use crate::adapters::db::{DB_FILENAME, Db};
use crate::adapters::persistence::{EpochClock, FileSettingsPersister, SettingsFile};
use crate::adapters::plugin_metadata::read_plugin_version;
use crate::adapters::repositories::{SqliteKvConfig, SqliteRomRegions};
use crate::adapters::retroarch::RetroArchConfigFiles;
use crate::adapters::romm::api::RommApi;
use crate::adapters::romm::http::{RetryListener, RetryPolicy, RommHttp};
use crate::domain::settings::{RESET_NOTICE_KEY, fold_legacy_save_sync_settings, migrate_settings, stored_version};
use crate::domain::version::MIN_REQUIRED_VERSION;
use crate::rpc::server::{EventHub, Handler, Registry};
use crate::services::connection::{ConnectionService, ConnectionServiceConfig};
use crate::services::ports::{SettingsPersister, SettingsStore};
use crate::services::settings::{SettingsService, SettingsServiceConfig};

/// Every callable name the frontend may call — the same surface as the Python
/// `Backend` (kept in sync by `tests/test_rust_backend_callables.py`).
const CALLABLE_NAMES_JSON: &str = include_str!("../callables.json");

/// Callables this backend implements; every other listed name answers
/// "not implemented in the Rust backend yet".
pub const PORTED_CALLABLES: &[&str] = &[
    "connect_with_credentials",
    "connect_with_pairing_code",
    "connect_with_token",
    "debug_log",
    "dismiss_settings_reset_notice",
    "fix_retroarch_input_driver",
    "frontend_log",
    "get_known_regions",
    "get_settings",
    "get_settings_reset_notice",
    "get_whitelist_settings",
    "probe_reachability",
    "save_collection_platform_groups",
    "save_log_level",
    "save_preferred_region",
    "save_server_url",
    "save_steam_input_setting",
    "sign_out",
    "test_connection",
    "update_whitelist_settings",
];

/// The full callable surface, sorted.
pub fn callable_names() -> Vec<String> {
    let mut names: Vec<String> =
        serde_json::from_str(CALLABLE_NAMES_JSON).expect("callables.json is a JSON string list");
    names.sort();
    names
}

/// Where the backend runs and how it reaches its host.
pub struct BackendEnv {
    pub settings_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub plugin_dir: PathBuf,
    pub user_home: PathBuf,
    pub retry: RetryPolicy,
    pub clock: EpochClock,
}

/// The wired services.
pub struct Backend {
    pub settings: SettingsStore,
    pub settings_service: Arc<SettingsService>,
    pub connection_service: Arc<ConnectionService>,
}

impl Backend {
    /// Wire adapters and services and run the startup work. Fails (and the
    /// plugin stays inert) when the database cannot be opened or migrated or
    /// the settings file cannot be read or written.
    pub async fn start(env: BackendEnv, hub: Arc<EventHub>) -> anyhow::Result<Self> {
        let (db, version) = Db::open(&env.runtime_dir.join(DB_FILENAME))
            .await
            .context("SQLite schema migration failed; plugin cannot start")?;
        tracing::info!("SQLite schema at version {version}");

        let file = Arc::new(SettingsFile::new(
            &env.settings_dir,
            &env.runtime_dir,
            env.clock.clone(),
        ));
        let settings = load_and_upgrade_settings(file.clone()).await?;
        let persister: Arc<dyn SettingsPersister> = Arc::new(FileSettingsPersister::new(file, settings.clone()));
        persister.save_settings().await.context("writing settings.json")?;

        let user_agent = format!("romm-emudecky-ng/{}", read_plugin_version(&env.plugin_dir).await);
        let retry_hub = hub.clone();
        let on_retry: RetryListener = Arc::new(move |attempt, max_attempts, delay_s| {
            retry_hub.emit(
                "server_retry_progress",
                vec![json!({"attempt": attempt, "max_attempts": max_attempts, "delay_s": delay_s})],
            );
        });
        let http = Arc::new(RommHttp::new(settings.clone(), user_agent, env.retry, Some(on_retry))?);
        let romm_api = Arc::new(RommApi::new(http));

        let settings_service = Arc::new(SettingsService::new(SettingsServiceConfig {
            settings: settings.clone(),
            persister: persister.clone(),
            regions: Arc::new(SqliteRomRegions::new(db.clone())),
            retroarch: Arc::new(RetroArchConfigFiles::new(&env.user_home)),
        }));
        let connection_service = Arc::new(ConnectionService::new(ConnectionServiceConfig {
            settings: settings.clone(),
            romm_api,
            persister,
            kv_config: Arc::new(SqliteKvConfig::new(db)),
            min_required_version: MIN_REQUIRED_VERSION,
        }));

        connection_service.migrate_legacy_credentials().await;
        Ok(Self {
            settings,
            settings_service,
            connection_service,
        })
    }

    /// The protocol registry: ported callables bound to their services, every
    /// other name bound to the "not implemented yet" error.
    pub fn registry(&self) -> Registry {
        let mut registry = Registry::new();
        for name in callable_names() {
            let handler = self.ported_handler(&name).unwrap_or_else(|| not_implemented(&name));
            registry.insert(name, handler);
        }
        registry
    }

    fn ported_handler(&self, name: &str) -> Option<Handler> {
        let s = self.settings_service.clone();
        let c = self.connection_service.clone();
        let h: Handler = match name {
            "test_connection" => bind(name, 0, 0, c, |c, _| async move { Ok(c.test_connection().await) }),
            "probe_reachability" => bind(name, 0, 0, c, |c, _| async move { Ok(c.probe_reachability().await) }),
            "connect_with_credentials" => bind(name, 3, 4, c, |c, a| async move {
                let url = opt_str(&a, 0, "romm_url")?;
                let user = req_str(&a, 1, "username")?;
                let pass = req_str(&a, 2, "password")?;
                Ok(c.establish_token(&url, &user, &pass, a.get(3)).await)
            }),
            "connect_with_token" => bind(name, 2, 3, c, |c, a| async move {
                let url = opt_str(&a, 0, "romm_url")?;
                let token = req_str(&a, 1, "token")?;
                Ok(c.establish_user_token(&url, &token, a.get(2)).await)
            }),
            "connect_with_pairing_code" => bind(name, 2, 3, c, |c, a| async move {
                let url = opt_str(&a, 0, "romm_url")?;
                let code = req_str(&a, 1, "code")?;
                Ok(c.establish_paired_token(&url, &code, a.get(2)).await)
            }),
            "sign_out" => bind(name, 0, 0, c, |c, _| async move { Ok(c.sign_out().await) }),
            "save_server_url" => bind(name, 1, 2, s, |s, a| async move {
                let url = req_str(&a, 0, "romm_url")?;
                Ok(s.save_server_url(&url, a.get(1)).await)
            }),
            "frontend_log" => bind(name, 2, 2, s, |s, a| async move {
                s.frontend_log(&loose_str(&a[0]), &loose_str(&a[1]));
                Ok(Value::Null)
            }),
            "debug_log" => bind(name, 1, 1, s, |s, a| async move {
                s.frontend_log("debug", &loose_str(&a[0]));
                Ok(Value::Null)
            }),
            "save_log_level" => bind(name, 1, 1, s, |s, a| async move {
                s.save_log_level(&a[0]).await.map_err(err)
            }),
            "save_steam_input_setting" => bind(name, 1, 1, s, |s, a| async move {
                s.save_steam_input_setting(&a[0]).await.map_err(err)
            }),
            "save_preferred_region" => bind(name, 1, 1, s, |s, a| async move {
                s.save_preferred_region(&a[0]).await.map_err(err)
            }),
            "get_known_regions" => bind(
                name,
                0,
                0,
                s,
                |s, _| async move { s.get_known_regions().await.map_err(err) },
            ),
            "fix_retroarch_input_driver" => {
                bind(
                    name,
                    0,
                    0,
                    s,
                    |s, _| async move { Ok(s.fix_retroarch_input_driver().await) },
                )
            }
            "get_settings" => bind(name, 0, 0, s, |s, _| async move { Ok(s.get_settings().await) }),
            "get_whitelist_settings" => bind(name, 0, 0, s, |s, _| async move { Ok(s.get_whitelist_settings()) }),
            "update_whitelist_settings" => bind(name, 2, 2, s, |s, a| async move {
                s.update_whitelist_settings(&a[0], &a[1]).await.map_err(err)
            }),
            "save_collection_platform_groups" => bind(name, 1, 1, s, |s, a| async move {
                s.save_collection_platform_groups(&a[0]).await.map_err(err)
            }),
            "get_settings_reset_notice" => bind(name, 0, 0, s, |s, _| async move { Ok(s.get_settings_reset_notice()) }),
            "dismiss_settings_reset_notice" => bind(name, 0, 0, s, |s, _| async move {
                s.dismiss_settings_reset_notice().await.map_err(err)
            }),
            _ => return None,
        };
        Some(h)
    }
}

/// Load `settings.json`, run the one-time legacy fold and the schema upgrade,
/// and record a corrupt-file reset as the persistent notice marker.
async fn load_and_upgrade_settings(file: Arc<SettingsFile>) -> anyhow::Result<SettingsStore> {
    tokio::task::spawn_blocking(move || {
        let loaded = file.load()?;
        let mut settings = loaded.settings;
        if stored_version(&settings) < 4 {
            settings = fold_legacy_save_sync_settings(&settings, file.load_legacy_save_sync_state().as_ref());
        }
        let mut settings = migrate_settings(&settings);
        if let Some(backup) = loaded.corrupt_backup {
            settings.insert(RESET_NOTICE_KEY.into(), json!({"backed_up_to": backup}));
        }
        anyhow::Ok(SettingsStore::new(settings))
    })
    .await?
}

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

fn not_implemented(name: &str) -> Handler {
    let message = format!("not implemented in the Rust backend yet: {name}");
    Arc::new(move |_| {
        let message = message.clone();
        Box::pin(async move { Err(message) })
    })
}

/// Bind a service method as a handler with an arity check of `min..=max`
/// positional arguments.
fn bind<S, F, Fut>(name: &str, min: usize, max: usize, service: Arc<S>, f: F) -> Handler
where
    S: Send + Sync + 'static,
    F: Fn(Arc<S>, Vec<Value>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, String>> + Send + 'static,
{
    let name = name.to_string();
    let f = Arc::new(f);
    Arc::new(move |args: Vec<Value>| {
        if args.len() < min || args.len() > max {
            let expected = if min == max {
                format!("{min}")
            } else {
                format!("{min} to {max}")
            };
            let message = format!(
                "{name}() takes {expected} positional arguments but {} were given",
                args.len()
            );
            return Box::pin(async move { Err(message) });
        }
        Box::pin(f(service.clone(), args))
    })
}

/// A required string argument.
fn req_str(args: &[Value], i: usize, name: &str) -> Result<String, String> {
    args.get(i)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("argument '{name}' must be a string"))
}

/// A string argument where `null` reads as empty (the "not configured" case).
fn opt_str(args: &[Value], i: usize, name: &str) -> Result<String, String> {
    match args.get(i) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(_) => req_str(args, i, name),
    }
}

/// Any JSON value as log text: strings verbatim, everything else as JSON.
fn loose_str(v: &Value) -> String {
    v.as_str().map_or_else(|| v.to_string(), str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_matches_python_backend_size_and_includes_ported() {
        let names = callable_names();
        assert_eq!(names.len(), 121);
        let mut dedup = names.clone();
        dedup.dedup();
        assert_eq!(dedup.len(), names.len());
        for ported in PORTED_CALLABLES {
            assert!(names.iter().any(|n| n == ported), "{ported} not in callables.json");
        }
        assert!(names.iter().all(|n| !n.starts_with('_')));
    }

    #[test]
    fn argument_helpers() {
        let args = vec![json!("x"), Value::Null, json!(5)];
        assert_eq!(req_str(&args, 0, "a").unwrap(), "x");
        assert!(req_str(&args, 1, "a").is_err());
        assert!(req_str(&args, 9, "a").is_err());
        assert_eq!(opt_str(&args, 1, "a").unwrap(), "");
        assert_eq!(opt_str(&args, 9, "a").unwrap(), "");
        assert!(opt_str(&args, 2, "a").is_err());
        assert_eq!(loose_str(&json!("hi")), "hi");
        assert_eq!(loose_str(&json!({"a": 1})), r#"{"a":1}"#);
    }
}
