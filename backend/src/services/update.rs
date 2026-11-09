//! Throttled update checks, cached separately for successful and failed fetches.
use std::sync::Arc;
use serde_json::{Value,json};
use tokio::{sync::Mutex,task::JoinSet};
use crate::{domain::{self_update,errors::failure},services::ports::{Clock,ReleaseFeed,ReleaseError,PluginMetadata}};
pub struct UpdateConfig{pub feed:Arc<dyn ReleaseFeed>,pub metadata:Arc<dyn PluginMetadata>,pub clock:Arc<dyn Clock>}
pub struct UpdateService{c:UpdateConfig,cache:Mutex<Option<(f64,Value)>>,tasks:Mutex<JoinSet<()>>}
impl UpdateService{
    pub fn new(c:UpdateConfig)->Arc<Self>{Arc::new(Self{c,cache:Mutex::new(None),tasks:Mutex::new(JoinSet::new())})}
    pub async fn check(&self,force:bool)->Value{
        let mut cache=self.cache.lock().await;
        if !force {if let Some((at,value))=&*cache{let ttl=if value["success"]==true{21600.0}else{900.0};if self.c.clock.epoch_seconds()-at<ttl{return value.clone();}}}
        let result=self.run_check().await;*cache=Some((self.c.clock.epoch_seconds(),result.clone()));result
    }
    fn fail(&self,reason:&str,message:&str,current:&str)->Value{let mut v=failure(reason,message);v["current_version"]=json!(current);v["checked_at"]=json!(self.c.clock.now_iso());v}
    async fn run_check(&self)->Value{
        let (current,name)=match self.c.metadata.metadata().await{Ok(m)=>m,Err(_)=>return self.fail("unknown","Could not read the installed plugin version","0.0.0")};
        let raw=match self.c.feed.latest().await{
            Ok(v)=>v,
            Err(ReleaseError::RateLimited)=>return self.fail("rate_limited","GitHub rate limit reached — try again later",&current),
            Err(ReleaseError::BadResponse)=>return self.fail("unknown","GitHub returned an unreadable response",&current),
            Err(ReleaseError::Unreachable)=>return self.fail("server_unreachable","Could not reach GitHub — check your internet connection",&current),
        };
        let mut v=self_update::parse(&raw).unwrap_or(json!({"latest_version":self_update::release_version(&raw),"notes":"","asset_url":"","sha256":"","html_url":self_update::RELEASES}));
        v["update_available"]=json!(!v["asset_url"].as_str().unwrap_or("").is_empty()&&self_update::is_newer(v["latest_version"].as_str().unwrap_or(""),&current));
        v["success"]=json!(true);v["current_version"]=json!(current);v["plugin_name"]=json!(name);v["checked_at"]=json!(self.c.clock.now_iso());v
    }
    pub async fn schedule(self:&Arc<Self>){let mut tasks=self.tasks.lock().await;while tasks.try_join_next().is_some(){}let s=self.clone();tasks.spawn(async move{s.check(false).await;});}
    pub async fn shutdown(&self){let mut tasks=self.tasks.lock().await;tasks.abort_all();while tasks.join_next().await.is_some(){}}
}
