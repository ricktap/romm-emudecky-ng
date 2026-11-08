//! RomM endpoints (RomM >= 4.9.0) over [`RommHttp`].

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Method;
use serde_json::{Value, json};

use crate::adapters::romm::http::RommHttp;
use crate::domain::errors::RommError;
use crate::services::ports::RommConnectionApi;

/// Timeout of the single-shot reachability probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const PAIRING_CODE_ENDPOINT: &str = "/api/client-tokens/exchange";
/// Case-folded `detail` needle of the exchange 404 for a deleted token (RomM
/// `client_tokens.py`), distinct from the invalid/expired-code 404.
const PAIRING_TOKEN_GONE_NEEDLE: &str = "token no longer exists";

/// Scopes of the minted token. `me.write` is deliberately absent: the token
/// cannot mint or delete tokens itself.
pub const TOKEN_SCOPES: [&str; 11] = [
    "me.read",
    "platforms.read",
    "roms.read",
    "roms.user.read",
    "collections.read",
    "firmware.read",
    "assets.read",
    "devices.read",
    "assets.write",
    "devices.write",
    "roms.user.write",
];

pub struct RommApi {
    http: Arc<RommHttp>,
    version: Mutex<Option<String>>,
}

impl RommApi {
    pub fn new(http: Arc<RommHttp>) -> Self {
        Self {
            http,
            version: Mutex::new(None),
        }
    }

    /// The server version detected by the last heartbeat probe.
    pub fn version(&self) -> Option<String> {
        self.version.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

#[async_trait]
impl RommConnectionApi for RommApi {
    fn set_version(&self, version: Option<String>) {
        *self.version.lock().unwrap_or_else(|p| p.into_inner()) = version;
    }

    async fn heartbeat(&self) -> Result<Value, RommError> {
        self.http.get_json("/api/heartbeat").await
    }

    async fn heartbeat_once(&self) -> Result<Value, RommError> {
        self.http.get_json_once("/api/heartbeat", PROBE_TIMEOUT).await
    }

    async fn list_platforms(&self) -> Result<Value, RommError> {
        self.http.get_json("/api/platforms").await
    }

    async fn get_current_user(&self) -> Result<Value, RommError> {
        self.http.get_json("/api/users/me").await
    }

    async fn mint_client_token(&self, username: &str, password: &str, token_name: &str) -> Result<Value, RommError> {
        let body = json!({"name": token_name, "scopes": TOKEN_SCOPES, "expires_in": "never"});
        self.http
            .basic_auth_request("/api/client-tokens", username, password, Method::POST, Some(&body))
            .await
    }

    async fn delete_client_token(&self, username: &str, password: &str, token_id: &str) -> Result<(), RommError> {
        let path = format!("/api/client-tokens/{token_id}");
        match self
            .http
            .basic_auth_request(&path, username, password, Method::DELETE, None)
            .await
        {
            Ok(_) | Err(RommError::NotFound { .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn exchange_pairing_code(&self, code: &str) -> Result<Value, RommError> {
        match self
            .http
            .unauthenticated_post_json(PAIRING_CODE_ENDPOINT, &json!({"code": code}))
            .await
        {
            Ok(v) => Ok(v),
            Err(RommError::NotFound { detail, .. }) => {
                if detail.is_some_and(|d| d.to_lowercase().contains(PAIRING_TOKEN_GONE_NEEDLE)) {
                    Err(RommError::PairingTokenGone)
                } else {
                    Err(RommError::PairingCodeInvalid)
                }
            }
            Err(RommError::Forbidden(_)) => Err(RommError::PairingOwnerDisabled),
            Err(RommError::Server { status: 429, .. }) => Err(RommError::PairingRateLimited),
            Err(e) => Err(e),
        }
    }
}
