//! Unauthenticated, strict-TLS GitHub release feed and plugin metadata reads.
use std::{path::PathBuf,time::Duration};
use async_trait::async_trait;
use serde_json::Value;
use crate::services::ports::{ReleaseFeed,ReleaseError,PluginMetadata};
/// The API answer is kept with its `ETag` and re-asked with `If-None-Match`. (Unauthenticated, a `304` still counts
/// against GitHub's 60-per-hour limit; the web redirect in `latest_tag` is what keeps routine checks off the API.)
pub struct GitHubReleases{client:reqwest::Client,url:String,web_latest:Option<String>,cached:tokio::sync::Mutex<Option<(String,Value)>>}
impl GitHubReleases{
    pub fn new(agent:&str,url:&str)->anyhow::Result<Self>{Ok(Self{client:reqwest::Client::builder().user_agent(agent).timeout(Duration::from_secs(10)).redirect(reqwest::redirect::Policy::none()).build()?,url:url.into(),web_latest:None,cached:Default::default()})}
    pub fn with_web_latest(mut self,url:&str)->Self{self.web_latest=Some(url.into());self}
}
/// Seconds GitHub asks us to wait: `Retry-After`, else the `X-RateLimit-Reset` epoch minus now.
fn retry_after(headers:&reqwest::header::HeaderMap)->Option<u64>{
    let num=|name:&str|headers.get(name)?.to_str().ok()?.trim().parse::<u64>().ok();
    if let Some(secs)=num("retry-after"){return Some(secs);}
    let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    num("x-ratelimit-reset").map(|at|at.saturating_sub(now))
}
/// `https://github.com/<owner>/<repo>/releases/tag/v1.2.3` -> `v1.2.3`; anything else is not a tag we trust.
fn tag_from_location(location:&str)->Option<String>{
    let tag=location.rsplit_once("/releases/tag/")?.1;
    (!tag.is_empty()&&tag.len()<=64&&tag.bytes().all(|b|b.is_ascii_alphanumeric()||matches!(b,b'.'|b'-'|b'+'|b'_'))).then(||tag.to_string())
}
#[async_trait]
impl ReleaseFeed for GitHubReleases{
    async fn latest_tag(&self)->Result<String,ReleaseError>{
        let url=self.web_latest.as_ref().ok_or(ReleaseError::Unreachable)?;
        let response=self.client.get(url).send().await.map_err(|_|ReleaseError::Unreachable)?;
        if !response.status().is_redirection(){return Err(ReleaseError::Unreachable);}
        response.headers().get("location").and_then(|l|l.to_str().ok()).and_then(tag_from_location).ok_or(ReleaseError::BadResponse)
    }
    async fn latest(&self)->Result<Value,ReleaseError>{
        let mut cached=self.cached.lock().await;
        let mut request=self.client.get(&self.url).header("Accept","application/vnd.github+json").header("X-GitHub-Api-Version","2022-11-28");
        if let Some((etag,_))=&*cached{request=request.header("If-None-Match",etag.as_str());}
        let response=request.send().await.map_err(|_|ReleaseError::Unreachable)?;
        let status=response.status();if matches!(status.as_u16(),403|429){return Err(ReleaseError::RateLimited(retry_after(response.headers())));}
        if status.as_u16()==304{return cached.as_ref().map(|(_,v)|v.clone()).ok_or(ReleaseError::BadResponse);}
        if !status.is_success(){return Err(ReleaseError::Unreachable);}
        let etag=response.headers().get("etag").and_then(|e|e.to_str().ok()).map(str::to_string);
        let value=response.json::<Value>().await.map_err(|_|ReleaseError::BadResponse).and_then(|v|if v.is_object(){Ok(v)}else{Err(ReleaseError::BadResponse)})?;
        *cached=etag.map(|e|(e,value.clone()));
        Ok(value)
    }
}
pub struct FilePluginMetadata{dir:PathBuf}
impl FilePluginMetadata{pub fn new(dir:PathBuf)->Self{Self{dir}}}
#[async_trait]
impl PluginMetadata for FilePluginMetadata{
    async fn metadata(&self)->anyhow::Result<(String,String)>{
        let version=crate::adapters::plugin_metadata::read_plugin_version(&self.dir).await;
        let name=tokio::fs::read(self.dir.join("plugin.json")).await.ok().and_then(|b|serde_json::from_slice::<Value>(&b).ok()).and_then(|v|v["name"].as_str().map(str::to_string)).unwrap_or_default();
        Ok((version,name))
    }
}

#[cfg(test)]mod tests{
    use super::*;
    use wiremock::{Mock,MockServer,ResponseTemplate,matchers::{path,header}};
    #[tokio::test]async fn feed_headers_statuses_invalid_json_and_redirects(){
        let server=MockServer::start().await;
        for (status,body,kind) in [(200,"{}","ok"),(200,"[]","bad"),(200,"broken","bad"),(403,"{}","rate"),(429,"{}","rate"),(500,"{}","offline"),(302,"{}","offline")]{
            server.reset().await;Mock::given(path("/latest")).and(header("user-agent","romm-emudecky-ng/test")).and(header("accept","application/vnd.github+json")).and(header("x-github-api-version","2022-11-28")).respond_with(ResponseTemplate::new(status).set_body_raw(body,"application/json").insert_header("location","https://invalid.example/")).expect(1).mount(&server).await;
            let result=GitHubReleases::new("romm-emudecky-ng/test",&format!("{}/latest",server.uri())).unwrap().latest().await;
            assert!(match kind{"ok"=>result.is_ok(),"bad"=>matches!(result,Err(ReleaseError::BadResponse)),"rate"=>matches!(result,Err(ReleaseError::RateLimited(_))),_=>matches!(result,Err(ReleaseError::Unreachable))});
            assert!(server.received_requests().await.unwrap()[0].headers.get("authorization").is_none());server.verify().await;
        }
        let url=server.uri();drop(server);assert!(matches!(GitHubReleases::new("test",&url).unwrap().latest().await,Err(ReleaseError::Unreachable)));
    }
    #[tokio::test]async fn latest_tag_reads_the_web_redirect_and_distrusts_anything_else(){
        let server=MockServer::start().await;
        let feed=GitHubReleases::new("t",&format!("{}/api",server.uri())).unwrap().with_web_latest(&format!("{}/latest",server.uri()));
        for (status,location,expected) in [
            (302,Some("https://github.com/o/r/releases/tag/v0.46.0"),Some("v0.46.0")),
            (301,Some("https://github.com/o/r/releases/tag/v1.0.0-rc.1"),Some("v1.0.0-rc.1")),
            (302,Some("https://github.com/o/r/releases"),None),
            (302,Some("https://github.com/o/r/releases/tag/"),None),
            (302,Some("https://github.com/o/r/releases/tag/v1%0d%0a"),None),
            (302,None,None),
            (200,None,None),
            (404,None,None),
        ]{
            server.reset().await;
            let mut reply=ResponseTemplate::new(status);if let Some(l)=location{reply=reply.insert_header("location",l);}
            Mock::given(path("/latest")).respond_with(reply).mount(&server).await;
            assert_eq!(feed.latest_tag().await.ok().as_deref(),expected,"{status} {location:?}");
        }
        assert!(matches!(GitHubReleases::new("t","http://x.invalid/api").unwrap().latest_tag().await,Err(ReleaseError::Unreachable)),"no web URL configured");
    }
    #[tokio::test]async fn the_api_answer_is_revalidated_with_its_etag_and_replaced_when_it_changes(){
        let server=MockServer::start().await;
        let feed=GitHubReleases::new("t",&format!("{}/api",server.uri())).unwrap();
        Mock::given(path("/api")).respond_with(ResponseTemplate::new(200).insert_header("etag","W/\"one\"").set_body_raw(r#"{"tag_name":"v1"}"#,"application/json")).up_to_n_times(1).mount(&server).await;
        assert_eq!(feed.latest().await.unwrap()["tag_name"],"v1");
        assert!(server.received_requests().await.unwrap()[0].headers.get("if-none-match").is_none(),"nothing to revalidate yet");
        server.reset().await;
        Mock::given(path("/api")).and(header("if-none-match","W/\"one\"")).respond_with(ResponseTemplate::new(304)).up_to_n_times(1).mount(&server).await;
        assert_eq!(feed.latest().await.unwrap()["tag_name"],"v1","a 304 is answered with the stored body");
        server.reset().await;
        Mock::given(path("/api")).and(header("if-none-match","W/\"one\"")).respond_with(ResponseTemplate::new(200).insert_header("etag","W/\"two\"").set_body_raw(r#"{"tag_name":"v2"}"#,"application/json")).up_to_n_times(1).mount(&server).await;
        assert_eq!(feed.latest().await.unwrap()["tag_name"],"v2");
        server.reset().await;
        Mock::given(path("/api")).and(header("if-none-match","W/\"two\"")).respond_with(ResponseTemplate::new(304)).mount(&server).await;
        assert_eq!(feed.latest().await.unwrap()["tag_name"],"v2","the new etag replaced the old one");
    }
    #[tokio::test]async fn a_304_without_a_stored_answer_is_not_trusted(){
        let server=MockServer::start().await;
        Mock::given(path("/api")).respond_with(ResponseTemplate::new(304)).mount(&server).await;
        assert!(matches!(GitHubReleases::new("t",&format!("{}/api",server.uri())).unwrap().latest().await,Err(ReleaseError::BadResponse)));
    }
    #[tokio::test]async fn a_rate_limit_reports_when_github_lifts_it(){
        let server=MockServer::start().await;let feed=GitHubReleases::new("t",&format!("{}/api",server.uri())).unwrap();
        Mock::given(path("/api")).respond_with(ResponseTemplate::new(403).insert_header("retry-after","120")).up_to_n_times(1).mount(&server).await;
        assert!(matches!(feed.latest().await,Err(ReleaseError::RateLimited(Some(120)))));
        server.reset().await;
        let reset=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()+600;
        Mock::given(path("/api")).respond_with(ResponseTemplate::new(403).insert_header("x-ratelimit-reset",reset.to_string().as_str())).up_to_n_times(1).mount(&server).await;
        match feed.latest().await{Err(ReleaseError::RateLimited(Some(s)))=>assert!((590..=600).contains(&s),"{s}"),other=>panic!("{other:?}")}
        server.reset().await;
        Mock::given(path("/api")).respond_with(ResponseTemplate::new(429)).mount(&server).await;
        assert!(matches!(feed.latest().await,Err(ReleaseError::RateLimited(None))),"no hint from GitHub");
    }
    #[tokio::test]async fn metadata_reads_installed_name_and_fails_closed_when_missing(){
        let d=tempfile::tempdir().unwrap();let m=FilePluginMetadata::new(d.path().into());assert_eq!(m.metadata().await.unwrap(),("0.0.0".into(),"".into()));
        tokio::fs::write(d.path().join("package.json"),br#"{"version":"0.33.2"}"#).await.unwrap();tokio::fs::write(d.path().join("plugin.json"),br#"{"name":"EmuDecky RomM NG"}"#).await.unwrap();assert_eq!(m.metadata().await.unwrap(),("0.33.2".into(),"EmuDecky RomM NG".into()));
        tokio::fs::write(d.path().join("plugin.json"),b"broken").await.unwrap();assert_eq!(m.metadata().await.unwrap().1,"");
    }
}
