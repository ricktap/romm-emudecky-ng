//! Stable representative choice and mint-time naming, matching the Python ranking.
use std::{cmp::{Ordering, Reverse}, collections::BTreeSet};
use serde_json::Value;
use crate::domain::settings::truthy;

fn text<'a>(v:&'a Value,k:&str)->&'a str {v[k].as_str().unwrap_or("")}
fn prerelease(v:&Value)->bool {
    v["tags"].as_array().into_iter().flatten().any(|t| {
        let tag=t.as_str().unwrap_or("").trim().to_lowercase();
        ["alpha","beta","proto","sample","demo"].iter().any(|m|tag==*m || tag.strip_prefix(m).is_some_and(|r|r.trim_start().chars().next().is_some_and(|c|c.is_numeric())))
    })
}
fn region(v:&Value,preferred:&str)->(u8,usize,String) {
    v["regions"].as_array().into_iter().flatten().map(|r| {
        let r=r.as_str().unwrap_or("").to_lowercase();
        if preferred!="auto" && r==preferred.to_lowercase() {(0,0,String::new())}
        else if let Some(i)=["world","usa","europe","japan"].iter().position(|s|*s==r) {(1,i,String::new())}
        else {(2,0,r)}
    }).min().unwrap_or((3,0,String::new()))
}
// Arbitrary-length digit runs compare by length and lexical value, avoiding overflow.
fn natural(s:&str)->Vec<(u8,usize,String)> {
    let s=s.to_lowercase();let mut parts=vec![];let mut current=String::new();let mut digits=None;
    for c in s.chars() {
        let d=c.is_ascii_digit();
        if digits.is_some_and(|prev|prev!=d) {parts.push((digits.unwrap(),std::mem::take(&mut current)));}
        digits=Some(d);current.push(c);
    }
    if let Some(d)=digits {parts.push((d,current));}
    parts.into_iter().map(|(d,s)|if d {let n=s.trim_start_matches('0');let n=if n.is_empty(){"0"}else{n};(1,n.len(),n.into())}else{(0,0,s)}).collect()
}
fn compare(a:&Value,b:&Value,preferred:&str)->Ordering {
    (prerelease(a),region(a,preferred),Reverse(natural(text(a,"revision"))),text(a,"fs_name_no_ext").to_lowercase(),a["rom_id"].as_i64().unwrap_or(0))
    .cmp(&(prerelease(b),region(b,preferred),Reverse(natural(text(b,"revision"))),text(b,"fs_name_no_ext").to_lowercase(),b["rom_id"].as_i64().unwrap_or(0)))
}
pub fn representative(members:&[Value],installed:&BTreeSet<i64>,bound:&BTreeSet<i64>,preferred:&str)->Result<i64,&'static str> {
    if members.is_empty(){return Err("cannot resolve a representative for an empty sibling group");}
    for leg in 0..4 {
        let winner=members.iter().filter(|m|match leg {0=>m["rom_id"].as_i64().is_some_and(|id|installed.contains(&id)),1=>m["rom_id"].as_i64().is_some_and(|id|bound.contains(&id)),2=>truthy(&m["is_main_sibling"]),_=>true}).min_by(|a,b|compare(a,b,preferred));
        if let Some(winner)=winner {return winner["rom_id"].as_i64().ok_or("invalid rom_id");}
    }
    unreachable!()
}
pub fn canonical_name(members:&[Value],preferred:&str)->Result<String,&'static str> {
    members.iter().min_by(|a,b|compare(a,b,preferred)).map(|m|text(m,"name").to_string()).ok_or("cannot derive a canonical name for an empty sibling group")
}
#[cfg(test)]
mod tests {
    use super::*;use serde_json::json;
    #[test]
    fn installed_binding_default_and_pure_rank_are_distinct() {
        let members=vec![json!({"rom_id":1,"name":"USA","regions":["USA"],"revision":"2"}),json!({"rom_id":2,"name":"Japan","regions":["Japan"],"is_main_sibling":true}),json!({"rom_id":3,"name":"Newest","regions":["USA"],"revision":"10"})];
        let empty=BTreeSet::new();
        assert_eq!(representative(&members,&BTreeSet::from([1]),&BTreeSet::from([3]),"auto"),Ok(1));
        assert_eq!(representative(&members,&empty,&BTreeSet::from([3]),"auto"),Ok(3));
        assert_eq!(representative(&members,&empty,&empty,"auto"),Ok(2));
        assert_eq!(canonical_name(&members,"auto"),Ok("Newest".into()));
        assert_eq!(canonical_name(&members,"Japan"),Ok("Japan".into()));
        assert!(representative(&[],&empty,&empty,"auto").is_err());assert!(canonical_name(&[],"auto").is_err());
    }
    #[test]
    fn retail_beats_prerelease_and_unknown_tags_are_neutral() {
        let mut members=vec![json!({"rom_id":1,"name":"Draft","regions":["World"],"tags":["Beta 1"]}),json!({"rom_id":2,"name":"Retail","regions":["Japan"]})];
        assert_eq!(canonical_name(&members,"auto"),Ok("Retail".into()));
        for tag in ["Betamax","Prototype","Sampler","Unl"] {members[0]["tags"]=json!([tag]);assert_eq!(canonical_name(&members,"auto"),Ok("Draft".into()));}
        assert!(natural("99999999999999999999999999999999")>natural("10"));
        assert_eq!(natural("Rev 0002"),natural("Rev 2"));
    }
}
