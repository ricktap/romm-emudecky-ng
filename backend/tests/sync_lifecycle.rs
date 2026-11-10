//! Deterministic lifecycle contracts using real composition, HTTP and SQLite.
mod common;
use std::{sync::{Arc,atomic::{AtomicU64,Ordering}},time::Duration};
use async_trait::async_trait;
use common::Dirs;
use romm_backend::{app::{Backend,BackendEnv},adapters::{persistence::system_clock,romm::http::RetryPolicy},rpc::server::EventHub,services::ports::{Clock,SyncIds,SyncSleeper}};
use serde_json::json;
use wiremock::{Mock,MockServer,ResponseTemplate,matchers::{method,path}};
struct Runtime {epoch:AtomicU64,mono:AtomicU64,next:AtomicU64,sleeps:AtomicU64,permits:tokio::sync::Semaphore}
impl Runtime {
    fn new()->Arc<Self>{Arc::new(Self{epoch:AtomicU64::new(1_800_000_000),mono:AtomicU64::new(0),next:AtomicU64::new(0),sleeps:AtomicU64::new(0),permits:tokio::sync::Semaphore::new(0)})}
    fn advance(&self,seconds:u64){self.epoch.fetch_add(seconds,Ordering::SeqCst);self.mono.fetch_add(seconds,Ordering::SeqCst);}
    async fn waiting(&self,count:u64){tokio::time::timeout(Duration::from_secs(5),async{while self.sleeps.load(Ordering::SeqCst)<count{tokio::task::yield_now().await;}}).await.expect("sync did not enter heartbeat wait");}
    fn wake(&self){self.permits.add_permits(1);}
}
impl Clock for Runtime {
    fn now_iso(&self)->String{format!("2026-01-01T00:00:{:02}+00:00",self.mono.load(Ordering::SeqCst)%60)}
    fn epoch_seconds(&self)->f64{self.epoch.load(Ordering::SeqCst) as f64}
    fn monotonic(&self)->f64{self.mono.load(Ordering::SeqCst) as f64}
}
impl SyncIds for Runtime {fn next_id(&self)->String{format!("run-{}",self.next.fetch_add(1,Ordering::SeqCst))}}
#[async_trait]impl SyncSleeper for Runtime {async fn sleep(&self,_:f64){self.sleeps.fetch_add(1,Ordering::SeqCst);self.permits.acquire().await.unwrap().forget();}}
async fn fixture()->(Dirs,MockServer,Backend,Arc<Runtime>){
    let dirs=Dirs::new();let server=MockServer::start().await;dirs.write_settings(&json!({"romm_url":server.uri()}));
    Mock::given(method("GET")).and(path("/api/platforms")).respond_with(ResponseTemplate::new(200).set_body_json(json!([{"id":1,"name":"Switch","slug":"switch","rom_count":1}]))).mount(&server).await;
    Mock::given(method("GET")).and(path("/api/roms")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"items":[{"id":1,"name":"Game","platform_id":1}]}))).mount(&server).await;
    let runtime=Runtime::new();let backend=Backend::start_with_sync_runtime(BackendEnv{settings_dir:dirs.settings(),runtime_dir:dirs.runtime(),plugin_dir:dirs.root.path().join("plugin"),user_home:dirs.root.path().join("home"),retry:RetryPolicy{max_attempts:1,base_delay:Duration::from_millis(1)},clock:system_clock()},Arc::new(EventHub::default()),runtime.clone(),runtime.clone(),runtime.clone()).await.unwrap();
    (dirs,server,backend,runtime)
}
async fn terminal(backend:&Backend,stage:&str){tokio::time::timeout(Duration::from_secs(5),async{loop{let p=backend.sync.get_sync_status();if p["stage"]==stage && p["running"]==false{break;}tokio::task::yield_now().await;}}).await.expect("sync did not terminate");}
async fn close(backend:Backend){backend.sync.shutdown().await;backend.downloads.shutdown().await;}

#[tokio::test]async fn preview_expiry_is_measured_by_injected_epoch_and_never_starts_a_run(){
    let(dirs,_server,backend,runtime)=fixture().await;let preview=backend.sync.preview().await;assert_eq!(preview["success"],true);runtime.advance(1801);
    assert_eq!(backend.sync.apply_preview(preview["preview_id"].as_str().unwrap()).await["reason"],"stale_preview");
    let db=rusqlite::Connection::open(dirs.db()).unwrap();assert_eq!(db.query_row("SELECT COUNT(*) FROM sync_runs",[],|r|r.get::<_,i64>(0)).unwrap(),0);close(backend).await;
}
#[tokio::test]async fn busy_apply_preserves_preview_and_explicit_cancel_rejects_the_inflight_ack(){
    let(_dirs,_server,backend,runtime)=fixture().await;let preview=backend.sync.preview().await;let id=preview["preview_id"].as_str().unwrap();backend.sync.start().await;runtime.waiting(1).await;
    assert_eq!(backend.sync.apply_preview(id).await["reason"],"sync_in_progress");let run=backend.sync.get_sync_status()["runId"].as_str().unwrap().to_string();backend.sync.cancel(Some(&run));
    assert_eq!(backend.sync.report(&json!({"1":123}),&run,"1").await.unwrap()["ignored"],true);runtime.wake();terminal(&backend,"cancelled").await;
    assert_eq!(backend.sync.apply_preview(id).await["success"],true);runtime.waiting(2).await;let run=backend.sync.get_sync_status()["runId"].as_str().unwrap().to_string();backend.sync.report(&json!({"1":123}),&run,"1").await.unwrap();runtime.wake();terminal(&backend,"done").await;close(backend).await;
}
#[tokio::test]async fn heartbeat_extends_wait_and_timeout_keeps_only_a_matching_late_ack_recoverable(){
    let(dirs,_server,backend,runtime)=fixture().await;backend.sync.start().await;runtime.waiting(1).await;let run=backend.sync.get_sync_status()["runId"].as_str().unwrap().to_string();
    runtime.advance(40);backend.sync.heartbeat();runtime.advance(40);runtime.wake();runtime.waiting(2).await;assert_eq!(backend.sync.get_sync_status()["running"],true);
    runtime.advance(21);runtime.wake();terminal(&backend,"cancelled").await;
    assert_eq!(backend.sync.report(&json!({"1":123}),&run,"wrong").await.unwrap()["ignored"],true);
    assert_eq!(backend.sync.report(&json!({"1":123}),&run,"1").await.unwrap()["count"],1);
    let db=rusqlite::Connection::open(dirs.db()).unwrap();assert_eq!(db.query_row("SELECT shortcut_app_id FROM roms WHERE rom_id=1",[],|r|r.get::<_,i64>(0)).unwrap(),123);
    backend.sync.start().await;runtime.waiting(3).await;assert_eq!(backend.sync.report(&json!({"1":999}),&run,"1").await.unwrap()["ignored"],true);
    backend.sync.cancel(None);runtime.wake();terminal(&backend,"cancelled").await;close(backend).await;
}
