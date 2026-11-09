//! One sequential sync run: fetch, prepare, dispatch, acknowledged commit, finalize.
use std::{collections::{BTreeMap,BTreeSet,HashSet},sync::{Arc,Mutex}};
use serde_json::{Value,json};
use crate::domain::{errors::failure,shortcut_data::build_shortcuts_data,sibling_group::compute_component_group_keys,sync_diff::{collapse_sibling_groups,select_stale_removals},sync_run::SyncRun,work_unit::WorkUnit};
use crate::services::{adoption::InstallAdoptionService,launch::LaunchService,library_fetcher::{LibraryFetcher,FetchError},library_state::{LibraryRunState,CancelOutcome},ports::*,sync_reporter::SyncReporter};

pub struct SyncOrchestratorConfig {
    pub fetcher:Arc<LibraryFetcher>,pub state:Arc<LibraryRunState>,pub reporter:Arc<SyncReporter>,
    pub library:Arc<dyn LibraryRepo>,pub registry:Arc<dyn SyncLibraryRepo>,pub baseline:Arc<dyn LibraryBaselineRepo>,
    pub delta:Arc<dyn RommLibraryDelta>,pub finalizer:Arc<dyn SyncFinalizeRepo>,pub runs:Arc<dyn SyncRunRepo>,
    pub launch:Arc<LaunchService>,pub adoption:Arc<InstallAdoptionService>,pub events:Arc<dyn EventSink>,
    pub clock:Arc<dyn Clock>,pub ids:Arc<dyn SyncIds>,pub sleeper:Arc<dyn SyncSleeper>,pub settings:SettingsStore,pub plugin_dir:String,pub artwork:Arc<crate::services::artwork::ArtworkService>,
}
pub struct SyncOrchestrator {
    config:SyncOrchestratorConfig,progress:Mutex<Value>,heartbeat:Mutex<f64>,task:tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,preview:tokio::sync::Mutex<Option<(String,f64)>>,abandoned:std::sync::atomic::AtomicBool,
}
impl SyncOrchestrator {
    pub fn new(config:SyncOrchestratorConfig)->Arc<Self>{Arc::new(Self{config,progress:Mutex::new(json!({"running":false,"stage":"","current":0,"total":0,"message":"","step":0,"totalSteps":0,"runId":""})),heartbeat:Mutex::new(0.0),task:tokio::sync::Mutex::new(None),preview:tokio::sync::Mutex::new(None),abandoned:std::sync::atomic::AtomicBool::new(false)})}
    pub async fn artwork_base64(&self,id:i64)->Value {self.config.artwork.get_base64(id).await}
    pub fn get_sync_status(&self)->Value {self.progress.lock().unwrap().clone()}
    pub fn heartbeat(&self)->Value {*self.heartbeat.lock().unwrap()=self.config.clock.monotonic();json!({"success":true})}
    pub fn cancel(&self,run_id:Option<&str>)->Value {
        let message=match self.config.state.request_cancel(run_id){CancelOutcome::NoSync=>"No sync in progress",CancelOutcome::Stale=>"Cancel ignored (stale run)",CancelOutcome::Cancelling=>"Sync cancelling..."};json!({"success":true,"message":message})
    }
    fn progress(&self,run:&str,stage:&str,message:String,current:usize,total:usize,step:usize,steps:usize,running:bool) {
        let payload=json!({"running":running,"stage":stage,"current":current,"total":total,"message":message,"step":step,"totalSteps":steps,"runId":run});
        *self.progress.lock().unwrap()=payload.clone();self.config.events.emit("sync_progress",payload);
    }
    pub async fn start(self:&Arc<Self>)->Value {
        let mut task_slot=self.task.lock().await;
        let run=self.config.ids.next_id();
        if !self.config.state.try_begin_run(run.clone()){return failure("sync_in_progress","Sync already in progress");}
        self.config.reporter.reset().await;
        self.abandoned.store(false,std::sync::atomic::Ordering::SeqCst);
        self.heartbeat();let this=self.clone();
        let task=tokio::spawn(async move{this.run(run).await;});*task_slot=Some(task);
        json!({"success":true,"message":"Sync started"})
    }
    pub async fn report(&self,mapping:&Value,run_id:&str,unit_id:&str)->anyhow::Result<Value> {
        let abandoned=self.abandoned.load(std::sync::atomic::Ordering::SeqCst);
        let current=self.config.state.current_run_id();
        if (self.config.state.is_cancelling() && !abandoned) || current.as_deref().is_some_and(|id|id!=run_id) || (current.is_none() && !abandoned) {return Ok(json!({"success":true,"count":0,"ignored":true}));}
        self.config.reporter.report(mapping,run_id,unit_id).await
    }
    pub async fn cancel_preview(&self)->Value {*self.preview.lock().await=None;json!({"success":true})}
    pub async fn apply_preview(self:&Arc<Self>,id:&str)->Value {
        let mut preview=self.preview.lock().await;
        let Some((expected,created))=preview.as_ref().filter(|(expected,_)|expected==id) else{return failure("stale_preview","Preview expired, please re-sync");};
        let _=expected;
        if self.config.clock.epoch_seconds()-created>1800.0 {*preview=None;return failure("stale_preview","Preview is older than 30 minutes, please re-run sync");}
        let result=self.start().await;
        if result["success"]==true{*preview=None;return json!({"success":true,"message":"Applying changes"});}
        result
    }
    pub async fn preview(&self)->Value {
        let run=self.config.ids.next_id();
        if !self.config.state.try_begin_run(run.clone()){return failure("sync_in_progress","Sync already in progress");}
        self.heartbeat();self.progress(&run,"discovering","Fetching platforms...".into(),0,0,0,0,true);
        let result=self.preview_inner(&run).await;
        let response=match result {
            Ok(value)=>value,
            Err(error)=>{*self.preview.lock().await=None;let cancelled=self.config.state.is_cancelling();let message=if cancelled{"Sync cancelled".to_string()}else{error.to_string()};self.progress(&run,if cancelled{"cancelled"}else{"error"},message.clone(),0,0,0,0,false);if cancelled{failure("cancelled",&message)}else if let Some(FetchError::Server(server))=error.downcast_ref::<FetchError>(){server.to_failure()}else{failure("unknown",&message)}}
        };
        self.config.state.finish_run(Some(&run));response
    }
    async fn preview_inner(&self,run:&str)->anyhow::Result<Value> {
        let queue=self.config.fetcher.build_work_queue().await?;let mut seen=HashSet::new();let mut all=vec![];let mut platform_ids=BTreeSet::new();let mut memberships=BTreeMap::new();
        let names:BTreeMap<String,String>=queue.iter().filter_map(|u|match u{WorkUnit::Platform{name,slug,..}=>Some((slug.clone(),name.clone())),_=>None}).collect();
        for (index,unit) in queue.iter().enumerate(){
            if self.config.state.is_cancelling(){anyhow::bail!("Sync cancelled");}
            self.progress(run,"fetching",format!("Fetching {}... ({}/{})",unit.name(),index+1,queue.len()),all.len(),0,index+1,queue.len(),true);
            let raw=match unit {
                WorkUnit::Platform{..}=>self.config.fetcher.fetch_platform_unit(unit,self.config.baseline.as_ref(),self.config.delta.as_ref()).await?.0,
                _=>{let(raw,ids)=self.config.fetcher.fetch_unit(unit,&mut seen).await?;memberships.insert(unit.name().to_string(),ids);raw}
            };
            for rom in &raw{if let Some(id)=rom["id"].as_i64(){seen.insert(id);if matches!(unit,WorkUnit::Platform{..}){platform_ids.insert(id);}}}all.extend(raw);
        }
        let (built,installed,_)=self.prepare(&mut all,false).await?;
        let installed=installed.iter().filter_map(Value::as_i64).collect();
        let mut registry=self.config.registry.registry().await?;registry.retain(|_,r|r["app_id"].as_i64().is_some());
        for entry in registry.values_mut(){let slug=entry["platform_slug"].as_str().unwrap_or("");entry["platform_name"]=json!(names.get(slug).map(String::as_str).unwrap_or(slug));}
        let emitted=collapse_sibling_groups(&built,&registry,&installed,true,self.config.settings.get_str("preferred_region").as_deref().unwrap_or("auto")).map_err(anyhow::Error::msg)?;
        let classification=crate::domain::sync_diff::classify_roms(&emitted,&registry,&names.values().cloned().collect());
        let previous=self.config.runs.latest_completed().await?;
        let old_platforms=previous.as_ref().and_then(|r|r.platforms_completed.clone()).unwrap_or_default();let old_collections=previous.and_then(|r|r.collections_completed).unwrap_or_default();
        if self.config.state.is_cancelling(){anyhow::bail!("Sync cancelled");}
        let id=self.config.ids.next_id();*self.preview.lock().await=Some((id.clone(),self.config.clock.epoch_seconds()));self.progress(run,"done","Preview ready".into(),0,0,0,0,false);
        Ok(json!({"success":true,"summary":{"new_count":classification.new.len(),"changed_count":classification.changed.len(),"unchanged_count":classification.unchanged_ids.len(),"remove_count":classification.stale.len(),"disabled_platform_remove_count":classification.disabled_count,
            "collection_diff":crate::domain::sync_diff::collection_diff(&memberships,&old_collections),"platform_collection_diff":crate::domain::sync_diff::platform_collection_diff(&emitted,Some(&platform_ids),&old_platforms,self.config.settings.truthy("collection_create_platform_groups"))},
            "new_names":classification.new.iter().take(10).map(|r|r["name"].clone()).collect::<Vec<_>>(),"changed_names":classification.changed.iter().take(10).map(|r|r["name"].clone()).collect::<Vec<_>>(),"preview_id":id}))
    }
    pub async fn shutdown(&self) {
        self.config.state.request_cancel(None);
        if let Some(task)=self.task.lock().await.take(){let _=task.await;}
    }
    async fn prepare(&self,raw:&mut [Value],adopt:bool)->anyhow::Result<(Vec<Value>,Vec<Value>,Vec<crate::domain::install_layout::AdoptionCandidate>)> {
        let registry=self.config.registry.registry().await?;
        let resident=registry.iter().filter_map(|(id,v)|Some((*id,v["sibling_group_key"].as_str()?.to_string()))).collect();
        let keys=compute_component_group_keys(raw,&resident);
        for rom in raw.iter_mut(){if let Some(id)=rom["id"].as_i64(){if let Some(key)=keys.get(&id){rom["sibling_group_key"]=json!(key);}}}
        let mut installed=BTreeMap::new();
        for rom in raw.iter(){let Some(id)=rom["id"].as_i64()else{continue};if let Some(install)=self.config.library.install(id).await?{
            let row=self.config.library.rom(id).await?;let path=self.config.launch.resolve_bake_path(&install,row.as_ref().and_then(|r|r.selected_disc.as_deref())).await;installed.insert(id,path);
        }}
        let candidates=if adopt{self.config.adoption.plan_adoptions(raw).await}else{vec![]};
        for c in &candidates{installed.entry(c.rom_id).or_insert(c.bake_path.clone());}
        let mut invocations=BTreeMap::new();
        for rom in raw.iter(){if let Some(id)=rom["id"].as_i64(){if installed.contains_key(&id){let emulator=self.config.launch.active_emulator_for_rom(id,rom["platform_slug"].as_str()).await;let resolved=self.config.launch.resolve_launch(emulator.as_ref());if resolved.status=="ok"{invocations.insert(id,resolved.launch_options);}}}}
        let built=build_shortcuts_data(raw,&self.config.plugin_dir,&installed,&invocations).map_err(anyhow::Error::msg)?;
        // The caller sets completeness: collection units must not rebind unseen siblings.
        Ok((built,installed.keys().map(|id|json!(id)).collect(),candidates))
    }
    async fn run(self:Arc<Self>,run_id:String) {
        let result=self.run_inner(&run_id).await;
        if let Err(error)=result {
            let message=error.to_string();
            if let Ok(Some(mut run))=self.config.runs.get(&run_id).await{if run.mark_errored(self.config.clock.now_iso(),message).is_ok(){let _=self.config.runs.save(&run).await;}}
            self.config.reporter.cancel_unit(&run_id).await;
            self.config.state.finish_run(Some(&run_id));
            self.progress(&run_id,"error",format!("Sync failed — {error}"),0,0,0,0,false);
        }
        self.config.state.finish_run(Some(&run_id));
    }
    async fn run_inner(&self,run_id:&str)->anyhow::Result<()> {
        let queue=self.config.fetcher.build_work_queue().await?;let total_roms:i64=queue.iter().map(WorkUnit::count).sum();
        self.config.events.emit("sync_plan",json!({"run_id":run_id,"units":queue,"total_units":queue.len(),"total_roms":total_roms}));
        if queue.is_empty(){self.progress(run_id,"done","Nothing to sync".into(),0,0,0,0,false);return Ok(());}
        let mut run=SyncRun::start(run_id.into(),self.config.clock.now_iso(),queue.iter().filter(|u|matches!(u,WorkUnit::Platform{..})).count() as i64,total_roms).map_err(anyhow::Error::msg)?;
        self.config.runs.save(&run).await?;
        let mut seen=HashSet::new();let mut platform_ids=BTreeSet::new();let mut memberships=BTreeMap::new();let mut committed=BTreeSet::new();let mut total_applied=0;let mut abandoned=false;
        let names:BTreeMap<String,String>=queue.iter().filter_map(|u|match u{WorkUnit::Platform{name,slug,..}=>Some((slug.clone(),name.clone())),_=>None}).collect();
        for (index,unit) in queue.iter().enumerate(){
            if self.config.state.is_cancelling(){break;}
            self.progress(run_id,"applying",format!("{} ({}/{})",unit.name(),index+1,queue.len()),0,0,index+1,queue.len(),true);
            let fetched=match unit{
                WorkUnit::Platform{..}=>self.config.fetcher.fetch_platform_unit(unit,self.config.baseline.as_ref(),self.config.delta.as_ref()).await,
                _=>self.config.fetcher.fetch_unit(unit,&mut seen).await.map(|(roms,ids)|{memberships.insert(unit.name().to_string(),ids);(roms,false)}),
            };
            let (mut raw,skipped)=match fetched{Err(FetchError::Cancelled)=>break,other=>other?};
            for rom in &raw{if let Some(id)=rom["id"].as_i64(){seen.insert(id);if matches!(unit,WorkUnit::Platform{..}){platform_ids.insert(id);}}}
            if self.config.state.is_cancelling(){break;}
            if skipped{total_applied+=raw.len();continue;}
            let (built,installed,candidates)=self.prepare(&mut raw,true).await?;
            let installed=installed.iter().filter_map(Value::as_i64).collect();let registry=self.config.registry.registry().await?;
            let mut emitted=collapse_sibling_groups(&built,&registry,&installed,matches!(unit,WorkUnit::Platform{..}),self.config.settings.get_str("preferred_region").as_deref().unwrap_or("auto")).map_err(anyhow::Error::msg)?;
            if self.config.state.is_cancelling(){break;}
            let artwork_ids:BTreeSet<_>=emitted.iter().filter_map(|e|e["bind_rom_id"].as_i64().or_else(||e["rom_id"].as_i64())).collect();
            let art_roms:Vec<_>=raw.iter().filter(|r|r["id"].as_i64().is_some_and(|id|artwork_ids.contains(&id))).cloned().collect();
            let covers=self.config.artwork.download(&art_roms,&self.config.state,&|current,total|self.progress(run_id,"applying",format!("Downloading artwork {current}/{total}"),current,total,index+1,queue.len(),true)).await;
            for entry in &mut emitted {let target=entry["bind_rom_id"].as_i64().or_else(||entry["rom_id"].as_i64()).unwrap();entry["cover_path"]=json!(covers.get(&target).cloned().unwrap_or_default());}
            if self.config.state.is_cancelling(){break;}
            let commit=SyncUnitCommit{built,raw,bindings:BTreeMap::new(),covers:BTreeMap::new(),synced_at:self.config.clock.now_iso(),cached_at:self.config.clock.epoch_seconds()};
            let id=unit.id_value();let id_string=id.as_str().map(str::to_string).unwrap_or_else(||id.to_string());
            let adopter=self.config.adoption.clone();
            let followup:crate::services::sync_reporter::UnitFollowup=Arc::new(move||{let adopter=adopter.clone();let candidates=candidates.clone();Box::pin(async move{adopter.commit_adoptions(&candidates).await;})});
            let mut ack=self.config.reporter.stage_with_followup(run_id.into(),id_string,emitted.clone(),commit,Some(followup)).await;
            self.heartbeat();self.config.events.emit("sync_apply_unit",json!({"run_id":run_id,"unit_type":if matches!(unit,WorkUnit::Platform{..}){"platform"}else{"collection"},"unit_id":id,"unit_name":unit.name(),"unit_index":index,"total_units":queue.len(),"shortcuts":emitted}));
            let applied=loop{
                match ack.try_recv(){Ok(mapping)=>break Some(mapping),Err(tokio::sync::oneshot::error::TryRecvError::Closed)=>anyhow::bail!("Unit acknowledgement closed"),_=>{}}
                if self.config.state.is_cancelling(){self.config.reporter.cancel_unit(run_id).await;break None;}
                if self.config.clock.monotonic()-*self.heartbeat.lock().unwrap()>60.0{abandoned=true;self.abandoned.store(true,std::sync::atomic::Ordering::SeqCst);self.config.state.request_cancel(None);break None;}
                self.config.sleeper.sleep(0.5).await;
            };
            if let Some(applied)=applied{total_applied+=applied.len();committed.extend(applied.values());}else{break;}
        }
        let cancelled=self.config.state.is_cancelling();
        if !cancelled{self.progress(run_id,"finalizing","Finalizing…".into(),0,0,queue.len(),queue.len(),true);}
        let registry=self.config.registry.registry().await?;
        // A partial collection view covers the game's existing shortcut even
        // when it fetched only an unbound version. Absence of the bound sibling
        // from that view cannot establish that the shortcut is stale.
        let covered_groups:BTreeSet<_>=seen.iter().filter_map(|id|registry.get(id)?["sibling_group_key"].as_str().map(str::to_string)).collect();
        for (id,row) in &registry {if row["sibling_group_key"].as_str().is_some_and(|key|covered_groups.contains(key)){seen.insert(*id);}}
        let stale=if cancelled{vec![]}else{select_stale_removals(&registry.iter().filter_map(|(id,r)|{if seen.contains(id){None}else{Some((*id,r["app_id"].as_i64()?))}}).collect::<Vec<_>>(),&committed)};
        self.config.events.emit("sync_stale",json!({"remove":stale.iter().map(|(id,app)|json!({"rom_id":id,"app_id":app})).collect::<Vec<_>>()}));
        let maps=self.config.finalizer.finalize(SyncFinalization{stale_ids:stale.iter().map(|(id,_)|*id).collect(),platform_names:names,platform_rom_ids:platform_ids,memberships,create_platform_groups:self.config.settings.truthy("collection_create_platform_groups")}).await?;
        let total=self.config.library.bound_rom_count().await? as usize;
        if cancelled{run.mark_cancelled(self.config.clock.now_iso(),"Sync cancelled".into())}else{run.complete(self.config.clock.now_iso(),maps["platform_app_ids"].as_object().unwrap().keys().cloned().collect(),maps["romm_collection_app_ids"].as_object().unwrap().keys().cloned().collect())}.map_err(anyhow::Error::msg)?;
        self.config.runs.save(&run).await?;
        self.config.state.finish_run(Some(run_id));
        self.config.events.emit("sync_collections",maps.clone());let mut payload=maps.clone();payload["total_games"]=json!(total_applied);if cancelled{payload["cancelled"]=json!(true);}self.config.events.emit("sync_complete",payload);
        self.progress(run_id,if cancelled{"cancelled"}else{"done"},if cancelled{format!("Sync cancelled: {total_applied} of {total} games processed")}else{format!("Sync complete: {total} games from {} platforms",maps["platform_app_ids"].as_object().map_or(0,|m|m.len()))},if cancelled{total_applied}else{total},total,0,0,false);
        if !abandoned{self.config.reporter.cancel_unit(run_id).await;}
        Ok(())
    }
}
