//! Filesystem operations on ROM files (the `DownloadFiles` port).
//!
//! Single-file operations use `tokio::fs`; directory walks, `statvfs` and ZIP
//! extraction run on the blocking pool. Extraction refuses any member that
//! would land outside its destination (ZIP slip) and any destination outside
//! the configured roms root.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use async_trait::async_trait;

use crate::services::ports::{DownloadFiles, ExtractProgress};
use crate::util::path_safety::{join, realpath, safe_path_component, unquote};

const EXTRACT_CHUNK: usize = 1024 * 1024;

#[derive(Clone, Default)]
pub struct LocalFiles;

fn ignore_missing(r: std::io::Result<()>) -> std::io::Result<()> {
    match r {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        // os.walk semantics: descend into real directories, list everything else.
        match entry.file_type() {
            Ok(t) if t.is_dir() => walk(&path, out),
            Ok(t) if t.is_symlink() && path.is_dir() => {}
            Ok(_) => out.push(path),
            Err(_) => {}
        }
    }
}

fn walk_files(dir: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(Path::new(dir), &mut out);
    out
}

fn inside(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

/// Read the central directory and every member to its end, so a truncated
/// archive or any CRC mismatch is an error. Nothing is written.
fn verify_blocking(archive: &str) -> anyhow::Result<()> {
    let file = std::fs::File::open(archive).with_context(|| format!("opening {archive}"))?;
    let mut zip = zip::ZipArchive::new(file).context("reading the ZIP archive")?;
    for i in 0..zip.len() {
        let mut member = zip.by_index(i)?;
        let name = member.name()?.to_string();
        std::io::copy(&mut member, &mut std::io::sink()).with_context(|| format!("checking ZIP member {name}"))?;
    }
    Ok(())
}

/// Streamed SHA-1 of a regular file (RomM publishes SHA-1 for its firmware files).
fn sha1_blocking(path: &str) -> Option<String> {
    use std::io::Read;
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
    }
    Some(ctx.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect())
}

fn extract_blocking(archive: &str, dest: &str, safe_root: &str, mut progress: ExtractProgress) -> anyhow::Result<()> {
    let real_dest = realpath(dest);
    let real_safe = realpath(safe_root);
    if !inside(&real_dest, &real_safe) {
        bail!("Extract directory would be outside safe root: {dest}");
    }
    let file = std::fs::File::open(archive).with_context(|| format!("opening {archive}"))?;
    let mut zip = zip::ZipArchive::new(file).context("reading the ZIP archive")?;
    let mut total: u64 = 0;
    for i in 0..zip.len() {
        let member = zip.by_index_raw(i)?;
        let name = member.name()?.to_string();
        let target = realpath(&join(&real_dest.to_string_lossy(), &[&name]));
        if !inside(&target, &real_dest) {
            bail!("ZIP member {name} would extract outside target directory");
        }
        total += member.size();
    }
    let mut extracted: u64 = 0;
    let mut buf = vec![0u8; EXTRACT_CHUNK];
    for i in 0..zip.len() {
        let mut member = zip.by_index(i)?;
        let target = PathBuf::from(join(&real_dest.to_string_lossy(), &[member.name()?.as_ref()]));
        if member.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&target).with_context(|| format!("creating {}", target.display()))?;
        loop {
            let n = std::io::Read::read(&mut member, &mut buf)?;
            if n == 0 {
                break;
            }
            std::io::Write::write_all(&mut out, &buf[..n]).with_context(|| format!("writing {}", target.display()))?;
            extracted += n as u64;
            progress(extracted, total);
        }
    }
    Ok(())
}

fn decode_blocking(dir: &Path) -> anyhow::Result<()> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    let mut names = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            decode_blocking(&path)?; // bottom-up
        }
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    for name in names {
        let decoded = unquote(&name);
        if decoded != name {
            safe_path_component(&decoded)?;
            std::fs::rename(dir.join(&name), dir.join(&decoded))?;
        }
    }
    Ok(())
}

fn statvfs_free(path: &str) -> std::io::Result<u64> {
    let c = std::ffi::CString::new(path).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path and `st` a writable statvfs.
    let rc = unsafe { libc::statvfs(c.as_ptr(), &raw mut st) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    #[allow(clippy::unnecessary_cast)]
    Ok(st.f_bavail as u64 * st.f_frsize as u64)
}

#[async_trait]
impl DownloadFiles for LocalFiles {
    async fn exists(&self, path: &str) -> bool {
        tokio::fs::try_exists(path).await.unwrap_or(false)
    }

    async fn remove_file(&self, path: &str) -> std::io::Result<()> {
        ignore_missing(tokio::fs::remove_file(path).await)
    }

    async fn remove_tree(&self, path: &str) -> std::io::Result<()> {
        ignore_missing(tokio::fs::remove_dir_all(path).await)
    }

    async fn make_dirs(&self, path: &str) -> std::io::Result<()> {
        tokio::fs::create_dir_all(path).await
    }

    async fn rename(&self, src: &str, dst: &str) -> std::io::Result<()> {
        tokio::fs::rename(src, dst).await
    }

    async fn disk_free(&self, path: &str) -> std::io::Result<u64> {
        let path = path.to_string();
        tokio::task::spawn_blocking(move || statvfs_free(&path)).await?
    }

    async fn file_size(&self, path: &str) -> u64 {
        tokio::fs::metadata(path).await.map_or(0, |m| m.len())
    }

    async fn regular_file_size(&self, path: &str) -> Option<u64> {
        tokio::fs::metadata(path).await.ok().filter(|m| m.is_file()).map(|m| m.len())
    }

    async fn is_dir(&self, path: &str) -> bool {
        tokio::fs::metadata(path).await.is_ok_and(|m| m.is_dir())
    }

    async fn list_entries(&self, dir: &str) -> Vec<(String, bool, u64)> {
        let Ok(mut rd) = tokio::fs::read_dir(dir).await else {
            return Vec::new();
        };
        let mut out = Vec::new();
        while let Ok(Some(entry)) = rd.next_entry().await {
            let Ok(meta) = tokio::fs::metadata(entry.path()).await else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if meta.is_dir() {
                out.push((name, true, 0));
            } else if meta.is_file() {
                out.push((name, false, meta.len()));
            }
        }
        out.sort();
        out
    }

    async fn walk_files_matching_suffixes(&self, base: &str, suffixes: &[&str]) -> Vec<String> {
        let base = base.to_string();
        let suffixes: Vec<String> = suffixes.iter().map(|s| s.to_string()).collect();
        tokio::task::spawn_blocking(move || {
            walk_files(&base)
                .into_iter()
                .filter(|p| {
                    p.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .is_some_and(|n| suffixes.iter().any(|s| n.ends_with(s.as_str())))
                })
                .map(|p| p.to_string_lossy().into_owned())
                .collect()
        })
        .await
        .unwrap_or_default()
    }

    async fn extract_zip(
        &self,
        archive: &str,
        dest: &str,
        safe_root: &str,
        progress: ExtractProgress,
    ) -> anyhow::Result<()> {
        let (archive, dest, safe_root) = (archive.to_string(), dest.to_string(), safe_root.to_string());
        tokio::task::spawn_blocking(move || extract_blocking(&archive, &dest, &safe_root, progress)).await?
    }

    async fn copy_file(&self, src: &str, dst: &str) -> std::io::Result<()> {
        let (src, dst) = (src.to_string(), dst.to_string());
        tokio::task::spawn_blocking(move || {
            let dst_path = std::path::Path::new(&dst);
            if let Some(parent) = dst_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let tmp = format!("{dst}.tmp");
            if let Err(e) = std::fs::copy(&src, &tmp).and_then(|_| std::fs::rename(&tmp, &dst)) {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
            Ok(())
        })
        .await
        .map_err(std::io::Error::other)?
    }

    async fn sha1_hex(&self, path: &str) -> Option<String> {
        let path = path.to_string();
        tokio::task::spawn_blocking(move || sha1_blocking(&path)).await.ok().flatten()
    }

    async fn verify_zip(&self, archive: &str) -> anyhow::Result<()> {
        let archive = archive.to_string();
        tokio::task::spawn_blocking(move || verify_blocking(&archive)).await?
    }

    async fn decode_url_encoded_names(&self, dir: &str) -> anyhow::Result<()> {
        let dir = PathBuf::from(dir);
        tokio::task::spawn_blocking(move || decode_blocking(&dir)).await?
    }

    async fn scan_files_with_sizes(&self, dir: &str) -> Vec<(String, u64)> {
        let dir = dir.to_string();
        tokio::task::spawn_blocking(move || {
            walk_files(&dir)
                .into_iter()
                .map(|p| {
                    let size = std::fs::metadata(&p).map_or(0, |m| m.len());
                    (p.to_string_lossy().into_owned(), size)
                })
                .collect()
        })
        .await
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    pub fn write_zip(path: &Path, members: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in members {
            if name.ends_with('/') {
                zip.add_directory(*name, opts).unwrap();
            } else {
                zip.start_file(*name, opts).unwrap();
                zip.write_all(data).unwrap();
            }
        }
        zip.finish().unwrap();
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn basic_operations() {
        let dir = tempfile::tempdir().unwrap();
        let f = LocalFiles;
        let a = dir.path().join("a/b");
        f.make_dirs(&s(&a)).await.unwrap();
        assert!(f.is_dir(&s(&a)).await);
        let file = a.join("x.bin");
        std::fs::write(&file, b"12345").unwrap();
        assert!(f.exists(&s(&file)).await);
        assert_eq!(f.file_size(&s(&file)).await, 5);
        assert_eq!(f.regular_file_size(&s(&file)).await, Some(5));
        assert_eq!(f.regular_file_size(&s(&a)).await, None);
        assert_eq!(f.file_size(&s(&a.join("missing"))).await, 0);
        let moved = a.join("y.bin");
        f.rename(&s(&file), &s(&moved)).await.unwrap();
        assert!(!f.exists(&s(&file)).await);
        f.remove_file(&s(&moved)).await.unwrap();
        f.remove_file(&s(&moved)).await.unwrap(); // idempotent
        f.remove_tree(&s(&dir.path().join("a"))).await.unwrap();
        f.remove_tree(&s(&dir.path().join("a"))).await.unwrap();
        assert!(f.disk_free(&s(dir.path())).await.unwrap() > 0);
        assert!(f.disk_free("/definitely/missing/path").await.is_err());
    }

    #[tokio::test]
    async fn listing_and_walking() {
        let dir = tempfile::tempdir().unwrap();
        let f = LocalFiles;
        std::fs::create_dir_all(dir.path().join("G/update")).unwrap();
        std::fs::write(dir.path().join("G/base.nsp"), b"xx").unwrap();
        std::fs::write(dir.path().join("G/update/u.nsp.tmp"), b"x").unwrap();
        std::fs::write(dir.path().join("top.xci"), b"xxx").unwrap();
        std::fs::write(dir.path().join("a.zip.tmp"), b"x").unwrap();
        let entries = f.list_entries(&s(dir.path())).await;
        assert_eq!(
            entries,
            vec![
                ("G".into(), true, 0),
                ("a.zip.tmp".into(), false, 1),
                ("top.xci".into(), false, 3)
            ]
        );
        assert!(f.list_entries("/missing/dir").await.is_empty());
        let mut tmps = f.walk_files_matching_suffixes(&s(dir.path()), &[".tmp"]).await;
        tmps.sort();
        assert_eq!(tmps.len(), 2);
        assert!(f.walk_files_matching_suffixes("/missing", &[".tmp"]).await.is_empty());
        let scanned = f.scan_files_with_sizes(&s(&dir.path().join("G"))).await;
        assert_eq!(scanned.len(), 2);
        assert!(scanned.contains(&(s(&dir.path().join("G/base.nsp")), 2)));
    }

    #[tokio::test]
    async fn extracts_with_progress() {
        let dir = tempfile::tempdir().unwrap();
        let roms = dir.path().join("roms");
        let dest = roms.join("switch/Game");
        std::fs::create_dir_all(&dest).unwrap();
        let archive = dir.path().join("g.zip");
        write_zip(&archive, &[("base.nsp", b"hello"), ("update/", b""), ("update/u.nsp", b"abc")]);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        LocalFiles
            .extract_zip(
                &s(&archive),
                &s(&dest),
                &s(&roms),
                Box::new(move |done, total| sink.lock().unwrap().push((done, total))),
            )
            .await
            .unwrap();
        assert_eq!(std::fs::read(dest.join("base.nsp")).unwrap(), b"hello");
        assert_eq!(std::fs::read(dest.join("update/u.nsp")).unwrap(), b"abc");
        assert_eq!(seen.lock().unwrap().last(), Some(&(8, 8)));
    }

    /// Switch folder games are 15 GB+, so RomM's archives carry ZIP64 records.
    /// `large_file(true)` forces ZIP64 headers into a tiny archive, exercising the
    /// same header path without writing 4 GB in a test.
    #[tokio::test]
    async fn extracts_zip64_archives_stored_and_deflated() {
        for method in [zip::CompressionMethod::Stored, zip::CompressionMethod::Deflated] {
            let dir = tempfile::tempdir().unwrap();
            let roms = dir.path().join("roms");
            let dest = roms.join("switch/Big Game");
            std::fs::create_dir_all(&dest).unwrap();
            let archive = dir.path().join("big.zip");
            {
                let file = std::fs::File::create(&archive).unwrap();
                let mut zip = zip::ZipWriter::new(file);
                let opts = zip::write::SimpleFileOptions::default().compression_method(method).large_file(true);
                zip.start_file("Big Game [0100000000000000][v0].xci", opts).unwrap();
                zip.write_all(b"xci-payload").unwrap();
                zip.start_file("updates/Big Game [v65536].nsp", opts).unwrap();
                zip.write_all(b"update").unwrap();
                zip.finish().unwrap();
            }
            LocalFiles
                .extract_zip(&s(&archive), &s(&dest), &s(&roms), Box::new(|_, _| {}))
                .await
                .unwrap_or_else(|e| panic!("{method:?}: {e:#}"));
            assert_eq!(
                std::fs::read(dest.join("Big Game [0100000000000000][v0].xci")).unwrap(),
                b"xci-payload",
                "{method:?}"
            );
            assert_eq!(std::fs::read(dest.join("updates/Big Game [v65536].nsp")).unwrap(), b"update");
        }
    }

    #[tokio::test]
    async fn copy_file_creates_the_directory_leaves_no_temp_file_and_a_failed_copy_leaves_the_target_alone() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.bin");
        std::fs::write(&src, b"keys").unwrap();
        let dst = dir.path().join("deep/er/dst.bin");
        LocalFiles.copy_file(&s(&src), &s(&dst)).await.unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"keys");
        assert!(!dir.path().join("deep/er/dst.bin.tmp").exists());
        std::fs::write(&dst, b"keep me").unwrap();
        let err = LocalFiles.copy_file(&s(&dir.path().join("missing")), &s(&dst)).await;
        assert!(err.is_err());
        assert_eq!(std::fs::read(&dst).unwrap(), b"keep me", "a failed copy never replaces the target");
        assert!(!dir.path().join("deep/er/dst.bin.tmp").exists());
    }

    #[tokio::test]
    async fn sha1_of_a_file_matches_the_known_digest_and_missing_or_directories_are_none() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("abc.txt");
        std::fs::write(&f, b"abc").unwrap();
        assert_eq!(
            LocalFiles.sha1_hex(&s(&f)).await.as_deref(),
            Some("a9993e364706816aba3e25717850c26c9cd0d89d")
        );
        let big = dir.path().join("big.bin");
        std::fs::write(&big, vec![7u8; 200_000]).unwrap(); // several read chunks
        assert_eq!(LocalFiles.sha1_hex(&s(&big)).await.unwrap().len(), 40);
        assert_eq!(LocalFiles.sha1_hex(&s(&dir.path().join("missing"))).await, None);
        assert_eq!(LocalFiles.sha1_hex(&s(dir.path())).await, None, "a directory is not a file");
    }

    #[tokio::test]
    async fn verify_zip_accepts_a_good_archive_and_rejects_truncated_or_corrupt_ones() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.zip");
        let payload: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
        write_zip(&good, &[("a.bin", &payload), ("sub/b.txt", b"hello")]);
        LocalFiles.verify_zip(&s(&good)).await.unwrap();

        let bytes = std::fs::read(&good).unwrap();
        // structurally broken: the central directory at the end is gone
        let truncated = dir.path().join("cut.zip");
        std::fs::write(&truncated, &bytes[..bytes.len() - 40]).unwrap();
        assert!(LocalFiles.verify_zip(&s(&truncated)).await.is_err());
        // structurally fine, but a data byte flipped: the CRC catches it
        let corrupt = dir.path().join("flipped.zip");
        let mut flipped = bytes.clone();
        flipped[100] ^= 0xff;
        std::fs::write(&corrupt, &flipped).unwrap();
        assert!(LocalFiles.verify_zip(&s(&corrupt)).await.is_err());
        assert!(LocalFiles.verify_zip(&s(&dir.path().join("missing.zip"))).await.is_err());
    }

    #[tokio::test]
    async fn extraction_refuses_zip_slip_and_outside_dest() {
        let dir = tempfile::tempdir().unwrap();
        let roms = dir.path().join("roms");
        let dest = roms.join("switch/Game");
        std::fs::create_dir_all(&dest).unwrap();
        let archive = dir.path().join("evil.zip");
        write_zip(&archive, &[("../../evil.sh", b"x")]);
        let abs = dir.path().join("abs.zip");
        write_zip(&abs, &[("/tmp/abs-evil.sh", b"x")]);
        assert!(
            LocalFiles
                .extract_zip(&s(&abs), &s(&dest), &s(&roms), Box::new(|_, _| {}))
                .await
                .is_err()
        );
        let err = LocalFiles
            .extract_zip(&s(&archive), &s(&dest), &s(&roms), Box::new(|_, _| {}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("outside target directory"));
        assert!(!dir.path().join("evil.sh").exists());
        let good = dir.path().join("good.zip");
        write_zip(&good, &[("a", b"x")]);
        let err = LocalFiles
            .extract_zip(&s(&good), &s(dir.path()), &s(&roms), Box::new(|_, _| {}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("outside safe root"));
        let not_zip = dir.path().join("x.zip");
        std::fs::write(&not_zip, b"not a zip").unwrap();
        assert!(
            LocalFiles
                .extract_zip(&s(&not_zip), &s(&dest), &s(&roms), Box::new(|_, _| {}))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn decodes_names_and_refuses_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join("x");
        std::fs::create_dir_all(d.join("Sub%20Dir")).unwrap();
        std::fs::write(d.join("Sub%20Dir/Game%20Title.cue"), b"").unwrap();
        std::fs::write(d.join("plain.bin"), b"").unwrap();
        LocalFiles.decode_url_encoded_names(&s(&d)).await.unwrap();
        assert!(d.join("Sub Dir/Game Title.cue").exists());
        assert!(d.join("plain.bin").exists());
        std::fs::write(d.join("%2e%2e%2fevil.sh"), b"").unwrap();
        assert!(LocalFiles.decode_url_encoded_names(&s(&d)).await.is_err());
        assert!(!dir.path().join("evil.sh").exists());
    }
}
