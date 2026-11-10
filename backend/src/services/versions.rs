//! Sibling picker and binding moves; no saves or game files migrate.
use std::{sync::Arc,collections::{BTreeMap,BTreeSet,HashSet}};
use serde_json::{Value,json};
use crate::{domain::{errors::failure,sibling_group::{target_in_group,compute_sibling_group_key},sibling_resolution::representative,version_picker::*},services::{ports::*,launch::LaunchService}};
pub struct VersionServiceConfig {
    pub repo:Arc<dyn VersionRepo>,pub romm:Arc<dyn RommRomApi>,pub settings:SettingsStore,
    pub launch:Arc<LaunchService>,pub saves:Arc<dyn SaveSafety>,pub clock:Arc<dyn Clock>,
    pub mutation_lock:Arc<tokio::sync::Mutex<()>>,
    pub reachable:Arc<dyn RommConnectionApi>,pub active:Arc<dyn Fn()->HashSet<i64>+Send+Sync>,
}
pub struct VersionService{c:VersionServiceConfig}
impl VersionService {
    pub fn new(c:VersionServiceConfig)->Self{Self{c}}
    pub async fn list(&self,app:i64)->anyhow::Result<Value>{
        let Some(group)=self.c.repo.group(app).await?else{return Ok(json!({"multi_version":false}));};
        let key=group.bound["sibling_group_key"].as_str();
        let ids:BTreeSet<_>=group.members.iter().filter_map(|m|m["rom_id"].as_i64()).collect();
        let bound=group.bound["rom_id"].as_i64().unwrap();
        let (detail,failed)=match self.c.romm.get_rom(bound).await{Ok(v)=>(v,false),Err(_)=>(Value::Null,true)};
        let mut entries=vec![];let mut ranks=vec![];
        for row in &group.members{
            let id=row["rom_id"].as_i64().unwrap();let label=stem(row["fs_name"].as_str().unwrap_or(""));
            entries.push(entry(row,id,row["name"].as_str().unwrap_or(""),label,true,row["installed"]==true,true));
            let mut rank=row.clone();rank["fs_name_no_ext"]=json!(label);ranks.push(rank);
        }
        let mut tasks=tokio::task::JoinSet::new();let mut stubs=vec![];let mut seen=BTreeSet::new();
        for stub in detail["sibling_roms"].as_array().into_iter().flatten(){
            let Some(id)=stub["id"].as_i64().filter(|id|*id>0&&!ids.contains(id))else{continue;};
            if !seen.insert(id){continue;}
            stubs.push((id,stub.clone()));
            let api=self.c.romm.clone();tasks.spawn(async move{(id,api.get_rom(id).await.ok())});
        }
        let mut details=BTreeMap::new();
        while let Some(task)=tasks.join_next().await{if let Ok((id,Some(raw)))=task{if raw["id"].as_i64()==Some(id){details.insert(id,raw);}}}
        for (id,stub) in stubs {
            let raw=details.get(&id);let local=self.c.repo.member(id).await?;
            let switchable=target_in_group(key,local.as_ref().map(|r|r["sibling_group_key"].as_str()),raw,true);
            let meta=raw_meta(raw.unwrap_or(&Value::Null));
            let name=raw.and_then(|r|r["name"].as_str()).filter(|s|!s.is_empty()).or_else(||stub["name"].as_str()).unwrap_or("");
            let fallback=id.to_string();let label=stub["fs_name_no_ext"].as_str().filter(|s|!s.is_empty()).or_else(||stub["name"].as_str().filter(|s|!s.is_empty())).unwrap_or(&fallback);
            entries.push(entry(&meta,id,name,label,false,false,switchable));
            if switchable{let mut rank=meta;rank["rom_id"]=json!(id);rank["fs_name_no_ext"]=json!(label);ranks.push(rank);}
        }
        if entries.len()<=1{return Ok(json!({"multi_version":false}));}
        let preferred=self.c.settings.get_str("preferred_region").unwrap_or("auto".into());
        let default=representative(&ranks,&BTreeSet::new(),&BTreeSet::new(),&preferred).ok();
        for e in &mut entries{e["active"]=json!(e["rom_id"]==bound);e["is_default"]=json!(e["rom_id"].as_i64()==default);}
        Ok(json!({"multi_version":true,"versions":entries,"server_query_failed":failed}))
    }
    pub async fn switch(&self,app:i64,target:i64,allow:bool)->anyhow::Result<Value>{
        let _mutation=self.c.mutation_lock.lock().await;
        let Some(group)=self.c.repo.group(app).await?else{return Ok(failure("not_found",&format!("No game is bound to shortcut {app}")));};
        let bound=group.bound["rom_id"].as_i64().unwrap();let key=group.bound["sibling_group_key"].as_str();
        let active=(self.c.active)();
        if active.contains(&target)||group.members.iter().any(|r|r["rom_id"].as_i64().is_some_and(|id|active.contains(&id))){return Ok(failure("download_in_progress","Pause or cancel the running download first."));}
        if target==bound{return Ok(self.success(app,target,group.bound["installed"]==true).await);}
        let local=self.c.repo.member(target).await?;
        if let Some(row)=&local{
            if !target_in_group(key,Some(row["sibling_group_key"].as_str()),None,false){return Ok(not_in_group());}
            if row["app_id"].as_i64().is_some_and(|id|id!=app){return Ok(bound_elsewhere(target));}
        }
        if !allow&&group.bound["installed"]==true&&self.c.saves.drift(bound).await.unwrap_or(false){
            return Ok(json!({"success":false,"reason":"unsynced_saves","message":"Unsynced saves on the current version.",
                "server_reachable":self.c.reachable.heartbeat_once().await.is_ok(),"unsynced_rom_id":bound,"unsynced_version_name":stem(group.bound["fs_name"].as_str().unwrap_or(""))}));
        }
        let server=if local.is_none(){
            let raw=match self.c.romm.get_rom(target).await{Ok(r)=>r,Err(_)=>return Ok(failure("server_unreachable","RomM server not reachable."))};
            if raw["id"].as_i64()!=Some(target){return Ok(failure("invalid_target",&format!("Version {target}'s server details could not be turned into a local version.")));}
            let sibling=raw["sibling_roms"].as_array().into_iter().flatten().any(|r|r["id"].as_i64()==Some(bound))||key.is_some_and(|k|compute_sibling_group_key(&raw)==k);
            if !target_in_group(key,None,Some(&raw),sibling){return Ok(not_in_group());}Some(raw)
        }else{None};
        // Recheck admission after a potentially slow fetch.
        let active=(self.c.active)();if active.contains(&target)||group.members.iter().any(|r|r["rom_id"].as_i64().is_some_and(|id|active.contains(&id))){return Ok(failure("download_in_progress","Pause or cancel the running download first."));}
        let result=self.c.repo.bind_version(&group,target,server,&self.c.clock.now_iso()).await?;
        if result["success"]!=true{return Ok(result);}
        Ok(self.success(app,target,result["target_installed"]==true).await)
    }
    async fn success(&self,app:i64,id:i64,installed:bool)->Value{
        let options=if installed{self.c.launch.relaunch_item_for_rom(id).await.and_then(|r|r["launch_options"].as_str().map(str::to_string)).unwrap_or_default()}else{String::new()};
        json!({"success":true,"rom_id":id,"target_installed":installed,"launch_options":options,"app_id":app})
    }
}
