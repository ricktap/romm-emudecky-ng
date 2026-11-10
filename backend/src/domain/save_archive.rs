//! A Switch save as one zip: build, read, validate and hash it the way RomM does.
//!
//! RomM stores a multi-file save as a single zip asset and identifies it by a content hash computed *per file entry*
//! (the zip framing is not part of it): the entries are sorted by name, rendered `name:md5hex`, joined with `\n`, and
//! the MD5 of that text is the hash. We must reproduce that scheme exactly or save sync would round-trip forever
//! (ADR-0016, ADR-0023 §3). Entry names are part of the hash, so they are role-based and stable across devices.
//! No I/O: everything works on byte buffers.
use std::{collections::BTreeSet, io::{Cursor, Read, Write}};

use md5::{Digest, Md5};
use zip::{write::SimpleFileOptions, CompressionMethod, DateTime, ZipArchive, ZipWriter};

/// Most files and bytes a save archive may hold: a guard against a hostile or corrupt archive, far above any real save.
pub const MAX_ENTRIES: usize = 20_000;
pub const MAX_TOTAL_BYTES: u64 = 1 << 30;

/// The first path component of every entry: the role the content has, not a machine-specific ID.
pub const ROLES: [&str; 3] = ["zero", "user", "index"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    pub path: String,
    pub data: Vec<u8>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ArchiveError {
    #[error("not a readable zip archive")]
    Unreadable,
    #[error("unsafe entry path: {0}")]
    UnsafePath(String),
    #[error("duplicate entry: {0}")]
    Duplicate(String),
    #[error("entry is a symbolic link: {0}")]
    Symlink(String),
    #[error("archive is too large or has too many entries")]
    TooLarge,
}

/// MD5 of `bytes` as lowercase hex.
pub fn md5_hex(bytes: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(bytes);
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// RomM's zip content hash from `(entry name, md5 of the entry's bytes)` pairs, byte for byte.
pub fn combine_entry_hashes(entries: &[(String, String)]) -> String {
    let mut sorted: Vec<&(String, String)> = entries.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let text = sorted.iter().map(|(name, digest)| format!("{name}:{digest}")).collect::<Vec<_>>().join("\n");
    md5_hex(text.as_bytes())
}

/// The content hash RomM computes for a save made of exactly these file entries.
pub fn content_hash(entries: &[ArchiveEntry]) -> String {
    let pairs: Vec<(String, String)> = entries.iter().map(|e| (e.path.clone(), md5_hex(&e.data))).collect();
    combine_entry_hashes(&pairs)
}

/// A relative path made of plain components under one of the [`ROLES`]: no absolute path, no `..`, no empty component,
/// no backslash, no NUL, no trailing slash.
pub fn validate_entry_path(path: &str) -> Result<(), ArchiveError> {
    let bad = || ArchiveError::UnsafePath(path.chars().take(120).collect());
    if path.is_empty() || path.starts_with('/') || path.ends_with('/') || path.contains('\\') || path.contains('\0') {
        return Err(bad());
    }
    let mut parts = path.split('/');
    let role = parts.next().unwrap_or("");
    if !ROLES.contains(&role) {
        return Err(bad());
    }
    let mut rest = 0;
    for part in parts {
        if part.is_empty() || part == "." || part == ".." {
            return Err(bad());
        }
        rest += 1;
    }
    if rest == 0 { Err(bad()) } else { Ok(()) }
}

/// A deterministic zip of `entries` (sorted by path, stored, fixed timestamp and mode), so the same files always give the
/// same bytes. Only files are written; an empty directory is not part of a save.
pub fn build(entries: &[ArchiveEntry]) -> Result<Vec<u8>, ArchiveError> {
    if entries.len() > MAX_ENTRIES || entries.iter().map(|e| e.data.len() as u64).sum::<u64>() > MAX_TOTAL_BYTES {
        return Err(ArchiveError::TooLarge);
    }
    let mut seen = BTreeSet::new();
    for entry in entries {
        validate_entry_path(&entry.path)?;
        if !seen.insert(entry.path.as_str()) {
            return Err(ArchiveError::Duplicate(entry.path.clone()));
        }
    }
    let mut sorted: Vec<&ArchiveEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(DateTime::default())
        .unix_permissions(0o644);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for entry in sorted {
        writer.start_file(entry.path.as_str(), options).map_err(|_| ArchiveError::Unreadable)?;
        writer.write_all(&entry.data).map_err(|_| ArchiveError::Unreadable)?;
    }
    writer.finish().map(Cursor::into_inner).map_err(|_| ArchiveError::Unreadable)
}

/// The file entries of an archive, validated. Directory entries are skipped (RomM skips them too); a symbolic link, an
/// unsafe or duplicate path, or a size beyond the limits fails the whole archive: nothing is ever extracted partially.
pub fn read(bytes: &[u8]) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|_| ArchiveError::Unreadable)?;
    if archive.len() > MAX_ENTRIES {
        return Err(ArchiveError::TooLarge);
    }
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|_| ArchiveError::Unreadable)?;
        let name = file.name().map(|n| n.into_owned()).map_err(|_| ArchiveError::UnsafePath("entry name is not valid UTF-8".into()))?;
        if name.ends_with('/') {
            continue;
        }
        if file.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            return Err(ArchiveError::Symlink(name));
        }
        validate_entry_path(&name)?;
        if !seen.insert(name.clone()) {
            return Err(ArchiveError::Duplicate(name));
        }
        total = total.saturating_add(file.size());
        if total > MAX_TOTAL_BYTES {
            return Err(ArchiveError::TooLarge);
        }
        let mut data = Vec::with_capacity(file.size().min(1 << 20) as usize);
        file.by_ref().take(MAX_TOTAL_BYTES + 1).read_to_end(&mut data).map_err(|_| ArchiveError::Unreadable)?;
        out.push(ArchiveEntry { path: name, data });
    }
    Ok(out)
}

/// RomM's content hash of an archive's bytes; `None` when it is not a zip (RomM then hashes the plain bytes).
pub fn archive_content_hash(bytes: &[u8]) -> Result<String, ArchiveError> {
    Ok(content_hash(&read(bytes)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(path: &str, data: &str) -> ArchiveEntry {
        ArchiveEntry { path: path.into(), data: data.as_bytes().to_vec() }
    }

    #[test]
    fn md5_matches_the_standard_vectors() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn the_combined_hash_is_romms_scheme_sorted_name_colon_md5_joined_by_newline() {
        let a = md5_hex(b"A");
        let b = md5_hex(b"B");
        // Order of the input does not matter; the text hashed is "a:<md5 A>\nb:<md5 B>".
        let expected = md5_hex(format!("a:{a}\nb:{b}").as_bytes());
        assert_eq!(combine_entry_hashes(&[("b".into(), b.clone()), ("a".into(), a.clone())]), expected);
        assert_eq!(combine_entry_hashes(&[("a".into(), a), ("b".into(), b)]), expected);
        assert_eq!(combine_entry_hashes(&[]), md5_hex(b""), "no file entries hash the empty string");
    }

    #[test]
    fn the_hash_equals_the_one_the_python_reference_computes() {
        // Computed independently with hashlib exactly as `combine_zip_entry_hashes` did (commit 5806808).
        let entries = [e("zero/T/b", "yy"), e("user/T/a", "x")];
        assert_eq!(content_hash(&entries), "e9cdce9b54c0684b9e97df870fd790fe");
        assert_eq!(archive_content_hash(&build(&entries).unwrap()).unwrap(), "e9cdce9b54c0684b9e97df870fd790fe");
    }

    #[test]
    fn the_name_is_part_of_the_hash_not_only_the_bytes() {
        assert_ne!(content_hash(&[e("user/T/a", "x")]), content_hash(&[e("user/T/b", "x")]));
        assert_ne!(content_hash(&[e("user/T/a", "x")]), content_hash(&[e("user/T/a", "y")]));
    }

    #[test]
    fn building_is_deterministic_and_independent_of_the_input_order() {
        let one = build(&[e("user/T/b", "2"), e("zero/T/a", "1")]).unwrap();
        let two = build(&[e("zero/T/a", "1"), e("user/T/b", "2")]).unwrap();
        assert_eq!(one, two);
        assert_eq!(one, build(&[e("zero/T/a", "1"), e("user/T/b", "2")]).unwrap());
    }

    #[test]
    fn a_round_trip_returns_the_files_and_the_same_hash() {
        let entries = vec![e("user/T/data/0", "hello"), e("zero/T/meta", "")];
        let bytes = build(&entries).unwrap();
        let mut back = read(&bytes).unwrap();
        back.sort_by(|a, b| a.path.cmp(&b.path));
        let mut want = entries.clone();
        want.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(back, want);
        assert_eq!(archive_content_hash(&bytes).unwrap(), content_hash(&entries));
    }

    #[test]
    fn the_archive_bytes_do_not_decide_the_hash_only_the_entries_do() {
        // A foreign zip (compressed, other timestamps, a directory entry) with the same files hashes the same.
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        writer.add_directory("user/T/", deflated).unwrap();
        writer.start_file("user/T/a", deflated).unwrap();
        writer.write_all(b"x").unwrap();
        let foreign = writer.finish().unwrap().into_inner();
        assert_ne!(foreign, build(&[e("user/T/a", "x")]).unwrap());
        assert_eq!(archive_content_hash(&foreign).unwrap(), content_hash(&[e("user/T/a", "x")]));
    }

    #[test]
    fn unsafe_and_unrooted_paths_are_refused_everywhere() {
        for bad in ["", "user", "user/", "/user/a", "user//a", "user/../a", "user/./a", "other/a", "user\\a", "user/a/", "user/a\0b", "../a"] {
            assert!(validate_entry_path(bad).is_err(), "{bad:?}");
            assert!(build(&[e(bad, "x")]).is_err(), "{bad:?}");
        }
        for ok in ["zero/T/a", "user/T/a/b/c", "index/0/save.meta", "index/ExtraData0"] {
            assert!(validate_entry_path(ok).is_ok(), "{ok:?}");
        }
    }

    #[test]
    fn duplicate_entries_are_refused_on_build() {
        assert_eq!(build(&[e("user/T/a", "1"), e("user/T/a", "2")]), Err(ArchiveError::Duplicate("user/T/a".into())));
    }

    #[test]
    fn reading_rejects_a_hostile_archive_as_a_whole() {
        let hostile = |name: &str| {
            let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
            writer.start_file(name, SimpleFileOptions::default()).unwrap();
            writer.write_all(b"x").unwrap();
            writer.finish().unwrap().into_inner()
        };
        assert!(matches!(read(&hostile("user/../../etc/passwd")), Err(ArchiveError::UnsafePath(_))));
        assert!(matches!(read(&hostile("/etc/passwd")), Err(ArchiveError::UnsafePath(_))));
        assert!(matches!(read(&hostile("other/x")), Err(ArchiveError::UnsafePath(_))));
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.add_symlink("user/T/link", "/etc/passwd", SimpleFileOptions::default()).unwrap();
        assert!(matches!(read(&writer.finish().unwrap().into_inner()), Err(ArchiveError::Symlink(_))));
    }

    #[test]
    fn garbage_is_not_a_zip() {
        assert_eq!(read(b"not a zip"), Err(ArchiveError::Unreadable));
        assert_eq!(read(b""), Err(ArchiveError::Unreadable));
    }

    #[test]
    fn building_rejects_an_oversized_archive() {
        let many: Vec<ArchiveEntry> = (0..=MAX_ENTRIES).map(|i| e(&format!("user/T/{i}"), "")).collect();
        assert_eq!(build(&many), Err(ArchiveError::TooLarge));
    }
}
