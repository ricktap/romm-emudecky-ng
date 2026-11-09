//! Explicit cover fetch/repair plus the legacy/pending read fallbacks.
use std::sync::Arc;
use base64::{Engine,engine::general_purpose::STANDARD};
use serde_json::{Value,json};
use crate::{domain::errors::failure,services::{artwork::ArtworkService,ports::{GameReadRepo,PendingRomReader,RommRomApi,SyncLibraryRepo}}};
pub struct GameCoverServiceConfig {
    pub artwork:Arc<ArtworkService>,pub romm:Arc<dyn RommRomApi>,pub records:Arc<dyn GameReadRepo>,
    pub registry:Arc<dyn SyncLibraryRepo>,pub pending:Arc<dyn PendingRomReader>,
}
pub struct GameCoverService {config:GameCoverServiceConfig}
fn cover_url(rom:&Value)->Option<&str>{rom["path_cover_large"].as_str().filter(|s|!s.is_empty()).or_else(||rom["path_cover_small"].as_str().filter(|s|!s.is_empty()))}
impl GameCoverService {
    pub fn new(config:GameCoverServiceConfig)->Self{Self{config}}
    async fn read(&self,path:&str)->Value{json!({"base64":self.config.artwork.assets.read(path).await.ok().map(|b|STANDARD.encode(b))})}
    pub async fn get_base64(&self,id:i64)->Value{
        let pending=self.config.pending.pending_rom(id).await;
        let mut path=pending.as_ref().and_then(|r|r["cover_path"].as_str()).filter(|p|!p.is_empty()).map(str::to_string);
        if path.is_none(){
            match self.config.registry.registry().await {
                Ok(rows)=>path=rows.get(&id).and_then(|r|r["cover_path"].as_str()).filter(|p|!p.is_empty()).map(str::to_string),
                Err(_)=>return json!({"base64":null}),
            }
        }
        if let Some(path)=path {return self.read(&path).await;}
        self.config.artwork.get_base64(id).await
    }
    pub async fn fetch(&self,id:i64)->Value{
        let art=&self.config.artwork;
        let _guard=art.lock.lock().await;
        let cache=art.assets.cache_path(id);
        if art.assets.exists(&cache).await{return self.read(&cache).await;}
        let Ok(rom)=self.config.romm.get_rom(id).await else{return json!({"base64":null});};
        let Some(url)=cover_url(&rom)else{return json!({"base64":null});};
        if art.download_atomic(url,&cache).await.is_err(){return json!({"base64":null});}
        self.read(&cache).await
    }
    pub async fn refresh(&self,id:i64)->anyhow::Result<Value>{
        let art=&self.config.artwork;
        let _guard=art.lock.lock().await;
        let rows=self.config.registry.registry().await?;
        let Some(app)=rows.get(&id).and_then(|r|r["app_id"].as_i64())else{return Ok(failure("not_synced","ROM is not synced to Steam"));};
        let Some(grid)=art.assets.grid_dir().await?else{return Ok(failure("no_grid_dir","Steam grid directory not found"));};
        let Ok(rom)=self.config.romm.get_rom(id).await else{return Ok(failure("server_unreachable","Could not fetch ROM from server"));};
        if rom.is_null() || rom.as_object().is_some_and(|r|r.is_empty()){return Ok(failure("server_unreachable","Could not fetch ROM from server"));}
        let Some(url)=cover_url(&rom)else{return Ok(failure("no_cover","ROM has no cover artwork"));};
        let cache=art.assets.cache_path(id);
        if art.download_atomic(url,&cache).await.is_err(){return Ok(failure("download_failed","Could not download cover artwork"));}
        // Check the binding again after HTTP; never publish onto a moved shortcut.
        if !self.config.records.remember_cover(id,app,&cache).await?{return Ok(failure("state_changed","Game shortcut changed while refreshing artwork. Try again."));}
        art.assets.copy_atomic(&cache,&format!("{grid}/{app}p.png")).await?;
        Ok(json!({"success":true,"message":"Cover refreshed","cover_path":cache}))
    }
}
