//! Validates frontend acknowledgements and commits one dispatched unit exactly once.
use std::{collections::BTreeMap,sync::Arc};
use serde_json::{Value,json};
use tokio::sync::{Mutex,oneshot};
use crate::services::ports::{SyncLibraryRepo,SyncUnitCommit};

type Bindings=BTreeMap<i64,i64>;
pub type UnitFollowup=Arc<dyn Fn()->std::pin::Pin<Box<dyn std::future::Future<Output=()>+Send>>+Send+Sync>;
struct PendingUnit {
    run_id:String,unit_id:String,emitted:BTreeMap<i64,Value>,commit:SyncUnitCommit,
    completion:Option<oneshot::Sender<Bindings>>,followup:Option<UnitFollowup>,
}
pub struct SyncReporter {repo:Arc<dyn SyncLibraryRepo>,pending:Mutex<Option<PendingUnit>>,assets:Option<Arc<dyn crate::services::ports::SteamAssets>>,settings:crate::services::ports::SettingsStore}
impl SyncReporter {
    pub fn new(repo:Arc<dyn SyncLibraryRepo>)->Self {Self{repo,pending:Mutex::new(None),assets:None,settings:Default::default()}}
    pub fn with_assets(repo:Arc<dyn SyncLibraryRepo>,assets:Arc<dyn crate::services::ports::SteamAssets>,settings:crate::services::ports::SettingsStore)->Self {Self{repo,pending:Mutex::new(None),assets:Some(assets),settings}}
    pub async fn reset(&self) {*self.pending.lock().await=None;}
    pub async fn stage(&self,run_id:String,unit_id:String,emitted:Vec<Value>,commit:SyncUnitCommit)->oneshot::Receiver<Bindings> {
        self.stage_with_followup(run_id,unit_id,emitted,commit,None).await
    }
    pub async fn stage_with_followup(&self,run_id:String,unit_id:String,emitted:Vec<Value>,commit:SyncUnitCommit,followup:Option<UnitFollowup>)->oneshot::Receiver<Bindings> {
        let (tx,rx)=oneshot::channel();
        *self.pending.lock().await=Some(PendingUnit {run_id,unit_id,emitted:emitted.into_iter().filter_map(|v|Some((v["rom_id"].as_i64()?,v))).collect(),commit,completion:Some(tx),followup});
        rx
    }
    /// Explicit cancellation discards the unit. A heartbeat timeout leaves it
    /// staged so a late matching ack still persists shortcuts already created.
    pub async fn cancel_unit(&self,run_id:&str) {
        let mut pending=self.pending.lock().await;
        if pending.as_ref().is_some_and(|p|p.run_id==run_id) {*pending=None;}
    }
    pub async fn report(&self,mapping:&Value,run_id:&str,unit_id:&str)->anyhow::Result<Value> {
        // Serialize duplicate acknowledgements through the commit. No other unit
        // can replace the pending stash between identity validation and persistence.
        let mut guard=self.pending.lock().await;
        let Some(pending)=guard.as_mut().filter(|p|p.run_id==run_id && p.unit_id==unit_id) else {
            return Ok(json!({"success":true,"count":0,"ignored":true}));
        };
        let Some(mapping)=mapping.as_object() else {return Ok(crate::domain::errors::failure("invalid_args","Expected ROM to app ID mapping"));};
        let mut bindings=BTreeMap::new();let mut acknowledged=BTreeMap::new();
        for (id,app) in mapping {
            let Ok(id)=id.parse::<i64>() else{return Ok(crate::domain::errors::failure("invalid_args","Invalid ROM ID"));};
            let Some(app)=app.as_i64().filter(|a|*a>0) else{return Ok(crate::domain::errors::failure("invalid_args","Invalid app ID"));};
            let Some(entry)=pending.emitted.get(&id) else {return Ok(crate::domain::errors::failure("invalid_args","ROM was not dispatched in this unit"));};
            let target=entry["bind_rom_id"].as_i64().unwrap_or(id);
            bindings.insert(target,app);acknowledged.insert(id,app);
        }
        let mut commit=pending.commit.clone();commit.bindings=bindings;
        if let Some(assets)=&self.assets {
            let grid=assets.grid_dir().await.ok().flatten();
            for (id,app) in &acknowledged {
                let entry=&pending.emitted[id];let target=entry["bind_rom_id"].as_i64().unwrap_or(*id);
                if let Some(path)=entry["cover_path"].as_str().filter(|p|!p.is_empty()) {
                    if let Some(grid)=&grid {if assets.exists(path).await {if let Err(e)=assets.copy_atomic(path,&format!("{grid}/{app}p.png")).await{tracing::warn!("Cannot publish cover: {e}");}}}
                    commit.covers.insert(target,path.to_string());
                }
            }
        }
        let apps:Vec<_>=commit.bindings.values().copied().collect();
        self.repo.commit_unit(commit).await?;
        if let Some(assets)=&self.assets {let mode=self.settings.get_str("steam_input_mode").unwrap_or("default".into());if mode!="default" {if let Err(e)=assets.steam_input(&apps,&mode).await{tracing::warn!("Cannot set Steam Input: {e}");}}}
        if let Some(followup)=pending.followup.take(){followup().await;}
        if let Some(done)=pending.completion.take(){let _=done.send(acknowledged);}
        let count=mapping.len();*guard=None;
        Ok(json!({"success":true,"count":count}))
    }
}
#[async_trait::async_trait]
impl crate::services::ports::PendingRomReader for SyncReporter {
    async fn pending_rom(&self,id:i64)->Option<Value> {
        self.pending.lock().await.as_ref()?.emitted.get(&id).cloned()
    }
}
#[cfg(test)]
mod tests {
    use super::*;use async_trait::async_trait;
    #[derive(Default)]struct Repo {commits:std::sync::Mutex<Vec<SyncUnitCommit>>,fail:std::sync::atomic::AtomicBool}
    #[async_trait]impl SyncLibraryRepo for Repo {
        async fn registry(&self)->anyhow::Result<BTreeMap<i64,Value>>{Ok(BTreeMap::new())}
        async fn commit_unit(&self,c:SyncUnitCommit)->anyhow::Result<()> {
            if self.fail.load(std::sync::atomic::Ordering::SeqCst){anyhow::bail!("disk full");}
            self.commits.lock().unwrap().push(c);Ok(())
        }
    }
    fn commit()->SyncUnitCommit{SyncUnitCommit{built:vec![json!({"rom_id":2})],raw:vec![],bindings:BTreeMap::new(),covers:BTreeMap::new(),synced_at:"now".into(),cached_at:1.0}}
    #[tokio::test]
    async fn matching_ack_translates_rebind_and_duplicate_or_foreign_ack_cannot_commit() {
        use crate::services::ports::PendingRomReader;
        let repo=Arc::new(Repo::default());let reporter=SyncReporter::new(repo.clone());
        let rx=reporter.stage("run".into(),"unit".into(),vec![json!({"rom_id":1,"bind_rom_id":2})],commit()).await;
        assert_eq!(reporter.pending_rom(1).await,Some(json!({"rom_id":1,"bind_rom_id":2})));
        assert_eq!(reporter.pending_rom(2).await,None);
        assert_eq!(reporter.report(&json!({"1":123}),"old","unit").await.unwrap()["ignored"],true);
        assert_eq!(reporter.report(&json!({"1":123}),"run","other").await.unwrap()["ignored"],true);
        assert_eq!(reporter.report(&json!({"1":123}),"run","unit").await.unwrap()["count"],1);
        assert_eq!(rx.await.unwrap(),BTreeMap::from([(1,123)]));
        assert_eq!(repo.commits.lock().unwrap()[0].bindings,BTreeMap::from([(2,123)]));
        assert_eq!(reporter.report(&json!({"1":123}),"run","unit").await.unwrap()["ignored"],true);
        assert_eq!(repo.commits.lock().unwrap().len(),1);
        assert_eq!(reporter.pending_rom(1).await,None);
    }
    #[tokio::test]
    async fn timeout_late_ack_commits_but_cancel_and_new_run_invalidate_old_work() {
        let repo=Arc::new(Repo::default());let reporter=SyncReporter::new(repo.clone());
        drop(reporter.stage("run".into(),"unit".into(),vec![json!({"rom_id":2})],commit()).await);
        reporter.report(&json!({"2":123}),"run","unit").await.unwrap();
        assert_eq!(repo.commits.lock().unwrap().len(),1);
        let rx=reporter.stage("old".into(),"unit".into(),vec![],commit()).await;
        reporter.cancel_unit("foreign").await;
        reporter.cancel_unit("old").await;assert!(rx.await.is_err());
        let old=reporter.stage("old".into(),"unit".into(),vec![],commit()).await;
        let new=reporter.stage("new".into(),"unit".into(),vec![],commit()).await;assert!(old.await.is_err());
        assert_eq!(reporter.report(&json!({}),"old","unit").await.unwrap()["ignored"],true);
        reporter.report(&json!({}),"new","unit").await.unwrap();assert!(new.await.unwrap().is_empty());
    }
    #[tokio::test]
    async fn post_commit_adoption_runs_once_after_persistence_even_for_a_late_ack() {
        let repo=Arc::new(Repo::default());let reporter=SyncReporter::new(repo.clone());
        let count=Arc::new(std::sync::atomic::AtomicUsize::new(0));let observed=count.clone();let check=repo.clone();
        let followup:UnitFollowup=Arc::new(move||{let observed=observed.clone();let check=check.clone();Box::pin(async move{assert_eq!(check.commits.lock().unwrap().len(),1);observed.fetch_add(1,std::sync::atomic::Ordering::SeqCst);})});
        drop(reporter.stage_with_followup("run".into(),"unit".into(),vec![json!({"rom_id":2})],commit(),Some(followup)).await);
        repo.fail.store(true,std::sync::atomic::Ordering::SeqCst);assert!(reporter.report(&json!({"2":123}),"run","unit").await.is_err());assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst),0);
        repo.fail.store(false,std::sync::atomic::Ordering::SeqCst);reporter.report(&json!({"2":123}),"run","unit").await.unwrap();reporter.report(&json!({"2":123}),"run","unit").await.unwrap();assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst),1);
    }
    #[tokio::test]
    async fn invalid_mapping_and_failed_commit_never_signal_completion_and_can_retry() {
        let repo=Arc::new(Repo::default());let reporter=SyncReporter::new(repo.clone());
        let mut rx=reporter.stage("run".into(),"unit".into(),vec![json!({"rom_id":2})],commit()).await;
        for mapping in [json!([]),json!({"bad":123}),json!({"2":-1}),json!({"3":123})] {assert_eq!(reporter.report(&mapping,"run","unit").await.unwrap()["success"],false);}
        repo.fail.store(true,std::sync::atomic::Ordering::SeqCst);
        assert!(reporter.report(&json!({"2":123}),"run","unit").await.is_err());assert!(rx.try_recv().is_err());
        repo.fail.store(false,std::sync::atomic::Ordering::SeqCst);
        reporter.report(&json!({"2":123}),"run","unit").await.unwrap();assert_eq!(rx.await.unwrap(),BTreeMap::from([(2,123)]));
    }
}
