//! Path containment guards for server-supplied path components.
//!
//! The lexical guards ([`safe_path_component`], [`coerce_safe_component`],
//! [`normpath`]) do no I/O. The containment guards ([`safe_join`],
//! [`safe_join_lexical`], [`is_safe_rom_path`]) resolve symlinks of the parts
//! that exist (Python's non-strict `os.path.realpath`), so a symlink planted
//! under the base cannot be used to escape it. Validating guards return
//! [`PathTraversal`] so call sites fail closed.

use std::path::{Component, Path, PathBuf};

/// An untrusted path component escapes (or is not) a single safe component.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{0}")]
pub struct PathTraversal(pub String);

/// Validate that `name` is a single safe file name component: no NUL, not
/// empty / `.` / `..`, no separator, and a fixed point of [`normpath`].
pub fn safe_path_component(name: &str) -> Result<&str, PathTraversal> {
    if name.contains('\0') {
        return Err(PathTraversal("path component contains a NUL byte".into()));
    }
    if matches!(name, "" | "." | "..") {
        return Err(PathTraversal(format!("not a valid path component: {name:?}")));
    }
    if name.contains('/') {
        return Err(PathTraversal(format!(
            "path component must not contain a separator: {name:?}"
        )));
    }
    if normpath(name) != name {
        return Err(PathTraversal(format!("path component is not normalized: {name:?}")));
    }
    Ok(name)
}

/// The last component of `name` (Python `os.path.basename`: text after the last `/`).
pub fn basename(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

/// Coerce `name` to a single safe component: strip any directory portion and
/// substitute `fallback` for a degenerate result (empty, whitespace, `.`,
/// `..`). Returns `(component, changed)`.
pub fn coerce_safe_component(name: &str, fallback: &str) -> (String, bool) {
    let component = basename(name);
    if matches!(component.trim(), "" | "." | "..") {
        return (fallback.to_string(), true);
    }
    (component.to_string(), component != name)
}

/// Lexical normalization (Python `os.path.normpath` on POSIX).
pub fn normpath(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    let initial_slashes = if path.starts_with("//") && !path.starts_with("///") {
        2
    } else if path.starts_with('/') {
        1
    } else {
        0
    };
    let mut out: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp != ".." || (initial_slashes == 0 && out.is_empty()) || out.last() == Some(&"..") {
            out.push(comp);
        } else if !out.is_empty() {
            out.pop();
        }
    }
    let joined = out.join("/");
    let prefix = "/".repeat(initial_slashes);
    let result = format!("{prefix}{joined}");
    if result.is_empty() { ".".into() } else { result }
}

/// Python `os.path.join` for POSIX string paths: an absolute part restarts.
pub fn join(base: &str, parts: &[&str]) -> String {
    let mut out = base.to_string();
    for part in parts {
        if part.starts_with('/') {
            out = (*part).to_string();
        } else if out.is_empty() || out.ends_with('/') {
            out.push_str(part);
        } else {
            out.push('/');
            out.push_str(part);
        }
    }
    out
}

/// Non-strict `realpath`: resolve every symlink in the existing prefix of
/// `path`, keep missing components lexically, collapse `.` and `..`.
pub fn realpath(path: &str) -> PathBuf {
    let abs = if path.starts_with('/') {
        PathBuf::from(path)
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")).join(path)
    };
    let mut resolved = PathBuf::from("/");
    resolve_into(&mut resolved, &abs, 0);
    resolved
}

fn resolve_into(resolved: &mut PathBuf, path: &Path, depth: u32) {
    for comp in path.components() {
        match comp {
            Component::RootDir => *resolved = PathBuf::from("/"),
            Component::CurDir | Component::Prefix(_) => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(name) => {
                let candidate = resolved.join(name);
                match std::fs::read_link(&candidate) {
                    Ok(target) if depth < 40 => {
                        if target.is_absolute() {
                            *resolved = PathBuf::from("/");
                        }
                        resolve_into(resolved, &target, depth + 1);
                    }
                    _ => *resolved = candidate,
                }
            }
        }
    }
}

fn under(child: &Path, base: &Path) -> bool {
    child != base && child.starts_with(base)
}

/// Join `parts` onto `base` and return the realpath; the result must resolve
/// strictly below `realpath(base)`.
pub fn safe_join(base: &str, parts: &[&str]) -> Result<PathBuf, PathTraversal> {
    let real_base = realpath(base);
    let joined = join(base, parts);
    let real_joined = realpath(&joined);
    if !under(&real_joined, &real_base) {
        return Err(PathTraversal(format!(
            "path escapes base directory: {joined:?} not under {base:?}"
        )));
    }
    Ok(real_joined)
}

/// [`safe_join`]'s guard, but return the lexically normalized join under the
/// caller's own spelling of `base` (a configured root reached through a
/// symlink stays spelled as configured). The lexical result must also sit
/// strictly below the normalized base.
pub fn safe_join_lexical(base: &str, parts: &[&str]) -> Result<String, PathTraversal> {
    safe_join(base, parts)?;
    let norm_base = normpath(base);
    let joined = normpath(&join(base, parts));
    let prefix = format!("{}/", norm_base.trim_end_matches('/'));
    if !joined.starts_with(&prefix) {
        return Err(PathTraversal(format!(
            "path escapes base directory: {joined:?} not under {base:?}"
        )));
    }
    Ok(joined)
}

/// True when `path` resolves strictly inside `roms_base` and at least two
/// components below it (a bare platform directory never qualifies).
pub fn is_safe_rom_path(path: &str, roms_base: &str) -> bool {
    let resolved = realpath(path);
    let base = realpath(roms_base);
    match resolved.strip_prefix(&base) {
        Ok(rel) if resolved != base => rel.components().count() >= 2,
        _ => false,
    }
}

/// Percent-decode `s` (Python `urllib.parse.unquote`, UTF-8 with replacement).
pub fn unquote(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Percent-encode `s` (Python `urllib.parse.quote`): letters, digits, `_.-~`
/// and the characters in `safe` stay; every other byte becomes `%XX`.
pub fn quote(s: &str, safe: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"_.-~".contains(&b) || (b.is_ascii() && safe.as_bytes().contains(&b)) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn component_validation() {
        assert_eq!(safe_path_component("game.nsp").unwrap(), "game.nsp");
        assert_eq!(safe_path_component("...").unwrap(), "...");
        for bad in ["", ".", "..", "a/b", "/abs", "x\0y", "../evil"] {
            assert!(safe_path_component(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn coercion() {
        assert_eq!(coerce_safe_component("a.gba", "rom_1"), ("a.gba".into(), false));
        assert_eq!(coerce_safe_component("../../etc/passwd", "rom_1"), ("passwd".into(), true));
        assert_eq!(coerce_safe_component("/abs/x", "rom_1"), ("x".into(), true));
        for degenerate in ["", "  ", ".", "..", "dir/", "a/.."] {
            assert_eq!(coerce_safe_component(degenerate, "rom_1"), ("rom_1".into(), true));
        }
    }

    #[test]
    fn normpath_matches_python() {
        assert_eq!(normpath("a/../b"), "b");
        assert_eq!(normpath("./x"), "x");
        assert_eq!(normpath("/a/b/../../.."), "/");
        assert_eq!(normpath("../x"), "../x");
        assert_eq!(normpath(""), ".");
        assert_eq!(normpath("//a"), "//a");
        assert_eq!(normpath("///a//b/"), "/a/b");
        assert_eq!(normpath("a/b/.."), "a");
        assert_eq!(normpath("a/.."), ".");
    }

    #[test]
    fn join_restarts_on_absolute() {
        assert_eq!(join("/r", &["a", "b"]), "/r/a/b");
        assert_eq!(join("/r/", &["a"]), "/r/a");
        assert_eq!(join("/r", &["/etc"]), "/etc");
        assert_eq!(join("", &["a"]), "a");
    }

    #[test]
    fn containment() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("roms");
        std::fs::create_dir_all(base.join("switch")).unwrap();
        let base_s = base.to_str().unwrap();
        assert!(safe_join(base_s, &["switch", "x.nsp"]).is_ok());
        assert!(safe_join(base_s, &["../escape"]).is_err());
        assert!(safe_join(base_s, &["."]).is_err());
        assert!(safe_join(base_s, &[""]).is_err());
        assert!(safe_join(base_s, &["/etc"]).is_err());
        // A symlink inside the base that points outside cannot escape.
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&outside, base.join("link")).unwrap();
        assert!(safe_join(base_s, &["link", "x"]).is_err());
        assert!(safe_join_lexical(base_s, &["link", "x"]).is_err());
        // `..` that resolves back inside is fine for safe_join but the lexical
        // result must still sit below the base.
        assert!(safe_join_lexical(base_s, &["switch/../switch/y"]).is_ok());
    }

    #[test]
    fn lexical_join_keeps_configured_spelling() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir_all(real.join("roms")).unwrap();
        let link = dir.path().join("mmcblk0p1");
        symlink(&real, &link).unwrap();
        let configured = link.join("roms");
        let got = safe_join_lexical(configured.to_str().unwrap(), &["switch"]).unwrap();
        assert_eq!(got, format!("{}/switch", configured.display()));
        let resolved = safe_join(configured.to_str().unwrap(), &["switch"]).unwrap();
        assert!(resolved.starts_with(realpath(real.to_str().unwrap())));
    }

    #[test]
    fn rom_path_safety() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("roms");
        std::fs::create_dir_all(base.join("gb")).unwrap();
        let b = base.to_str().unwrap();
        assert!(is_safe_rom_path(&format!("{b}/gb/file.gb"), b));
        assert!(!is_safe_rom_path(&format!("{b}/gb"), b));
        assert!(!is_safe_rom_path(b, b));
        assert!(!is_safe_rom_path("/etc/passwd", b));
        assert!(!is_safe_rom_path(&format!("{b}/gb/../../x/y"), b));
    }

    #[test]
    fn quoting_matches_python() {
        let f = quote("Mario Kart [v0].nsp", "");
        assert_eq!(f, "Mario%20Kart%20%5Bv0%5D.nsp");
        assert_eq!(
            quote(&format!("/api/roms/1/content/{f}"), "/:?=&@"),
            "/api/roms/1/content/Mario%2520Kart%2520%255Bv0%255D.nsp"
        );
        assert_eq!(quote("a/b", ""), "a%2Fb");
        assert_eq!(quote("é", ""), "%C3%A9");
    }

    #[test]
    fn unquoting() {
        assert_eq!(unquote("Disc%201.cue"), "Disc 1.cue");
        assert_eq!(unquote("%2e%2e%2fevil.sh"), "../evil.sh");
        assert_eq!(unquote("100%"), "100%");
        assert_eq!(unquote("%zz"), "%zz");
        assert_eq!(unquote("%C3%A9"), "é");
        assert_eq!(unquote("a%2"), "a%2");
    }
}
