//! ConnectionService — RomM reachability, the version gate, and the Client API
//! Token lifecycle (credential mint, pasted token, pairing code, sign-out, and
//! the startup upgrade of a stored-password install).
//!
//! Sign-in discipline: the candidate URL is held in memory only while probing,
//! with the old server's bearer cleared (or the new token bound to the
//! candidate origin) so a credential never reaches a host it was not issued
//! for. Nothing is persisted until the sign-in succeeds, and a failure rolls
//! the in-memory auth state back. A successful sign-in to a different origin
//! forgets the registered device id, which is bound to its minting server.

use std::sync::Arc;

use serde_json::{Value, json};
use tokio::sync::Mutex as AsyncMutex;

use crate::domain::errors::{RommError, failure, reason, unknown_failure};
use crate::domain::url_host::{is_valid_server_url, normalize_origin, same_origin};
use crate::domain::version::meets_min_version;
use crate::services::ports::{KvConfigRepo, RommConnectionApi, SettingsPersister, SettingsStore};

const NO_SERVER_URL_MESSAGE: &str = "No server URL configured";
const INVALID_URL_MESSAGE: &str = "Enter a valid http(s):// server URL";
const FORBIDDEN_TOKEN_MESSAGE: &str = "Your RomM account cannot create API tokens \u{2014} ask your admin to grant \
     token permissions or use an account with a higher role.";
const USER_TOKEN_INVALID_MESSAGE: &str =
    "The API token is invalid or has been revoked. Create a new token in RomM and try again.";
const USER_TOKEN_SCOPE_MESSAGE: &str =
    "The API token is missing required permissions (scopes). Grant the scopes listed in the plugin docs and try again.";
const ENTER_PAIRING_CODE_MESSAGE: &str = "Enter the pairing code from RomM";
const PAIRING_CODE_INVALID_MESSAGE: &str = "Pairing code is invalid or has expired \u{2014} generate a new one in RomM \
     and try again (codes are valid for 60 seconds).";
const PAIRING_TOKEN_GONE_MESSAGE: &str =
    "The token this pairing code was created for no longer exists in RomM. Create a new API token, then pair again.";
const PAIRING_OWNER_DISABLED_MESSAGE: &str =
    "This RomM account is disabled \u{2014} ask your administrator to re-enable it, then try again.";
const PAIRING_RATE_LIMITED_MESSAGE: &str = "Too many attempts \u{2014} wait a minute and generate a new code.";
const RATE_LIMITED_REASON: &str = "rate_limited";
const NO_USABLE_TOKEN_MESSAGE: &str = "RomM did not return a usable token";
const SIGNED_OUT_MESSAGE: &str = "Signed out. The token is still valid in RomM \u{2014} revoke it there \
     (Settings \u{2192} API Tokens) if you no longer want it.";

/// `kv_config` key of the registered server device id (bound to its origin).
const DEVICE_ID_KEY: &str = "device_id";
/// `kv_config` key of the "sign in again for cross-device playtime" notice.
const PLAYTIME_SCOPE_NOTICE_KEY: &str = "playtime_scope_notice";

const AUTH_STATE_KEYS: [&str; 8] = [
    "romm_url",
    "romm_url_external",
    "romm_api_token_origins",
    "romm_allow_insecure_ssl",
    "romm_api_token",
    "romm_api_token_id",
    "romm_api_token_origin",
    "romm_api_token_source",
];

/// Normalize a pairing code as RomM does: drop whitespace and `-`, uppercase.
fn normalize_pairing_code(code: &str) -> String {
    code.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_uppercase()
}

/// Everything [`ConnectionService`] depends on.
pub struct ConnectionServiceConfig {
    pub settings: SettingsStore,
    pub romm_api: Arc<dyn RommConnectionApi>,
    pub persister: Arc<dyn SettingsPersister>,
    pub kv_config: Arc<dyn KvConfigRepo>,
    pub min_required_version: &'static [u64],
}

pub struct ConnectionService {
    settings: SettingsStore,
    romm_api: Arc<dyn RommConnectionApi>,
    persister: Arc<dyn SettingsPersister>,
    kv_config: Arc<dyn KvConfigRepo>,
    min_required_version: &'static [u64],
    /// Serializes the flows that rewrite the auth state, so two concurrent
    /// sign-ins cannot interleave their snapshot/restore of the same keys.
    auth_lock: AsyncMutex<()>,
}

type Snapshot = Vec<(&'static str, Value)>;

/// The success-path token a sign-in obtained.
struct NewToken {
    raw: String,
    id: Value,
    source: &'static str,
}

impl ConnectionService {
    pub fn new(config: ConnectionServiceConfig) -> Self {
        Self {
            settings: config.settings,
            romm_api: config.romm_api,
            persister: config.persister,
            kv_config: config.kv_config,
            min_required_version: config.min_required_version,
            auth_lock: AsyncMutex::new(()),
        }
    }

    /// Probe the configured server: heartbeat (version), an authenticated
    /// platform list, then the version gate.
    pub async fn test_connection(&self) -> Value {
        if !self.settings.truthy("romm_url") {
            return failure(reason::CONFIG_ERROR, NO_SERVER_URL_MESSAGE);
        }
        if !self.settings.truthy("romm_api_token") {
            return failure(reason::CONFIG_ERROR, "Not signed in \u{2014} sign in to RomM first");
        }
        let version = match self.probe_version().await {
            Ok(v) => v,
            Err(e) => {
                self.romm_api.set_version(None);
                return e.to_failure();
            }
        };
        if let Err(e) = self.romm_api.list_platforms().await {
            let mut resp = e.to_failure();
            if resp["reason"] != reason::AUTH_FAILED {
                let msg = format!(
                    "Server reachable but API request failed: {}",
                    resp["message"].as_str().unwrap_or("")
                );
                resp["message"] = json!(msg);
            }
            return resp;
        }
        if let Some(err) = self.version_gate_error(version.as_deref()) {
            return err;
        }
        success_result(version.as_deref())
    }

    /// A single short heartbeat: `{online: bool}`. No version gate, no state.
    pub async fn probe_reachability(&self) -> Value {
        match self.romm_api.heartbeat_once().await {
            Ok(_) => json!({"online": true}),
            Err(e) => {
                tracing::debug!("probe_reachability heartbeat failed: {e}");
                json!({"online": false})
            }
        }
    }

    /// Mint a Client API Token from one-time credentials and store it.
    pub async fn establish_token(
        &self,
        romm_url: &str,
        username: &str,
        password: &str,
        allow_insecure_ssl: Option<&Value>,
    ) -> Value {
        let trimmed = match validate_url(romm_url) {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        let _guard = self.auth_lock.lock().await;
        let snapshot = self.snapshot_auth_state();
        let old_token_id = snapshot_value(&snapshot, "romm_api_token_id");
        let old_origins = snapshot_origins(&snapshot);
        let old_source = snapshot_value(&snapshot, "romm_api_token_source");

        self.hold_candidate(&trimmed, allow_insecure_ssl);
        self.clear_token_trio();

        let version = match self.probe_and_gate(&snapshot).await {
            Ok(v) => v,
            Err(resp) => return resp,
        };

        // Never DELETE a user-supplied token; only replay the DELETE against the
        // server the old token was minted on.
        if old_source.as_str() != Some("user") && !old_token_id.is_null() {
            if any_origin_matches(&old_origins, &trimmed) {
                let id = match &old_token_id {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                if let Err(e) = self.romm_api.delete_client_token(username, password, &id).await {
                    tracing::warn!("Failed to delete previous Client API Token: {e}");
                }
            } else {
                tracing::info!(
                    "Previous token was minted for a different/unknown server; \
                     skipping DELETE to avoid replaying it against the current server"
                );
            }
        }

        let minted = match self
            .romm_api
            .mint_client_token(username, password, &self.token_name())
            .await
        {
            Ok(m) => m,
            Err(RommError::Forbidden(_)) => {
                self.restore_auth_state(&snapshot);
                return failure(reason::AUTH_FAILED, FORBIDDEN_TOKEN_MESSAGE);
            }
            Err(e) => {
                self.restore_auth_state(&snapshot);
                return e.to_failure();
            }
        };
        let raw = minted
            .get("raw_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let id = minted.get("id").filter(|v| !v.is_null());
        let (Some(raw), Some(id)) = (raw, id) else {
            self.restore_auth_state(&snapshot);
            return failure(reason::SERVER_UNREACHABLE, NO_USABLE_TOKEN_MESSAGE);
        };
        let token = NewToken {
            raw: raw.to_string(),
            id: id.clone(),
            source: "minted",
        };
        if let Err(e) = self.persist_token(&token, normalize_origin(&trimmed)).await {
            self.restore_auth_state(&snapshot);
            return unknown_failure(&e);
        }
        self.after_sign_in(&old_origins, &trimmed).await;
        success_result(version.as_deref())
    }

    /// Store a token the user pasted (the OIDC path) after validating it.
    pub async fn establish_user_token(&self, romm_url: &str, token: &str, allow_insecure_ssl: Option<&Value>) -> Value {
        let trimmed = match validate_url(romm_url) {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        let trimmed_token = token.trim();
        if trimmed_token.is_empty() {
            return failure(reason::CONFIG_ERROR, "Enter your RomM API token");
        }
        let _guard = self.auth_lock.lock().await;
        let snapshot = self.snapshot_auth_state();
        let old_origins = snapshot_origins(&snapshot);

        self.hold_candidate(&trimmed, allow_insecure_ssl);
        self.bind_candidate_token(trimmed_token, &trimmed);

        let version = match self.probe_and_gate(&snapshot).await {
            Ok(v) => v,
            Err(resp) => return resp,
        };
        self.validate_and_persist_user_token(trimmed_token, &trimmed, &old_origins, version, &snapshot)
            .await
    }

    /// Exchange a one-time RomM pairing code for a token and store it.
    pub async fn establish_paired_token(
        &self,
        romm_url: &str,
        code: &str,
        allow_insecure_ssl: Option<&Value>,
    ) -> Value {
        let trimmed = match validate_url(romm_url) {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        let normalized = normalize_pairing_code(code);
        if normalized.is_empty() {
            return failure(reason::CONFIG_ERROR, ENTER_PAIRING_CODE_MESSAGE);
        }
        let _guard = self.auth_lock.lock().await;
        let snapshot = self.snapshot_auth_state();
        let old_origins = snapshot_origins(&snapshot);

        self.hold_candidate(&trimmed, allow_insecure_ssl);
        self.clear_token_trio();

        let version = match self.probe_and_gate(&snapshot).await {
            Ok(v) => v,
            Err(resp) => return resp,
        };

        let exchanged = match self.romm_api.exchange_pairing_code(&normalized).await {
            Ok(v) => v,
            Err(e) => {
                self.restore_auth_state(&snapshot);
                return match e {
                    RommError::PairingCodeInvalid => failure(reason::AUTH_FAILED, PAIRING_CODE_INVALID_MESSAGE),
                    RommError::PairingTokenGone => failure(reason::AUTH_FAILED, PAIRING_TOKEN_GONE_MESSAGE),
                    RommError::PairingOwnerDisabled => failure(reason::AUTH_FAILED, PAIRING_OWNER_DISABLED_MESSAGE),
                    RommError::PairingRateLimited => failure(RATE_LIMITED_REASON, PAIRING_RATE_LIMITED_MESSAGE),
                    other => other.to_failure(),
                };
            }
        };
        let Some(raw) = exchanged
            .get("raw_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            self.restore_auth_state(&snapshot);
            return failure(reason::SERVER_UNREACHABLE, NO_USABLE_TOKEN_MESSAGE);
        };
        let raw = raw.to_string();
        self.bind_candidate_token(&raw, &trimmed);
        self.validate_and_persist_user_token(&raw, &trimmed, &old_origins, version, &snapshot)
            .await
    }

    /// Forget the stored token on this device (local only; one atomic save).
    /// URL and SSL flag are kept. A failed save restores the token.
    pub async fn sign_out(&self) -> Value {
        let _guard = self.auth_lock.lock().await;
        let snapshot = self.snapshot_auth_state();
        self.clear_token_trio();
        self.settings.set("romm_api_token_source", Value::Null);
        if let Err(e) = self.persister.save_settings().await {
            self.restore_auth_state(&snapshot);
            return unknown_failure(&e);
        }
        self.romm_api.set_version(None);
        json!({"success": true, "message": SIGNED_OUT_MESSAGE})
    }

    /// Upgrade a stored-password install to a token on startup. Never fails:
    /// any error leaves the credentials in place and the plugin inert.
    pub async fn migrate_legacy_credentials(&self) {
        if self.settings.truthy("romm_api_token") {
            return;
        }
        let (Some(username), Some(password)) = (self.settings.get_str("romm_user"), self.settings.get_str("romm_pass"))
        else {
            return;
        };
        if username.is_empty() || password.is_empty() {
            return;
        }
        let _guard = self.auth_lock.lock().await;
        let minted = match self
            .romm_api
            .mint_client_token(&username, &password, &self.token_name())
            .await
        {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("Legacy credential migration failed: {e}");
                return;
            }
        };
        let raw = minted
            .get("raw_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let id = minted.get("id").filter(|v| !v.is_null());
        let (Some(raw), Some(id)) = (raw, id) else {
            tracing::warn!("Legacy credential migration failed: {NO_USABLE_TOKEN_MESSAGE}");
            return;
        };
        let url = self.settings.get_str("romm_url").unwrap_or_default();
        let token = NewToken {
            raw: raw.to_string(),
            id: id.clone(),
            source: "minted",
        };
        if let Err(e) = self.persist_token(&token, normalize_origin(&url)).await {
            tracing::warn!("Legacy credential migration failed: {e:#}");
            return;
        }
        tracing::info!("Migrated legacy credentials to a Client API Token");
    }

    // ── internals ────────────────────────────────────────────────────────

    async fn validate_and_persist_user_token(
        &self,
        raw: &str,
        trimmed: &str,
        old_origins: &[String],
        version: Option<String>,
        snapshot: &Snapshot,
    ) -> Value {
        if let Err(e) = self.romm_api.get_current_user().await {
            self.restore_auth_state(snapshot);
            return match e {
                RommError::Auth(_) => failure(reason::AUTH_FAILED, USER_TOKEN_INVALID_MESSAGE),
                RommError::Forbidden(_) => failure(reason::AUTH_FAILED, USER_TOKEN_SCOPE_MESSAGE),
                other => other.to_failure(),
            };
        }
        let token = NewToken {
            raw: raw.to_string(),
            id: Value::Null,
            source: "user",
        };
        if let Err(e) = self.persist_token(&token, normalize_origin(trimmed)).await {
            self.restore_auth_state(snapshot);
            return unknown_failure(&e);
        }
        self.after_sign_in(old_origins, trimmed).await;
        success_result(version.as_deref())
    }

    /// Heartbeat + version gate for a sign-in; restores `snapshot` on failure.
    async fn probe_and_gate(&self, snapshot: &Snapshot) -> Result<Option<String>, Value> {
        let version = match self.probe_version().await {
            Ok(v) => v,
            Err(e) => {
                self.restore_auth_state(snapshot);
                self.romm_api.set_version(None);
                return Err(e.to_failure());
            }
        };
        if let Some(err) = self.version_gate_error(version.as_deref()) {
            self.restore_auth_state(snapshot);
            return Err(err);
        }
        Ok(version)
    }

    /// Heartbeat, cache the server version, return it. `SYSTEM.VERSION` is
    /// server-controlled: anything but a string becomes `None`.
    async fn probe_version(&self) -> Result<Option<String>, RommError> {
        let heartbeat = self.romm_api.heartbeat().await?;
        let version = heartbeat
            .get("SYSTEM")
            .and_then(|s| s.get("VERSION"))
            .and_then(Value::as_str)
            .map(str::to_string);
        self.romm_api.set_version(version.clone());
        if let Some(v) = version.as_deref().filter(|v| !v.is_empty()) {
            tracing::info!("RomM server version: {v}");
        }
        Ok(version)
    }

    /// `version_error` when `version` is below the floor; `development` builds
    /// and an absent version pass.
    fn version_gate_error(&self, version: Option<&str>) -> Option<Value> {
        let version = version.filter(|v| !v.is_empty() && *v != "development")?;
        if meets_min_version(version, self.min_required_version) {
            return None;
        }
        let min = self
            .min_required_version
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(".");
        Some(json!({
            "success": false,
            "reason": reason::VERSION_ERROR,
            "message": format!(
                "This plugin requires RomM {min} or newer. Your server is running {version}. \
                 Please update your RomM server to continue using this plugin."
            ),
            "romm_version": version,
        }))
    }

    fn snapshot_auth_state(&self) -> Snapshot {
        AUTH_STATE_KEYS
            .iter()
            .map(|k| (*k, self.settings.get(k).unwrap_or(Value::Null)))
            .collect()
    }

    /// Restore the in-memory auth state; disk is untouched.
    fn restore_auth_state(&self, snapshot: &Snapshot) {
        self.settings.with(|m| {
            for (k, v) in snapshot {
                m.insert((*k).to_string(), v.clone());
            }
        });
    }

    fn hold_candidate(&self, trimmed: &str, allow_insecure_ssl: Option<&Value>) {
        self.settings.set("romm_url", json!(trimmed));
        if let Some(flag) = allow_insecure_ssl.filter(|v| !v.is_null()) {
            self.settings
                .set("romm_allow_insecure_ssl", json!(crate::domain::settings::truthy(flag)));
        }
    }

    fn clear_token_trio(&self) {
        for key in ["romm_api_token", "romm_api_token_id", "romm_api_token_origin", "romm_api_token_origins"] {
            self.settings.set(key, Value::Null);
        }
    }

    /// Hold `token` in memory, host-bound to the candidate URL's origin, so the
    /// auth-header guard attaches it (and no older bearer) to the validation probe.
    fn bind_candidate_token(&self, token: &str, trimmed: &str) {
        self.settings.set("romm_api_token", json!(token));
        self.settings.set("romm_api_token_id", Value::Null);
        self.settings
            .set("romm_api_token_origin", json!(normalize_origin(trimmed)));
        self.settings
            .set("romm_api_token_origins", json!(self.bound_origins(normalize_origin(trimmed))));
        self.settings.set("romm_api_token_source", json!("user"));
    }

    /// Store the token, retire legacy credentials, and save once.
    async fn persist_token(&self, token: &NewToken, origin: Option<String>) -> anyhow::Result<()> {
        let external = self.settings.get_str("romm_url_external").unwrap_or_default();
        self.settings.with(|m| {
            m.insert("romm_api_token".into(), json!(token.raw));
            m.insert("romm_api_token_id".into(), token.id.clone());
            m.insert("romm_api_token_origins".into(), json!(bound_origins(&external, origin.clone())));
            m.insert("romm_api_token_origin".into(), json!(origin));
            m.insert("romm_api_token_source".into(), json!(token.source));
            m.shift_remove("romm_user");
            m.shift_remove("romm_pass");
        });
        self.persister.save_settings().await
    }

    /// Success-path side effects, both best-effort: forget the device id on an
    /// origin change, and clear the playtime re-sign-in notice.
    async fn after_sign_in(&self, old_origins: &[String], new_url: &str) {
        // The device id belongs to the server, not to one address: forget it only when none of the origins the
        // old token was bound to (internal or external) is the server we just signed in to.
        let new_external = self.settings.get_str("romm_url_external").unwrap_or_default();
        if !any_origin_matches(old_origins, new_url)
            && !(!new_external.is_empty() && any_origin_matches(old_origins, &new_external))
            && let Err(e) = self.kv_config.delete(DEVICE_ID_KEY).await
        {
            tracing::warn!("Could not clear device id after origin change: {e:#}");
        }
        let cleared = async {
            if self.kv_config.get(PLAYTIME_SCOPE_NOTICE_KEY).await?.is_some() {
                self.kv_config.delete(PLAYTIME_SCOPE_NOTICE_KEY).await?;
            }
            anyhow::Ok(())
        };
        if let Err(e) = cleared.await {
            tracing::warn!("Could not clear playtime scope notice after sign-in: {e:#}");
        }
    }

    /// The origins a token minted now is bound to: the sign-in URL's and the external URL's (D6).
    fn bound_origins(&self, primary: Option<String>) -> Vec<String> {
        bound_origins(&self.settings.get_str("romm_url_external").unwrap_or_default(), primary)
    }

    fn token_name(&self) -> String {
        let device = self
            .settings
            .get("device_name")
            .filter(crate::domain::settings::truthy)
            .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string))
            .unwrap_or_else(|| "Steam Deck".into());
        format!("romm-emudecky-ng ({device})")
    }
}

fn validate_url(romm_url: &str) -> Result<String, Value> {
    if romm_url.is_empty() {
        return Err(failure(reason::CONFIG_ERROR, NO_SERVER_URL_MESSAGE));
    }
    let trimmed = romm_url.trim();
    if !is_valid_server_url(trimmed) {
        return Err(failure(reason::CONFIG_ERROR, INVALID_URL_MESSAGE));
    }
    Ok(trimmed.to_string())
}

/// `[primary, external]` origins, deduplicated, without blanks.
pub fn bound_origins(external_url: &str, primary: Option<String>) -> Vec<String> {
    let mut out: Vec<String> = primary.into_iter().collect();
    if let Some(ext) = normalize_origin(external_url.trim())
        && !out.contains(&ext)
    {
        out.push(ext);
    }
    out
}

/// The origins the snapshotted token was bound to: the list, or the single legacy origin.
fn snapshot_origins(snapshot: &Snapshot) -> Vec<String> {
    if let Value::Array(list) = snapshot_value(snapshot, "romm_api_token_origins") {
        let found: Vec<String> = list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
        if !found.is_empty() {
            return found;
        }
    }
    snapshot_value(snapshot, "romm_api_token_origin").as_str().map(str::to_string).into_iter().collect()
}

fn any_origin_matches(origins: &[String], url: &str) -> bool {
    origins.iter().any(|o| same_origin(Some(o), Some(url)))
}

fn snapshot_value(snapshot: &Snapshot, key: &str) -> Value {
    snapshot
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.clone())
        .unwrap_or(Value::Null)
}

fn success_result(version: Option<&str>) -> Value {
    match version.filter(|v| !v.is_empty()) {
        Some("development") => json!({"success": true, "message": "Connected to RomM", "romm_version": "development"}),
        Some(v) => json!({"success": true, "message": format!("Connected to RomM {v}"), "romm_version": v}),
        None => json!({"success": true, "message": "Connected to RomM"}),
    }
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
