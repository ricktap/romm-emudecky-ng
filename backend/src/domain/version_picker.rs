//! Version-picker projections and canonical failures, shared by read/write paths.
use serde_json::{Value,json};
use crate::domain::{errors::failure,settings::truthy};
pub fn not_in_group()->Value{failure("not_in_group","This version's metadata match conflicts with this game's — fix the match in RomM to switch to it.")}
pub fn bound_elsewhere(id:i64)->Value{failure("bound_elsewhere",&format!("Version {id} is already used by another shortcut."))}
pub fn stem(fs:&str)->&str{fs.rfind('.').filter(|i|*i>0).map_or(fs,|i|&fs[..i])}
pub fn raw_meta(raw:&Value)->Value{
    let mut m=json!({"revision":raw["revision"].as_str().unwrap_or(""),"is_main_sibling":truthy(&raw["rom_user"]["is_main_sibling"])});
    for k in ["regions","languages","tags"]{m[k]=raw[k].as_array().cloned().map(Value::Array).unwrap_or(json!([]));}m
}
pub fn entry(row:&Value,id:i64,name:&str,label:&str,synced:bool,installed:bool,switchable:bool)->Value{
    let mut e=json!({"rom_id":id,"name":name,"label":label,"synced":synced,"installed":installed,"switchable":switchable});
    for k in ["regions","languages","revision","tags"]{e[k]=row[k].clone();}e
}
