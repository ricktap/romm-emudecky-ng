//! Python game-page/artwork contracts with real SQLite and files at the I/O edges.
use std::{collections::HashMap, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use crate::adapters::{game_reads::SqliteGameReads, steam_assets::LocalSteamAssets, sync_library::SqliteSyncLibrary};
use super::{artwork::ArtworkService, game_covers::{GameCoverService, GameCoverServiceConfig},
    game_detail::{GameDetailService, GameDetailServiceConfig, empty_metadata}, ports::*,
    steamgrid::{SteamGridService, SteamGridServiceConfig}, testing::{DownloadEnv, FakePersister}};

#[derive(Default)]
struct Pending(Mutex<HashMap<i64, Value>>);
#[async_trait]
impl PendingRomReader for Pending {
    async fn pending_rom(&self, id: i64) -> Option<Value> { self.0.lock().unwrap().get(&id).cloned() }
}

#[derive(Default)]
struct Api {
    responses: Mutex<HashMap<String, Value>>,
    calls: Mutex<Vec<(String, String)>>,
    status: Mutex<Option<u16>>,
    fail_image: AtomicBool,
    downloads: Mutex<Vec<String>>,
}
impl Api {
    fn respond(&self, path: &str, value: Value) { self.responses.lock().unwrap().insert(path.into(), value); }
}
#[async_trait]
impl SteamGridApi for Api {
    async fn request(&self, path: &str, key: &str) -> Result<Value, SgdbError> {
        self.calls.lock().unwrap().push((path.into(), key.into()));
        if let Some(status) = *self.status.lock().unwrap() { return Err(SgdbError::Http(status)); }
        self.responses.lock().unwrap().get(path).cloned().ok_or(SgdbError::Connection)
    }
    async fn download_image(&self, url: &str, dest: &str) -> anyhow::Result<()> {
        self.downloads.lock().unwrap().push(url.into());
        anyhow::ensure!(!self.fail_image.load(Ordering::SeqCst), "CDN unavailable");
        std::fs::create_dir_all(std::path::Path::new(dest).parent().unwrap())?;
        std::fs::write(dest, url.as_bytes())?;
        Ok(())
    }
}

struct GridEnv {
    e: DownloadEnv, api: Arc<Api>, pending: Arc<Pending>, assets: Arc<LocalSteamAssets>,
    repo: Arc<SqliteGameReads>, persister: Arc<FakePersister>, svc: SteamGridService,
}
impl GridEnv {
    async fn new() -> Self {
        let e = DownloadEnv::new().await;
        let api = Arc::new(Api::default());
        let pending = Arc::new(Pending::default());
        let assets = Arc::new(LocalSteamAssets::new(e.dir.path().join("home"), e.dir.path().join("artwork")));
        let repo = Arc::new(SqliteGameReads::new(e.db.clone()));
        let persister = Arc::new(FakePersister::default());
        let svc = SteamGridService::new(SteamGridServiceConfig {
            api: api.clone(), romm: e.romm.clone(), assets: assets.clone(),
            registry: Arc::new(SqliteSyncLibrary::new(e.db.clone())), records: repo.clone(),
            pending: pending.clone(), settings: e.settings.clone(), persister: persister.clone(),
        });
        Self { e, api, pending, assets, repo, persister, svc }
    }
    fn key(&self) { self.e.settings.set("steamgriddb_api_key", json!("test-key")); }
    fn path(&self, id: i64, kind: &str) -> String { format!("{}/{}_{}.png", self.assets.cache_dir(), id, kind) }
    fn write(&self, id: i64, kind: &str, bytes: &[u8]) {
        std::fs::create_dir_all(self.assets.cache_dir()).unwrap();
        std::fs::write(self.path(id, kind), bytes).unwrap();
    }
}

#[tokio::test]
async fn game_page_metadata_defaults_are_offline_and_adoption_runs_only_when_uninstalled() {
    let e = DownloadEnv::new().await;
    e.seed_rom(1, "switch", "game.nsp", Some(99), None).await;
    let adopted = Arc::new(Mutex::new(Vec::new()));
    let calls = adopted.clone();
    let s = GameDetailService::new(GameDetailServiceConfig {
        repo: Arc::new(SqliteGameReads::new(e.db.clone())), settings: e.settings.clone(), clock: e.clock.clone(),
        schedule_adoption: Arc::new(move |id| calls.lock().unwrap().push(id)),
    });
    assert_eq!(s.detail(999).await.unwrap(), json!({"found":false}));
    assert_eq!(s.metadata(1).await.unwrap(), empty_metadata());
    assert_eq!(s.all_metadata().await.unwrap(), json!({}));
    let detail = s.detail(99).await.unwrap();
    assert_eq!(detail, json!({"found":true,"rom_id":1,"rom_name":"Game 1","platform_slug":"switch",
        "platform_name":"switch","installed":false,"save_sync_enabled":false,"save_status":null,
        "save_sync_display":null,"metadata":null,"bios_status":null,"bios_level":null,"bios_label":null,
        "rom_file":"game.nsp","ra_id":null,"achievement_summary":null,"stale_fields":["metadata"],
        "regions":[],"languages":[],"revision":"","tags":[],"is_main_sibling":false}));
    assert_eq!(*adopted.lock().unwrap(), vec![1]);
    e.sql("INSERT INTO rom_installs (rom_id,file_path,rom_dir,platform_slug,system,installed_at) VALUES (1,'/storage/chosen.nsp','/storage','switch','switch','t');").await;
    assert_eq!(s.detail(99).await.unwrap()["rom_file"], "chosen.nsp");
    assert_eq!(adopted.lock().unwrap().len(), 1);
    assert!(e.romm.get_rom_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cached_detail_keeps_version_fields_save_projections_and_exact_ttl_boundary() {
    let e = DownloadEnv::new().await;
    e.seed_rom(1, "switch", "game.nsp", Some(99), None).await;
    e.settings.set("save_sync_enabled", json!(true));
    e.sql("UPDATE roms SET regions='[\"EU\"]',languages='[\"en\"]',revision='rev 2',tags='[\"verified\"]',is_main_sibling=1,ra_id=88 WHERE rom_id=1;
        INSERT INTO kv_config VALUES ('platform_names','{\"switch\":\"Nintendo Switch\"}');
        INSERT INTO rom_metadata (rom_id,summary,genres,companies,game_modes,steam_categories,player_count,first_release_date,average_rating,cached_at)
        VALUES (1,'cached','[\"Adventure\"]','[\"Nintendo\"]','[\"Single player\"]','[2]','1',123,92.5,1766620800);
        INSERT INTO rom_save_states (rom_id) VALUES (1);").await;
    let repo = Arc::new(SqliteGameReads::new(e.db.clone()));
    let s = GameDetailService::new(GameDetailServiceConfig { repo, settings:e.settings.clone(), clock:e.clock.clone(), schedule_adoption:Arc::new(|_|{}) });
    let d = s.detail(99).await.unwrap();
    assert_eq!(d["stale_fields"], json!([])); // exactly seven days
    assert_eq!(d["save_sync_display"], json!({"status":"none","label":"No saves","last_sync_check_at":null}));
    assert_eq!(d["platform_name"], "Nintendo Switch");
    for (field, value) in [("regions",json!(["EU"])),("languages",json!(["en"])),("revision",json!("rev 2")),("tags",json!(["verified"])),("is_main_sibling",json!(true)),("ra_id",json!(88)),("save_sync_enabled",json!(true))] { assert_eq!(d[field],value); }
    let m = s.metadata(1).await.unwrap();
    assert_eq!(m,json!({"summary":"cached","genres":["Adventure"],"companies":["Nintendo"],"game_modes":["Single player"],"steam_categories":[2],"player_count":"1","first_release_date":123,"average_rating":92.5,"cached_at":1766620800.0}));
    assert_eq!(s.all_metadata().await.unwrap(),json!({"1":m}));
    e.sql("UPDATE rom_metadata SET cached_at=cached_at-1,genres='{}',companies='null',game_modes='42',steam_categories='false';
        UPDATE kv_config SET value='broken';
        INSERT INTO rom_save_files (rom_id,filename,last_sync_hash) VALUES (1,'save.bin','hash');").await;
    let d = s.detail(99).await.unwrap();
    assert_eq!(d["platform_name"],"switch");
    assert_eq!(d["stale_fields"],json!(["metadata"]));
    assert_eq!(d["metadata"]["genres"],json!([]));
    assert_eq!(d["save_status"]["files"],json!([{"filename":"save.bin","status":"synced","last_sync_at":null}]));
    assert_eq!(d["save_sync_display"],json!({"status":"synced","label":"Not synced","last_sync_check_at":null}));
    e.sql("UPDATE rom_save_states SET last_sync_check_at='check'; UPDATE rom_save_files SET last_sync_at='time';").await;
    let d = s.detail(99).await.unwrap();
    assert_eq!(d["save_sync_display"],json!({"status":"synced","label":null,"last_sync_check_at":"check"}));
    assert_eq!(d["save_status"]["files"][0]["last_sync_at"],"time");
    assert!(e.romm.get_rom_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn passive_artwork_is_cache_first_without_key_and_never_resolves_from_romm() {
    let g = GridEnv::new().await;
    assert_eq!(g.svc.artwork(1,1).await,json!({"base64":null,"no_api_key":true}));
    assert_eq!(g.svc.artwork(1,i64::MIN).await,json!({"base64":null,"no_api_key":false}));
    g.write(1,"hero",b"cached");
    assert_eq!(g.svc.artwork(1,1).await,json!({"base64":STANDARD.encode(b"cached"),"no_api_key":false}));
    g.key();
    assert_eq!(g.svc.artwork(1,2).await,json!({"base64":null,"no_api_key":false}));
    assert!(g.api.calls.lock().unwrap().is_empty());
    assert!(g.e.romm.get_rom_calls.lock().unwrap().is_empty());
    g.pending.0.lock().unwrap().insert(1,json!({"sgdb_id":8}));
    g.api.respond("/logos/game/8",json!({"success":true,"data":[{"url":"pending"}]}));
    assert_eq!(g.svc.artwork(1,2).await["base64"],STANDARD.encode(b"pending"));
    g.e.seed_rom(1,"switch","game",Some(99),None).await;
    g.e.sql("UPDATE roms SET sgdb_id=9;").await;
    g.api.respond("/icons/game/9",json!({"success":true,"data":[{"url":"stored"}]}));
    assert_eq!(g.svc.artwork(1,4).await["base64"],STANDARD.encode(b"stored"));
    assert_eq!(g.api.calls.lock().unwrap().len(),2);
    assert!(g.e.romm.get_rom_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn explicit_resolution_prefers_romm_then_local_then_igdb_and_preserves_children() {
    let g = GridEnv::new().await;
    assert_eq!(g.svc.resolution(1).await.unwrap(),json!({"decision":"no_api_key"}));
    assert!(g.e.romm.get_rom_calls.lock().unwrap().is_empty());
    g.key();
    g.e.seed_rom(1,"switch","game",Some(99),None).await;
    g.e.sql("UPDATE roms SET sgdb_id=8,emulator_override='Ryujinx',selected_disc='pin.nsp';
        INSERT INTO rom_playtime (rom_id,total_seconds) VALUES (1,123);
        INSERT INTO rom_save_states (rom_id) VALUES (1);").await;
    g.e.romm.set_rom(json!({"id":1,"sgdb_id":9,"igdb_id":3}));
    assert_eq!(g.svc.resolution(1).await.unwrap(),json!({"decision":"resolved","sgdb_id":9}));
    g.e.romm.set_rom(json!({"id":1,"igdb_id":3}));
    assert_eq!(g.svc.resolution(1).await.unwrap()["sgdb_id"],9);
    g.e.romm.roms.lock().unwrap().remove(&1);
    assert_eq!(g.svc.resolution(1).await.unwrap()["sgdb_id"],9); // server offline
    g.e.sql("UPDATE roms SET sgdb_id=NULL;").await;
    g.e.romm.set_rom(json!({"id":1,"igdb_id":3}));
    g.api.respond("/games/igdb/3",json!({"success":true,"data":{"id":12}}));
    assert_eq!(g.svc.resolution(1).await.unwrap()["sgdb_id"],12);
    let preserved: (Option<i64>,String,String,i64,i64) = g.e.db.call(|c|c.query_row("SELECT sgdb_id,emulator_override,selected_disc,(SELECT total_seconds FROM rom_playtime WHERE rom_id=1),(SELECT count(*) FROM rom_save_states WHERE rom_id=1) FROM roms WHERE rom_id=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))).await.unwrap();
    assert_eq!(preserved,(Some(12),"Ryujinx".into(),"pin.nsp".into(),123,1));
    // Resolving an unsynced ROM is useful but cannot create registry rows.
    g.e.romm.set_rom(json!({"id":2,"sgdb_id":20}));
    assert_eq!(g.svc.resolution(2).await.unwrap()["sgdb_id"],20);
    assert!(g.repo.game_detail(20).await.unwrap().is_none());
}

#[tokio::test]
async fn picker_search_encodes_caps_and_keeps_failed_thumbnails_optional() {
    let g = GridEnv::new().await;
    assert_eq!(g.svc.search("game").await["reason"],"no_api_key");
    g.key();
    assert_eq!(g.svc.search("offline").await["reason"],"server_unreachable");
    g.e.romm.set_rom(json!({"id":1,"igdb_id":7,"name":"Zelda / é?"}));
    g.api.respond("/search/autocomplete/Zelda%20/%20%C3%A9%3F",json!({"success":true,"data":(1..10).map(|id|json!({"id":id,"name":format!("Game {id}"),"release_date":0})).collect::<Vec<_>>()}));
    g.api.respond("/grids/game/1?limit=1",json!({"success":true,"data":[{"thumb":"small","url":"big"}]}));
    g.api.respond("/grids/game/2?limit=1",json!({"success":true,"data":[{"url":"fallback"}]}));
    let r = g.svc.resolution(1).await.unwrap();
    assert_eq!(r["decision"],"needs_pick");
    assert_eq!(r["candidates"].as_array().unwrap().len(),6);
    assert_eq!(r["candidates"][0],json!({"id":1,"name":"Game 1","release_year":1970,"thumb_url":"small"}));
    assert_eq!(r["candidates"][1]["thumb_url"],"fallback");
    assert_eq!(r["candidates"][2]["thumb_url"],Value::Null);
    assert!(!g.api.calls.lock().unwrap().iter().any(|(p,_)|p.contains("game/7?")));
}

#[tokio::test]
async fn manual_picks_replace_all_pixels_without_remembering_the_chosen_id() {
    let g = GridEnv::new().await;
    g.key();
    g.e.seed_rom(1,"switch","game",Some(99),None).await;
    g.e.sql("UPDATE roms SET sgdb_id=8;").await;
    for kind in crate::domain::sgdb_artwork::ASSETS {
        g.write(1,kind,b"old");
        g.api.respond(&crate::domain::sgdb_artwork::endpoint(kind,12).unwrap(),json!({"success":true,"data":[{"url":kind}]}));
    }
    assert_eq!(g.svc.apply(1,12).await.unwrap(),json!({"success":true}));
    for kind in crate::domain::sgdb_artwork::ASSETS { assert_eq!(std::fs::read(g.path(1,kind)).unwrap(),kind.as_bytes()); }
    let registry = SqliteSyncLibrary::new(g.e.db.clone()).registry().await.unwrap();
    assert_eq!(registry[&1]["sgdb_id"],8);
    g.api.fail_image.store(true,Ordering::SeqCst);
    assert_eq!(g.svc.apply(1,12).await.unwrap(),json!({"success":true}));
    assert!(!std::path::Path::new(&g.path(1,"hero")).exists());
    assert_eq!(g.svc.artwork(1,1).await,json!({"base64":null,"no_api_key":false}));
}

#[tokio::test]
async fn key_mask_and_failed_persistence_never_replace_the_saved_key() {
    let g = GridEnv::new().await;
    assert_eq!(g.svc.verify(None).await["reason"],"no_api_key");
    g.key();
    for v in ["", "••••"] { assert_eq!(g.svc.save_key(v).await.unwrap()["success"],true); }
    assert_eq!(g.persister.saves(),0);
    g.api.respond("/search/autocomplete/test",json!({"success":true}));
    for key in [None,Some(""),Some("••••")] { assert_eq!(g.svc.verify(key).await["success"],true); }
    assert!(g.api.calls.lock().unwrap().iter().all(|(_,k)|k=="test-key"));
    assert_eq!(g.svc.verify(Some("temporary")).await["success"],true);
    assert_eq!(g.e.settings.get_str("steamgriddb_api_key").as_deref(),Some("test-key"));
    for status in [401,403,429,500] {
        *g.api.status.lock().unwrap()=Some(status);
        assert_eq!(g.svc.verify(None).await["reason"],if status==401||status==403{"auth_failed"}else{"server_unreachable"});
    }
    *g.api.status.lock().unwrap()=None;
    g.api.respond("/search/autocomplete/test",json!({"success":false}));
    assert_eq!(g.svc.verify(None).await["reason"],"auth_failed");
    g.api.responses.lock().unwrap().clear();
    assert_eq!(g.svc.verify(None).await["reason"],"server_unreachable");
    g.svc.save_key("new-key").await.unwrap();
    assert_eq!(g.persister.saves(),1);
    g.persister.fail_with("disk full");
    assert!(g.svc.save_key("lost-key").await.is_err());
    assert_eq!(g.e.settings.get_str("steamgriddb_api_key").as_deref(),Some("new-key"));
    g.e.settings.remove("steamgriddb_api_key");
    assert!(g.svc.save_key("lost-key").await.is_err());
    assert_eq!(g.e.settings.get("steamgriddb_api_key"),None);
}

#[tokio::test]
async fn artwork_pruning_keeps_bound_roms_and_is_safe_if_registry_read_fails() {
    let g = GridEnv::new().await;
    g.e.seed_rom(1,"switch","game",Some(99),None).await;
    g.e.seed_rom(2,"switch","game",None,None).await;
    for id in [1,2,3] { g.write(id,"hero",b"art"); }
    std::fs::write(format!("{}.tmp",g.path(1,"hero")),b"partial").unwrap();
    g.e.sql("ALTER TABLE roms RENAME TO unavailable;").await;
    assert!(g.svc.prune().await.is_err());
    assert!(std::path::Path::new(&g.path(3,"hero")).exists());
    g.e.sql("ALTER TABLE unavailable RENAME TO roms;").await;
    g.svc.prune().await.unwrap();
    assert_eq!(std::fs::read(g.path(1,"hero")).unwrap(),b"art");
    assert_eq!(std::fs::read_dir(g.assets.cache_dir()).unwrap().count(),1);
}

#[derive(Default)]
struct Covers {
    fail: AtomicBool, calls: Mutex<Vec<String>>, after_write: Mutex<Option<Arc<dyn Fn()+Send+Sync>>>,
}
#[async_trait]
impl RommCoverApi for Covers {
    async fn download_cover(&self, url: &str, dest: &str) -> anyhow::Result<()> {
        self.calls.lock().unwrap().push(url.into());
        std::fs::create_dir_all(std::path::Path::new(dest).parent().unwrap())?;
        std::fs::write(dest,b"new cover")?;
        tokio::task::yield_now().await;
        if let Some(callback)=self.after_write.lock().unwrap().as_ref() { callback(); }
        anyhow::ensure!(!self.fail.load(Ordering::SeqCst),"connection lost after partial write");
        Ok(())
    }
}
struct CoverEnv { g: GridEnv, api: Arc<Covers>, art: Arc<ArtworkService>, svc: GameCoverService }
impl CoverEnv {
    async fn new() -> Self {
        let g=GridEnv::new().await;
        let api=Arc::new(Covers::default());
        let assets=Arc::new(LocalSteamAssets::new(g.e.dir.path().join("home"),g.e.dir.path().join("covers")));
        let registry=Arc::new(SqliteSyncLibrary::new(g.e.db.clone()));
        let art=Arc::new(ArtworkService { api:api.clone(),assets,registry:registry.clone(),lock:tokio::sync::Mutex::new(()) });
        let svc=GameCoverService::new(GameCoverServiceConfig {artwork:art.clone(),romm:g.e.romm.clone(),records:g.repo.clone(),registry,pending:g.pending.clone()});
        Self{g,api,art,svc}
    }
    fn grid(&self) -> String {
        let path=self.g.e.dir.path().join("home/.local/share/Steam/userdata/1/config/grid");
        std::fs::create_dir_all(&path).unwrap(); path.to_str().unwrap().into()
    }
}

#[tokio::test]
async fn cover_reads_honor_pending_and_legacy_paths_before_cache_and_staging() {
    let c=CoverEnv::new().await;
    c.g.e.seed_rom(1,"switch","game",Some(99),None).await;
    assert_eq!(c.svc.get_base64(1).await,json!({"base64":null}));
    let grid=c.grid();
    std::fs::write(format!("{grid}/99p.png"),b"grid").unwrap();
    assert_eq!(c.svc.get_base64(1).await["base64"],STANDARD.encode(b"grid"));
    std::fs::write(format!("{grid}/romm_1_cover.png"),b"staged").unwrap();
    assert_eq!(c.svc.get_base64(1).await["base64"],STANDARD.encode(b"staged"));
    c.art.assets.copy_atomic(&format!("{grid}/99p.png"),&c.art.assets.cache_path(1)).await.unwrap();
    assert_eq!(c.svc.get_base64(1).await["base64"],STANDARD.encode(b"grid"));
    c.g.e.sql("UPDATE roms SET cover_path='/missing/legacy.png';").await;
    assert_eq!(c.svc.get_base64(1).await["base64"],Value::Null);
    c.g.pending.0.lock().unwrap().insert(1,json!({"cover_path":format!("{grid}/romm_1_cover.png")}));
    assert_eq!(c.svc.get_base64(1).await["base64"],STANDARD.encode(b"staged"));
    assert!(c.g.e.romm.get_rom_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn fetch_cover_works_for_unsynced_roms_and_does_not_repeat_http_on_cache_hits() {
    let c=CoverEnv::new().await;
    assert_eq!(c.svc.fetch(1).await,json!({"base64":null}));
    c.g.e.romm.set_rom(json!({"id":1}));
    assert_eq!(c.svc.fetch(1).await,json!({"base64":null}));
    c.g.e.romm.set_rom(json!({"id":1,"path_cover_small":"/small"}));
    c.api.fail.store(true,Ordering::SeqCst);
    assert_eq!(c.svc.fetch(1).await,json!({"base64":null}));
    assert!(!std::path::Path::new(&format!("{}.tmp",c.art.assets.cache_path(1))).exists());
    c.api.fail.store(false,Ordering::SeqCst);
    assert_eq!(c.svc.fetch(1).await["base64"],STANDARD.encode(b"new cover"));
    c.g.e.romm.get_rom_calls.lock().unwrap().clear();
    assert_eq!(c.svc.fetch(1).await["base64"],STANDARD.encode(b"new cover"));
    assert!(c.g.e.romm.get_rom_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn refresh_cover_failures_preserve_published_art_and_reject_moved_binding() {
    let c=CoverEnv::new().await;
    assert_eq!(c.svc.refresh(1).await.unwrap()["reason"],"not_synced");
    c.g.e.seed_rom(1,"switch","game",Some(99),None).await;
    assert_eq!(c.svc.refresh(1).await.unwrap()["reason"],"no_grid_dir");
    let grid=c.grid();
    assert_eq!(c.svc.refresh(1).await.unwrap()["reason"],"server_unreachable");
    c.g.e.romm.set_rom(json!({"id":1}));
    assert_eq!(c.svc.refresh(1).await.unwrap()["reason"],"no_cover");
    c.g.e.romm.set_rom(json!({"id":1,"path_cover_large":"/large","path_cover_small":"/small"}));
    let cache=c.art.assets.cache_path(1);
    std::fs::create_dir_all(c.art.assets.cache_dir()).unwrap();
    std::fs::write(&cache,b"old").unwrap();
    std::fs::write(format!("{grid}/99p.png"),b"old grid").unwrap();
    c.api.fail.store(true,Ordering::SeqCst);
    assert_eq!(c.svc.refresh(1).await.unwrap()["reason"],"download_failed");
    assert_eq!(std::fs::read(&cache).unwrap(),b"old");
    assert_eq!(std::fs::read(format!("{grid}/99p.png")).unwrap(),b"old grid");
    assert!(!std::path::Path::new(&format!("{cache}.tmp")).exists());
    c.api.fail.store(false,Ordering::SeqCst);
    assert_eq!(c.svc.refresh(1).await.unwrap(),json!({"success":true,"message":"Cover refreshed","cover_path":cache}));
    assert_eq!(std::fs::read(format!("{grid}/99p.png")).unwrap(),b"new cover");
    assert!(c.api.calls.lock().unwrap().iter().all(|p|p=="/large"));
    assert!(!std::path::Path::new(&format!("{grid}/../shortcuts.vdf")).exists());
    let db_path=c.g.e.dir.path().join("db.sqlite");
    *c.api.after_write.lock().unwrap()=Some(Arc::new(move ||{
        rusqlite::Connection::open(&db_path).unwrap().execute("UPDATE roms SET shortcut_app_id=100 WHERE rom_id=1",[]).unwrap();
    }));
    assert_eq!(c.svc.refresh(1).await.unwrap()["reason"],"state_changed");
    assert!(!std::path::Path::new(&format!("{grid}/100p.png")).exists());
}

#[tokio::test]
async fn simultaneous_sync_fetch_and_refresh_share_atomic_cover_writes() {
    let c=CoverEnv::new().await;
    c.g.e.seed_rom(1,"switch","game",Some(99),None).await;
    let grid=c.grid();
    c.g.e.romm.set_rom(json!({"id":1,"path_cover_large":"/large"}));
    let state=super::library_state::LibraryRunState::default();
    let roms=[json!({"id":1,"path_cover_large":"/large"})];
    let (sync,fetch,refresh)=tokio::join!(
        c.art.download(&roms,&state,&|_,_|{}),c.svc.fetch(1),c.svc.refresh(1));
    assert_eq!(sync[&1],c.art.assets.cache_path(1));
    assert_eq!(fetch["base64"],STANDARD.encode(b"new cover"));
    assert_eq!(refresh.unwrap()["success"],true);
    assert_eq!(std::fs::read(format!("{grid}/99p.png")).unwrap(),b"new cover");
    assert!(!std::path::Path::new(&format!("{}.tmp",c.art.assets.cache_path(1))).exists());
}
