//! Standalone save discovery is deliberately unavailable until the step-5 ADR.
//! No RetroArch filename guesses or save writes are permitted for Switch.
use async_trait::async_trait;
use serde_json::{Value,json};
use crate::{adapters::db::Db,services::ports::{SaveSafety,SettingsStore}};
pub struct SwitchSaveSafety{db:Db,settings:SettingsStore}
impl SwitchSaveSafety{pub fn new(db:Db,settings:SettingsStore)->Self{Self{db,settings}}}
#[async_trait]
impl SaveSafety for SwitchSaveSafety{
    async fn status(&self,_:i64)->anyhow::Result<Value>{anyhow::bail!("Standalone save sync is unsupported until a save provider is configured")}
    async fn tracked(&self,id:i64)->anyhow::Result<bool>{self.db.call(move|c|c.query_row("SELECT EXISTS(SELECT 1 FROM rom_save_files WHERE rom_id=?1) OR EXISTS(SELECT 1 FROM rom_save_states WHERE rom_id=?1 AND slots!='{}')",[id],|r|r.get(0))).await}
    async fn drift(&self,_:i64)->anyhow::Result<bool>{Ok(false)}
    // Standalone emulators have no libretro core; legacy detect_core_change
    // likewise reports false when active_core is None.
    async fn core_change(&self,_:i64)->anyhow::Result<Value>{Ok(json!({"changed":false}))}
    async fn post_exit(&self,_:i64)->anyhow::Result<Value>{
        Ok(if self.settings.truthy("save_sync_enabled"){json!({"success":false,"reason":"unsupported","synced":0})}else{json!({"success":true,"synced":0})})
    }
}
