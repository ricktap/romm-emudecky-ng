//! Pure conversion of RomM payloads to the frontend shortcut shape.
use std::collections::BTreeMap;
use serde_json::{Value,json};
use crate::domain::{launch::build_launch_options,settings::truthy,sibling_group::compute_sibling_group_key};

pub fn build_shortcuts_data(roms:&[Value],plugin_dir:&str,installed:&BTreeMap<i64,String>,invocations:&BTreeMap<i64,String>)->Result<Vec<Value>,String> {
    let root=plugin_dir.trim_end_matches('/');
    roms.iter().map(|rom| {
        let id=rom["id"].as_i64().ok_or("invalid ROM id")?;
        let name=rom["name"].as_str().ok_or("invalid ROM name")?;
        let fs=rom["fs_name"].as_str().unwrap_or("");
        let stem=fs.rfind('.').filter(|i|*i>0).map_or(fs,|i|&fs[..i]);
        let options=installed.get(&id).zip(invocations.get(&id).filter(|s|!s.is_empty())).map(|(path,invocation)|build_launch_options(invocation,path)).unwrap_or_default();
        let mut built=json!({"rom_id":id,"name":name,"fs_name":fs,"fs_name_no_ext":rom["fs_name_no_ext"].as_str().filter(|s|!s.is_empty()).unwrap_or(stem),
            "exe":format!("{root}/bin/rom-launcher-ng"),"start_dir":format!("{root}/bin"),"launch_options":options,
            "platform_name":rom.get("platform_name").cloned().unwrap_or(json!("Unknown")),"platform_slug":rom.get("platform_slug").cloned().unwrap_or(json!("")),
            "igdb_id":rom["igdb_id"],"sgdb_id":rom["sgdb_id"],"ra_id":rom["ra_id"],"cover_path":"",
            "sibling_group_key":rom["sibling_group_key"].as_str().filter(|s|!s.is_empty()).map(str::to_string).unwrap_or_else(||compute_sibling_group_key(rom)),
            "revision":rom["revision"].as_str().unwrap_or(""),"is_main_sibling":truthy(&rom["rom_user"]["is_main_sibling"])});
        for key in ["regions","languages","tags"] {built[key]=rom[key].as_array().cloned().map(Value::Array).unwrap_or(json!([]));}
        Ok(built)
    }).collect()
}

/// Map and validate the metadata block before opening the write transaction.
/// A malformed block is skipped independently of its ROM identity.
pub fn build_rom_metadata(rom:&Value,cached_at:f64)->Result<Value,String> {
    let meta=rom.get("metadatum").filter(|v|!v.is_null()).cloned().unwrap_or(json!({}));
    if !meta.is_object(){return Err("invalid metadatum".into());}
    let mut out=json!({"summary":rom["summary"].as_str().unwrap_or(""),"player_count":meta["player_count"].as_str().unwrap_or(""),"cached_at":cached_at});
    for key in ["genres","companies","game_modes"] {
        let values=match meta.get(key) {None|Some(Value::Null)=>vec![],Some(Value::Array(a)) if a.iter().all(Value::is_string)=>a.clone(),_=>return Err(format!("invalid {key}"))};
        out[key]=json!(values);
    }
    let release=&meta["first_release_date"];
    out["first_release_date"]=if release.is_null(){Value::Null}else{
        let n=release.as_i64().or_else(||release.as_str().and_then(|s|s.parse().ok())).ok_or("invalid release date")?;
        json!(n.div_euclid(1000))
    };
    let rating=&meta["average_rating"];
    out["average_rating"]=if rating.is_null(){Value::Null}else{json!(rating.as_f64().or_else(||rating.as_str().and_then(|s|s.parse().ok())).filter(|n|n.is_finite()).ok_or("invalid rating")?)};
    let mut cats=std::collections::BTreeSet::from([28]);
    for genre in out["genres"].as_array().unwrap() {
        let id=match genre.as_str().unwrap(){"Action"|"RPG"|"Role-playing (RPG)"|"Role-playing"=>Some(21),"Adventure"=>Some(25),"Strategy"=>Some(2),"Simulation"=>Some(28),"Sport"|"Sports"=>Some(18),"Racing"=>Some(9),"Puzzle"=>Some(4),_=>None};
        if let Some(id)=id {cats.insert(id);}
    }
    for mode in out["game_modes"].as_array().unwrap() {
        let id=match mode.as_str().unwrap(){"Single player"=>Some(2),"Multiplayer"=>Some(1),"Co-operative"=>Some(9),"Split screen"=>Some(24),"MMO"=>Some(20),_=>None};
        if let Some(id)=id {cats.insert(id);}
    }
    out["steam_categories"]=json!(cats);
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shortcut_shape_preserves_resident_keys_and_quotes_shell_characters() {
        let raw=vec![json!({"id":1,"name":"Game","fs_name":"Game.nsp","sibling_group_key":"igdb:7:5","rom_user":null}),json!({"id":2,"name":"Uninstalled","platform_id":5})];
        let built=build_shortcuts_data(&raw,"/plugin",&BTreeMap::from([(1,"/roms/$(touch bad).nsp".into())]),&BTreeMap::from([(1,"emulator -f -g".into())])).unwrap();
        assert_eq!(built[0]["launch_options"],"emulator -f -g \"/roms/\\$(touch bad).nsp\"");
        assert_eq!(built[0]["fs_name_no_ext"],"Game");assert_eq!(built[0]["sibling_group_key"],"igdb:7:5");
        assert_eq!(built[0]["exe"],"/plugin/bin/rom-launcher-ng");assert_eq!(built[0]["is_main_sibling"],false);
        assert_eq!(built[1]["launch_options"],"");assert_eq!(built[1]["sibling_group_key"],"romm:2:5");
        let unresolved=build_shortcuts_data(&raw,"/plugin",&BTreeMap::from([(1,"/roms/Game.nsp".into())]),&BTreeMap::new()).unwrap();
        assert_eq!(unresolved[0]["launch_options"],"");
        assert!(build_shortcuts_data(&[json!({})],"p",&BTreeMap::new(),&BTreeMap::new()).is_err());
    }
    #[test]
    fn metadata_maps_wire_numbers_and_categories_and_rejects_bad_blocks() {
        let out=build_rom_metadata(&json!({"summary":"Hi","metadatum":{"first_release_date":"1234567","average_rating":"8.5","genres":["Action","Adventure"],"game_modes":["Multiplayer","Split screen"]}}),5.0).unwrap();
        assert_eq!(out["first_release_date"],1234);assert_eq!(out["average_rating"],8.5);assert_eq!(out["steam_categories"],json!([1,21,24,25,28]));
        assert_eq!(build_rom_metadata(&json!({}),5.0).unwrap()["steam_categories"],json!([28]));
        for m in [json!([]),json!({"genres":false}),json!({"average_rating":"bad"}),json!({"first_release_date":"bad"})] {assert!(build_rom_metadata(&json!({"metadatum":m}),1.0).is_err());}
    }
}
