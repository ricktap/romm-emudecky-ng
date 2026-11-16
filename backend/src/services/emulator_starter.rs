//! The "Eden" entry in the Steam library: a non-Steam shortcut that opens Eden itself (its window, no game), so its
//! settings are reachable from Game Mode. The frontend creates it with `SteamClient` (its exe is Eden itself, never the
//! ROM launcher, so library sync never treats it as a ROM); this service says whether and where Eden is installed and
//! remembers the shortcut's app id, so a shortcut the user removed is not created again.
use std::sync::Arc;
use serde_json::{Value, json};
use crate::{domain::errors::failure, services::ports::{EmulatorLocator, SettingsPersister, SettingsStore}};

const SETTING: &str = "eden_starter_app_id";

pub struct EmulatorStarterService {
    locator: Arc<dyn EmulatorLocator>,
    settings: SettingsStore,
    persister: Arc<dyn SettingsPersister>,
}

impl EmulatorStarterService {
    pub fn new(locator: Arc<dyn EmulatorLocator>, settings: SettingsStore, persister: Arc<dyn SettingsPersister>) -> Self {
        Self { locator, settings, persister }
    }

    /// `{success, name, exe, start_dir, app_id}`: `exe` is null while Eden is not installed; `app_id` is the shortcut
    /// made (or adopted) before, null when there was none.
    pub fn starter(&self) -> Value {
        let exe = self.locator.installed("switch").get("eden").cloned();
        let start_dir = exe.as_deref().and_then(|e| e.rsplit_once('/')).map(|(dir, _)| dir.to_string());
        let app_id = self.settings.get(SETTING).and_then(|v| v.as_i64()).filter(|id| *id > 0);
        json!({"success": true, "name": "Eden", "exe": exe, "start_dir": start_dir, "app_id": app_id})
    }

    /// Remember the shortcut's app id (the one just created, or an existing "Eden" shortcut that was adopted).
    pub async fn remember(&self, app_id: i64) -> Value {
        if app_id <= 0 {
            return failure("invalid_app_id", "The app id must be positive");
        }
        let old = self.settings.get(SETTING);
        self.settings.set(SETTING, json!(app_id));
        if let Err(e) = self.persister.save_settings().await {
            match old {
                Some(v) => self.settings.set(SETTING, v),
                None => {
                    self.settings.remove(SETTING);
                }
            }
            tracing::warn!("Cannot save the Eden shortcut id: {e}");
            return failure("save_failed", "Could not save the settings");
        }
        json!({"success": true})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::testing::{DownloadEnv, FakePersister};

    async fn service(env: &DownloadEnv, persister: Arc<FakePersister>) -> EmulatorStarterService {
        EmulatorStarterService::new(env.locator.clone(), env.settings.clone(), persister)
    }

    #[tokio::test]
    async fn reports_where_eden_is_and_the_shortcut_made_before() {
        let env = DownloadEnv::new().await;
        let persister = Arc::new(FakePersister::default());
        let s = service(&env, persister.clone()).await;
        env.locator.installed.lock().unwrap().clear();
        assert_eq!(s.starter(), json!({"success": true, "name": "Eden", "exe": null, "start_dir": null, "app_id": null}));
        env.locator.installed.lock().unwrap().insert("eden".into(), "/home/u/Applications/Eden.AppImage".into());
        assert_eq!(s.starter()["exe"], "/home/u/Applications/Eden.AppImage");
        assert_eq!(s.starter()["start_dir"], "/home/u/Applications");
        assert_eq!(s.remember(4242).await, json!({"success": true}));
        assert_eq!(persister.saves(), 1);
        assert_eq!(s.starter()["app_id"], 4242);
    }

    #[tokio::test]
    async fn a_bad_id_or_a_failed_save_changes_nothing() {
        let env = DownloadEnv::new().await;
        let persister = Arc::new(FakePersister::default());
        let s = service(&env, persister.clone()).await;
        assert_eq!(s.remember(0).await["reason"], "invalid_app_id");
        assert_eq!(s.remember(-5).await["reason"], "invalid_app_id");
        assert_eq!(s.remember(7).await["success"], true);
        persister.fail_with("disk full");
        assert_eq!(s.remember(8).await["reason"], "save_failed");
        assert_eq!(s.starter()["app_id"], 7, "the previous id is kept");
        // Never saved before and the save fails: still none.
        let env = DownloadEnv::new().await;
        let persister = Arc::new(FakePersister::default());
        persister.fail_with("disk full");
        let s = service(&env, persister).await;
        assert_eq!(s.remember(9).await["reason"], "save_failed");
        assert_eq!(s.starter()["app_id"], Value::Null);
    }
}
