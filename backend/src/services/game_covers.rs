//! Explicit cover fetch/repair plus the legacy/pending read fallbacks, and the game page's screenshot gallery.
use std::{collections::HashMap,sync::{Arc,Mutex}};
use base64::{Engine,engine::general_purpose::STANDARD};
use serde_json::{Value,json};
use crate::{domain::errors::failure,services::{artwork::ArtworkService,ports::{GameReadRepo,PendingRomReader,RommRomApi,SyncLibraryRepo}}};
pub struct GameCoverServiceConfig {
    pub artwork:Arc<ArtworkService>,pub romm:Arc<dyn RommRomApi>,pub records:Arc<dyn GameReadRepo>,
    pub registry:Arc<dyn SyncLibraryRepo>,pub pending:Arc<dyn PendingRomReader>,
}
pub struct GameCoverService {config:GameCoverServiceConfig,shots:Mutex<HashMap<i64,Vec<String>>>}
/// The gallery shows at most this many screenshots per game.
pub const MAX_SCREENSHOTS:usize=8;
/// RomM's screenshot paths (uploaded ones first, then scraped resources); only server-relative ones, which go to
/// the RomM server with its credentials. External IGDB URLs are left out.
fn screenshot_paths(rom:&Value)->Vec<String>{
    rom["merged_screenshots"].as_array().map(|list|list.iter().filter_map(Value::as_str)
        .filter(|p|p.starts_with('/')&&!p.starts_with("//")).take(MAX_SCREENSHOTS).map(str::to_string).collect()).unwrap_or_default()
}
/// The MIME type of a downloaded image, from its first bytes (JPEG when unknown).
pub fn image_mime(bytes:&[u8])->&'static str{
    if bytes.starts_with(b"\x89PNG"){"image/png"}
    else if bytes.starts_with(b"GIF8"){"image/gif"}
    else if bytes.len()>=12&&&bytes[..4]==b"RIFF"&&&bytes[8..12]==b"WEBP"{"image/webp"}
    else{"image/jpeg"}
}
fn cover_url(rom:&Value)->Option<&str>{rom["path_cover_large"].as_str().filter(|s|!s.is_empty()).or_else(||rom["path_cover_small"].as_str().filter(|s|!s.is_empty()))}
impl GameCoverService {
    pub fn new(config:GameCoverServiceConfig)->Self{Self{config,shots:Mutex::new(HashMap::new())}}
    fn shot_path(&self,id:i64,index:usize)->String{format!("{}/{id}_shot{index}.img",self.config.artwork.assets.cache_dir())}
    /// How many screenshots the gallery has. Online it asks RomM (and remembers the paths for the image reads);
    /// offline it counts the cached ones, so the gallery still works away from home.
    pub async fn screenshots(&self,id:i64)->Value{
        if let Ok(rom)=self.config.romm.get_rom(id).await{
            if rom.is_object()&&rom.as_object().is_some_and(|r|!r.is_empty()){
                let paths=screenshot_paths(&rom);let count=paths.len();
                self.shots.lock().unwrap_or_else(|e|e.into_inner()).insert(id,paths);
                return json!({"success":true,"count":count});
            }
        }
        let mut cached=0;
        while cached<MAX_SCREENSHOTS&&self.config.artwork.assets.exists(&self.shot_path(id,cached)).await{cached+=1;}
        if cached>0{return json!({"success":true,"count":cached});}
        failure("server_unreachable","Could not fetch screenshots from the server")
    }
    /// One screenshot as base64 with its MIME type; cache first, then a download from RomM.
    pub async fn screenshot(&self,id:i64,index:i64)->Value{
        let none=json!({"base64":null});
        let Some(index)=usize::try_from(index).ok().filter(|i|*i<MAX_SCREENSHOTS)else{return none};
        let art=&self.config.artwork;
        let cache=self.shot_path(id,index);
        if !art.assets.exists(&cache).await{
            let known=self.shots.lock().unwrap_or_else(|e|e.into_inner()).get(&id).cloned();
            let paths=match known{Some(p)=>p,None=>match self.config.romm.get_rom(id).await{Ok(rom)=>screenshot_paths(&rom),Err(_)=>return none}};
            let Some(url)=paths.get(index)else{return none};
            let _guard=art.lock.lock().await;
            if !art.assets.exists(&cache).await&&art.download_atomic(url,&cache).await.is_err(){return none;}
        }
        match art.assets.read(&cache).await{
            Ok(bytes)if !bytes.is_empty()=>json!({"base64":STANDARD.encode(&bytes),"mime":image_mime(&bytes)}),
            _=>none,
        }
    }
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
