//! Offline detail/metadata reads. BIOS and achievements remain removed for Switch.
use std::sync::Arc;
use serde_json::{Value,json};
use crate::{services::ports::{Clock,GameReadRepo,SettingsStore}, util::path_safety::basename};

pub struct GameDetailServiceConfig {
    pub repo:Arc<dyn GameReadRepo>, pub settings:SettingsStore, pub clock:Arc<dyn Clock>,
    pub schedule_adoption:Arc<dyn Fn(i64)+Send+Sync>,
}
pub struct GameDetailService { config:GameDetailServiceConfig }
pub fn empty_metadata()->Value {json!({"summary":"","genres":[],"companies":[],"first_release_date":null,
    "average_rating":null,"game_modes":[],"player_count":"","cached_at":0.0,"steam_categories":[]})}

impl GameDetailService {
    pub fn new(config:GameDetailServiceConfig)->Self {Self{config}}
    pub async fn metadata(&self,id:i64)->anyhow::Result<Value>{Ok(self.config.repo.metadata(id).await?.unwrap_or_else(empty_metadata))}
    pub async fn all_metadata(&self)->anyhow::Result<Value>{self.config.repo.all_metadata().await}
    pub async fn detail(&self,app:i64)->anyhow::Result<Value>{
        let Some(s)=self.config.repo.game_detail(app).await? else{return Ok(json!({"found":false}));};
        let r=&s.rom;let slug=r["platform_slug"].as_str().unwrap_or("");
        let installed=s.install_file.is_some();
        if !installed {(self.config.schedule_adoption)(r["rom_id"].as_i64().unwrap());}
        let stale=if s.metadata.as_ref().is_none_or(|m|self.config.clock.epoch_seconds()-m["cached_at"].as_f64().unwrap_or(0.0)>7.0*24.0*3600.0){vec!["metadata"]}else{vec![]};
        let display=s.save_status.as_ref().map(|s|{
            if s["files"].as_array().is_none_or(Vec::is_empty){json!({"status":"none","label":"No saves","last_sync_check_at":null})}
            else if s["last_sync_check_at"].as_str().is_some_and(|t|!t.is_empty()){json!({"status":"synced","label":null,"last_sync_check_at":s["last_sync_check_at"]})}
            else{json!({"status":"synced","label":"Not synced","last_sync_check_at":null})}
        });
        Ok(json!({"found":true,"rom_id":r["rom_id"],"rom_name":r["name"],"platform_slug":slug,
            "platform_name":s.platform_names[slug].as_str().unwrap_or(slug),"installed":installed,
            "save_sync_enabled":self.config.settings.truthy("save_sync_enabled"),"save_status":s.save_status,"save_sync_display":display,
            "metadata":s.metadata,"bios_status":null,"bios_level":null,"bios_label":null,
            "rom_file":s.install_file.as_deref().filter(|s|!s.is_empty()).map(basename).unwrap_or(r["fs_name"].as_str().unwrap_or("")),
            "ra_id":r["ra_id"],"achievement_summary":null,"stale_fields":stale,
            "regions":r["regions"],"languages":r["languages"],"revision":r["revision"],"tags":r["tags"],"is_main_sibling":r["is_main_sibling"]}))
    }
}
