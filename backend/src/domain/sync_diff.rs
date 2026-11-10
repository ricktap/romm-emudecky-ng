//! Group-aware shortcut dispatch. Partial collection views never infer deletion.
use std::collections::{BTreeMap,BTreeSet};
use serde_json::{Value,json};
use crate::domain::{settings::truthy,sibling_resolution::{representative,canonical_name}};

pub fn collapse_sibling_groups(shortcuts:&[Value],registry:&BTreeMap<i64,Value>,installed:&BTreeSet<i64>,complete:bool,preferred:&str)->Result<Vec<Value>,String> {
    let mut groups:Vec<(Option<String>,Vec<Value>)>=vec![];
    let fetched:BTreeMap<_,_>=shortcuts.iter().filter_map(|m|Some((m["rom_id"].as_i64()?,m["sibling_group_key"].as_str().map(str::to_string)))).collect();
    for member in shortcuts {
        let key=member["sibling_group_key"].as_str().map(str::to_string);
        if let Some((_,members))=groups.iter_mut().find(|(k,_)|*k==key) {members.push(member.clone());}
        else {groups.push((key,vec![member.clone()]));}
    }
    let mut bound:BTreeMap<Option<String>,Vec<(i64,&Value)>>=BTreeMap::new();
    let mut bound_ids=BTreeSet::new();
    for (&id,entry) in registry {
        if !truthy(&entry["app_id"]) {continue;}
        bound_ids.insert(id);
        let key=fetched.get(&id).cloned().unwrap_or_else(||entry["sibling_group_key"].as_str().map(str::to_string));
        bound.entry(key).or_default().push((id,entry));
    }
    let mut emitted=vec![];
    for (key,members) in groups {
        let existing=bound.get(&key).cloned().unwrap_or_default();
        let surviving:BTreeSet<_>=existing.iter().filter(|(id,_)|members.iter().any(|m|m["rom_id"].as_i64()==Some(*id))).map(|(id,_)|*id).collect();
        if !surviving.is_empty() {emitted.extend(members.into_iter().filter(|m|m["rom_id"].as_i64().is_some_and(|id|surviving.contains(&id))));continue;}
        if !existing.is_empty() && !complete {continue;}
        let winner=representative(&members,installed,&bound_ids,preferred).map_err(str::to_string)?;
        let mut entry=members.iter().find(|m|m["rom_id"].as_i64()==Some(winner)).unwrap().clone();
        if existing.is_empty() {entry["name"]=json!(canonical_name(&members,preferred).map_err(str::to_string)?);}
        else {
            let (kept,reg)=existing.iter().min_by_key(|(id,_)|id).unwrap();
            entry["rom_id"]=json!(kept);entry["bind_rom_id"]=json!(winner);
            for key in ["name","platform_slug"] {if let Some(value)=reg.get(key){entry[key]=value.clone();}}
            entry["fs_name"]=reg.get("fs_name").cloned().unwrap_or(json!(""));
        }
        emitted.push(entry);
    }
    Ok(emitted)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn members()->Vec<Value> {vec![json!({"rom_id":1,"name":"USA","sibling_group_key":"igdb:7:5","regions":["USA"],"launch_options":"usa"}),json!({"rom_id":2,"name":"Japan","sibling_group_key":"igdb:7:5","regions":["Japan"],"launch_options":"japan"})]}
    #[test]
    fn mint_uses_canonical_name_but_installed_representative_launch() {
        let raw=members();let out=collapse_sibling_groups(&raw,&BTreeMap::new(),&BTreeSet::from([2]),true,"auto").unwrap();
        assert_eq!(out.len(),1);assert_eq!(out[0]["rom_id"],2);assert_eq!(out[0]["name"],"USA");assert_eq!(out[0]["launch_options"],"japan");assert_eq!(raw[1]["name"],"Japan");
    }
    #[test]
    fn grandfather_survivors_and_rebind_only_complete_views() {
        let registry=BTreeMap::from([(9,json!({"app_id":123,"name":"Sticky","fs_name":"old.nsp","platform_slug":"switch","sibling_group_key":"igdb:7:5"}))]);
        assert!(collapse_sibling_groups(&members(),&registry,&BTreeSet::new(),false,"auto").unwrap().is_empty());
        let out=collapse_sibling_groups(&members(),&registry,&BTreeSet::new(),true,"auto").unwrap();
        assert_eq!(out[0]["rom_id"],9);assert_eq!(out[0]["bind_rom_id"],1);assert_eq!(out[0]["name"],"Sticky");assert_eq!(out[0]["fs_name"],"old.nsp");
        let registry=BTreeMap::from([(1,json!({"app_id":123,"sibling_group_key":null})),(2,json!({"app_id":456,"sibling_group_key":null}))]);
        let out=collapse_sibling_groups(&members(),&registry,&BTreeSet::new(),true,"auto").unwrap();assert_eq!(out.len(),2);
    }
}

#[derive(Debug,Default,PartialEq)]
pub struct Classification {
    pub new:Vec<Value>,pub changed:Vec<Value>,pub unchanged_ids:Vec<i64>,pub stale:Vec<i64>,pub disabled_count:usize,
}
pub fn classify_roms(shortcuts:&[Value],registry:&BTreeMap<i64,Value>,platform_names:&BTreeSet<String>)->Classification {
    let mut out=Classification::default();let mut current=BTreeSet::new();
    for entry in shortcuts {
        let Some(id)=entry["rom_id"].as_i64() else{continue};current.insert(id);
        let reg=registry.get(&id);
        match reg.filter(|r|truthy(&r["app_id"])) {
            None=>out.new.push(entry.clone()),
            Some(reg) if reg["name"]!=entry["name"] || reg["platform_slug"]!=entry["platform_slug"] || reg["fs_name"]!=entry.get("fs_name").cloned().unwrap_or(json!("")) => {
                let mut changed=entry.clone();changed["existing_app_id"]=reg["app_id"].clone();out.changed.push(changed);
            }
            _=>out.unchanged_ids.push(id),
        }
    }
    for (&id,reg) in registry {
        if !current.contains(&id) {out.stale.push(id);if !reg["platform_name"].as_str().is_some_and(|n|platform_names.contains(n)){out.disabled_count+=1;}}
    }
    out
}
pub fn select_stale_removals(candidates:&[(i64,i64)],synced_app_ids:&BTreeSet<i64>)->Vec<(i64,i64)> {
    candidates.iter().copied().filter(|(_,app)|!synced_app_ids.contains(app)).collect()
}
pub fn include_platform_member(id:i64,platform_ids:Option<&BTreeSet<i64>>,create_groups:bool)->bool {
    create_groups || platform_ids.is_none_or(|ids|ids.contains(&id))
}
pub fn collection_diff(memberships:&BTreeMap<String,Vec<i64>>,previous:&[String])->Value {
    let current:BTreeSet<_>=memberships.keys().cloned().collect();let previous:BTreeSet<_>=previous.iter().cloned().collect();
    let added:Vec<_>=current.difference(&previous).collect();let removed:Vec<_>=previous.difference(&current).collect();
    json!({"has_changes":!added.is_empty() || !removed.is_empty() || !current.is_empty(),"added":added,"removed":removed})
}
pub fn platform_collection_diff(shortcuts:&[Value],platform_ids:Option<&BTreeSet<i64>>,previous:&[String],create_groups:bool)->Value {
    let current:BTreeSet<_>=shortcuts.iter().filter(|m|m["rom_id"].as_i64().is_some_and(|id|include_platform_member(id,platform_ids,create_groups))).filter_map(|m|m["platform_name"].as_str().filter(|s|!s.is_empty()).map(str::to_string)).collect();
    let previous:BTreeSet<_>=previous.iter().cloned().collect();let added=current.difference(&previous).count();let removed=previous.difference(&current).count();
    json!({"has_changes":added>0||removed>0,"added_count":added,"removed_count":removed})
}
#[cfg(test)]
mod diff_tests {
    use super::*;
    #[test]
    fn identity_diff_ignores_display_names_and_stale_scan_protects_reused_app_ids() {
        let registry=BTreeMap::from([(1,json!({"app_id":123,"name":"A","fs_name":"a","platform_slug":"switch","platform_name":"Old name"})),(2,json!({"app_id":456,"name":"B","fs_name":"b","platform_slug":"switch","platform_name":"Switch"})),(3,json!({"app_id":789,"platform_name":"Disabled"}))]);
        let current=vec![json!({"rom_id":1,"name":"A","fs_name":"a","platform_slug":"switch","platform_name":"New name"}),json!({"rom_id":2,"name":"Changed","fs_name":"b","platform_slug":"switch"}),json!({"rom_id":4,"name":"New"})];
        let diff=classify_roms(&current,&registry,&BTreeSet::from(["Switch".into()]));
        assert_eq!(diff.new.len(),1);assert_eq!(diff.changed[0]["existing_app_id"],456);assert_eq!(diff.unchanged_ids,vec![1]);assert_eq!(diff.stale,vec![3]);assert_eq!(diff.disabled_count,1);assert!(current[1].get("existing_app_id").is_none());
        assert_eq!(select_stale_removals(&[(1,123),(3,789)],&BTreeSet::from([123])),vec![(3,789)]);
    }
    #[test]
    fn collection_diffs_distinguish_no_tracking_from_no_platforms() {
        assert!(include_platform_member(1,None,false));assert!(!include_platform_member(1,Some(&BTreeSet::new()),false));assert!(include_platform_member(1,Some(&BTreeSet::new()),true));
        assert_eq!(collection_diff(&BTreeMap::from([("New".into(),vec![1])]),&["Old".into()]),json!({"has_changes":true,"added":["New"],"removed":["Old"]}));
        assert_eq!(collection_diff(&BTreeMap::from([("Same".into(),vec![1])]),&["Same".into()])["has_changes"],true);
        let shortcuts=vec![json!({"rom_id":1,"platform_name":"Switch"})];
        assert_eq!(platform_collection_diff(&shortcuts,Some(&BTreeSet::new()),&["Old".into()],false),json!({"has_changes":true,"added_count":0,"removed_count":1}));
        assert_eq!(platform_collection_diff(&shortcuts,None,&[],false)["added_count"],1);
    }
}
