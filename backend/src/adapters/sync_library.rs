//! Unit commits share one transaction: identity, version metadata and cached detail.
use std::collections::BTreeMap;
use async_trait::async_trait;
use rusqlite::{params,OptionalExtension};
use serde_json::{Value,json};
use crate::{adapters::db::Db,domain::{shortcut_data::build_rom_metadata,settings::truthy},services::ports::{SyncLibraryRepo,SyncUnitCommit}};
#[derive(Clone)]
pub struct SqliteSyncLibrary {db:Db}
impl SqliteSyncLibrary {pub fn new(db:Db)->Self{Self{db}}}
fn string<'a>(v:&'a Value,k:&str)->&'a str{v[k].as_str().unwrap_or("")}
fn array(v:&Value,k:&str)->String{v[k].as_array().map(|a|json!(a).to_string()).unwrap_or("[]".into())}
#[async_trait]
impl SyncLibraryRepo for SqliteSyncLibrary {
    async fn registry(&self)->anyhow::Result<BTreeMap<i64,Value>> {
        self.db.call(|c| {
            let mut stmt=c.prepare("SELECT rom_id,shortcut_app_id,name,fs_name,platform_slug,sibling_group_key,cover_path,sgdb_id,ra_id FROM roms ORDER BY rom_id")?;
            let rows=stmt.query_map([],|r|Ok((r.get::<_,i64>(0)?,json!({"app_id":r.get::<_,Option<i64>>(1)?,"name":r.get::<_,String>(2)?,"fs_name":r.get::<_,String>(3)?,"platform_slug":r.get::<_,String>(4)?,"sibling_group_key":r.get::<_,Option<String>>(5)?,"cover_path":r.get::<_,Option<String>>(6)?,"sgdb_id":r.get::<_,Option<i64>>(7)?,"ra_id":r.get::<_,Option<i64>>(8)?}))))?;
            rows.collect()
        }).await
    }
    async fn commit_unit(&self,commit:SyncUnitCommit)->anyhow::Result<()> {
        // Validate optional metadata before the transaction. A malformed cache block
        // must not prevent the valid identity/version rows from being committed.
        let metadata:BTreeMap<_,_>=commit.raw.iter().filter(|r|r.get("metadatum").is_some_and(truthy)).filter_map(|r|Some((r["id"].as_i64()?,build_rom_metadata(r,commit.cached_at).ok()?))).collect();
        self.db.call(move |c| {
            let tx=c.unchecked_transaction()?;
            for built in &commit.built {
                let Some(id)=built["rom_id"].as_i64().filter(|id|*id>0) else{continue};
                let slug=string(built,"platform_slug");if slug.is_empty(){continue;}
                let existing:Option<(Option<i64>,Option<String>,Option<i64>,Option<i64>)>=tx.query_row("SELECT shortcut_app_id,cover_path,sgdb_id,ra_id FROM roms WHERE rom_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
                let app=commit.bindings.get(&id).copied().or_else(||existing.as_ref().and_then(|e|e.0));
                if let Some(app)=app {tx.execute("UPDATE roms SET shortcut_app_id=NULL WHERE shortcut_app_id=?1 AND rom_id!=?2",params![app,id])?;}
                let cover=commit.covers.get(&id).filter(|s|!s.is_empty()).cloned().or_else(||existing.as_ref().and_then(|e|e.1.clone()));
                let sgdb=built["sgdb_id"].as_i64().or_else(||existing.as_ref().and_then(|e|e.2));
                let ra=built["ra_id"].as_i64().or_else(||existing.as_ref().and_then(|e|e.3));
                tx.execute("INSERT INTO roms (rom_id,platform_slug,name,fs_name,shortcut_app_id,last_synced_at,cover_path,igdb_id,sgdb_id,ra_id,sibling_group_key,regions,languages,revision,tags,is_main_sibling)
                    VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)
                    ON CONFLICT(rom_id) DO UPDATE SET platform_slug=excluded.platform_slug,name=excluded.name,fs_name=excluded.fs_name,shortcut_app_id=excluded.shortcut_app_id,last_synced_at=excluded.last_synced_at,cover_path=excluded.cover_path,igdb_id=excluded.igdb_id,sgdb_id=excluded.sgdb_id,ra_id=excluded.ra_id,sibling_group_key=excluded.sibling_group_key,regions=excluded.regions,languages=excluded.languages,revision=excluded.revision,tags=excluded.tags,is_main_sibling=excluded.is_main_sibling",
                    params![id,slug,string(built,"name"),string(built,"fs_name"),app,commit.synced_at,cover,built["igdb_id"].as_i64(),sgdb,ra,built["sibling_group_key"].as_str(),array(built,"regions"),array(built,"languages"),string(built,"revision"),array(built,"tags"),i64::from(truthy(&built["is_main_sibling"]))])?;
                if let Some(meta)=metadata.get(&id) {
                    tx.execute("INSERT INTO rom_metadata (rom_id,summary,genres,companies,game_modes,steam_categories,player_count,first_release_date,average_rating,cached_at)
                        VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(rom_id) DO UPDATE SET summary=excluded.summary,genres=excluded.genres,companies=excluded.companies,game_modes=excluded.game_modes,steam_categories=excluded.steam_categories,player_count=excluded.player_count,first_release_date=excluded.first_release_date,average_rating=excluded.average_rating,cached_at=excluded.cached_at",
                        params![id,string(meta,"summary"),array(meta,"genres"),array(meta,"companies"),array(meta,"game_modes"),array(meta,"steam_categories"),string(meta,"player_count"),meta["first_release_date"].as_i64(),meta["average_rating"].as_f64(),commit.cached_at])?;
                }
            }
            tx.commit()
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn commit()->SyncUnitCommit {
        SyncUnitCommit {built:vec![json!({"rom_id":1,"name":"A","fs_name":"a.nsp","platform_slug":"switch","sibling_group_key":"igdb:7:1","regions":["USA"]}),json!({"rom_id":2,"name":"B","fs_name":"b.nsp","platform_slug":"switch","sibling_group_key":"igdb:7:1"})],raw:vec![json!({"id":1,"summary":"Cached","metadatum":{"genres":["Action"]}})],bindings:BTreeMap::from([(1,123)]),covers:BTreeMap::from([(1,"/grid/123p.png".into())]),synced_at:"now".into(),cached_at:5.0}
    }
    async fn fixture()->(tempfile::TempDir,Db,SqliteSyncLibrary) {
        let dir=tempfile::tempdir().unwrap();let(db,_)=Db::open(&dir.path().join("sync.db")).await.unwrap();let repo=SqliteSyncLibrary::new(db.clone());(dir,db,repo)
    }
    #[tokio::test]
    async fn all_siblings_persist_only_acked_bind_and_repeated_sync_preserves_children_pins_and_artwork() {
        let(_dir,db,repo)=fixture().await;
        assert!(repo.registry().await.unwrap().is_empty());
        repo.commit_unit(commit()).await.unwrap();
        let rows=repo.registry().await.unwrap();assert_eq!(rows.len(),2);assert_eq!(rows[&1]["app_id"],123);assert_eq!(rows[&2]["app_id"],Value::Null);
        db.call(|c|c.execute_batch("UPDATE roms SET emulator_override='Eden',selected_disc='base.nsp',sgdb_id=77,ra_id=88 WHERE rom_id=1;
            INSERT INTO rom_installs (rom_id,file_path,platform_slug,system,installed_at) VALUES (1,'/roms/base.nsp','switch','switch','before');
            INSERT INTO rom_playtime (rom_id,total_seconds,session_count) VALUES (1,42,1);")).await.unwrap();
        let mut next=commit();next.bindings.clear();next.covers.clear();next.raw=vec![json!({"id":1})];
        repo.commit_unit(next).await.unwrap();
        let row=repo.registry().await.unwrap()[&1].clone();assert_eq!(row["app_id"],123);assert_eq!(row["sgdb_id"],77);assert_eq!(row["ra_id"],88);assert_eq!(row["cover_path"],"/grid/123p.png");
        db.call(|c| {
            assert_eq!(c.query_row("SELECT emulator_override,selected_disc FROM roms WHERE rom_id=1",[],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?,("Eden".into(),"base.nsp".into()));
            assert_eq!(c.query_row("SELECT COUNT(*) FROM rom_installs",[],|r|r.get::<_,i64>(0))?,1);
            assert_eq!(c.query_row("SELECT total_seconds FROM rom_playtime WHERE rom_id=1",[],|r|r.get::<_,i64>(0))?,42);
            assert_eq!(c.query_row("SELECT summary FROM rom_metadata WHERE rom_id=1",[],|r|r.get::<_,String>(0))?,"Cached");Ok(())
        }).await.unwrap();
    }
    #[tokio::test]
    async fn rebind_unbinds_the_previous_holder_and_malformed_metadata_preserves_cache() {
        let(_dir,db,repo)=fixture().await;repo.commit_unit(commit()).await.unwrap();
        let mut next=commit();next.bindings=BTreeMap::from([(2,123)]);next.covers.clear();next.raw=vec![json!({"id":1,"metadatum":{"average_rating":"bad"}})];
        repo.commit_unit(next).await.unwrap();let rows=repo.registry().await.unwrap();assert_eq!(rows[&1]["app_id"],Value::Null);assert_eq!(rows[&2]["app_id"],123);
        db.call(|c|{assert_eq!(c.query_row("SELECT summary FROM rom_metadata WHERE rom_id=1",[],|r|r.get::<_,String>(0))?,"Cached");Ok(())}).await.unwrap();
    }
    #[tokio::test]
    async fn unit_failure_rolls_back_identity_binding_and_cache_together() {
        let(_dir,db,repo)=fixture().await;
        db.call(|c|c.execute_batch("CREATE TRIGGER reject_second BEFORE INSERT ON roms WHEN NEW.rom_id=2 BEGIN SELECT RAISE(ABORT,'simulated disk failure'); END;")).await.unwrap();
        assert!(repo.commit_unit(commit()).await.is_err());assert!(repo.registry().await.unwrap().is_empty());
        db.call(|c|{assert_eq!(c.query_row("SELECT COUNT(*) FROM rom_metadata",[],|r|r.get::<_,i64>(0))?,0);Ok(())}).await.unwrap();
    }
}

#[async_trait]
impl crate::services::ports::SyncFinalizeRepo for SqliteSyncLibrary {
    async fn finalize(&self,request:crate::services::ports::SyncFinalization)->anyhow::Result<Value> {
        self.db.call(move|c|{
            let tx=c.unchecked_transaction()?;
            for id in request.stale_ids {tx.execute("UPDATE roms SET shortcut_app_id=NULL WHERE rom_id=?1",[id])?;}
            tx.execute("INSERT INTO kv_config(key,value) VALUES ('platform_names',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[json!(request.platform_names).to_string()])?;
            let mut rows=BTreeMap::new();
            {let mut stmt=tx.prepare("SELECT rom_id,shortcut_app_id,platform_slug,sibling_group_key FROM roms ORDER BY rom_id")?;
            let result=stmt.query_map([],|r|Ok((r.get::<_,i64>(0)?,(r.get::<_,Option<i64>>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?))))?;
            for row in result {let (id,value)=row?;rows.insert(id,value);}}
            let mut platforms:BTreeMap<String,Vec<i64>>=BTreeMap::new();let mut group_binding=BTreeMap::new();
            for (&id,(app,slug,group)) in &rows {
                let Some(app)=app else{continue};
                if request.create_platform_groups || request.platform_rom_ids.contains(&id) {platforms.entry(request.platform_names.get(slug).unwrap_or(slug).clone()).or_default().push(*app);}
                if let Some(group)=group {group_binding.entry(group.clone()).or_insert(*app);}
            }
            let mut collections:BTreeMap<String,Vec<i64>>=BTreeMap::new();
            for (name,ids) in request.memberships {
                let mut seen=std::collections::BTreeSet::new();let mut apps=vec![];
                for id in ids {
                    if let Some((app,_,group))=rows.get(&id) {
                        let app=app.or_else(||group.as_ref().and_then(|k|group_binding.get(k).copied()));
                        if let Some(app)=app {if seen.insert(app){apps.push(app);}}
                    }
                }
                if !apps.is_empty(){collections.insert(name,apps);}
            }
            tx.commit()?;Ok(json!({"platform_app_ids":platforms,"romm_collection_app_ids":collections}))
        }).await
    }
}

#[async_trait]
impl crate::services::ports::ShortcutBindings for SqliteSyncLibrary {
    async fn unbind(&self,ids:Vec<i64>)->anyhow::Result<()> {
        self.db.call(move|c|{let tx=c.unchecked_transaction()?;for id in ids{tx.execute("UPDATE roms SET shortcut_app_id=NULL WHERE rom_id=?1",[id])?;}tx.commit()}).await
    }
    async fn reconcile(&self,live:std::collections::BTreeSet<i64>)->anyhow::Result<usize>{
        self.db.call(move|c|{let tx=c.unchecked_transaction()?;let mut missing=vec![];{
            let mut stmt=tx.prepare("SELECT rom_id,shortcut_app_id FROM roms WHERE shortcut_app_id IS NOT NULL")?;
            for row in stmt.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?)))?{let(id,app)=row?;if !live.contains(&app){missing.push(id);}}
        }for id in &missing{tx.execute("UPDATE roms SET shortcut_app_id=NULL WHERE rom_id=?1",[id])?;}tx.commit()?;Ok(missing.len())}).await
    }
}
