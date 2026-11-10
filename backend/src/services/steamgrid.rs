//! SteamGridDB resolution, user picks and cache-only/passive artwork reads.
use std::sync::Arc;
use base64::{Engine,engine::general_purpose::STANDARD};
use serde_json::{Value,json};
use tokio::sync::Mutex;
use crate::{domain::{errors::failure,sgdb_artwork::{self,ASSETS}},services::ports::{GameReadRepo,PendingRomReader,RommRomApi,SettingsPersister,SettingsStore,SgdbError,SteamAssets,SteamGridApi,SyncLibraryRepo},util::path_safety::quote};

pub struct SteamGridServiceConfig {
    pub api:Arc<dyn SteamGridApi>,pub romm:Arc<dyn RommRomApi>,pub assets:Arc<dyn SteamAssets>,
    pub registry:Arc<dyn SyncLibraryRepo>,pub records:Arc<dyn GameReadRepo>,pub pending:Arc<dyn PendingRomReader>,
    pub settings:SettingsStore,pub persister:Arc<dyn SettingsPersister>,
}
pub struct SteamGridService {config:SteamGridServiceConfig,art_lock:Mutex<()>,key_lock:Mutex<()>}
impl SteamGridService {
    pub fn new(config:SteamGridServiceConfig)->Self{Self{config,art_lock:Mutex::new(()),key_lock:Mutex::new(())}}
    fn key(&self)->String{self.config.settings.get_str("steamgriddb_api_key").unwrap_or_default()}
    fn path(&self,id:i64,kind:&str)->String{format!("{}/{}_{}.png",self.config.assets.cache_dir(),id,kind)}
    async fn cached(&self,path:&str)->Option<String>{self.config.assets.read(path).await.ok().filter(|b|!b.is_empty()).map(|b|STANDARD.encode(b))}
    async fn state_id(&self,id:i64)->anyhow::Result<Option<i64>>{
        let rows=self.config.registry.registry().await?;
        if let Some(sgdb)=rows.get(&id).and_then(|r|r["sgdb_id"].as_i64()).filter(|id|*id!=0){return Ok(Some(sgdb));}
        Ok(self.config.pending.pending_rom(id).await.and_then(|r|r["sgdb_id"].as_i64()).filter(|id|*id!=0))
    }
    async fn download(&self,id:i64,game:i64,kind:&str,key:&str)->Option<String>{
        let dest=self.path(id,kind);
        if self.config.assets.exists(&dest).await{return Some(dest);}
        let endpoint=sgdb_artwork::endpoint(kind,game)?;
        let response=self.config.api.request(&endpoint,key).await.ok()?;
        let url=sgdb_artwork::first_image(&response,false)?;
        self.config.api.download_image(url,&dest).await.ok()?;
        Some(dest)
    }
    pub async fn artwork(&self,id:i64,asset:i64)->Value{
        let Some(kind)=sgdb_artwork::asset_name(asset)else{return json!({"base64":null,"no_api_key":false});};
        let _guard=self.art_lock.lock().await;
        let path=self.path(id,kind);
        if let Some(bytes)=self.cached(&path).await{return json!({"base64":bytes,"no_api_key":false});}
        let key=self.key();if key.is_empty(){return json!({"base64":null,"no_api_key":true});}
        if let Ok(Some(game))=self.state_id(id).await {
            if let Some(path)=self.download(id,game,kind,&key).await{return json!({"base64":self.cached(&path).await,"no_api_key":false});}
        }
        json!({"base64":null,"no_api_key":false})
    }
    pub async fn resolution(&self,id:i64)->anyhow::Result<Value>{
        let key=self.key();if key.is_empty(){return Ok(json!({"decision":"no_api_key"}));}
        let state=self.state_id(id).await?;
        let rom=self.config.romm.get_rom(id).await.unwrap_or(Value::Null);
        if let Some(game)=rom["sgdb_id"].as_i64(){
            if Some(game)!=state{self.config.records.remember_sgdb_id(id,game).await?;}
            return Ok(json!({"decision":"resolved","sgdb_id":game}));
        }
        if let Some(game)=state{return Ok(json!({"decision":"resolved","sgdb_id":game}));}
        if let Some(igdb)=rom["igdb_id"].as_i64().filter(|id|*id!=0){
            if let Ok(response)=self.config.api.request(&format!("/games/igdb/{igdb}"),&key).await {
                if response["success"]==true{if let Some(game)=response["data"]["id"].as_i64().filter(|id|*id!=0){
                    self.config.records.remember_sgdb_id(id,game).await?;return Ok(json!({"decision":"resolved","sgdb_id":game}));
                }}
            }
        }
        let search=self.search(rom["name"].as_str().unwrap_or("")).await;
        Ok(json!({"decision":"needs_pick","candidates":search["games"]}))
    }
    pub async fn search(&self,term:&str)->Value{
        let key=self.key();if key.is_empty(){let mut result=failure("no_api_key","No SteamGridDB API key configured");result["games"]=json!([]);return result;}
        let response=match self.config.api.request(&format!("/search/autocomplete/{}",quote(term,"/")),&key).await {
            Ok(r)=>r,Err(e)=>{let mut result=failure("server_unreachable",&format!("SteamGridDB search failed: {e}"));result["games"]=json!([]);return result;}
        };
        let mut games=sgdb_artwork::candidates(&response);games.truncate(6);
        // Independent thumbnails may fail without invalidating the candidates.
        let mut tasks=tokio::task::JoinSet::new();
        for (index,game) in games.iter().enumerate(){let api=self.config.api.clone();let key=key.clone();let id=game["id"].clone();
            tasks.spawn(async move {let image=api.request(&format!("/grids/game/{id}?limit=1"),&key).await.ok();(index,image.as_ref().and_then(|v|sgdb_artwork::first_image(v,true)).map(str::to_string))});
        }
        for game in &mut games{game["thumb_url"]=Value::Null;}
        while let Some(result)=tasks.join_next().await{if let Ok((index,thumb))=result{games[index]["thumb_url"]=json!(thumb);}}
        json!({"success":true,"games":games})
    }
    pub async fn apply(&self,id:i64,game:i64)->anyhow::Result<Value>{
        let _guard=self.art_lock.lock().await;
        for kind in ASSETS{if self.config.assets.remove(&self.path(id,kind)).await.is_err(){tracing::warn!(rom_id=id,asset=kind,"Cannot clear cached artwork");}}
        let key=self.key();
        if !key.is_empty(){for kind in ASSETS{self.download(id,game,kind,&key).await;}}
        // A manual choice paints pixels; only authoritative resolution persists IDs.
        Ok(json!({"success":true}))
    }
    pub async fn verify(&self,key:Option<&str>)->Value{
        let saved=self.key();let key=key.filter(|s|!s.is_empty()&&*s!="••••").unwrap_or(&saved);
        if key.is_empty(){return failure("no_api_key","No API key configured");}
        match self.config.api.request("/search/autocomplete/test",key).await {
            Ok(v)if v["success"]==true=>json!({"success":true,"message":"API key is valid"}),
            Ok(_)=>failure("auth_failed","API key rejected by SteamGridDB"),
            Err(SgdbError::Http(401|403))=>failure("auth_failed","Invalid API key"),
            Err(SgdbError::Http(status))=>failure("server_unreachable",&format!("SteamGridDB error: HTTP {status}")),
            Err(e)=>failure("server_unreachable",&format!("Connection failed: {e}")),
        }
    }
    pub async fn save_key(&self,key:&str)->anyhow::Result<Value>{
        let _guard=self.key_lock.lock().await;
        if !key.is_empty()&&key!="••••"{
            let old=self.config.settings.get("steamgriddb_api_key");
            self.config.settings.set("steamgriddb_api_key",json!(key));
            if let Err(e)=self.config.persister.save_settings().await {
                match old{Some(v)=>self.config.settings.set("steamgriddb_api_key",v),None=>{self.config.settings.remove("steamgriddb_api_key");}}
                return Err(e);
            }
        }
        Ok(json!({"success":true,"message":"SteamGridDB API key saved"}))
    }
    pub async fn prune(&self)->anyhow::Result<()>{
        let rows=self.config.registry.registry().await?;let dir=self.config.assets.cache_dir();
        for name in self.config.assets.files_in(&dir).await?{
            let bound=name.split_once('_').and_then(|(id,_)|id.parse::<i64>().ok()).and_then(|id|rows.get(&id)).is_some_and(|r|r["app_id"].is_number());
            if name.ends_with(".tmp")||!bound{if self.config.assets.remove(&format!("{dir}/{name}")).await.is_err(){tracing::warn!("Cannot remove orphaned SteamGridDB artwork");}}
        }Ok(())
    }
}
