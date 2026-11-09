//! Shortcut removal unbinds ROM rows; installs and per-ROM history survive.
use std::{collections::BTreeSet,sync::Arc};
use serde_json::{Value,json};
use base64::{Engine,engine::{general_purpose::{GeneralPurpose,GeneralPurposeConfig},DecodePaddingMode},alphabet};
use crate::services::ports::{SyncLibraryRepo,ShortcutBindings,SteamAssets,KvConfigRepo};
pub struct ShortcutRemovalService {pub registry:Arc<dyn SyncLibraryRepo>,pub bindings:Arc<dyn ShortcutBindings>,pub assets:Arc<dyn SteamAssets>,pub kv:Arc<dyn KvConfigRepo>}
fn id(v:&Value)->Option<i64>{v.as_i64().or_else(||v.as_str().and_then(|s|s.parse().ok()))}
impl ShortcutRemovalService {
    pub async fn removal_set(&self,slug:Option<&str>)->anyhow::Result<Value>{
        let rows=self.registry.registry().await?;let selected:Vec<_>=rows.iter().filter(|(_,r)|slug.is_none_or(|slug|r["platform_slug"].as_str()==Some(slug))).collect();
        let mut out=json!({"success":true,"app_ids":selected.iter().filter_map(|(_,r)|r["app_id"].as_i64()).collect::<Vec<_>>(),"rom_ids":selected.iter().map(|(id,_)|id.to_string()).collect::<Vec<_>>()});
        if let Some(slug)=slug{let names:Value=self.kv.get("platform_names").await?.and_then(|s|serde_json::from_str(&s).ok()).unwrap_or(json!({}));out["platform_name"]=json!(names[slug].as_str().unwrap_or(slug));}Ok(out)
    }
    pub async fn report(&self,ids:&Value)->anyhow::Result<Value>{
        let raw=ids.as_array().ok_or_else(||anyhow::anyhow!("Expected ROM ID list"))?;let ids:Vec<_>=raw.iter().map(|v|id(v).ok_or_else(||anyhow::anyhow!("Invalid ROM ID"))).collect::<Result<_,_>>()?;
        let registry=self.registry.registry().await?;let apps:Vec<_>=ids.iter().filter_map(|id|registry.get(id)?["app_id"].as_i64()).collect();
        if let Err(e)=self.assets.steam_input(&apps,"default").await{tracing::warn!("Cannot reset Steam Input: {e}");}
        if let Some(grid)=self.assets.grid_dir().await?{
            for id in &ids{if let Some(entry)=registry.get(id){
                let cache=self.assets.cache_path(*id);let cache_exists=self.assets.exists(&cache).await;
                if cache_exists{self.assets.remove(&cache).await?;}else if let Some(app)=entry["app_id"].as_i64(){self.assets.remove(&format!("{grid}/{app}p.png")).await?;}
                self.assets.remove(&format!("{grid}/romm_{id}_cover.png")).await?;
            }}
        }
        self.bindings.unbind(ids).await?;Ok(json!({"success":true,"message":format!("Removed {} shortcuts",raw.len())}))
    }
    pub async fn reconcile(&self,live:&Value)->anyhow::Result<Value>{
        let live:BTreeSet<_>=live.as_array().ok_or_else(||anyhow::anyhow!("Expected app ID list"))?.iter().filter_map(id).collect();
        let count=self.bindings.reconcile(live).await?;Ok(json!({"success":true,"unbound_count":count,"message":format!("Unbound {count} stale shortcut(s)")}))
    }
    pub async fn save_icon(&self,app:i64,encoded:&str)->Value{
        let decoder=GeneralPurpose::new(&alphabet::STANDARD,GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true).with_decode_padding_mode(DecodePaddingMode::Indifferent));
        let bytes=match decoder.decode(encoded){Ok(b)=>b,Err(_)=>return crate::domain::errors::failure("invalid_payload","Failed to decode icon data")};
        match self.assets.write_icon(app,&bytes).await {Ok(Some(path))=>json!({"success":true,"icon_path":path}),_=>json!({"success":false})}
    }
}
