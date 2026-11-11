//! SettingsService — user-facing settings reads/writes and frontend-log routing.
//!
//! Owns every callable that reads or mutates the live settings document from
//! the frontend. Disk writes go through the [`SettingsPersister`] port; minting
//! and storing the Client API Token is `ConnectionService`'s job, not this one.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::domain::errors::failure;
use crate::domain::settings::{RESET_NOTICE_KEY, truthy};
use crate::domain::url_host::is_valid_server_url;
use crate::services::ports::{RomRegionsReader, SettingsPersister, SettingsStore};

const MASK_PLACEHOLDER: &str = "\u{2022}\u{2022}\u{2022}\u{2022}";
const VALID_LOG_LEVELS: [&str; 4] = ["debug", "info", "warn", "error"];
const VALID_STEAM_INPUT_MODES: [&str; 3] = ["default", "force_on", "force_off"];
const AUTO_REGION: &str = "auto";

/// Everything [`SettingsService`] depends on.
pub struct SettingsServiceConfig {
    pub settings: SettingsStore,
    pub persister: Arc<dyn SettingsPersister>,
    pub regions: Arc<dyn RomRegionsReader>,
}

pub struct SettingsService {
    settings: SettingsStore,
    persister: Arc<dyn SettingsPersister>,
    regions: Arc<dyn RomRegionsReader>,
}

fn level_rank(level: &str) -> Option<u8> {
    VALID_LOG_LEVELS.iter().position(|l| *l == level).map(|p| p as u8)
}

impl SettingsService {
    pub fn new(config: SettingsServiceConfig) -> Self {
        Self {
            settings: config.settings,
            persister: config.persister,
            regions: config.regions,
        }
    }

    /// Persist the trimmed server URL and, when given, the insecure-SSL flag.
    ///
    /// A blank or non-http(s) URL is rejected without writing. The stored
    /// token's origin is deliberately not re-stamped: pointing the URL at a
    /// different origin leaves the token mismatched until the next sign-in.
    pub async fn save_server_url(&self, romm_url: &str, allow_insecure_ssl: Option<&Value>) -> Value {
        let trimmed = romm_url.trim();
        if !is_valid_server_url(trimmed) {
            return failure("config_error", "Enter a valid http(s):// server URL");
        }
        self.settings.set("romm_url", json!(trimmed));
        if let Some(flag) = allow_insecure_ssl.filter(|v| !v.is_null()) {
            self.settings.set("romm_allow_insecure_ssl", json!(truthy(flag)));
        }
        match self.persister.save_settings().await {
            Ok(()) => json!({"success": true, "message": "Settings saved"}),
            Err(err) => {
                tracing::error!("Failed to save settings: {err:#}");
                failure("save_failed", &format!("Save failed: {err}"))
            }
        }
    }

    /// Persist the optional external RomM URL (used when the internal one cannot be reached); blank clears it.
    ///
    /// The user typing it is what binds the stored token to that origin too (D6): the bearer may then go to the
    /// internal or the external origin and to nothing else. Clearing it takes the origin out again.
    pub async fn save_external_url(&self, url: &str) -> Value {
        let trimmed = url.trim();
        if !trimmed.is_empty() && !is_valid_server_url(trimmed) {
            return failure("config_error", "Enter a valid http(s):// server URL");
        }
        self.settings.set("romm_url_external", json!(trimmed));
        if let Some(primary) = self.settings.get_str("romm_api_token_origin") {
            let origins = crate::services::connection::bound_origins(trimmed, Some(primary));
            self.settings.set("romm_api_token_origins", json!(origins));
        }
        match self.persister.save_settings().await {
            Ok(()) => json!({"success": true, "message": "Settings saved"}),
            Err(err) => {
                tracing::error!("Failed to save settings: {err:#}");
                failure("save_failed", &format!("Save failed: {err}"))
            }
        }
    }

    /// The read-shape settings for the frontend. The token itself is never
    /// returned (only `has_token`); the SteamGridDB key is masked.
    pub async fn get_settings(&self) -> Value {
        let s = self.settings.snapshot();
        let get = |key: &str, default: Value| s.get(key).cloned().unwrap_or(default);
        json!({
            "romm_url": get("romm_url", json!("")),
            "romm_url_external": get("romm_url_external", json!("")),
            "has_token": s.get("romm_api_token").is_some_and(truthy),
            "steam_input_mode": get("steam_input_mode", json!("default")),
            "sgdb_api_key_masked": if s.get("steamgriddb_api_key").is_some_and(truthy) { MASK_PLACEHOLDER } else { "" },
            "log_level": get("log_level", json!("warn")),
            "romm_allow_insecure_ssl": get("romm_allow_insecure_ssl", json!(false)),
            "collection_create_platform_groups": get("collection_create_platform_groups", json!(false)),
            "preferred_region": get("preferred_region", json!(AUTO_REGION)),
            "platform_cores": get("platform_cores", json!({})),
        })
    }

    pub async fn save_log_level(&self, level: &Value) -> anyhow::Result<Value> {
        let Some(level) = level.as_str().filter(|l| VALID_LOG_LEVELS.contains(l)) else {
            return Ok(failure("invalid_log_level", "Invalid log level"));
        };
        self.settings.set("log_level", json!(level));
        self.persist_ok().await
    }

    /// Persist the preferred sibling-group region. Blank means `auto`; any
    /// string is accepted (the region vocabulary is open-ended).
    pub async fn save_preferred_region(&self, region: &Value) -> anyhow::Result<Value> {
        let Some(region) = region.as_str() else {
            return Ok(failure("invalid_region", "Invalid region"));
        };
        let trimmed = region.trim();
        let value = if trimmed.is_empty() { AUTO_REGION } else { trimmed };
        self.settings.set("preferred_region", json!(value));
        self.persist_ok().await
    }

    /// The distinct region values in the locally synced library, sorted.
    pub async fn get_known_regions(&self) -> anyhow::Result<Value> {
        let all: BTreeSet<String> = self.regions.all_rom_regions().await?.into_iter().flatten().collect();
        Ok(json!(all.into_iter().collect::<Vec<_>>()))
    }

    /// Log a frontend message when it meets the configured `log_level`.
    /// Unknown levels rank as `debug`.
    pub fn frontend_log(&self, level: &str, message: &str) {
        let configured = self.settings.get_str("log_level").unwrap_or_else(|| "warn".into());
        let threshold = level_rank(&configured).unwrap_or(2);
        if level_rank(level).unwrap_or(0) < threshold {
            return;
        }
        match level {
            "error" => tracing::error!("[FE] {message}"),
            "warn" => tracing::warn!("[FE] {message}"),
            _ => tracing::info!("[FE] {message}"),
        }
    }

    pub async fn save_steam_input_setting(&self, mode: &Value) -> anyhow::Result<Value> {
        let Some(mode_str) = mode.as_str().filter(|m| VALID_STEAM_INPUT_MODES.contains(m)) else {
            let shown = mode.as_str().map_or_else(|| mode.to_string(), str::to_string);
            return Ok(failure("invalid_mode", &format!("Invalid mode: {shown}")));
        };
        self.settings.set("steam_input_mode", json!(mode_str));
        self.persist_ok().await
    }

    pub fn get_whitelist_settings(&self) -> Value {
        json!({
            "disabled_defaults": self.settings.get("whitelist_disabled_defaults").unwrap_or_else(|| json!([])),
            "custom_names": self.settings.get("whitelist_custom_names").unwrap_or_else(|| json!([])),
        })
    }

    /// Persist the whitelist; both arguments must be lists of strings.
    pub async fn update_whitelist_settings(
        &self,
        disabled_defaults: &Value,
        custom_names: &Value,
    ) -> anyhow::Result<Value> {
        let is_str_list = |v: &Value| v.as_array().is_some_and(|a| a.iter().all(Value::is_string));
        if !is_str_list(disabled_defaults) {
            return Ok(failure(
                "invalid_whitelist",
                "disabled_defaults must be a list of strings",
            ));
        }
        if !is_str_list(custom_names) {
            return Ok(failure("invalid_whitelist", "custom_names must be a list of strings"));
        }
        self.settings
            .set("whitelist_disabled_defaults", disabled_defaults.clone());
        self.settings.set("whitelist_custom_names", custom_names.clone());
        self.persist_ok().await
    }

    pub async fn save_collection_platform_groups(&self, enabled: &Value) -> anyhow::Result<Value> {
        self.settings
            .set("collection_create_platform_groups", json!(truthy(enabled)));
        self.persist_ok().await
    }

    /// `{pending, backed_up_to}` for the corrupt-settings reset marker.
    /// Non-consuming: only [`Self::dismiss_settings_reset_notice`] clears it.
    pub fn get_settings_reset_notice(&self) -> Value {
        let notice = self.settings.get(RESET_NOTICE_KEY).filter(|v| !v.is_null());
        let backed_up_to = notice
            .as_ref()
            .and_then(|n| n.get("backed_up_to").cloned())
            .unwrap_or(Value::Null);
        json!({"pending": notice.is_some(), "backed_up_to": backed_up_to})
    }

    /// The user's explicit acknowledgement: drop the marker and persist.
    pub async fn dismiss_settings_reset_notice(&self) -> anyhow::Result<Value> {
        self.settings.remove(RESET_NOTICE_KEY);
        self.persister.save_settings().await?;
        Ok(json!({"success": true}))
    }

    /// Save and answer `{success: true}`; a disk failure propagates as an
    /// error frame, as the Python callables did.
    async fn persist_ok(&self) -> anyhow::Result<Value> {
        self.persister.save_settings().await?;
        Ok(json!({"success": true}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::testing::{FakePersister, FakeRegions};
    use serde_json::Map;

    struct Fixture {
        settings: SettingsStore,
        persister: Arc<FakePersister>,
        regions: Arc<FakeRegions>,
        service: SettingsService,
    }

    fn fixture() -> Fixture {
        let settings = SettingsStore::new(Map::new());
        let persister = Arc::new(FakePersister::default());
        let regions = Arc::new(FakeRegions::default());
        let service = SettingsService::new(SettingsServiceConfig {
            settings: settings.clone(),
            persister: persister.clone(),
            regions: regions.clone(),
        });
        Fixture {
            settings,
            persister,
            regions,
            service,
        }
    }

    #[tokio::test]
    async fn save_external_url_binds_and_unbinds_the_token_origin() {
        let f = fixture();
        f.settings.set("romm_api_token", json!("tok"));
        f.settings.set("romm_api_token_origin", json!("http://romm.local"));
        let r = f.service.save_external_url("  https://romm.example.com/  ").await;
        assert_eq!(r["success"], true);
        assert_eq!(f.settings.get("romm_url_external"), Some(json!("https://romm.example.com/")));
        assert_eq!(
            f.settings.get("romm_api_token_origins"),
            Some(json!(["http://romm.local", "https://romm.example.com"]))
        );
        assert_eq!(f.service.get_settings().await["romm_url_external"], "https://romm.example.com/");
        // Clearing it takes the origin out again.
        assert_eq!(f.service.save_external_url("").await["success"], true);
        assert_eq!(f.settings.get("romm_url_external"), Some(json!("")));
        assert_eq!(f.settings.get("romm_api_token_origins"), Some(json!(["http://romm.local"])));
        assert_eq!(f.persister.saves(), 2);
    }

    #[tokio::test]
    async fn save_external_url_rejects_a_bad_url_and_writes_nothing() {
        let f = fixture();
        let r = f.service.save_external_url("ftp://nope").await;
        assert_eq!(r["success"], false);
        assert_eq!(r["reason"], "config_error");
        assert!(!f.settings.contains("romm_url_external"));
        assert_eq!(f.persister.saves(), 0);
    }

    #[tokio::test]
    async fn save_server_url_persists_trimmed_and_ssl_flag() {
        let f = fixture();
        f.settings.set("romm_api_token", json!("tok"));
        let r = f.service.save_server_url("  http://romm.local  ", None).await;
        assert_eq!(r, json!({"success": true, "message": "Settings saved"}));
        assert_eq!(f.settings.get("romm_url"), Some(json!("http://romm.local")));
        assert_eq!(f.settings.get("romm_api_token"), Some(json!("tok")));
        assert!(!f.settings.contains("romm_allow_insecure_ssl"));
        assert_eq!(f.persister.saves(), 1);
        f.service.save_server_url("http://romm.local", Some(&json!(true))).await;
        assert_eq!(f.settings.get("romm_allow_insecure_ssl"), Some(json!(true)));
        f.service
            .save_server_url("http://romm.local", Some(&json!(false)))
            .await;
        assert_eq!(f.settings.get("romm_allow_insecure_ssl"), Some(json!(false)));
        f.service.save_server_url("http://romm.local", Some(&Value::Null)).await;
        assert_eq!(f.settings.get("romm_allow_insecure_ssl"), Some(json!(false)));
    }

    #[tokio::test]
    async fn save_server_url_rejects_invalid_without_writing() {
        for bad in ["", "   ", "romm.local", "ftp://romm.local", "https://"] {
            let f = fixture();
            let r = f.service.save_server_url(bad, None).await;
            assert_eq!(
                r,
                json!({"success": false, "reason": "config_error", "message": "Enter a valid http(s):// server URL"})
            );
            assert!(!f.settings.contains("romm_url"));
            assert_eq!(f.persister.saves(), 0);
        }
    }

    #[tokio::test]
    async fn save_server_url_persist_failure() {
        let f = fixture();
        f.persister.fail_with("disk full");
        let r = f.service.save_server_url("http://romm.local", None).await;
        assert_eq!(r["success"], false);
        assert_eq!(r["reason"], "save_failed");
        assert!(r["message"].as_str().unwrap().contains("disk full"));
    }

    #[tokio::test]
    async fn get_settings_happy_path_and_masking() {
        let f = fixture();
        for (k, v) in [
            ("romm_url", json!("http://romm.local")),
            ("romm_api_token", json!("rmm_secret")),
            ("steam_input_mode", json!("force_on")),
            ("steamgriddb_api_key", json!("longkey")),
            ("log_level", json!("info")),
            ("romm_allow_insecure_ssl", json!(true)),
            ("collection_create_platform_groups", json!(true)),
            ("preferred_region", json!("USA")),
            ("romm_user", json!("alice")),
            ("romm_pass", json!("pw")),
        ] {
            f.settings.set(k, v);
        }
        let r = f.service.get_settings().await;
        assert_eq!(
            r,
            json!({
                "romm_url": "http://romm.local",
                "romm_url_external": "",
                "has_token": true,
                "steam_input_mode": "force_on",
                "sgdb_api_key_masked": "\u{2022}\u{2022}\u{2022}\u{2022}",
                "log_level": "info",
                "romm_allow_insecure_ssl": true,
                "collection_create_platform_groups": true,
                "preferred_region": "USA",
                "platform_cores": {},
            })
        );
        let text = r.to_string();
        assert!(!text.contains("rmm_secret") && !text.contains("longkey") && !text.contains("alice"));
    }

    #[tokio::test]
    async fn get_settings_defaults_and_has_token() {
        let f = fixture();
        let r = f.service.get_settings().await;
        assert_eq!(r["romm_url"], "");
        assert_eq!(r["has_token"], false);
        assert_eq!(r["sgdb_api_key_masked"], "");
        assert_eq!(r["steam_input_mode"], "default");
        assert_eq!(r["log_level"], "warn");
        assert_eq!(r["romm_allow_insecure_ssl"], false);
        assert_eq!(r["collection_create_platform_groups"], false);
        assert_eq!(r["preferred_region"], "auto");
        for empty in [Value::Null, json!("")] {
            f.settings.set("romm_api_token", empty);
            assert_eq!(f.service.get_settings().await["has_token"], false);
        }
    }

    #[tokio::test]
    async fn save_log_level() {
        let f = fixture();
        for level in VALID_LOG_LEVELS {
            assert_eq!(
                f.service.save_log_level(&json!(level)).await.unwrap(),
                json!({"success": true})
            );
            assert_eq!(f.settings.get("log_level"), Some(json!(level)));
        }
        assert_eq!(f.persister.saves(), 4);
        for bad in [json!("verbose"), json!(""), json!(3)] {
            let r = f.service.save_log_level(&bad).await.unwrap();
            assert_eq!(
                r,
                json!({"success": false, "reason": "invalid_log_level", "message": "Invalid log level"})
            );
        }
        assert_eq!(f.settings.get("log_level"), Some(json!("error")));
        assert_eq!(f.persister.saves(), 4);
    }

    #[tokio::test]
    async fn save_preferred_region() {
        let f = fixture();
        for region in ["auto", "USA", "Europe", "Japan", "World"] {
            assert_eq!(
                f.service.save_preferred_region(&json!(region)).await.unwrap(),
                json!({"success": true})
            );
            assert_eq!(f.settings.get("preferred_region"), Some(json!(region)));
        }
        f.service.save_preferred_region(&json!("  Europe ")).await.unwrap();
        assert_eq!(f.settings.get("preferred_region"), Some(json!("Europe")));
        f.service.save_preferred_region(&json!("   ")).await.unwrap();
        assert_eq!(f.settings.get("preferred_region"), Some(json!("auto")));
        let saves = f.persister.saves();
        for bad in [json!(null), json!(5), json!(["USA"])] {
            let r = f.service.save_preferred_region(&bad).await.unwrap();
            assert_eq!(
                r,
                json!({"success": false, "reason": "invalid_region", "message": "Invalid region"})
            );
        }
        assert_eq!(f.persister.saves(), saves);
    }

    #[tokio::test]
    async fn known_regions_distinct_sorted() {
        let f = fixture();
        assert_eq!(f.service.get_known_regions().await.unwrap(), json!([]));
        f.regions.set(vec![
            vec!["USA".into(), "Europe".into()],
            vec![],
            vec!["Japan".into(), "USA".into()],
        ]);
        assert_eq!(
            f.service.get_known_regions().await.unwrap(),
            json!(["Europe", "Japan", "USA"])
        );
    }

    #[test]
    fn frontend_log_threshold() {
        let f = fixture();
        let ranks = |configured: Option<&str>, level: &str| {
            let threshold = configured.and_then(level_rank).unwrap_or(2);
            level_rank(level).unwrap_or(0) >= threshold
        };
        assert!(!ranks(Some("warn"), "info"));
        assert!(!ranks(Some("warn"), "debug"));
        assert!(ranks(Some("warn"), "warn"));
        assert!(ranks(Some("warn"), "error"));
        assert!(ranks(Some("debug"), "debug"));
        assert!(ranks(Some("debug"), "info"));
        assert!(ranks(Some("debug"), "bogus"));
        assert!(!ranks(None, "info"));
        // Smoke: the real method accepts every level without panicking.
        f.settings.set("log_level", json!("debug"));
        for level in ["debug", "info", "warn", "error", "bogus"] {
            f.service.frontend_log(level, "hello");
        }
    }

    #[tokio::test]
    async fn steam_input_setting() {
        let f = fixture();
        for mode in VALID_STEAM_INPUT_MODES {
            assert_eq!(
                f.service.save_steam_input_setting(&json!(mode)).await.unwrap(),
                json!({"success": true})
            );
            assert_eq!(f.settings.get("steam_input_mode"), Some(json!(mode)));
        }
        let r = f.service.save_steam_input_setting(&json!("turbo")).await.unwrap();
        assert_eq!(
            r,
            json!({"success": false, "reason": "invalid_mode", "message": "Invalid mode: turbo"})
        );
        let r = f.service.save_steam_input_setting(&json!("")).await.unwrap();
        assert_eq!(r["reason"], "invalid_mode");
        assert_eq!(f.persister.saves(), 3);
    }

    #[tokio::test]
    async fn whitelist() {
        let f = fixture();
        assert_eq!(
            f.service.get_whitelist_settings(),
            json!({"disabled_defaults": [], "custom_names": []})
        );
        let r = f
            .service
            .update_whitelist_settings(&json!(["Steam"]), &json!(["My Game"]))
            .await
            .unwrap();
        assert_eq!(r, json!({"success": true}));
        assert_eq!(
            f.service.get_whitelist_settings(),
            json!({"disabled_defaults": ["Steam"], "custom_names": ["My Game"]})
        );
        assert_eq!(
            f.service
                .update_whitelist_settings(&json!([]), &json!([]))
                .await
                .unwrap(),
            json!({"success": true})
        );
        assert_eq!(f.persister.saves(), 2);
        for (dd, cn, msg) in [
            (json!("x"), json!([]), "disabled_defaults must be a list of strings"),
            (json!([1]), json!([]), "disabled_defaults must be a list of strings"),
            (json!([]), json!({"a": 1}), "custom_names must be a list of strings"),
            (json!([]), json!(["ok", 2]), "custom_names must be a list of strings"),
        ] {
            let r = f.service.update_whitelist_settings(&dd, &cn).await.unwrap();
            assert_eq!(
                r,
                json!({"success": false, "reason": "invalid_whitelist", "message": msg})
            );
        }
        assert_eq!(f.persister.saves(), 2);
    }

    #[tokio::test]
    async fn collection_platform_groups_coerces() {
        let f = fixture();
        for (input, expected) in [
            (json!(true), true),
            (json!(false), false),
            (json!(1), true),
            (json!(""), false),
        ] {
            assert_eq!(
                f.service.save_collection_platform_groups(&input).await.unwrap(),
                json!({"success": true})
            );
            assert_eq!(
                f.settings.get("collection_create_platform_groups"),
                Some(json!(expected))
            );
        }
    }

    #[tokio::test]
    async fn reset_notice_read_and_dismiss() {
        let f = fixture();
        assert_eq!(
            f.service.get_settings_reset_notice(),
            json!({"pending": false, "backed_up_to": null})
        );
        f.settings
            .set(RESET_NOTICE_KEY, json!({"backed_up_to": "settings.json.corrupt-42"}));
        let first = f.service.get_settings_reset_notice();
        assert_eq!(
            first,
            json!({"pending": true, "backed_up_to": "settings.json.corrupt-42"})
        );
        assert_eq!(f.service.get_settings_reset_notice(), first, "non-consuming");
        assert_eq!(
            f.service.dismiss_settings_reset_notice().await.unwrap(),
            json!({"success": true})
        );
        assert!(!f.settings.contains(RESET_NOTICE_KEY));
        assert_eq!(f.persister.saves(), 1);
        // Idempotent: dismissing again still persists.
        f.service.dismiss_settings_reset_notice().await.unwrap();
        assert_eq!(f.persister.saves(), 2);
    }
}
