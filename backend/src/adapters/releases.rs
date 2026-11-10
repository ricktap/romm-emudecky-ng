//! Unauthenticated, strict-TLS GitHub release feed and plugin metadata reads.
use std::{path::PathBuf,time::Duration};
use async_trait::async_trait;
use serde_json::Value;
use crate::services::ports::{ReleaseFeed,ReleaseError,PluginMetadata};
pub struct GitHubReleases{client:reqwest::Client,url:String}
impl GitHubReleases{
    pub fn new(agent:&str,url:&str)->anyhow::Result<Self>{Ok(Self{client:reqwest::Client::builder().user_agent(agent).timeout(Duration::from_secs(10)).redirect(reqwest::redirect::Policy::none()).build()?,url:url.into()})}
}
#[async_trait]
impl ReleaseFeed for GitHubReleases{
    async fn latest(&self)->Result<Value,ReleaseError>{
        let response=self.client.get(&self.url).header("Accept","application/vnd.github+json").header("X-GitHub-Api-Version","2022-11-28").send().await.map_err(|_|ReleaseError::Unreachable)?;
        let status=response.status();if matches!(status.as_u16(),403|429){return Err(ReleaseError::RateLimited);}
        if !status.is_success(){return Err(ReleaseError::Unreachable);}
        response.json::<Value>().await.map_err(|_|ReleaseError::BadResponse).and_then(|v|if v.is_object(){Ok(v)}else{Err(ReleaseError::BadResponse)})
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
            assert!(match kind{"ok"=>result.is_ok(),"bad"=>matches!(result,Err(ReleaseError::BadResponse)),"rate"=>matches!(result,Err(ReleaseError::RateLimited)),_=>matches!(result,Err(ReleaseError::Unreachable))});
            assert!(server.received_requests().await.unwrap()[0].headers.get("authorization").is_none());server.verify().await;
        }
        let url=server.uri();drop(server);assert!(matches!(GitHubReleases::new("test",&url).unwrap().latest().await,Err(ReleaseError::Unreachable)));
    }
    #[tokio::test]async fn metadata_reads_installed_name_and_fails_closed_when_missing(){
        let d=tempfile::tempdir().unwrap();let m=FilePluginMetadata::new(d.path().into());assert_eq!(m.metadata().await.unwrap(),("0.0.0".into(),"".into()));
        tokio::fs::write(d.path().join("package.json"),br#"{"version":"0.33.2"}"#).await.unwrap();tokio::fs::write(d.path().join("plugin.json"),br#"{"name":"EmuDecky RomM NG"}"#).await.unwrap();assert_eq!(m.metadata().await.unwrap(),("0.33.2".into(),"EmuDecky RomM NG".into()));
        tokio::fs::write(d.path().join("plugin.json"),b"broken").await.unwrap();assert_eq!(m.metadata().await.unwrap().1,"");
    }
}
