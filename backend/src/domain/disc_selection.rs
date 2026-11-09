//! Launch-file candidates of a folder install, and resolving the persisted
//! per-game pick (`roms.selected_disc`) over them.
//!
//! Switch folder installs hold a base game plus update/DLC files of the same
//! container type; the default launch file is the largest. Resolution only
//! chooses the path baked into the launch command; it never rewrites the
//! install's `file_path`.

use crate::domain::rom_files::splitext;
use crate::util::path_safety::basename;

/// One launch-file candidate (`filename` is the persisted selection key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disc {
    pub filename: String,
    pub path: String,
    pub label: String,
    pub index: usize,
}

/// Every file whose extension (case-insensitive) is in `extensions`, largest
/// first, ties broken by lowercase basename.
pub fn enumerate_size_ranked(files: &[(String, u64)], extensions: &[&str]) -> Vec<Disc> {
    let mut candidates: Vec<&(String, u64)> = files
        .iter()
        .filter(|(path, _)| extensions.contains(&splitext(path).1.to_lowercase().as_str()))
        .collect();
    candidates.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| basename(&a.0).to_lowercase().cmp(&basename(&b.0).to_lowercase()))
    });
    candidates
        .into_iter()
        .enumerate()
        .map(|(i, (path, _))| {
            let filename = basename(path).to_string();
            Disc {
                label: splitext(&filename).0.to_string(),
                filename,
                path: path.clone(),
                index: i + 1,
            }
        })
        .collect()
}

/// The path to bake over size-ranked `candidates`: a valid pin wins; a stale
/// pin degrades to the default (largest) and reports `stale`; no candidates
/// falls back to `file_path`. Returns `(path, stale)`.
pub fn resolve_size_ranked_path(file_path: &str, candidates: &[Disc], selected: Option<&str>) -> (String, bool) {
    let Some(first) = candidates.first() else {
        return (file_path.to_string(), false);
    };
    let mut stale = false;
    if let Some(selected) = selected {
        if let Some(c) = candidates.iter().find(|c| c.filename == selected) {
            return (c.path.clone(), false);
        }
        stale = true;
    }
    (first.path.clone(), stale)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXTS: &[&str] = &[".xci", ".nsp"];

    fn files(list: &[(&str, u64)]) -> Vec<(String, u64)> {
        list.iter().map(|(p, s)| (p.to_string(), *s)).collect()
    }

    #[test]
    fn ranks_largest_first_and_filters() {
        let discs = enumerate_size_ranked(
            &files(&[
                ("/g/update/u.nsp", 10),
                ("/g/base.XCI", 100),
                ("/g/readme.txt", 999),
                ("/g/B.nsp", 10),
            ]),
            EXTS,
        );
        let names: Vec<_> = discs.iter().map(|d| d.filename.as_str()).collect();
        assert_eq!(names, ["base.XCI", "B.nsp", "u.nsp"]);
        assert_eq!(discs[0].label, "base");
        assert_eq!(discs[0].index, 1);
        assert_eq!(discs[2].path, "/g/update/u.nsp");
    }

    #[test]
    fn resolves_pin_default_and_stale() {
        let discs = enumerate_size_ranked(&files(&[("/g/a.xci", 5), ("/g/b.nsp", 1)]), EXTS);
        assert_eq!(resolve_size_ranked_path("/g", &discs, None), ("/g/a.xci".into(), false));
        assert_eq!(resolve_size_ranked_path("/g", &discs, Some("b.nsp")), ("/g/b.nsp".into(), false));
        assert_eq!(resolve_size_ranked_path("/g", &discs, Some("gone.nsp")), ("/g/a.xci".into(), true));
        assert_eq!(resolve_size_ranked_path("/g/x", &[], Some("a")), ("/g/x".into(), false));
    }
}
