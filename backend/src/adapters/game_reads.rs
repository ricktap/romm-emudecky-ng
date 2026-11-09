//! Offline game-page projections and narrow artwork cross-reference writes.
use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use serde_json::{Value, json};
use crate::{adapters::db::Db, services::ports::{GameDetailSnapshot, GameReadRepo}};

pub struct SqliteGameReads { db: Db }
impl SqliteGameReads { pub fn new(db: Db) -> Self { Self { db } } }

fn array(raw: String) -> Value {
    serde_json::from_str::<Value>(&raw).ok().filter(Value::is_array).unwrap_or(json!([]))
}

const META: &str = "summary,genres,companies,first_release_date,average_rating,game_modes,player_count,cached_at,steam_categories";
fn metadata_row(r: &Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({"summary": r.get::<_,String>(0)?, "genres": array(r.get(1)?), "companies": array(r.get(2)?),
        "first_release_date": r.get::<_,Option<i64>>(3)?, "average_rating": r.get::<_,Option<f64>>(4)?,
        "game_modes": array(r.get(5)?), "player_count": r.get::<_,String>(6)?, "cached_at": r.get::<_,f64>(7)?,
        "steam_categories": array(r.get(8)?)}))
}
fn metadata(c: &rusqlite::Connection, id: i64) -> rusqlite::Result<Option<Value>> {
    c.query_row(&format!("SELECT {META} FROM rom_metadata WHERE rom_id=?1"), [id], metadata_row).optional()
}

#[async_trait]
impl GameReadRepo for SqliteGameReads {
    async fn metadata(&self, id: i64) -> anyhow::Result<Option<Value>> { self.db.call(move |c| metadata(c,id)).await }
    async fn all_metadata(&self) -> anyhow::Result<Value> {
        self.db.call(|c| {
            let mut stmt=c.prepare(&format!("SELECT {META},rom_id FROM rom_metadata ORDER BY rom_id"))?;
            let rows=stmt.query_map([], |r| Ok((r.get::<_,i64>(9)?.to_string(),metadata_row(r)?)))?;
            Ok(Value::Object(rows.collect::<rusqlite::Result<_>>()?))
        }).await
    }
    async fn game_detail(&self, app: i64) -> anyhow::Result<Option<GameDetailSnapshot>> {
        self.db.call(move |c| {
            let tx=c.unchecked_transaction()?;
            let rom=tx.query_row("SELECT rom_id,name,fs_name,platform_slug,regions,languages,revision,tags,is_main_sibling,ra_id FROM roms WHERE shortcut_app_id=?1",[app],|r|Ok(json!({
                "rom_id":r.get::<_,i64>(0)?,"name":r.get::<_,String>(1)?,"fs_name":r.get::<_,String>(2)?,"platform_slug":r.get::<_,String>(3)?,
                "regions":array(r.get(4)?),"languages":array(r.get(5)?),"revision":r.get::<_,String>(6)?,"tags":array(r.get(7)?),"is_main_sibling":r.get::<_,i64>(8)?!=0,
                "ra_id":r.get::<_,Option<i64>>(9)?}))).optional()?;
            let Some(rom)=rom else { return Ok(None); };
            let id=rom["rom_id"].as_i64().unwrap();
            let install_file=tx.query_row("SELECT file_path FROM rom_installs WHERE rom_id=?1",[id],|r|r.get(0)).optional()?;
            let meta=metadata(&tx,id)?;
            let check:Option<Option<String>>=tx.query_row("SELECT last_sync_check_at FROM rom_save_states WHERE rom_id=?1",[id],|r|r.get(0)).optional()?;
            let save_status=if let Some(check)=check {
                let mut stmt=tx.prepare("SELECT filename,last_sync_at FROM rom_save_files WHERE rom_id=?1 ORDER BY filename")?;
                let files=stmt.query_map([id],|r| {let time:String=r.get(1)?;Ok(json!({"filename":r.get::<_,String>(0)?,"status":"synced","last_sync_at":if time.is_empty(){None}else{Some(time)}}))})?.collect::<rusqlite::Result<Vec<_>>>()?;
                Some(json!({"files":files,"last_sync_check_at":check,"conflicts":[]}))
            } else {None};
            let names=tx.query_row("SELECT value FROM kv_config WHERE key='platform_names'",[],|r|r.get::<_,String>(0)).optional()?.and_then(|s|serde_json::from_str::<Value>(&s).ok()).filter(Value::is_object).unwrap_or(json!({}));
            tx.commit()?;
            Ok(Some(GameDetailSnapshot { rom, install_file, metadata:meta, save_status, platform_names:names }))
        }).await
    }
    async fn remember_sgdb_id(&self, id:i64, sgdb:i64)->anyhow::Result<()> {
        self.db.call(move |c| c.execute("UPDATE roms SET sgdb_id=?1 WHERE rom_id=?2",params![sgdb,id]).map(|_|())).await
    }
    async fn remember_cover(&self, id:i64, app:i64, path:&str)->anyhow::Result<bool> {
        let path=path.to_string();
        self.db.call(move |c| c.execute("UPDATE roms SET cover_path=?1 WHERE rom_id=?2 AND shortcut_app_id=?3",params![path,id,app]).map(|n|n>0)).await
    }
}
