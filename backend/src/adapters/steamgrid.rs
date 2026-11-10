//! Separate SGDB client: no RomM credentials, strict TLS, unauthenticated CDN requests.
use std::{path::Path,sync::Arc,time::Duration};
use async_trait::async_trait;
use reqwest::{Client,redirect::Policy};
use serde_json::Value;
use crate::services::ports::{SgdbError,SteamGridApi};

pub const API_BASE:&str="https://www.steamgriddb.com/api/v2";
pub struct SteamGridHttp {client:Client,image_client:Client,base:String}
impl SteamGridHttp {
    pub fn new(user_agent:&str,base:&str)->anyhow::Result<Arc<Self>>{
        Ok(Arc::new(Self{client:Client::builder().user_agent(user_agent).timeout(Duration::from_secs(30))
            .redirect(Policy::none()).build()?,image_client:Client::builder().user_agent(user_agent).timeout(Duration::from_secs(30))
            .redirect(Policy::limited(10)).build()?,base:base.trim_end_matches('/').into()}))
    }
}
#[async_trait]
impl SteamGridApi for SteamGridHttp {
    async fn request(&self,path:&str,key:&str)->Result<Value,SgdbError>{
        // Paths are built by the domain helpers, never taken from remote URLs.
        if !path.starts_with('/')||path.starts_with("//"){return Err(SgdbError::BadResponse);}
        let response=self.client.get(format!("{}{path}",self.base)).bearer_auth(key).send().await.map_err(|_|SgdbError::Connection)?;
        if !response.status().is_success(){return Err(SgdbError::Http(response.status().as_u16()));}
        response.json().await.map_err(|_|SgdbError::BadResponse)
    }
    async fn download_image(&self,url:&str,dest:&str)->anyhow::Result<()>{
        let url=reqwest::Url::parse(url)?;
        anyhow::ensure!(["http","https"].contains(&url.scheme()),"Invalid artwork URL");
        let tmp=format!("{dest}.tmp");
        let result=async {
            let mut response=self.image_client.get(url).send().await?.error_for_status()?;
            anyhow::ensure!(response.status().is_success(),"Artwork redirect refused");
            if let Some(parent)=Path::new(dest).parent(){tokio::fs::create_dir_all(parent).await?;}
            let mut out=tokio::fs::File::create(&tmp).await?;
            use tokio::io::AsyncWriteExt;
            while let Some(bytes)=response.chunk().await?{out.write_all(&bytes).await?;}
            out.sync_all().await?;
            drop(out);
            tokio::fs::rename(&tmp,dest).await?;
            Ok::<_,anyhow::Error>(())
        }.await;
        if result.is_err(){let _=tokio::fs::remove_file(&tmp).await;}
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::{method, path, header, header_exists}};
    use serde_json::json;

    #[tokio::test]
    async fn api_auth_is_separate_from_cdn_requests_and_redirects() {
        let api = MockServer::start().await;
        let cdn = MockServer::start().await;
        Mock::given(method("GET")).and(path("/api/v2/heroes/game/9"))
            .and(header("authorization","Bearer sgdb-only"))
            .and(header("user-agent","romm-emudecky-ng/test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true,"data":[]})))
            .expect(1).mount(&api).await;
        Mock::given(path("/image")).respond_with(ResponseTemplate::new(302).insert_header("Location",format!("{}/final",cdn.uri())))
            .expect(1).mount(&api).await;
        Mock::given(path("/final")).and(header("user-agent","romm-emudecky-ng/test"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"pixels"))
            .expect(1).mount(&cdn).await;
        let client=SteamGridHttp::new("romm-emudecky-ng/test",&format!("{}/api/v2",api.uri())).unwrap();
        assert_eq!(client.request("/heroes/game/9","sgdb-only").await.unwrap()["success"],true);
        let dir=tempfile::tempdir().unwrap();let dest=dir.path().join("nested/image.png");
        client.download_image(&format!("{}/image",api.uri()),dest.to_str().unwrap()).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(),b"pixels");
        assert!(!dest.with_extension("png.tmp").exists());
        let requests=api.received_requests().await.unwrap();
        assert!(!requests.iter().find(|r|r.url.path()=="/image").unwrap().headers.contains_key("authorization"));
        for r in cdn.received_requests().await.unwrap(){ assert!(!r.headers.contains_key("authorization")); }
    }

    #[tokio::test]
    async fn api_failures_are_typed_and_never_follow_authenticated_redirects() {
        let api=MockServer::start().await;
        Mock::given(path("/auth")).respond_with(ResponseTemplate::new(403)).mount(&api).await;
        Mock::given(path("/bad")).respond_with(ResponseTemplate::new(200).set_body_string("not json")).mount(&api).await;
        Mock::given(path("/redirect")).respond_with(ResponseTemplate::new(302).insert_header("Location",format!("{}/leak",api.uri()))).mount(&api).await;
        Mock::given(path("/leak")).and(header_exists("authorization")).respond_with(ResponseTemplate::new(200)).expect(0).mount(&api).await;
        let client=SteamGridHttp::new("romm-emudecky-ng/test",&api.uri()).unwrap();
        assert!(matches!(client.request("/auth","secret").await,Err(SgdbError::Http(403))));
        assert!(matches!(client.request("/bad","secret").await,Err(SgdbError::BadResponse)));
        assert!(matches!(client.request("/redirect","secret").await,Err(SgdbError::Http(302))));
        for invalid in ["https://evil.invalid", "//evil.invalid"] {assert!(matches!(client.request(invalid,"secret").await,Err(SgdbError::BadResponse)));}
        let unreachable=SteamGridHttp::new("test","http://127.0.0.1:0").unwrap();
        assert!(matches!(unreachable.request("/x","secret").await,Err(SgdbError::Connection)));
    }

    #[tokio::test]
    async fn failed_image_writes_preserve_existing_pixels_and_clean_partial_sidecar() {
        let api=MockServer::start().await;
        Mock::given(path("/missing")).respond_with(ResponseTemplate::new(404)).mount(&api).await;
        let client=SteamGridHttp::new("test",&api.uri()).unwrap();
        let dir=tempfile::tempdir().unwrap();let dest=dir.path().join("image.png");
        std::fs::write(&dest,b"old").unwrap();
        std::fs::write(dest.with_extension("png.tmp"),b"partial").unwrap();
        assert!(client.download_image(&format!("{}/missing",api.uri()),dest.to_str().unwrap()).await.is_err());
        assert_eq!(std::fs::read(&dest).unwrap(),b"old");
        assert!(!dest.with_extension("png.tmp").exists());
        assert!(client.download_image("file:///etc/passwd",dest.to_str().unwrap()).await.is_err());
        // A successful HTTP transfer followed by a failed rename must also clean up.
        Mock::given(path("/pixels")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"new")).mount(&api).await;
        let blocked=dir.path().join("directory.png");std::fs::create_dir(&blocked).unwrap();
        assert!(client.download_image(&format!("{}/pixels",api.uri()),blocked.to_str().unwrap()).await.is_err());
        assert!(!blocked.with_extension("png.tmp").exists());
    }

    #[tokio::test]
    async fn interrupted_image_body_cannot_replace_the_old_cache() {
        use tokio::{net::TcpListener, io::{AsyncReadExt,AsyncWriteExt}};
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url=format!("http://{}/image",listener.local_addr().unwrap());
        let server=tokio::spawn(async move {
            let (mut socket,_)=listener.accept().await.unwrap();
            let mut request=[0;4096];socket.read(&mut request).await.unwrap();
            socket.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\nconnection: close\r\n\r\npartial").await.unwrap();
        });
        let client=SteamGridHttp::new("test",API_BASE).unwrap();
        let dir=tempfile::tempdir().unwrap();let dest=dir.path().join("image.png");
        std::fs::write(&dest,b"old").unwrap();
        assert!(client.download_image(&url,dest.to_str().unwrap()).await.is_err());
        server.await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(),b"old");
        assert!(!dest.with_extension("png.tmp").exists());
    }
}
