//! Connected components over fresh RomM sibling edges; resident keys are authoritative.
use std::collections::{BTreeMap, BTreeSet};
use serde_json::Value;
const SOURCES: [(&str,&str);8] = [("igdb_id","igdb"),("ss_id","ss"),("moby_id","moby"),("ra_id","ra"),("hasheous_id","hasheous"),("launchbox_id","launchbox"),("tgdb_id","tgdb"),("flashpoint_id","flashpoint")];
fn scalar(v:&Value)->String { match v {Value::Null=>"None".into(),Value::String(s)=>s.clone(),_=>v.to_string()} }
pub fn compute_sibling_group_key(rom:&Value)->String {
    for (field,source) in SOURCES { if !rom[field].is_null() { return format!("{source}:{}:{}",scalar(&rom[field]),scalar(&rom["platform_id"])); } }
    format!("romm:{}:{}",scalar(&rom["id"]),scalar(&rom["platform_id"]))
}
fn parse(key:&str)->Option<(&str,&str)> {
    let parts:Vec<_>=key.split(':').collect();
    if parts.len()==3 && SOURCES.iter().any(|(_,s)|*s==parts[0]) {Some((parts[0],parts[1]))} else {None}
}
fn root(parents:&BTreeMap<i64,i64>,mut id:i64)->i64 { while parents[&id]!=id {id=parents[&id];} id }

pub fn compute_component_group_keys(roms:&[Value],resident_keys:&BTreeMap<i64,String>)->BTreeMap<i64,String> {
    let mut residents=resident_keys.clone();
    let mut fresh=BTreeMap::new();
    for rom in roms {
        let Some(id)=rom["id"].as_i64() else {continue};
        if let Some(key)=rom["sibling_group_key"].as_str().filter(|s|!s.is_empty()) {residents.insert(id,key.into());}
        else {fresh.insert(id,rom);}
    }
    let mut parents:BTreeMap<_,_>=fresh.keys().map(|id|(*id,*id)).collect();
    let mut candidates:BTreeMap<i64,Vec<(String,String)>>=BTreeMap::new();
    for (&id,rom) in &fresh {
        for sibling in rom["sibling_roms"].as_array().into_iter().flatten() {
            let Some(target)=sibling["id"].as_i64().filter(|t|*t>0 && *t!=id) else {continue};
            if fresh.contains_key(&target) {
                let a=root(&parents,id);let b=root(&parents,target);
                parents.insert(a.max(b),a.min(b));
            } else if let Some((source,value))=residents.get(&target).and_then(|key|parse(key)) {
                candidates.entry(id).or_default().push((source.into(),value.into()));
            }
        }
    }
    let mut components:BTreeMap<i64,Vec<i64>>=BTreeMap::new();
    for &id in fresh.keys() {components.entry(root(&parents,id)).or_default().push(id);}
    let mut result=BTreeMap::new();
    for members in components.values() {
        let mut values:BTreeMap<String,BTreeSet<String>>=BTreeMap::new();
        for id in members {
            for (field,source) in SOURCES {let value=&fresh[id][field];if !value.is_null(){values.entry(source.into()).or_default().insert(scalar(value));}}
            for (source,value) in candidates.get(id).into_iter().flatten() {values.entry(source.clone()).or_default().insert(value.clone());}
        }
        let canonical=SOURCES.iter().find_map(|(_,s)|values.get(*s).map(|v|(*s,v)));
        for id in members {
            let rom=fresh[id];
            let key=match canonical {Some((source,v)) if v.len()==1=>format!("{source}:{}:{}",v.first().unwrap(),scalar(&rom["platform_id"])),_=>compute_sibling_group_key(rom)};
            result.insert(*id,key);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;use serde_json::json;
    #[test]fn target_membership_accepts_uneven_coverage_but_refuses_canonical_conflicts(){
        let key=Some("igdb:7:5");
        assert!(target_in_group(key,Some(key),None,false));assert!(!target_in_group(key,Some(None),None,true));assert!(!target_in_group(key,Some(Some("igdb:8:5")),None,true));
        assert!(target_in_group(key,None,Some(&json!({"ss_id":9})),true));assert!(target_in_group(key,None,Some(&json!({"igdb_id":7})),true));assert!(!target_in_group(key,None,Some(&json!({"igdb_id":8})),true));assert!(!target_in_group(key,None,None,true));assert!(!target_in_group(key,None,Some(&json!({})),false));
        assert!(!target_in_group(Some("romm:1:5"),None,Some(&json!({})),true));assert!(target_in_group(None,None,None,true));assert!(!target_in_group(None,None,None,false));
    }
    #[test]
    fn uneven_coverage_components_are_stable_and_bridges_refuse_merge() {
        let mut roms=vec![json!({"id":1,"platform_id":5,"igdb_id":7,"ss_id":9,"sibling_roms":[{"id":2}]}),json!({"id":2,"platform_id":5,"ss_id":9,"sibling_roms":[{"id":3}]}),json!({"id":3,"platform_id":5,"sibling_roms":[{"id":1}]}),json!({"id":4,"platform_id":5})];
        let keys=compute_component_group_keys(&roms,&BTreeMap::new());
        assert_eq!(keys[&1],"igdb:7:5");assert_eq!(keys[&2],keys[&1]);assert_eq!(keys[&3],keys[&1]);assert_eq!(keys[&4],"romm:4:5");
        roms.reverse();assert_eq!(compute_component_group_keys(&roms,&BTreeMap::new()),keys);
        roms[1]["igdb_id"]=json!(8);
        let keys=compute_component_group_keys(&roms,&BTreeMap::new());
        assert_eq!(keys[&1],"igdb:7:5");assert_eq!(keys[&2],"ss:9:5");assert_eq!(keys[&3],"igdb:8:5");
    }
    #[test]
    fn resident_candidates_keep_keys_and_unknown_edges_do_nothing() {
        let roms=vec![json!({"id":1,"platform_id":5,"sibling_roms":[{"id":2},{"id":99},{"id":0},{"id":1}]}),json!({"id":2,"platform_id":5,"sibling_group_key":"igdb:7:5"})];
        assert_eq!(compute_component_group_keys(&roms,&BTreeMap::new()),BTreeMap::from([(1,"igdb:7:5".into())]));
        assert_eq!(compute_component_group_keys(&roms[..1],&BTreeMap::from([(2,"igdb:7:5".into())]))[&1],"igdb:7:5");
        assert_eq!(compute_component_group_keys(&roms[..1],&BTreeMap::from([(2,"romm:2:5".into())]))[&1],"romm:1:5");
    }
}

/// Shared authority for picker enablement and the switch write guard.
pub fn target_in_group(bound:Option<&str>, local:Option<Option<&str>>, detail:Option<&Value>, server_sibling:bool)->bool {
    if local.is_none() && !server_sibling{return false;}
    let Some(bound)=bound else{return true;};
    if let Some(key)=local{return key==Some(bound);}
    let Some((source,value))=parse(bound)else{return false;};
    let Some(detail)=detail else{return false;};
    let field=SOURCES.iter().find(|(_,s)|*s==source).unwrap().0;
    detail[field].is_null() || scalar(&detail[field])==value
}
