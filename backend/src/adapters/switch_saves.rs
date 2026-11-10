//! The `LocalSaves` port over the emulators' real save folders (blocking file work runs off the async executor).
use std::{collections::BTreeMap, path::PathBuf};

use async_trait::async_trait;

use crate::adapters::switch_save_files::SwitchFiles;
use crate::domain::{
    save_archive::ArchiveEntry,
    switch_save::{FolderStamp, ProfileInfo, TitleId},
};
use crate::services::saves::{HostIdentity, LocalError, LocalRead, LocalRequest, LocalSaves};

#[derive(Clone)]
pub struct SwitchSaves {
    files: SwitchFiles,
}

impl SwitchSaves {
    pub fn new(home: PathBuf) -> Self {
        Self { files: SwitchFiles::new(home) }
    }
}

/// A missing emulator or save folder is "unsupported for now"; any other failure is a real error.
fn classify(message: String) -> LocalError {
    if message.contains("not found") || message.contains("not supported") {
        LocalError::Unsupported(message)
    } else {
        LocalError::Io(message)
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, LocalError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| LocalError::Io(e.to_string()))?.map_err(classify)
}

#[async_trait]
impl LocalSaves for SwitchSaves {
    async fn profiles(&self, emulator: &str) -> Result<Vec<ProfileInfo>, LocalError> {
        let (files, emulator) = (self.files.clone(), emulator.to_string());
        blocking(move || files.profiles(&emulator)).await
    }

    async fn read(&self, req: &LocalRequest) -> Result<LocalRead, LocalError> {
        let (files, req) = (self.files.clone(), req.clone());
        let (entries, newest_mtime, path) = blocking(move || files.read(&req.emulator, req.title, req.profile.as_deref())).await?;
        Ok(LocalRead { entries, newest_mtime, path })
    }

    async fn write(&self, req: &LocalRequest, entries: &[ArchiveEntry]) -> Result<Vec<String>, LocalError> {
        let (files, req, entries) = (self.files.clone(), req.clone(), entries.to_vec());
        blocking(move || files.write(&req.emulator, req.title, req.profile.as_deref(), &entries)).await
    }

    async fn quarantine(&self, req: &LocalRequest) -> Result<Vec<String>, LocalError> {
        let (files, req) = (self.files.clone(), req.clone());
        blocking(move || files.quarantine(&req.emulator, req.title, req.profile.as_deref())).await
    }

    async fn snapshot(&self, emulator: &str) -> Result<BTreeMap<TitleId, FolderStamp>, LocalError> {
        let (files, emulator) = (self.files.clone(), emulator.to_string());
        blocking(move || files.snapshot(&emulator)).await
    }
}

/// Hostname from the kernel and the stable machine id from `/etc/machine-id` (unreadable = no fingerprint).
pub struct LinuxHost;

impl HostIdentity for LinuxHost {
    fn hostname(&self) -> String {
        std::fs::read_to_string("/proc/sys/kernel/hostname")
            .or_else(|_| std::fs::read_to_string("/etc/hostname"))
            .map(|h| h.trim().to_string())
            .ok()
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| "steamdeck".into())
    }

    fn machine_id(&self) -> Option<String> {
        std::fs::read_to_string("/etc/machine-id").ok().map(|m| m.trim().to_string()).filter(|m| !m.is_empty())
    }
}
