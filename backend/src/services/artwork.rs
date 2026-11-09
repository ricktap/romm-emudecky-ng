//! Per-ROM cache, single-version grid seeding, and atomic cover downloads.
use std::{collections::BTreeMap,sync::Arc};
use serde_json::{Value,json};
use base64::{Engine,engine::general_purpose::STANDARD};
use crate::services::{library_state::LibraryRunState,ports::{RommCoverApi,SteamAssets,SyncLibraryRepo}};
pub struct ArtworkService {pub api:Arc<dyn RommCoverApi>,pub assets:Arc<dyn SteamAssets>,pub registry:Arc<dyn SyncLibraryRepo>}
impl ArtworkService {
    /// Read-only cover callable used by the frontend before it acknowledges a unit.
    pub async fn get_base64(&self,id:i64)->Value {
        let cache=self.assets.cache_path(id);
        if self.assets.exists(&cache).await{return json!({"base64":self.assets.read(&cache).await.ok().map(|bytes|STANDARD.encode(bytes))});}
        let grid=self.assets.grid_dir().await.ok().flatten();
        if let Some(grid)=grid {
            let registry=self.registry.registry().await.unwrap_or_default();
            let staging=format!("{grid}/romm_{id}_cover.png");
            let app=registry.get(&id).and_then(|r|r["app_id"].as_i64());
            let candidates=[Some(staging),app.map(|app|format!("{grid}/{app}p.png"))];
            for path in candidates.into_iter().flatten(){if self.assets.exists(&path).await{return json!({"base64":self.assets.read(&path).await.ok().map(|bytes|STANDARD.encode(bytes))});}}
        }
        json!({"base64":null})
    }

    /// Startup housekeeping reads the registry successfully before deleting anything.
    pub async fn prune(&self)->anyhow::Result<()> {
        let registry=self.registry.registry().await?;
        if let Some(grid)=self.assets.grid_dir().await? {
            for name in self.assets.files_in(&grid).await? {
                let Some(id)=name.strip_prefix("romm_").and_then(|n|n.strip_suffix("_cover.png")).and_then(|n|n.parse::<i64>().ok())else{continue};
                let app=registry.get(&id).and_then(|r|r["app_id"].as_i64());
                let orphan=match app{None=>true,Some(app)=>self.assets.exists(&format!("{grid}/{app}p.png")).await};
                if orphan{self.assets.remove(&format!("{grid}/{name}")).await?;}
            }
        }
        let cache=self.assets.cache_dir();
        for name in self.assets.files_in(&cache).await? {
            if name.ends_with(".tmp") || name.strip_suffix(".png").and_then(|n|n.parse::<i64>().ok()).is_some_and(|id|!registry.contains_key(&id)) {
                self.assets.remove(&format!("{cache}/{name}")).await?;
            }
        }Ok(())
    }

    pub async fn download(&self,roms:&[Value],state:&LibraryRunState,on_progress:&(dyn Fn(usize,usize)+Send+Sync))->BTreeMap<i64,String> {
        let mut result=BTreeMap::new();let grid=match self.assets.grid_dir().await{Ok(Some(g))=>g,_=>return result};
        let registry=match self.registry.registry().await{Ok(r)=>r,Err(e)=>{tracing::warn!("Cannot read artwork registry: {e}");return result;}};
        for (index,rom) in roms.iter().enumerate(){
            if state.is_cancelling(){break;}
            on_progress(index+1,roms.len());
            let Some(id)=rom["id"].as_i64()else{continue};let Some(url)=rom["path_cover_large"].as_str().filter(|s|!s.is_empty()).or_else(||rom["path_cover_small"].as_str().filter(|s|!s.is_empty()))else{continue};
            let cache=self.assets.cache_path(id);
            if self.assets.exists(&cache).await{result.insert(id,cache);continue;}
            if let Some(entry)=registry.get(&id){
                let group=entry["sibling_group_key"].as_str();
                let count=group.map_or(1,|key|registry.values().filter(|r|r["sibling_group_key"].as_str()==Some(key)).count());
                if count<=1{if let Some(app)=entry["app_id"].as_i64(){let final_path=format!("{grid}/{app}p.png");if self.assets.exists(&final_path).await && self.assets.copy_atomic(&final_path,&cache).await.is_ok(){result.insert(id,cache);continue;}}}
            }
            let tmp=format!("{cache}.tmp");
            let downloaded=match self.api.download_cover(url,&tmp).await{Ok(())=>self.assets.rename(&tmp,&cache).await,Err(e)=>Err(e)};
            match downloaded{Ok(())=>{result.insert(id,cache);},Err(_)=>{let _=self.assets.remove(&tmp).await;tracing::warn!(rom_id=id,"Cover download failed");}}
        }result
    }
}
