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
        if let Some((at,value))=&*cache{
            // A forced check cannot succeed before GitHub's reset either, so a rate limit is answered from the cache.
            if (!force||value["reason"]=="rate_limited")&&self.c.clock.epoch_seconds()-at<Self::ttl(value){return value.clone();}
        }
        let result=self.run_check().await;*cache=Some((self.c.clock.epoch_seconds(),result.clone()));result
    }
    /// Successes are kept 6 h, failures 15 min; a rate limit until GitHub says it lifts (1 min to 1 h).
    fn ttl(v:&Value)->f64{
        if v["success"]==true{return 21600.0;}
        if v["reason"]=="rate_limited"{if let Some(s)=v["retry_after_seconds"].as_f64(){return s.clamp(60.0,3600.0);}}
        900.0
    }
    fn rate_limited(&self,retry:Option<u64>,current:&str)->Value{
        let message=match retry{Some(s)=>format!("GitHub rate limit reached — try again in about {} min",s.div_ceil(60).max(1)),None=>"GitHub rate limit reached — try again later".into()};
        let mut v=self.fail("rate_limited",&message,current);if let Some(s)=retry{v["retry_after_seconds"]=json!(s);}v
    }
    fn fail(&self,reason:&str,message:&str,current:&str)->Value{let mut v=failure(reason,message);v["current_version"]=json!(current);v["checked_at"]=json!(self.c.clock.now_iso());v}
    async fn run_check(&self)->Value{
        let (current,name)=match self.c.metadata.metadata().await{Ok(m)=>m,Err(_)=>return self.fail("unknown","Could not read the installed plugin version","0.0.0")};
        // Routine checks stay off the rate-limited API: the web redirect names the newest tag, and only a newer one
        // needs the API (asset digest, notes). A redirect that fails or looks wrong falls back to the API.
        if let Ok(tag)=self.c.feed.latest_tag().await{
            if let Some(latest)=self_update::release_version(&json!({"tag_name":tag})){
                if !self_update::is_newer(&latest,&current){
                    return json!({"success":true,"update_available":false,"latest_version":latest,"notes":"","asset_url":"","sha256":"","html_url":self_update::RELEASES,"current_version":current,"plugin_name":name,"checked_at":self.c.clock.now_iso()});
                }
            }
        }
        let raw=match self.c.feed.latest().await{
            Ok(v)=>v,
            Err(ReleaseError::RateLimited(retry))=>return self.rate_limited(retry,&current),
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
