//! RomM endpoints (RomM >= 4.9.0) over [`RommHttp`].

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Method;
use serde_json::{Value, json};

use crate::adapters::romm::http::RommHttp;
use crate::domain::errors::RommError;
use crate::services::ports::{RommConnectionApi, RommLibraryApi, RommRomApi, TransferHooks, TransferMode, TransferOutcome};
use crate::util::path_safety::quote;

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

#[async_trait]
impl RommLibraryApi for RommApi {
    async fn list_platforms(&self) -> Result<Value, RommError> {
        self.http.get_json("/api/platforms").await
    }

    async fn list_collections(&self) -> Result<Value, RommError> {
        self.http.get_json("/api/collections").await.map(collection_list)
    }

    async fn list_smart_collections(&self) -> Result<Value, RommError> {
        self.http.get_json("/api/collections/smart").await.map(collection_list)
    }

    async fn list_virtual_collections(&self, collection_type: &str) -> Result<Value, RommError> {
        self.http
            .get_json(&format!("/api/collections/virtual?type={}", quote(collection_type, "")))
            .await
            .map(collection_list)
    }
}

// Match the Python API adapter: non-list collection responses are empty;
// transport failures still propagate for the service's scoped-error policy.
fn collection_list(value: Value) -> Value {
    if value.is_array() { value } else { json!([]) }
}

#[async_trait]
impl RommRomApi for RommApi {
    async fn get_rom(&self, rom_id: i64) -> Result<Value, RommError> {
        self.http.get_json(&format!("/api/roms/{rom_id}")).await
    }

    async fn download_rom_content(
        &self,
        rom_id: i64,
        file_name: &str,
        dest: &str,
        mode: TransferMode,
        hooks: TransferHooks,
    ) -> anyhow::Result<TransferOutcome> {
        let path = format!("/api/roms/{rom_id}/content/{}", quote(file_name, ""));
        self.http.download(&path, dest, mode, hooks).await
    }
}

#[async_trait]
impl crate::services::ports::RommLibraryPages for RommApi {
    async fn page(&self, unit: &crate::domain::work_unit::WorkUnit, offset: usize) -> Result<Value, RommError> {
        let (key, value) = unit.query_filter();
        self.http.get_json(&format!("/api/roms?{key}={}&limit=50&offset={offset}", quote(&value, ""))).await
    }
}

#[async_trait]
impl crate::services::ports::RommLibraryDelta for RommApi {
    async fn updated_after(&self, platform_id: i64, timestamp: &str) -> Result<Value, RommError> {
        self.http.get_json(&format!("/api/roms?platform_ids={platform_id}&limit=1&offset=0&updated_after={}", quote(timestamp, "/"))).await
    }
}

#[async_trait]
impl crate::services::ports::RommCoverApi for RommApi {
    async fn download_cover(&self,url:&str,dest:&str)->anyhow::Result<()> {
        // RomM cover paths are relative; the HTTP adapter binds credentials to
        // the configured server and quotes spaces through the normal download path.
        anyhow::ensure!(url.starts_with('/')&&!url.starts_with("//"),"Invalid RomM cover path");
        self.http.download(url,dest,TransferMode::Single{resume:false},TransferHooks::basic(Box::new(|_,_|{}),tokio_util::sync::CancellationToken::new())).await.map(|_|())
    }
}

#[async_trait]
impl crate::services::ports::RommPlaytimeApi for RommApi {
    async fn ingest(&self,device:&str,sessions:Vec<Value>)->Result<Value,crate::services::ports::PlaytimeApiError>{
        self.http.post_json_once("/api/play-sessions",&json!({"device_id":device,"sessions":sessions})).await
    }
    async fn sessions(&self,id:i64)->Result<Vec<Value>,RommError>{
        let mut result=vec![];
        for page in 0..50{
            let response=self.http.get_json(&format!("/api/play-sessions?rom_id={id}&limit=100&offset={}",page*100)).await?;
            let rows=response.as_array().or_else(||response["items"].as_array());
            let Some(rows)=rows else{break;};result.extend(rows.iter().cloned());if rows.len()<100{break;}
        }Ok(result)
    }
}

#[async_trait]
impl crate::services::ports::RommFirmwareApi for RommApi {
    async fn list_firmware(&self) -> Result<Value, RommError> {
        self.http.get_json("/api/firmware").await
    }

    async fn download_firmware_content(
        &self,
        firmware_id: i64,
        file_name: &str,
        dest: &str,
        mode: TransferMode,
        hooks: TransferHooks,
    ) -> anyhow::Result<TransferOutcome> {
        let path = format!("/api/firmware/{firmware_id}/content/{}", quote(file_name, ""));
        self.http.download(&path, dest, mode, hooks).await
    }
}

#[async_trait]
impl crate::services::ports::RommSavesApi for RommApi {
    async fn list_saves(&self, rom_id: i64, device_id: Option<&str>, slot: Option<&str>) -> Result<Vec<Value>, RommError> {
        let mut path = format!("/api/saves?rom_id={rom_id}");
        if let Some(d) = device_id {
            path.push_str(&format!("&device_id={}", quote(d, "")));
        }
        if let Some(s) = slot {
            path.push_str(&format!("&slot={}", quote(s, "")));
        }
        Ok(self.http.get_json(&path).await?.as_array().cloned().unwrap_or_default())
    }

    async fn upload_save(&self, up: crate::services::ports::SaveUpload) -> Result<Value, RommError> {
        let mut params = format!("rom_id={}&emulator={}", up.rom_id, quote(&up.emulator, ""));
        if let Some(d) = &up.device_id {
            params.push_str(&format!("&device_id={}", quote(d, "")));
        }
        if let Some(s) = &up.slot {
            params.push_str(&format!("&slot={}", quote(s, "")));
        }
        if up.overwrite {
            params.push_str("&overwrite=true");
        }
        if let Some(id) = up.save_id {
            return self.http.upload_save_file(Method::PUT, &format!("/api/saves/{id}?{params}"), &up.file_name, up.data).await;
        }
        // POST is the only call that stacks versions (PUT updates in place). RomM's autocleanup is off by default, so a
        // retained-version cap needs the flag as well as the limit.
        if let Some(limit) = up.autocleanup_limit {
            params.push_str(&format!("&autocleanup=true&autocleanup_limit={limit}"));
        }
        self.http.upload_save_file(Method::POST, &format!("/api/saves?{params}"), &up.file_name, up.data).await
    }

    async fn download_save(&self, save_id: i64, device_id: Option<&str>) -> Result<Vec<u8>, RommError> {
        let mut path = format!("/api/saves/{save_id}/content");
        if let Some(d) = device_id {
            path.push_str(&format!("?device_id={}&optimistic=true", quote(d, "")));
        }
        self.http.get_bytes(&path, 1 << 30).await
    }

    async fn confirm_download(&self, save_id: i64, device_id: &str) -> Result<Value, RommError> {
        self.http.send_json(Method::POST, &format!("/api/saves/{save_id}/downloaded"), &json!({"device_id": device_id})).await
    }

    async fn delete_saves(&self, save_ids: &[i64]) -> Result<Value, RommError> {
        self.http.send_json(Method::POST, "/api/saves/delete", &json!({"saves": save_ids})).await
    }

    async fn negotiate(&self, device_id: &str, saves: &[Value]) -> Result<Value, RommError> {
        self.http.send_json(Method::POST, "/api/sync/negotiate", &json!({"device_id": device_id, "saves": saves})).await
    }

    async fn complete_session(&self, session_id: i64, completed: u32, failed: u32) -> Result<Value, RommError> {
        self.http
            .send_json(
                Method::POST,
                &format!("/api/sync/sessions/{session_id}/complete"),
                &json!({"operations_completed": completed, "operations_failed": failed}),
            )
            .await
    }

    async fn register_device(&self, name: &str, platform: &str, client: &str, client_version: &str, hostname: Option<&str>) -> Result<Value, RommError> {
        let mut payload = json!({"name": name, "platform": platform, "client": client, "client_version": client_version});
        if let Some(h) = hostname {
            payload["hostname"] = json!(h);
        }
        self.http.send_json(Method::POST, "/api/devices", &payload).await
    }

    async fn list_devices(&self) -> Result<Vec<Value>, RommError> {
        Ok(self.http.get_json("/api/devices").await?.as_array().cloned().unwrap_or_default())
    }

    async fn update_device(&self, device_id: &str, fields: &Value) -> Result<Value, RommError> {
        self.http.send_json(Method::PUT, &format!("/api/devices/{}", quote(device_id, "")), fields).await
    }
}
