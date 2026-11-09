//! Offline-safe local playtime plus a durable, bounded native-session outbox.
use std::{sync::Arc,collections::{BTreeMap,VecDeque}};
use serde_json::{Value,json};
use tokio::sync::Mutex;
use crate::{domain::{errors::RommError,playtime::{projection,rejected_indices}},services::ports::*};
pub struct PlaytimeServiceConfig {pub repo:Arc<dyn PlaytimeRepo>,pub api:Arc<dyn RommPlaytimeApi>,pub kv:Arc<dyn KvConfigRepo>,pub clock:Arc<dyn Clock>,pub saves:Arc<dyn SaveSafety>}
pub struct PlaytimeService {c:PlaytimeServiceConfig,flush_lock:Mutex<()>,tasks:Mutex<tokio::task::JoinSet<()>>}
impl PlaytimeService {
    pub fn new(c:PlaytimeServiceConfig)->Arc<Self>{Arc::new(Self{c,flush_lock:Mutex::new(()),tasks:Mutex::new(tokio::task::JoinSet::new())})}
    pub async fn start(&self,id:i64)->anyhow::Result<Value>{
        Ok(if self.c.repo.begin(id,&self.c.clock.now_iso(),self.c.clock.monotonic()).await?{json!({"success":true})}else{crate::domain::errors::failure("unknown_rom","Unknown ROM")})
    }
    pub async fn end(&self,id:i64)->anyhow::Result<Value>{
        let result=self.c.repo.finish(id,&self.c.clock.now_iso(),self.c.clock.monotonic()).await?;
        if result["success"]==true{self.flush().await;}Ok(result)
    }
    pub async fn schedule_flush(self:&Arc<Self>){let mut tasks=self.tasks.lock().await;while tasks.try_join_next().is_some(){}let s=self.clone();tasks.spawn(async move{s.flush().await;});}
    pub async fn shutdown(&self){let mut tasks=self.tasks.lock().await;tasks.abort_all();while tasks.join_next().await.is_some(){}}
    pub async fn all(&self)->anyhow::Result<Value>{self.c.repo.all().await}
    pub async fn scope_notice(&self)->anyhow::Result<Value>{Ok(json!({"pending":self.c.kv.get("playtime_scope_notice").await?.is_some()}))}
    pub async fn flush(&self){if let Err(e)=self.flush_inner().await{tracing::debug!("Play-session flush failed: {e}");}}
    async fn flush_inner(&self)->anyhow::Result<()> {
        let _guard=self.flush_lock.lock().await;
        let mut groups:BTreeMap<String,Vec<PendingSession>>=BTreeMap::new();
        for row in self.c.repo.pending(100).await?{groups.entry(row.device.clone()).or_default().push(row);}
        let mut outcomes=vec![];
        for (device,rows) in groups {
            // Every rejection either shrinks a batch or falls back to isolated rows.
            let mut queue=VecDeque::from([rows]);
            while let Some(rows)=queue.pop_front(){
                let batch=rows.iter().map(|r|json!({"rom_id":r.rom_id,"start_time":r.start,"end_time":r.end,"duration_ms":r.duration_ms})).collect();
                match self.c.api.ingest(&device,batch).await {
                    Ok(response)=>{for (i,row) in rows.into_iter().enumerate(){
                        let status=response["results"].as_array().into_iter().flatten().rev().find(|r|r["index"].as_u64()==Some(i as u64)).and_then(|r|r["status"].as_str());
                        match status{Some("created"|"duplicate"|"skipped")=>outcomes.push((row,true)),Some(_)=>{let drop_row=row.attempts>=4;outcomes.push((row,drop_row));},None=>{}}
                    }},
                    Err(PlaytimeApiError::Rejected(detail))=>{
                        let indices=rejected_indices(&detail,rows.len());
                        if !indices.is_empty(){let mut survivors=vec![];for (i,row) in rows.into_iter().enumerate(){if indices.contains(&i){outcomes.push((row,true));}else{survivors.push(row);}}if !survivors.is_empty(){queue.push_back(survivors);}}
                        else if rows.len()>1{for row in rows{queue.push_back(vec![row]);}}
                        else{let row=rows.into_iter().next().unwrap();let drop_row=row.attempts>=4;outcomes.push((row,drop_row));}
                    },
                    Err(_)=>{} // Offline: keep every unacknowledged row untouched.
                }
            }
        }
        self.c.repo.apply_outcomes(outcomes).await
    }
    pub async fn reconcile(&self,id:i64)->anyhow::Result<Value>{
        self.flush().await;
        let (p,failed)=match self.c.api.sessions(id).await {
            Ok(rows)=>{self.c.kv.delete("playtime_scope_notice").await?;(self.c.repo.reconcile(id,rows).await?,false)},
            Err(e)=>{if matches!(e,RommError::Forbidden(_)){self.c.kv.set("playtime_scope_notice","1").await?;}(self.c.repo.get(id).await?.unwrap_or_default(),true)}
        };
        let mut result=projection(&p);result["server_query_failed"]=json!(failed);Ok(result)
    }
    pub async fn finalize(&self,id:i64)->Value {
        let total=self.end(id).await.ok().filter(|r|r["success"]==true).map(|r|r["total_seconds"].clone());
        let result=self.c.saves.post_exit(id).await.unwrap_or(json!({}));
        let offline=result["offline"]==true;let success=result["success"]==true;
        let synced=result["synced"].as_i64();let conflicts=result["conflicts"].as_array().cloned().unwrap_or_default();
        let benign=matches!(result["reason"].as_str(),Some("unsupported"|"savefiles_in_content_dir"));
        let body=if benign{None}else if offline{Some("Server offline — saves will sync next time")}else if success&&synced.is_some_and(|s|s>0){Some("Saves synced with RomM")}else if success{None}else{Some(result["message"].as_str().filter(|s|!s.is_empty()).unwrap_or("Failed to sync saves after exit"))};
        let count=conflicts.len();
        json!({"total_seconds":total,"sync":{"offline":offline,"success":success,"synced":if benign{Some(0)}else{synced},"conflicts":conflicts,
            "toast_title":body.map(|_|"RomM Save Sync"),"toast_body":body,"conflicts_toast":if count==0{None}else{Some(format!("{count} save conflict{} need resolution",if count==1{""}else{"s"}))}},"migration":null})
    }
}
