//! Steam user discovery and atomic artwork/localconfig writes. Never touches shortcuts.vdf.
use std::path::{Path,PathBuf};
use async_trait::async_trait;
use crate::{domain::steam_input,services::ports::SteamAssets};
pub struct LocalSteamAssets {home:PathBuf,cache:PathBuf,lock:tokio::sync::Mutex<()>}
impl LocalSteamAssets {
    pub fn new(home:PathBuf,cache:PathBuf)->Self{Self{home,cache,lock:tokio::sync::Mutex::new(())}}
    async fn user_dir(&self)->Option<PathBuf>{
        for suffix in [".local/share/Steam/userdata",".steam/steam/userdata"] {
            let base=self.home.join(suffix);let Ok(mut entries)=tokio::fs::read_dir(&base).await else{continue};let mut users=vec![];
            while let Ok(Some(entry))=entries.next_entry().await{
                if entry.file_name().to_str().is_some_and(|s|!s.is_empty()&&s.chars().all(|c|c.is_ascii_digit())){
                    if let Ok(meta)=entry.metadata().await{if meta.is_dir(){users.push((meta.modified().ok(),entry.path()));}}
                }
            }
            users.sort_by(|a,b|b.0.cmp(&a.0).then_with(||a.1.cmp(&b.1)));if let Some((_,path))=users.into_iter().next(){return Some(path);}
        }None
    }
}
async fn atomic_bytes(dest:&Path,bytes:&[u8])->anyhow::Result<()> {
    if let Some(parent)=dest.parent(){tokio::fs::create_dir_all(parent).await?;}
    let tmp=PathBuf::from(format!("{}.tmp",dest.display()));
    let result=async{tokio::fs::write(&tmp,bytes).await?;let file=tokio::fs::File::open(&tmp).await?;file.sync_all().await?;tokio::fs::rename(&tmp,dest).await?;Ok::<_,anyhow::Error>(())}.await;
    if result.is_err(){let _=tokio::fs::remove_file(&tmp).await;}result
}
#[async_trait]
impl SteamAssets for LocalSteamAssets {
    async fn grid_dir(&self)->anyhow::Result<Option<String>>{let Some(user)=self.user_dir().await else{return Ok(None)};let grid=user.join("config/grid");tokio::fs::create_dir_all(&grid).await?;Ok(Some(grid.to_string_lossy().into_owned()))}
    fn cache_path(&self,id:i64)->String{self.cache.join(format!("{id}.png")).to_string_lossy().into_owned()}
    fn cache_dir(&self)->String {self.cache.to_string_lossy().into_owned()}
    async fn files_in(&self,dir:&str)->anyhow::Result<Vec<String>> {
        let mut entries=match tokio::fs::read_dir(dir).await{Ok(e)=>e,Err(e)if e.kind()==std::io::ErrorKind::NotFound=>return Ok(vec![]),Err(e)=>return Err(e.into())};let mut names=vec![];
        while let Some(entry)=entries.next_entry().await?{if entry.file_type().await?.is_file(){if let Some(name)=entry.file_name().to_str(){names.push(name.to_string());}}}Ok(names)
    }
    async fn exists(&self,path:&str)->bool{tokio::fs::metadata(path).await.is_ok_and(|m|m.is_file())}
    async fn read(&self,path:&str)->anyhow::Result<Vec<u8>>{Ok(tokio::fs::read(path).await?)}
    async fn copy_atomic(&self,src:&str,dest:&str)->anyhow::Result<()>{let _guard=self.lock.lock().await;atomic_bytes(Path::new(dest),&tokio::fs::read(src).await?).await}
    async fn rename(&self,src:&str,dest:&str)->anyhow::Result<()>{tokio::fs::rename(src,dest).await?;Ok(())}
    async fn remove(&self,path:&str)->anyhow::Result<()>{match tokio::fs::remove_file(path).await{Ok(())=>Ok(()),Err(e)if e.kind()==std::io::ErrorKind::NotFound=>Ok(()),Err(e)=>Err(e.into())}}
    async fn write_icon(&self,app:i64,bytes:&[u8])->anyhow::Result<Option<String>>{let _guard=self.lock.lock().await;let Some(grid)=self.grid_dir().await?else{return Ok(None)};let digest=ring::digest::digest(&ring::digest::SHA256,bytes);let hash:String=digest.as_ref().iter().map(|b|format!("{b:02x}")).collect();
        // An older request completing later must not overwrite the file selected
        // by a newer frontend generation. Steam receives the immutable path.
        let path=Path::new(&grid).join(format!("{app}_icon_{hash}.png"));atomic_bytes(&path,bytes).await?;
        atomic_bytes(&Path::new(&grid).join(format!("{app}_icon.png")),bytes).await?;
        Ok(Some(path.to_string_lossy().into_owned()))}
    async fn steam_input(&self,ids:&[i64],mode:&str)->anyhow::Result<()> {
        if ids.is_empty(){return Ok(());}let _guard=self.lock.lock().await;let Some(user)=self.user_dir().await else{return Ok(())};let path=user.join("config/localconfig.vdf");
        let original=match tokio::fs::read(&path).await{Ok(b)=>b,Err(e)if e.kind()==std::io::ErrorKind::NotFound=>return Ok(()),Err(e)=>return Err(e.into())};
        let mut data=steam_input::parse(std::str::from_utf8(&original)?).map_err(anyhow::Error::msg)?;
        if !steam_input::patch(&mut data,ids,mode).map_err(anyhow::Error::msg)?{return Ok(());}
        let encoded=steam_input::render(&data);let decoded=steam_input::parse(&encoded).map_err(anyhow::Error::msg)?;
        anyhow::ensure!(decoded==data,"localconfig VDF round-trip mismatch");
        // Persist the original before replacing a shared Steam config file.
        atomic_bytes(&PathBuf::from(format!("{}.romm-backup",path.display())),&original).await?;
        atomic_bytes(&path,encoded.as_bytes()).await
    }
}

#[cfg(test)]mod tests {
    use super::*;
    #[tokio::test]async fn icons_are_atomic_and_config_is_backed_up_roundtripped_and_preserves_unknown_keys(){
        let dir=tempfile::tempdir().unwrap();let home=dir.path().join("home");let user=home.join(".local/share/Steam/userdata/123/config");tokio::fs::create_dir_all(&user).await.unwrap();
        let original=b"\"UserLocalConfigStore\" { \"Other\" \"keep\" \"Apps\" { \"456\" { \"LaunchOptions\" \"-f\" } \"789\" { \"UseSteamControllerConfig\" \"2\" } } }";
        tokio::fs::write(user.join("localconfig.vdf"),original).await.unwrap();let assets=LocalSteamAssets::new(home,dir.path().join("covers"));
        assets.steam_input(&[456],"force_off").await.unwrap();assert_eq!(tokio::fs::read(user.join("localconfig.vdf.romm-backup")).await.unwrap(),original);
        let patched=tokio::fs::read_to_string(user.join("localconfig.vdf")).await.unwrap();assert!(patched.contains("LaunchOptions"));assert!(patched.contains("Other"));assert!(patched.contains("789"));
        assert_eq!(steam_input::parse(&patched).unwrap(),steam_input::parse(&steam_input::render(&steam_input::parse(&patched).unwrap())).unwrap());
        assets.steam_input(&[456],"default").await.unwrap();let reset=tokio::fs::read_to_string(user.join("localconfig.vdf")).await.unwrap();assert!(reset.contains("LaunchOptions"));assert_eq!(reset.matches("UseSteamControllerConfig").count(),1);
        assert!(assets.write_icon(456,b"icon").await.unwrap().is_some());assert_eq!(tokio::fs::read(user.join("grid/456_icon.png")).await.unwrap(),b"icon");assert!(!user.join("grid/456_icon.png.tmp").exists());assert!(!user.join("shortcuts.vdf").exists());
        let cache=assets.cache_path(1);assets.copy_atomic(user.join("grid/456_icon.png").to_str().unwrap(),&cache).await.unwrap();assert!(assets.exists(&cache).await);assets.remove(&cache).await.unwrap();assets.remove(&cache).await.unwrap();
    }
    #[tokio::test]async fn icon_paths_are_immutable_across_out_of_order_frontend_generations(){
        let dir=tempfile::tempdir().unwrap();tokio::fs::create_dir_all(dir.path().join(".steam/steam/userdata/1")).await.unwrap();let assets=LocalSteamAssets::new(dir.path().into(),dir.path().join("covers"));
        let newer=assets.write_icon(123,b"new").await.unwrap().unwrap();let older=assets.write_icon(123,b"old").await.unwrap().unwrap();
        assert_ne!(newer,older);assert_eq!(tokio::fs::read(&newer).await.unwrap(),b"new");assert_eq!(tokio::fs::read(&older).await.unwrap(),b"old");
        assert_eq!(assets.write_icon(123,b"new").await.unwrap().unwrap(),newer);
    }
    #[tokio::test]async fn malformed_vdf_is_preserved_and_missing_steam_is_a_noop(){
        let dir=tempfile::tempdir().unwrap();let assets=LocalSteamAssets::new(dir.path().into(),dir.path().join("covers"));assert_eq!(assets.grid_dir().await.unwrap(),None);assert!(assets.write_icon(1,b"x").await.unwrap().is_none());assets.steam_input(&[1],"force_on").await.unwrap();
        let config=dir.path().join(".steam/steam/userdata/1/config");tokio::fs::create_dir_all(&config).await.unwrap();tokio::fs::write(config.join("localconfig.vdf"),b"broken").await.unwrap();assert!(assets.steam_input(&[1],"force_on").await.is_err());assert_eq!(tokio::fs::read(config.join("localconfig.vdf")).await.unwrap(),b"broken");assert!(!config.join("localconfig.vdf.romm-backup").exists());
    }
}
