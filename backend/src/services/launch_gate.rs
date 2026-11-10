//! The pre-launch gate and local drift surface; save discovery is injected.
use std::sync::Arc;
use serde_json::{Value,json};
use crate::services::ports::{VersionRepo,SaveSafety,SettingsStore};
pub struct LaunchGateConfig{pub repo:Arc<dyn VersionRepo>,pub saves:Arc<dyn SaveSafety>,pub settings:SettingsStore}
pub struct LaunchGate{c:LaunchGateConfig}
fn verdict(action:&str,reason:Option<&str>,title:Option<&str>,body:Option<&str>)->Value{json!({"action":action,"reason":reason,"toast_title":title,"toast_body":body})}
impl LaunchGate{
    pub fn new(c:LaunchGateConfig)->Self{Self{c}}
    pub async fn evaluate(&self,app:i64)->anyhow::Result<Value>{
        let allow=||verdict("allow",None,None,None);
        let Some(g)=self.c.repo.group(app).await?else{return Ok(allow());};
        if g.bound["installed"]!=true{return Ok(verdict("block",Some("not_installed"),Some("EmuDecky RomM Manager"),Some("ROM not downloaded. Open the game page to download it first.")));}
        if !self.c.settings.truthy("save_sync_enabled"){return Ok(allow());}
        let id=g.bound["rom_id"].as_i64().unwrap();
        match self.c.saves.status(id).await{
            Ok(s) if crate::domain::settings::truthy(&s["conflicts"])=>Ok(verdict("block",Some("save_conflict"),Some("RomM Save Sync"),Some("Save conflict detected — open game page to resolve before playing"))),
            Err(_) if self.c.saves.tracked(id).await?=>Ok(verdict("warn",Some("save_status_failed"),Some("RomM Save Sync"),Some("Save-status check failed — retry?"))),
            _=>Ok(allow()),
        }
    }
    pub async fn drift(&self,id:i64)->Value{json!({"rom_id":id,"drifted":self.c.saves.drift(id).await.unwrap_or(false)})}
    pub async fn core_change(&self,id:i64)->Value{self.c.saves.core_change(id).await.unwrap_or(json!({"changed":false}))}
}
