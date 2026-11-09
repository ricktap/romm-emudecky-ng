//! Installed-game actions. Download admission cannot race a destructive uninstall.
use std::sync::Arc;
use serde_json::{Value,json};
use crate::{domain::errors::failure,services::{ports::*,downloads::DownloadService,rom_removal::RomRemovalService}};
pub struct GameActionsConfig{pub library:Arc<dyn LibraryRepo>,pub registry:Arc<dyn SyncLibraryRepo>,pub assets:Arc<dyn SteamAssets>,pub removal:Arc<RomRemovalService>,pub downloads:Arc<DownloadService>,pub settings:SettingsStore}
pub struct GameActions{c:GameActionsConfig}
impl GameActions{
    pub fn new(c:GameActionsConfig)->Self{Self{c}}
    pub async fn remove(&self,id:i64)->Value{
        let _guard=self.c.downloads.mutation_lock.lock().await;
        if self.c.downloads.active_download_rom_ids().contains(&id){return failure("download_in_progress","Cancel the running download first.");}
        let result=self.c.removal.remove_rom(id).await;
        if result["success"]==true{self.c.downloads.evict(id).await;}result
    }
    pub async fn uninstall_all(&self)->anyhow::Result<Value>{
        let _guard=self.c.downloads.mutation_lock.lock().await;
        if !self.c.downloads.active_download_rom_ids().is_empty(){return Ok(failure("download_in_progress","Cancel the running downloads first."));}
        let installs=self.c.library.installs().await?;
        let mut count=0;let mut errors=vec![];let mut apps=vec![];let mut failed=std::collections::BTreeSet::new();
        for install in installs{
            let id=install.rom_id;let result=self.c.removal.remove_rom(id).await;
            if result["success"]==true{
                count+=1;if let Some(app)=self.c.library.rom(id).await?.and_then(|r|r.shortcut_app_id){apps.push(app);}
                self.c.downloads.evict(id).await;
            }else{failed.insert(id);errors.push(json!({"rom_id":id.to_string(),"error":result["message"]}));}
        }
        // Clear stopped downloads too, but leave failed installs retryable.
        for entry in self.c.downloads.get_download_queue()["downloads"].as_array().into_iter().flatten(){
            if let Some(id)=entry["rom_id"].as_i64().filter(|id|!failed.contains(id)){self.c.downloads.evict(id).await;}
        }
        Ok(json!({"success":errors.is_empty(),"removed_count":count,"errors":errors,"app_ids":apps}))
    }
    pub async fn apply_input(&self)->anyhow::Result<Value>{
        let rows=self.c.registry.registry().await?;let apps:Vec<_>=rows.values().filter_map(|r|r["app_id"].as_i64()).collect();
        if apps.is_empty(){return Ok(json!({"success":true,"message":"No shortcuts to update"}));}
        let mode=self.c.settings.get_str("steam_input_mode").unwrap_or("default".into());
        if self.c.assets.steam_input(&apps,&mode).await.is_err(){return Ok(failure("unknown","Operation failed"));}
        Ok(json!({"success":true,"message":format!("Steam Input set to '{mode}' for {} shortcuts",apps.len())}))
    }
}
