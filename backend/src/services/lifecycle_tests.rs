//! Retained step-3b contracts over real SQLite and files; only outer edges fake.
use std::{sync::{Arc,Mutex},collections::{HashSet,VecDeque}};
use async_trait::async_trait;
use serde_json::{Value,json};
use super::{ports::*,testing::*,versions::*,launch_gate::*,playtime::*,update::*};
use crate::{adapters::{versions::SqliteVersions,playtime::SqlitePlaytime,switch_save_safety::SwitchSaveSafety},domain::errors::RommError};

#[derive(Default)]
struct Saves {status:Mutex<Value>,fail:Mutex<bool>,tracked:Mutex<bool>,drift:Mutex<bool>,exit:Mutex<Value>}
#[async_trait]impl SaveSafety for Saves {
    async fn status(&self,_:i64)->anyhow::Result<Value>{if *self.fail.lock().unwrap(){anyhow::bail!("offline")};Ok(self.status.lock().unwrap().clone())}
    async fn tracked(&self,_:i64)->anyhow::Result<bool>{Ok(*self.tracked.lock().unwrap())}
    async fn drift(&self,_:i64)->anyhow::Result<bool>{if *self.fail.lock().unwrap(){anyhow::bail!("offline")};Ok(*self.drift.lock().unwrap())}
    async fn core_change(&self,_:i64)->anyhow::Result<Value>{if *self.fail.lock().unwrap(){anyhow::bail!("offline")};Ok(json!({"changed":true}))}
    async fn post_exit(&self,_:i64)->anyhow::Result<Value>{if *self.fail.lock().unwrap(){anyhow::bail!("offline")};Ok(self.exit.lock().unwrap().clone())}
}
fn versions(e:&DownloadEnv,s:Arc<Saves>,active:Arc<Mutex<HashSet<i64>>>)->VersionService {
    VersionService::new(VersionServiceConfig{repo:Arc::new(SqliteVersions::new(e.db.clone())),romm:e.romm.clone(),settings:e.settings.clone(),launch:e.launch.clone(),saves:s,clock:e.clock.clone(),mutation_lock:e.svc.mutation_lock.clone(),reachable:Arc::new(FakeRomm::new(e.settings.clone())),active:Arc::new(move||active.lock().unwrap().clone())})
}
async fn install(e:&DownloadEnv,id:i64,name:&str) {
    let path=e.roms().join("switch").join(name);std::fs::create_dir_all(path.parent().unwrap()).unwrap();std::fs::write(&path,b"rom").unwrap();
    e.db.call(move|c|c.execute("INSERT INTO rom_installs (rom_id,file_path,platform_slug,system,installed_at) VALUES (?1,?2,'switch','switch','t')",rusqlite::params![id,path.to_str().unwrap()])).await.unwrap();
}
#[tokio::test]async fn picker_offline_local_dimensions_and_narrow_binding_preserve_children(){
    let e=DownloadEnv::new().await;e.seed_rom(1,"switch","Game (US).xci",Some(99),Some("igdb:7:5")).await;e.seed_rom(2,"switch","Game (EU).nsp",None,Some("igdb:7:5")).await;
    install(&e,1,"Game (US).xci").await;install(&e,2,"Game (EU).nsp").await;
    e.sql("UPDATE roms SET regions='[\"Europe\"]',languages='[\"en\"]',revision='rev2',tags='[\"demo\"]',emulator_override='eden',selected_disc='Game (EU).nsp' WHERE rom_id=2; INSERT INTO rom_playtime(rom_id,total_seconds,session_count) VALUES(2,42,1);").await;
    let repo=SqliteVersions::new(e.db.clone());let s=versions(&e,Arc::new(Saves::default()),Arc::default());
    assert_eq!(s.list(0).await.unwrap(),json!({"multi_version":false}));
    let list=s.list(99).await.unwrap();assert_eq!(list["server_query_failed"],true);assert_eq!(list["versions"][1]["revision"],"rev2");assert_eq!(list["versions"][1]["installed"],true);
    let r=s.switch(99,2,false).await.unwrap();assert_eq!(r["success"],true);assert_eq!(r["app_id"],99);assert!(r["launch_options"].as_str().unwrap().contains("Game (EU).nsp"));
    assert!(repo.member(1).await.unwrap().unwrap()["app_id"].is_null());let row=repo.member(2).await.unwrap().unwrap();assert_eq!(row["emulator_override"],"eden");assert_eq!(row["selected_disc"],"Game (EU).nsp");
    assert_eq!(SqlitePlaytime::new(e.db.clone()).get(2).await.unwrap().unwrap().total_seconds,42);
    assert_eq!(s.switch(99,2,false).await.unwrap(),r);assert_eq!(s.switch(0,2,false).await.unwrap()["reason"],"not_found");
}
#[tokio::test]async fn picker_server_order_metadata_guards_and_inserted_version_survive_restart(){
    let e=DownloadEnv::new().await;e.seed_rom(1,"switch","Game.xci",Some(99),Some("igdb:7:5")).await;
    e.romm.set_rom(json!({"id":1,"sibling_roms":[{"id":9,"name":"Nine"},{"id":3,"name":"Three"},{"id":9},{"id":0}]}));
    e.romm.set_rom(json!({"id":9,"platform_slug":"switch","platform_id":5,"igdb_id":7,"name":"Nine","fs_name":"Nine.nsp","regions":["Europe"],"revision":"r2","rom_user":{"is_main_sibling":true}}));
    e.romm.set_rom(json!({"id":3,"igdb_id":999}));
    let s=versions(&e,Arc::new(Saves::default()),Arc::default());let l=s.list(99).await.unwrap();assert_eq!(l["versions"][1]["rom_id"],9);assert_eq!(l["versions"][1]["revision"],"r2");assert_eq!(l["versions"][2]["switchable"],false);
    assert_eq!(s.switch(99,3,false).await.unwrap()["reason"],"not_in_group");assert_eq!(s.switch(99,4,false).await.unwrap()["reason"],"server_unreachable");
    let r=s.switch(99,9,false).await.unwrap();assert_eq!(r,json!({"success":true,"rom_id":9,"app_id":99,"target_installed":false,"launch_options":""}));
    let fresh=SqliteVersions::new(e.db.clone());let g=fresh.group(99).await.unwrap().unwrap();assert_eq!(g.bound["rom_id"],9);assert_eq!(g.bound["regions"],json!(["Europe"]));assert_eq!(g.bound["is_main_sibling"],true);assert_eq!(g.bound["sibling_group_key"],"igdb:7:5");
}
#[tokio::test]async fn version_guards_downloads_drift_other_binding_and_stale_snapshot(){
    let e=DownloadEnv::new().await;for (id,app,key) in [(1,Some(99),"igdb:7:5"),(2,None,"igdb:7:5"),(3,Some(100),"igdb:7:5"),(4,None,"igdb:8:5")]{e.seed_rom(id,"switch","Game.nsp",app,Some(key)).await;}install(&e,1,"Game.nsp").await;
    let saves=Arc::new(Saves::default());*saves.drift.lock().unwrap()=true;let active=Arc::new(Mutex::new(HashSet::from([2])));let s=versions(&e,saves,active.clone());
    assert_eq!(s.switch(99,2,false).await.unwrap()["reason"],"download_in_progress");active.lock().unwrap().clear();
    assert_eq!(s.switch(99,3,false).await.unwrap()["reason"],"bound_elsewhere");assert_eq!(s.switch(99,4,false).await.unwrap()["reason"],"not_in_group");
    let r=s.switch(99,2,false).await.unwrap();assert_eq!(r["reason"],"unsynced_saves");assert_eq!(r["unsynced_rom_id"],1);
    let repo=SqliteVersions::new(e.db.clone());let snapshot=repo.group(99).await.unwrap().unwrap();e.sql("UPDATE roms SET selected_disc='changed.nsp' WHERE rom_id=2").await;
    assert_eq!(repo.bind_version(&snapshot,2,None,"t").await.unwrap()["reason"],"state_changed");assert_eq!(repo.group(99).await.unwrap().unwrap().bound["rom_id"],1);
    assert_eq!(s.switch(99,2,true).await.unwrap()["success"],true);
}
#[tokio::test]async fn launch_gate_conflicts_disabled_and_failed_status_with_tracked_saves(){
    let e=DownloadEnv::new().await;e.seed_rom(1,"switch","Game.nsp",Some(99),None).await;let saves=Arc::new(Saves::default());
    let g=LaunchGate::new(LaunchGateConfig{repo:Arc::new(SqliteVersions::new(e.db.clone())),saves:saves.clone(),settings:e.settings.clone()});
    assert_eq!(g.evaluate(0).await.unwrap()["action"],"allow");assert_eq!(g.evaluate(99).await.unwrap()["reason"],"not_installed");install(&e,1,"Game.nsp").await;
    *saves.status.lock().unwrap()=json!({"conflicts":[{}]});assert_eq!(g.evaluate(99).await.unwrap()["action"],"allow");e.settings.set("save_sync_enabled",json!(true));assert_eq!(g.evaluate(99).await.unwrap()["reason"],"save_conflict");
    *saves.status.lock().unwrap()=json!({"files":[{"status":"conflict"}],"conflicts":[]});assert_eq!(g.evaluate(99).await.unwrap()["action"],"allow");
    *saves.fail.lock().unwrap()=true;assert_eq!(g.evaluate(99).await.unwrap()["action"],"allow");*saves.tracked.lock().unwrap()=true;assert_eq!(g.evaluate(99).await.unwrap()["action"],"warn");
    assert_eq!(g.drift(1).await,json!({"rom_id":1,"drifted":false}));assert_eq!(g.core_change(1).await,json!({"changed":false}));
}
#[tokio::test]async fn standalone_save_boundary_never_guesses_paths(){
    let e=DownloadEnv::new().await;e.seed_rom(1,"switch","Game.nsp",Some(99),None).await;let s=SwitchSaveSafety::new(e.db.clone(),e.settings.clone());
    assert!(s.status(1).await.is_err());assert!(!s.tracked(1).await.unwrap());assert!(!s.drift(1).await.unwrap());assert_eq!(s.core_change(1).await.unwrap(),json!({"changed":false}));assert_eq!(s.post_exit(1).await.unwrap()["success"],true);
    e.settings.set("save_sync_enabled",json!(true));assert_eq!(s.post_exit(1).await.unwrap()["reason"],"unsupported");
    e.sql("INSERT INTO rom_save_states(rom_id,slots) VALUES(1,'{\"a\":{}}')").await;assert!(s.tracked(1).await.unwrap());
}

#[derive(Default)]struct Time(Mutex<(f64,f64)>);
impl Time{fn at(&self,wall:f64,mono:f64){*self.0.lock().unwrap()=(wall,mono);}}
impl Clock for Time {
    fn epoch_seconds(&self)->f64{self.0.lock().unwrap().0}
    fn monotonic(&self)->f64{self.0.lock().unwrap().1}
    fn now_iso(&self)->String{crate::domain::download_failure::iso_from_epoch(1767225600+self.epoch_seconds()as u64)}
}
#[derive(Default)]struct Sessions{responses:Mutex<VecDeque<Result<Value,PlaytimeApiError>>>,calls:Mutex<Vec<(String,Vec<Value>)>>,rows:Mutex<Vec<Value>>,forbidden:Mutex<bool>}
#[async_trait]impl RommPlaytimeApi for Sessions{
    async fn ingest(&self,d:&str,rows:Vec<Value>)->Result<Value,PlaytimeApiError>{self.calls.lock().unwrap().push((d.into(),rows.clone()));self.responses.lock().unwrap().pop_front().unwrap_or_else(||Ok(json!({"results":(0..rows.len()).map(|i|json!({"index":i,"status":"created"})).collect::<Vec<_>>()})))}
    async fn sessions(&self,_:i64)->Result<Vec<Value>,RommError>{if *self.forbidden.lock().unwrap(){Err(RommError::Forbidden("missing scope".into()))}else{Ok(self.rows.lock().unwrap().clone())}}
}
fn playtime(e:&DownloadEnv,api:Arc<Sessions>,time:Arc<Time>,saves:Arc<Saves>)->Arc<PlaytimeService>{PlaytimeService::new(PlaytimeServiceConfig{repo:Arc::new(SqlitePlaytime::new(e.db.clone())),api,kv:Arc::new(crate::adapters::repositories::SqliteKvConfig::new(e.db.clone())),clock:time,saves})}
async fn pending(e:&DownloadEnv,count:i64){e.db.call(move|c|{for i in 0..count{c.execute("INSERT INTO rom_playtime_sessions(rom_id,start_time,device_id,end_time,duration_ms) VALUES(1,?1,'old-device','end',1000)",[format!("s{i:03}")])?;}Ok(())}).await.unwrap();}
#[tokio::test]async fn local_session_fold_and_outbox_are_atomic_restart_safe_and_suspend_aware(){
    let e=DownloadEnv::new().await;e.seed_rom(1,"switch","G.nsp",None,None).await;let repo=SqlitePlaytime::new(e.db.clone());let api=Arc::new(Sessions::default());let t=Arc::new(Time::default());let s=playtime(&e,api.clone(),t.clone(),Arc::default());
    assert_eq!(s.start(999).await.unwrap()["reason"],"unknown_rom");assert_eq!(s.end(1).await.unwrap()["reason"],"no_active_session");e.sql("INSERT INTO kv_config(key,value) VALUES('device_id','device')").await;
    t.at(0.0,100.0);assert_eq!(s.start(1).await.unwrap()["success"],true);t.at(3600.0,160.0);
    api.responses.lock().unwrap().push_back(Err(RommError::Connection("offline".into()).into()));
    let r=s.end(1).await.unwrap();assert_eq!(r["duration_sec"],60);assert_eq!(r["session_count"],1);assert!(repo.get(1).await.unwrap().unwrap().last_start.is_none());assert_eq!(repo.pending(100).await.unwrap()[0].duration_ms,60000);
    let fresh=playtime(&e,api,t.clone(),Arc::default());fresh.flush().await;assert!(repo.pending(100).await.unwrap().is_empty());assert_eq!(fresh.all().await.unwrap()["playtime"]["1"]["total_seconds"],60);
    t.at(4000.0,200.0);s.start(1).await.unwrap();e.sql("CREATE TRIGGER reject_fold BEFORE UPDATE ON rom_playtime BEGIN SELECT RAISE(ABORT,'disk full'); END;").await;t.at(4010.0,210.0);assert!(s.end(1).await.is_err());assert!(repo.pending(100).await.unwrap().is_empty());assert!(repo.get(1).await.unwrap().unwrap().last_start.is_some());e.sql("DROP TRIGGER reject_fold").await;
    assert_eq!(s.end(1).await.unwrap()["total_seconds"],70);
}
#[tokio::test]async fn unregistered_and_subsecond_sessions_remain_local_and_bad_markers_survive(){
    let e=DownloadEnv::new().await;e.seed_rom(1,"switch","G.nsp",None,None).await;let repo=SqlitePlaytime::new(e.db.clone());
    repo.begin(1,"2026-01-01T00:00:00Z",0.0).await.unwrap();assert_eq!(repo.finish(1,"2026-01-01T00:00:10Z",10.0).await.unwrap()["duration_sec"],10);assert!(repo.pending(100).await.unwrap().is_empty());
    e.sql("INSERT INTO kv_config(key,value) VALUES('device_id','d')").await;repo.begin(1,"2026-01-01T00:00:10.1Z",10.1).await.unwrap();assert_eq!(repo.finish(1,"2026-01-01T00:00:10.9Z",10.9).await.unwrap()["duration_sec"],0);assert!(repo.pending(100).await.unwrap().is_empty());
    repo.begin(1,"bad",0.0).await.unwrap();assert_eq!(repo.finish(1,"2026-01-01T00:00:11Z",11.0).await.unwrap()["reason"],"unknown");assert_eq!(repo.get(1).await.unwrap().unwrap().last_start.as_deref(),Some("bad"));
}
#[tokio::test]async fn flush_correlates_indices_and_retries_only_acknowledged_errors(){
    let e=DownloadEnv::new().await;e.seed_rom(1,"switch","G.nsp",None,None).await;pending(&e,5).await;let api=Arc::new(Sessions::default());
    api.responses.lock().unwrap().push_back(Ok(json!({"results":[{"index":2,"status":"skipped"},{"index":0,"status":"duplicate"},{"index":1,"status":"error"},{"index":3,"status":"future-status"},{"index":99,"status":"created"},{"status":"created"}]})));
    let s=playtime(&e,api.clone(),Arc::default(),Arc::default());s.flush().await;let repo=SqlitePlaytime::new(e.db.clone());let rows=repo.pending(100).await.unwrap();assert_eq!(rows.iter().map(|r|(r.start.as_str(),r.attempts)).collect::<Vec<_>>(),vec![("s001",1),("s003",1),("s004",0)]);
    e.sql("UPDATE rom_playtime_sessions SET attempts=4").await;api.responses.lock().unwrap().push_back(Ok(json!({"results":[{"index":0,"status":"error"},{"index":1,"status":"unknown"}]})));s.flush().await;let rows=repo.pending(100).await.unwrap();assert_eq!(rows.len(),1);assert_eq!(rows[0].start,"s004");assert_eq!(rows[0].attempts,4);
    s.flush().await;assert!(repo.pending(100).await.unwrap().is_empty());assert_eq!(api.calls.lock().unwrap()[0].0,"old-device");
}
#[tokio::test]async fn indexed_422_heals_repeated_poison_and_unindexed_422_isolates_each_row(){
    for indexed in [true,false]{let e=DownloadEnv::new().await;e.seed_rom(1,"switch","G.nsp",None,None).await;pending(&e,3).await;let api=Arc::new(Sessions::default());
        {let mut r=api.responses.lock().unwrap();r.push_back(Err(PlaytimeApiError::Rejected(if indexed{json!([{"loc":["body","sessions",0]}])}else{Value::Null})));r.push_back(Err(PlaytimeApiError::Rejected(json!([{"loc":["body","sessions",0]}]))));if !indexed{r.push_back(Err(RommError::Connection("offline".into()).into()));}}
        playtime(&e,api.clone(),Arc::default(),Arc::default()).flush().await;let rows=SqlitePlaytime::new(e.db.clone()).pending(100).await.unwrap();
        if indexed{assert!(rows.is_empty());assert_eq!(api.calls.lock().unwrap().iter().map(|c|c.1.len()).collect::<Vec<_>>(),vec![3,2,1]);}else{assert_eq!(rows.len(),1);assert_eq!(rows[0].start,"s001");assert_eq!(rows[0].attempts,0);}
    }
}
#[tokio::test]async fn outbox_bounded_device_grouping_and_conditional_ack_protect_new_recording(){
    let e=DownloadEnv::new().await;e.seed_rom(1,"switch","G.nsp",None,None).await;pending(&e,102).await;e.sql("UPDATE rom_playtime_sessions SET device_id='other-device' WHERE start_time='s001'").await;
    let repo=SqlitePlaytime::new(e.db.clone());let stale=repo.pending(1).await.unwrap().remove(0);e.sql("UPDATE rom_playtime_sessions SET duration_ms=2000 WHERE start_time='s000'").await;repo.apply_outcomes(vec![(stale,true)]).await.unwrap();assert_eq!(repo.pending(1).await.unwrap()[0].duration_ms,2000);
    let api=Arc::new(Sessions::default());let s=playtime(&e,api.clone(),Arc::default(),Arc::default());tokio::join!(s.flush(),s.flush());assert!(repo.pending(200).await.unwrap().is_empty());let calls=api.calls.lock().unwrap();assert_eq!(calls.iter().map(|c|c.1.len()).sum::<usize>(),102);assert_eq!(calls.len(),3);assert_eq!(calls[1].0,"other-device");
}
#[tokio::test]async fn reconcile_monotonic_scope_notice_and_empty_orphan_behavior(){
    let e=DownloadEnv::new().await;e.seed_rom(1,"switch","G.nsp",None,None).await;let api=Arc::new(Sessions::default());let s=playtime(&e,api.clone(),Arc::default(),Arc::default());
    assert_eq!(s.scope_notice().await.unwrap(),json!({"pending":false}));assert_eq!(s.reconcile(1).await.unwrap()["total_seconds"],0);assert!(SqlitePlaytime::new(e.db.clone()).get(1).await.unwrap().is_none());
    *api.rows.lock().unwrap()=vec![json!({"duration_ms":125000,"end_time":"2026-01-01T00:05:00Z"})];assert_eq!(s.reconcile(1).await.unwrap()["total_seconds"],125);assert_eq!(s.reconcile(999).await.unwrap()["total_seconds"],0);
    *api.forbidden.lock().unwrap()=true;let r=s.reconcile(1).await.unwrap();assert_eq!(r["server_query_failed"],true);assert_eq!(r["total_seconds"],125);assert_eq!(s.scope_notice().await.unwrap()["pending"],true);
    *api.forbidden.lock().unwrap()=false;api.rows.lock().unwrap().clear();assert_eq!(s.reconcile(1).await.unwrap()["total_seconds"],125);assert_eq!(s.scope_notice().await.unwrap()["pending"],false);
}
#[tokio::test]async fn finalize_renders_offline_conflicts_success_and_failure_independently_of_playtime(){
    let e=DownloadEnv::new().await;let saves=Arc::new(Saves::default());let s=playtime(&e,Arc::default(),Arc::default(),saves.clone());
    for (raw,body) in [(json!({"offline":true,"success":true}),Some("Server offline — saves will sync next time")),(json!({"success":true,"synced":1}),Some("Saves synced with RomM")),(json!({"success":true,"synced":0}),None),(json!({"success":false,"message":"Sign in again"}),Some("Sign in again")),(json!({"success":false,"reason":"unsupported"}),None)]{
        *saves.exit.lock().unwrap()=raw;let r=s.finalize(1).await;assert!(r["total_seconds"].is_null());assert_eq!(r["sync"]["toast_body"],json!(body));assert!(r["migration"].is_null());
    }
    for count in [1,2]{*saves.exit.lock().unwrap()=json!({"success":false,"conflicts":vec![json!({});count]});assert_eq!(s.finalize(1).await["sync"]["conflicts_toast"],format!("{count} save conflict{} need resolution",if count==1{""}else{"s"}));}
    *saves.fail.lock().unwrap()=true;assert_eq!(s.finalize(1).await["sync"]["toast_body"],"Failed to sync saves after exit");s.schedule_flush().await;s.shutdown().await;
}

#[derive(Default)]struct Feed{responses:Mutex<VecDeque<Result<Value,ReleaseError>>>,calls:std::sync::atomic::AtomicUsize}
#[async_trait]impl ReleaseFeed for Feed{async fn latest(&self)->Result<Value,ReleaseError>{self.calls.fetch_add(1,std::sync::atomic::Ordering::SeqCst);self.responses.lock().unwrap().pop_front().unwrap_or(Ok(json!({})))} }
struct Metadata;
#[async_trait]impl PluginMetadata for Metadata{async fn metadata(&self)->anyhow::Result<(String,String)>{Ok(("0.33.2".into(),"EmuDecky RomM NG".into()))}}
#[tokio::test]async fn update_cache_success_failure_ttls_force_and_concurrent_checks(){
    let f=Arc::new(Feed::default());let t=Arc::new(Time::default());let s=UpdateService::new(UpdateConfig{feed:f.clone(),metadata:Arc::new(Metadata),clock:t.clone()});
    let release=json!({"tag_name":"v0.40.0","assets":[{"name":"romm-emudecky-ng.zip","browser_download_url":"https://github.com/ricktap/romm-emudecky-ng/releases/download/v0.40.0/romm-emudecky-ng.zip","digest":format!("sha256:{}","a".repeat(64))}]});f.responses.lock().unwrap().push_back(Ok(release));
    let (a,b)=tokio::join!(s.check(false),s.check(false));assert_eq!(a,b);assert_eq!(a["update_available"],true);assert_eq!(a["plugin_name"],"EmuDecky RomM NG");assert_eq!(f.calls.load(std::sync::atomic::Ordering::SeqCst),1);
    t.at(21599.0,0.0);assert_eq!(s.check(false).await,a);t.at(21600.0,0.0);f.responses.lock().unwrap().push_back(Err(ReleaseError::RateLimited(None)));assert_eq!(s.check(false).await["reason"],"rate_limited");
    t.at(22499.0,0.0);assert_eq!(s.check(false).await["reason"],"rate_limited");t.at(22500.0,0.0);f.responses.lock().unwrap().push_back(Err(ReleaseError::BadResponse));assert_eq!(s.check(false).await["reason"],"unknown");
    f.responses.lock().unwrap().push_back(Err(ReleaseError::Unreachable));assert_eq!(s.check(true).await["reason"],"server_unreachable");assert_eq!(s.check(true).await["update_available"],false);s.schedule().await;s.shutdown().await;
}

/// A feed with the web redirect: `tag` is what `releases/latest` redirects to, `api` what the API answers.
struct TagFeed{tag:Mutex<Result<String,ReleaseError>>,api:Mutex<VecDeque<Result<Value,ReleaseError>>>,api_calls:std::sync::atomic::AtomicUsize}
impl TagFeed{fn new(tag:Result<&str,ReleaseError>)->Arc<Self>{Arc::new(Self{tag:Mutex::new(tag.map(str::to_string)),api:Mutex::default(),api_calls:Default::default()})}fn api_calls(&self)->usize{self.api_calls.load(std::sync::atomic::Ordering::SeqCst)}}
#[async_trait]impl ReleaseFeed for TagFeed{
    async fn latest(&self)->Result<Value,ReleaseError>{self.api_calls.fetch_add(1,std::sync::atomic::Ordering::SeqCst);self.api.lock().unwrap().pop_front().unwrap_or(Ok(json!({})))}
    async fn latest_tag(&self)->Result<String,ReleaseError>{match &*self.tag.lock().unwrap(){Ok(t)=>Ok(t.clone()),Err(ReleaseError::BadResponse)=>Err(ReleaseError::BadResponse),Err(_)=>Err(ReleaseError::Unreachable)}}
}
fn release_json(v:&str)->Value{json!({"tag_name":format!("v{v}"),"assets":[{"name":"romm-emudecky-ng.zip","browser_download_url":format!("https://github.com/ricktap/romm-emudecky-ng/releases/download/v{v}/romm-emudecky-ng.zip"),"digest":format!("sha256:{}","a".repeat(64))}]})}
fn update_service(f:Arc<TagFeed>,t:Arc<Time>)->Arc<UpdateService>{UpdateService::new(UpdateConfig{feed:f,metadata:Arc::new(Metadata),clock:t})}
#[tokio::test]async fn an_up_to_date_install_never_touches_the_api(){
    let f=TagFeed::new(Ok("v0.33.2"));let s=update_service(f.clone(),Arc::new(Time::default()));
    let r=s.check(false).await;
    assert_eq!((r["success"].clone(),r["update_available"].clone(),r["latest_version"].clone()),(json!(true),json!(false),json!("0.33.2")));
    assert_eq!(f.api_calls(),0);
    assert_eq!(s.check(true).await["update_available"],false);assert_eq!(f.api_calls(),0,"a forced check is still answered by the redirect");
}
#[tokio::test]async fn a_newer_tag_asks_the_api_once_for_the_asset_and_digest(){
    let f=TagFeed::new(Ok("v0.46.0"));f.api.lock().unwrap().push_back(Ok(release_json("0.46.0")));
    let s=update_service(f.clone(),Arc::new(Time::default()));
    let r=s.check(false).await;
    assert_eq!((r["update_available"].clone(),r["latest_version"].clone()),(json!(true),json!("0.46.0")));
    assert!(r["asset_url"].as_str().unwrap().ends_with("v0.46.0/romm-emudecky-ng.zip"));assert_eq!(f.api_calls(),1);
}
#[tokio::test]async fn a_redirect_that_fails_or_looks_wrong_falls_back_to_the_api(){
    for tag in [Err(ReleaseError::Unreachable),Err(ReleaseError::BadResponse),Ok("not-a-version")]{
        let f=TagFeed::new(tag);f.api.lock().unwrap().push_back(Ok(release_json("0.46.0")));
        let s=update_service(f.clone(),Arc::new(Time::default()));
        assert_eq!(s.check(false).await["update_available"],true);assert_eq!(f.api_calls(),1);
    }
}
#[tokio::test]async fn a_rate_limit_is_remembered_until_github_lifts_it_even_for_a_forced_check(){
    let f=TagFeed::new(Ok("v0.46.0"));let t=Arc::new(Time::default());let s=update_service(f.clone(),t.clone());
    f.api.lock().unwrap().push_back(Err(ReleaseError::RateLimited(Some(1800))));
    let r=s.check(false).await;
    assert_eq!((r["reason"].clone(),r["retry_after_seconds"].clone()),(json!("rate_limited"),json!(1800)));
    assert!(r["message"].as_str().unwrap().contains("30 min"),"{r}");assert_eq!(f.api_calls(),1);
    t.at(1799.0,0.0);assert_eq!(s.check(true).await["reason"],"rate_limited");assert_eq!(f.api_calls(),1,"a forced check inside the window makes no request");
    t.at(1800.0,0.0);f.api.lock().unwrap().push_back(Ok(release_json("0.46.0")));
    assert_eq!(s.check(true).await["update_available"],true);assert_eq!(f.api_calls(),2);
}
#[tokio::test]async fn a_rate_limit_wait_is_clamped_and_defaults_to_fifteen_minutes(){
    for (retry,window) in [(Some(5u64),60.0),(Some(99999),3600.0),(None,900.0)]{
        let f=TagFeed::new(Ok("v0.46.0"));let t=Arc::new(Time::default());let s=update_service(f.clone(),t.clone());
        f.api.lock().unwrap().push_back(Err(ReleaseError::RateLimited(retry)));f.api.lock().unwrap().push_back(Ok(release_json("0.46.0")));
        s.check(false).await;t.at(window-1.0,0.0);assert_eq!(s.check(false).await["reason"],"rate_limited","{retry:?} still limited");assert_eq!(f.api_calls(),1);
        t.at(window,0.0);assert_eq!(s.check(false).await["update_available"],true,"{retry:?} asks again");assert_eq!(f.api_calls(),2);
    }
}

fn actions(e:&DownloadEnv)->super::game_actions::GameActions{
    use crate::adapters::{sync_library::SqliteSyncLibrary,steam_assets::LocalSteamAssets};
    use super::{game_actions::*,rom_removal::*};
    GameActions::new(GameActionsConfig{library:e.library.clone(),registry:Arc::new(SqliteSyncLibrary::new(e.db.clone())),assets:Arc::new(LocalSteamAssets::new(e.dir.path().join("home"),e.dir.path().join("covers"))),removal:Arc::new(RomRemovalService::new(RomRemovalServiceConfig{library:e.library.clone(),files:e.files.clone(),paths:Arc::new(FixedPaths{roms:e.roms_s(),bios:String::new()})})),downloads:e.svc.clone(),settings:e.settings.clone()})
}
#[tokio::test]async fn uninstall_keeps_identity_children_and_shared_folder_and_reports_partial_failure(){
    let e=DownloadEnv::new().await;for id in [1,2]{e.seed_rom(id,"switch",&format!("G{id}.nsp"),Some(98+id),None).await;install(&e,id,&format!("G{id}.nsp")).await;}
    e.sql("INSERT INTO rom_playtime(rom_id,total_seconds,session_count) VALUES(1,42,1); INSERT INTO rom_save_states(rom_id,slots) VALUES(1,'{\"a\":{}}'); CREATE TRIGGER reject_remove BEFORE DELETE ON rom_installs WHEN OLD.rom_id=2 BEGIN SELECT RAISE(ABORT,'disk error'); END;").await;
    let s=actions(&e);let r=s.uninstall_all().await.unwrap();assert_eq!(r["success"],false);assert_eq!(r["removed_count"],1);assert_eq!(r["app_ids"],json!([99]));assert_eq!(r["errors"][0]["rom_id"],"2");assert_eq!(e.library.rom(1).await.unwrap().unwrap().shortcut_app_id,Some(99));assert_eq!(SqlitePlaytime::new(e.db.clone()).get(1).await.unwrap().unwrap().total_seconds,42);assert!(SwitchSaveSafety::new(e.db.clone(),e.settings.clone()).tracked(1).await.unwrap());assert!(e.roms().join("switch").is_dir());assert!(e.library.install(2).await.unwrap().is_some());assert_eq!(s.remove(1).await["reason"],"not_installed");
    e.sql("DROP TRIGGER reject_remove").await;assert_eq!(s.uninstall_all().await.unwrap()["removed_count"],1);assert_eq!(s.uninstall_all().await.unwrap(),json!({"success":true,"removed_count":0,"errors":[],"app_ids":[]}));
}
#[tokio::test]async fn running_download_blocks_removal_and_paused_partial_is_evicted(){
    let e=DownloadEnv::new().await;e.seed_rom(1,"switch","G.nsp",Some(99),None).await;e.romm.set_rom(switch_rom(1,"G.nsp",100));e.romm.set_content(1,payload(100));*e.romm.stall_at.lock().unwrap()=Some(20);
    assert_eq!(e.svc.start_download(1).await["success"],true);e.romm.stalled.notified().await;let s=actions(&e);assert_eq!(s.remove(1).await["reason"],"download_in_progress");assert_eq!(s.uninstall_all().await.unwrap()["reason"],"download_in_progress");
    e.svc.pause_download(1);e.wait_status(1,"paused").await;let jobs=e.jobs.all().await.unwrap();let tmp=jobs[0].tmp_path.clone();assert!(std::path::Path::new(&tmp).exists());
    assert_eq!(s.uninstall_all().await.unwrap()["success"],true);assert!(e.entry(1).is_none());assert!(e.jobs.all().await.unwrap().is_empty());assert!(!std::path::Path::new(&tmp).exists());e.svc.shutdown().await;
}
#[tokio::test]async fn steam_input_bulk_targets_bound_shortcuts_preserves_unknown_keys_and_returns_failures(){
    let e=DownloadEnv::new().await;let s=actions(&e);assert_eq!(s.apply_input().await.unwrap(),json!({"success":true,"message":"No shortcuts to update"}));
    for (id,app) in [(1,Some(99)),(2,None),(3,Some(101))]{e.seed_rom(id,"switch","G.nsp",app,None).await;}
    let dir=e.dir.path().join("home/.local/share/Steam/userdata/1/config");std::fs::create_dir_all(&dir).unwrap();let path=dir.join("localconfig.vdf");let original="\"UserLocalConfigStore\" { \"Other\" \"keep\" \"Apps\" { \"99\" { \"LaunchOptions\" \"-f\" } \"500\" { \"UseSteamControllerConfig\" \"2\" } } }";std::fs::write(&path,original).unwrap();
    for (mode,want) in [("force_off","0"),("force_on","2")]{e.settings.set("steam_input_mode",json!(mode));let r=s.apply_input().await.unwrap();assert_eq!(r["message"],format!("Steam Input set to '{mode}' for 2 shortcuts"));let text=std::fs::read_to_string(&path).unwrap();assert!(text.contains("LaunchOptions"));assert!(text.contains("Other"));assert!(text.contains(&format!("\"UseSteamControllerConfig\"\t\"{want}\"")));}
    e.settings.set("steam_input_mode",json!("default"));s.apply_input().await.unwrap();let text=std::fs::read_to_string(&path).unwrap();assert_eq!(text.matches("UseSteamControllerConfig").count(),1);assert!(!dir.join("shortcuts.vdf").exists());
    std::fs::write(&path,"broken").unwrap();assert_eq!(s.apply_input().await.unwrap(),json!({"success":false,"reason":"unknown","message":"Operation failed"}));assert_eq!(std::fs::read_to_string(path).unwrap(),"broken");
}

#[tokio::test]async fn picker_default_is_independent_of_binding_and_installation(){
    let e=DownloadEnv::new().await;e.seed_rom(1,"switch","USA.nsp",Some(99),Some("igdb:7:5")).await;e.seed_rom(2,"switch","Europe.nsp",None,Some("igdb:7:5")).await;install(&e,1,"USA.nsp").await;
    e.sql("UPDATE roms SET regions='[\"USA\"]' WHERE rom_id=1;UPDATE roms SET regions='[\"Europe\"]' WHERE rom_id=2;").await;let s=versions(&e,Arc::default(),Arc::default());e.settings.set("preferred_region",json!("Europe"));let r=s.list(99).await.unwrap();assert_eq!(r["versions"][0]["active"],true);assert_eq!(r["versions"][1]["is_default"],true);
    e.sql("UPDATE roms SET is_main_sibling=1 WHERE rom_id=1").await;assert_eq!(s.list(99).await.unwrap()["versions"][0]["is_default"],true);
}
#[tokio::test]async fn server_target_wrong_id_and_late_binding_never_replace_local_data(){
    let e=DownloadEnv::new().await;e.seed_rom(1,"switch","Game.nsp",Some(99),Some("igdb:7:5")).await;let repo=SqliteVersions::new(e.db.clone());let s=versions(&e,Arc::default(),Arc::default());
    e.romm.roms.lock().unwrap().insert(2,Ok(json!({"id":88,"igdb_id":7,"platform_id":5})));assert_eq!(s.switch(99,2,false).await.unwrap()["reason"],"invalid_target");assert!(repo.member(88).await.unwrap().is_none());
    let snapshot=repo.group(99).await.unwrap().unwrap();e.seed_rom(2,"switch","Game2.nsp",Some(101),Some("igdb:7:5")).await;assert_eq!(repo.bind_version(&snapshot,2,Some(json!({"id":2})),"t").await.unwrap()["reason"],"state_changed");assert_eq!(repo.member(2).await.unwrap().unwrap()["app_id"],101);
}
struct BadMetadata;
#[async_trait]impl PluginMetadata for BadMetadata{async fn metadata(&self)->anyhow::Result<(String,String)>{anyhow::bail!("unreadable")}}
#[tokio::test]async fn updater_metadata_failure_is_canonical_and_does_not_fetch(){
    let f=Arc::new(Feed::default());let s=UpdateService::new(UpdateConfig{feed:f.clone(),metadata:Arc::new(BadMetadata),clock:Arc::new(Time::default())});let r=s.check(false).await;assert_eq!(r["success"],false);assert_eq!(r["reason"],"unknown");assert_eq!(r["current_version"],"0.0.0");assert_eq!(f.calls.load(std::sync::atomic::Ordering::SeqCst),0);
}
