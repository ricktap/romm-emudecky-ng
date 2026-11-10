//! Validate the one trusted plugin release feed; never installs an update.
use serde_json::{Value,json};
pub const RELEASES:&str="https://github.com/ricktap/romm-emudecky-ng/releases";
pub const LATEST:&str="https://api.github.com/repos/ricktap/romm-emudecky-ng/releases/latest";
pub const LATEST_PAGE:&str="https://github.com/ricktap/romm-emudecky-ng/releases/latest";
pub const DOWNLOAD:&str="https://github.com/ricktap/romm-emudecky-ng/releases/download/";
pub fn version(s:&str)->Option<[u64;3]>{
    let s=s.trim().strip_prefix('v').unwrap_or(s.trim());let p:Vec<_>=s.split('.').collect();
    if p.len()!=3{return None;}
    let n=|s:&str|if !s.is_empty()&&s.bytes().all(|c|c.is_ascii_digit()){s.parse().ok()}else{None};
    Some([n(p[0])?,n(p[1])?,n(p[2])?])
}
pub fn release_version(v:&Value)->Option<String>{let s=v["tag_name"].as_str()?.trim();version(s)?;Some(s.strip_prefix('v').unwrap_or(s).into())}
pub fn parse(v:&Value)->Option<Value>{
    if v["draft"]==true||v["prerelease"]==true{return None;}
    let tag=release_version(v)?;
    let asset=v["assets"].as_array()?.iter().find(|a|a["name"]=="romm-emudecky-ng.zip")?;
    let url=asset["browser_download_url"].as_str()?;
    if !url.starts_with(DOWNLOAD)||url.contains(".."){return None;}
    let sha=asset["digest"].as_str()?.strip_prefix("sha256:")?;
    if sha.len()!=64||!sha.bytes().all(|b|b.is_ascii_hexdigit()){return None;}
    let page=v["html_url"].as_str().filter(|s|s.starts_with("https://github.com/ricktap/romm-emudecky-ng/")).unwrap_or(RELEASES);
    Some(json!({"latest_version":tag,"notes":v["body"].as_str().unwrap_or(""),"asset_url":url,"sha256":sha.to_lowercase(),"html_url":page}))
}
/// `current` may carry a pre-release suffix (`0.40.0-rc.1`): it counts as older than the same stable version
/// and newer than any lower one. `latest` is always a plain release (see [`parse`]).
pub fn is_newer(latest:&str,current:&str)->bool{
    let cur=current.trim();let cur=cur.strip_prefix('v').unwrap_or(cur);
    let (core,pre)=match cur.split_once('-'){Some((c,_))=>(c,true),None=>(cur,false)};
    let current_key=version(core).map(|v|(v,!pre)).unwrap_or(([0,0,0],true));
    version(latest).is_some_and(|l|(l,true)>current_key)
}

#[cfg(test)]mod tests{
    use super::*;
    fn release()->Value{json!({"tag_name":"v0.40.0","body":"notes","html_url":"https://github.com/ricktap/romm-emudecky-ng/releases/tag/v0.40.0","assets":[{"name":"romm-emudecky-ng.zip","browser_download_url":format!("{DOWNLOAD}v0.40.0/romm-emudecky-ng.zip"),"digest":format!("sha256:{}","A".repeat(64))}]})}
    #[test]fn only_plain_versions_and_trusted_hashed_assets_are_installable(){
        let r=release();assert_eq!(parse(&r).unwrap()["sha256"],"a".repeat(64));
        for field in ["draft","prerelease"]{let mut v=r.clone();v[field]=json!(true);assert!(parse(&v).is_none());}
        for tag in ["v0.40.0-beta.1","0.40.0+build","V0.40.0","0.40","garbage"]{let mut v=r.clone();v["tag_name"]=json!(tag);assert!(parse(&v).is_none());}
        for url in ["https://evil.invalid/x","https://github.com/ricktap/romm-emudecky-ng/releases/download/../bad"]{let mut v=r.clone();v["assets"][0]["browser_download_url"]=json!(url);assert!(parse(&v).is_none());}
        for digest in ["sha256:abc","md5:bad",""]{let mut v=r.clone();v["assets"][0]["digest"]=json!(digest);assert!(parse(&v).is_none());}
        let mut v=r;v["html_url"]=json!("https://evil.invalid/");assert_eq!(parse(&v).unwrap()["html_url"],RELEASES);
        assert!(is_newer("v1.0.0","0.99.99"));assert!(!is_newer("0.40.0","0.40.0"));assert!(is_newer("0.40.0","broken"));assert!(!is_newer("1.0.0-rc1","0.0.0"));
        // a release candidate is not offered an older stable, but is offered its own final release
        assert!(!is_newer("0.33.2","0.40.0-rc.1"));assert!(!is_newer("0.40.0","0.40.1-rc.1"));
        assert!(is_newer("0.40.0","0.40.0-rc.1"));assert!(is_newer("0.40.1","v0.40.0-rc.1"));assert!(is_newer("0.40.0","garbage-rc"));
        assert!(parse(&Value::Null).is_none());
    }
}
