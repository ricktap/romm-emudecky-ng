//! Durable session folds and outbox changes share the existing SQLite actor.
use async_trait::async_trait;
use rusqlite::{Connection,OptionalExtension,params};
use serde_json::{Value,json};
use crate::{adapters::db::Db,domain::{playtime as kernel,errors::failure},services::ports::{Playtime,PendingSession,PlaytimeRepo}};
pub struct SqlitePlaytime {db:Db}
impl SqlitePlaytime {pub fn new(db:Db)->Self{Self{db}}}
fn read(c:&Connection,id:i64)->rusqlite::Result<Option<Playtime>> {
    c.query_row("SELECT total_seconds,session_count,last_session_start,last_session_start_monotonic,last_session_duration_sec,last_played FROM rom_playtime WHERE rom_id=?1",[id],|r|Ok(Playtime{total_seconds:r.get(0)?,session_count:r.get(1)?,last_start:r.get(2)?,monotonic_start:r.get(3)?,last_duration:r.get(4)?,last_played:r.get(5)?})).optional()
}
fn write(c:&Connection,id:i64,p:&Playtime)->rusqlite::Result<()> {
    c.execute("INSERT INTO rom_playtime (rom_id,total_seconds,session_count,last_session_start,last_session_start_monotonic,last_session_duration_sec,last_played) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(rom_id) DO UPDATE SET total_seconds=excluded.total_seconds,session_count=excluded.session_count,last_session_start=excluded.last_session_start,last_session_start_monotonic=excluded.last_session_start_monotonic,last_session_duration_sec=excluded.last_session_duration_sec,last_played=excluded.last_played",params![id,p.total_seconds,p.session_count,p.last_start,p.monotonic_start,p.last_duration,p.last_played])?;Ok(())
}
fn exists(c:&Connection,id:i64)->rusqlite::Result<bool>{c.query_row("SELECT EXISTS(SELECT 1 FROM roms WHERE rom_id=?1)",[id],|r|r.get(0))}
#[async_trait]
impl PlaytimeRepo for SqlitePlaytime {
    async fn begin(&self,id:i64,now:&str,mono:f64)->anyhow::Result<bool>{let now=now.to_string();self.db.call(move|c|{
        let tx=c.transaction()?;if !exists(&tx,id)?{return Ok(false);}
        let mut p=read(&tx,id)?.unwrap_or_default();p.last_start=Some(now);p.monotonic_start=Some(mono);
        write(&tx,id,&p)?;tx.commit()?;Ok(true)
    }).await}
    async fn finish(&self,id:i64,now:&str,mono:f64)->anyhow::Result<Value>{let now=now.to_string();self.db.call(move|c|{
        let tx=c.transaction()?;
        let Some(mut p)=read(&tx,id)?.filter(|p|p.last_start.is_some())else{return Ok(failure("no_active_session","No active session"));};
        let Some((start,duration))=kernel::finish(&mut p,&now,mono)else{return Ok(failure("unknown","Failed to calculate session duration"));};
        let device:Option<String>=tx.query_row("SELECT value FROM kv_config WHERE key='device_id'",[],|r|r.get(0)).optional()?;
        if let Some(device)=device.filter(|d|!d.is_empty()).filter(|_|kernel::ingestable(&start,&now)){
            tx.execute("INSERT INTO rom_playtime_sessions (rom_id,start_time,device_id,end_time,duration_ms) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(rom_id,start_time) DO UPDATE SET device_id=excluded.device_id,end_time=excluded.end_time,duration_ms=excluded.duration_ms,attempts=0",params![id,start,device,now,duration*1000])?;
        }
        write(&tx,id,&p)?;tx.commit()?;
        Ok(json!({"success":true,"duration_sec":duration,"total_seconds":p.total_seconds,"session_count":p.session_count}))
    }).await}
    async fn get(&self,id:i64)->anyhow::Result<Option<Playtime>>{self.db.call(move|c|read(c,id)).await}
    async fn all(&self)->anyhow::Result<Value>{self.db.call(|c|{
        let mut stmt=c.prepare("SELECT rom_id,total_seconds,session_count,last_played FROM rom_playtime ORDER BY rom_id")?;
        let rows=stmt.query_map([],|r|Ok((r.get::<_,i64>(0)?.to_string(),json!({"total_seconds":r.get::<_,i64>(1)?,"session_count":r.get::<_,i64>(2)?,"last_played":r.get::<_,Option<String>>(3)?}))))?;
        Ok(json!({"playtime":rows.collect::<rusqlite::Result<serde_json::Map<String,Value>>>()?}))
    }).await}
    async fn pending(&self,limit:usize)->anyhow::Result<Vec<PendingSession>>{self.db.call(move|c|{
        let mut stmt=c.prepare("SELECT rom_id,start_time,device_id,end_time,duration_ms,attempts FROM rom_playtime_sessions ORDER BY rom_id,start_time LIMIT ?1")?;
        stmt.query_map([limit as i64],|r|Ok(PendingSession{rom_id:r.get(0)?,start:r.get(1)?,device:r.get(2)?,end:r.get(3)?,duration_ms:r.get(4)?,attempts:r.get(5)?}))?.collect()
    }).await}
    async fn apply_outcomes(&self,outcomes:Vec<(PendingSession,bool)>)->anyhow::Result<()>{self.db.call(move|c|{
        let tx=c.transaction()?;for (r,drop_row) in outcomes {
            let sql=if drop_row{"DELETE FROM rom_playtime_sessions WHERE rom_id=?1 AND start_time=?2 AND device_id=?3 AND end_time=?4 AND duration_ms=?5 AND attempts=?6"}else{"UPDATE rom_playtime_sessions SET attempts=attempts+1 WHERE rom_id=?1 AND start_time=?2 AND device_id=?3 AND end_time=?4 AND duration_ms=?5 AND attempts=?6"};
            tx.execute(sql,params![r.rom_id,r.start,r.device,r.end,r.duration_ms,r.attempts])?;
        }tx.commit()
    }).await}
    async fn reconcile(&self,id:i64,rows:Vec<Value>)->anyhow::Result<Playtime>{self.db.call(move|c|{
        let tx=c.transaction()?;if !exists(&tx,id)?{return Ok(Playtime::default());}
        let old=read(&tx,id)?;if old.is_none()&&rows.is_empty(){return Ok(Playtime::default());}
        let mut p=old.unwrap_or_default();kernel::reconcile(&mut p,&rows);write(&tx,id,&p)?;tx.commit()?;Ok(p)
    }).await}
}
