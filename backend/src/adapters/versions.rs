//! Version snapshots and conditional binding moves. Never replaces identity rows.
use async_trait::async_trait;
use rusqlite::{Connection,OptionalExtension,params};
use serde_json::{Value,json};
use crate::{adapters::db::Db,domain::{errors::failure,sibling_group::{compute_sibling_group_key,target_in_group},settings::truthy},services::ports::{VersionGroup,VersionRepo}};
pub struct SqliteVersions{db:Db}
impl SqliteVersions{pub fn new(db:Db)->Self{Self{db}}}
fn array(raw:String)->Value{serde_json::from_str::<Value>(&raw).ok().filter(Value::is_array).unwrap_or(json!([]))}
fn member(c:&Connection,id:i64)->rusqlite::Result<Option<Value>>{
    c.query_row("SELECT rom_id,name,fs_name,platform_slug,sibling_group_key,shortcut_app_id,regions,languages,revision,tags,is_main_sibling,emulator_override,selected_disc,EXISTS(SELECT 1 FROM rom_installs i WHERE i.rom_id=roms.rom_id) FROM roms WHERE rom_id=?1",[id],|r|Ok(json!({
        "rom_id":r.get::<_,i64>(0)?,"name":r.get::<_,String>(1)?,"fs_name":r.get::<_,String>(2)?,"platform_slug":r.get::<_,String>(3)?,"sibling_group_key":r.get::<_,Option<String>>(4)?,"app_id":r.get::<_,Option<i64>>(5)?,
        "regions":array(r.get(6)?),"languages":array(r.get(7)?),"revision":r.get::<_,String>(8)?,"tags":array(r.get(9)?),"is_main_sibling":r.get::<_,bool>(10)?,"emulator_override":r.get::<_,Option<String>>(11)?,"selected_disc":r.get::<_,Option<String>>(12)?,"installed":r.get::<_,bool>(13)?}))).optional()
}
fn group(c:&Connection,app:i64)->rusqlite::Result<Option<VersionGroup>>{
    let id=c.query_row("SELECT rom_id FROM roms WHERE shortcut_app_id=?1",[app],|r|r.get(0)).optional()?;
    let Some(bound)=id.map(|id|member(c,id)).transpose()?.flatten()else{return Ok(None);};
    let ids=if let Some(key)=bound["sibling_group_key"].as_str(){let mut stmt=c.prepare("SELECT rom_id FROM roms WHERE sibling_group_key=?1 ORDER BY rom_id")?;stmt.query_map([key],|r|r.get::<_,i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?}else{vec![bound["rom_id"].as_i64().unwrap()]};
    let members=ids.into_iter().map(|id|member(c,id).map(Option::unwrap)).collect::<rusqlite::Result<_>>()?;
    Ok(Some(VersionGroup{bound,members}))
}
#[async_trait]
impl VersionRepo for SqliteVersions{
    async fn group(&self,app:i64)->anyhow::Result<Option<VersionGroup>>{self.db.call(move|c|{let tx=c.unchecked_transaction()?;let result=group(&tx,app)?;tx.commit()?;Ok(result)}).await}
    async fn member(&self,id:i64)->anyhow::Result<Option<Value>>{self.db.call(move|c|member(c,id)).await}
    async fn bind_version(&self,expected:&VersionGroup,target:i64,server:Option<Value>,now:&str)->anyhow::Result<Value>{
        let expected=expected.clone();let now=now.to_string();self.db.call(move|c|{
            let tx=c.transaction()?;let app=expected.bound["app_id"].as_i64().unwrap();
            if group(&tx,app)?.as_ref()!=Some(&expected){return Ok(failure("state_changed","Game state changed while switching versions. Try again."));}
            let key=expected.bound["sibling_group_key"].as_str();
            if let Some(row)=member(&tx,target)?{
                if !target_in_group(key,Some(row["sibling_group_key"].as_str()),None,false){return Ok(crate::domain::version_picker::not_in_group());}
                if row["app_id"].as_i64().is_some_and(|id|id!=app){return Ok(crate::domain::version_picker::bound_elsewhere(target));}
            }else{
                let Some(raw)=server else{return Ok(crate::domain::version_picker::not_in_group());};
                let slug=raw["platform_slug"].as_str().filter(|s|!s.is_empty()).or_else(||expected.bound["platform_slug"].as_str()).unwrap_or("");
                if target<=0 || raw["id"].as_i64()!=Some(target) || slug.is_empty(){return Ok(failure("invalid_target",&format!("Version {target}'s server details could not be turned into a local version.")));}
                let group_key=key.map(str::to_string).unwrap_or_else(||compute_sibling_group_key(&raw));
                let arrays=|k:&str|raw[k].as_array().cloned().map(Value::Array).unwrap_or(json!([])).to_string();
                tx.execute("INSERT INTO roms (rom_id,platform_slug,name,fs_name,last_synced_at,igdb_id,sibling_group_key,regions,languages,revision,tags,is_main_sibling) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![target,slug,raw["name"].as_str().unwrap_or(""),raw["fs_name"].as_str().unwrap_or(""),now,raw["igdb_id"].as_i64(),group_key,arrays("regions"),arrays("languages"),raw["revision"].as_str().unwrap_or(""),arrays("tags"),truthy(&raw["rom_user"]["is_main_sibling"])])?;
            }
            tx.execute("UPDATE roms SET shortcut_app_id=NULL WHERE shortcut_app_id=?1",[app])?;
            tx.execute("UPDATE roms SET shortcut_app_id=?1 WHERE rom_id=?2",params![app,target])?;
            let installed=member(&tx,target)?.unwrap()["installed"].clone();tx.commit()?;
            Ok(json!({"success":true,"target_installed":installed}))
        }).await
    }
}
