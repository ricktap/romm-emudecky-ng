//! Matching hand-copied games to RomM ROMs by normalized title.
//!
//! The fallback adoption rule for systems whose games arrive under free-form
//! names (Switch dumps keep scene names, title-id and version tags, while RomM
//! sanitizes its folder names): a disk entry and a RomM ROM belong together
//! only when their titles are exactly equal after normalization, and only when
//! that pairing is one-to-one. Sizes only pick a folder's launch file.

use std::collections::{BTreeMap, BTreeSet};

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::canonical_combining_class;

use crate::domain::disc_selection::enumerate_size_ranked;
use crate::domain::rom_files::splitext;

/// Container extensions dropped from the end of a name before comparing.
const TITLE_EXTENSIONS: [&str; 4] = [".xci", ".nsp", ".nsz", ".xcz"];

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Replace every `[...]` and `(...)` group with a space.
fn drop_bracket_groups(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let close = match chars[i] {
            '[' => Some(']'),
            '(' => Some(')'),
            _ => None,
        };
        if let Some(close) = close
            && let Some(len) = chars[i + 1..].iter().position(|c| *c == close)
        {
            out.push(' ');
            i += len + 2;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Replace a trailing `nsw-<group>` scene suffix (at a word boundary) with a space.
fn drop_scene_suffix(text: &str) -> String {
    let run_start = text
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_ascii_lowercase() || c.is_ascii_digit())
        .last()
        .map(|(i, _)| i);
    let Some(run_start) = run_start else {
        return text.to_string();
    };
    let Some(prefix_start) = run_start.checked_sub(4) else {
        return text.to_string();
    };
    if text.get(prefix_start..run_start) != Some("nsw-") {
        return text.to_string();
    }
    if text[..prefix_start].chars().next_back().is_some_and(is_word) {
        return text.to_string();
    }
    format!("{} ", &text[..prefix_start])
}

/// Reduce a ROM or file/folder name to a comparable title key: NFKD without
/// combining marks, lowercase, one trailing `.xci`/`.nsp`/`.nsz`/`.xcz`
/// dropped, every bracket group and a trailing `NSW-<group>` dropped, every
/// run of non-alphanumerics turned into one space. Empty means "no title".
pub fn normalize_title(name: &str) -> String {
    let folded: String = name.nfkd().filter(|c| canonical_combining_class(*c) == 0).collect();
    let mut text = folded.to_lowercase().trim().to_string();
    for ext in TITLE_EXTENSIONS {
        if text.ends_with(ext) {
            text.truncate(text.len() - ext.len());
            break;
        }
    }
    let text = drop_bracket_groups(&text).trim().to_string();
    let text = drop_scene_suffix(&text);
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// One top-level entry of a system folder. `files` are the `(name, size)`
/// regular files directly inside a directory entry; for a file entry it is the
/// entry itself. `claimed` marks an entry an existing install points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskEntry {
    pub name: String,
    pub is_dir: bool,
    pub files: Vec<(String, u64)>,
    pub claimed: bool,
}

/// A RomM ROM paired with a disk entry by normalized title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleMatch {
    pub rom_id: i64,
    pub entry_name: String,
    pub is_dir: bool,
    pub launch_file: String,
}

/// The launch file of `entry`: the entry itself when its extension is in
/// `extensions`, or the largest qualifying top-level file of a directory.
pub fn entry_launch_file(entry: &DiskEntry, extensions: &[&str]) -> Option<String> {
    if !entry.is_dir {
        let ext = splitext(&entry.name).1.to_lowercase();
        return extensions.contains(&ext.as_str()).then(|| entry.name.clone());
    }
    enumerate_size_ranked(&entry.files, extensions)
        .into_iter()
        .next()
        .map(|d| d.filename)
}

/// Pair RomM ROMs (`(rom_id, fs_name)` of every ROM of the system) with
/// qualifying disk entries whose normalized titles are equal. A title pairs
/// only when exactly one ROM and one qualifying entry carry it; claimed
/// entries count for ambiguity but are never returned. Sorted by `rom_id`.
pub fn pair_by_title(roms: &[(i64, String)], entries: &[DiskEntry], extensions: &[&str]) -> Vec<TitleMatch> {
    let mut rom_ids_by_title: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    for (rom_id, fs_name) in roms {
        let title = normalize_title(fs_name);
        if !title.is_empty() {
            rom_ids_by_title.entry(title).or_default().insert(*rom_id);
        }
    }
    let mut entries_by_title: BTreeMap<String, Vec<(&DiskEntry, String)>> = BTreeMap::new();
    for entry in entries {
        let title = normalize_title(&entry.name);
        if title.is_empty() || !rom_ids_by_title.contains_key(&title) {
            continue;
        }
        if let Some(launch) = entry_launch_file(entry, extensions) {
            entries_by_title.entry(title).or_default().push((entry, launch));
        }
    }
    let mut matches: Vec<TitleMatch> = entries_by_title
        .into_iter()
        .filter_map(|(title, found)| {
            let ids = &rom_ids_by_title[&title];
            if ids.len() != 1 || found.len() != 1 {
                return None;
            }
            let (entry, launch) = &found[0];
            if entry.claimed {
                return None;
            }
            Some(TitleMatch {
                rom_id: *ids.iter().next()?,
                entry_name: entry.name.clone(),
                is_dir: entry.is_dir,
                launch_file: launch.clone(),
            })
        })
        .collect();
    matches.sort_by_key(|m| m.rom_id);
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXTS: &[&str] = &[".xci", ".nsp"];
    const PAD: u64 = 7_985_954_816; // XCI cart padding: four different games share this exact size

    fn dir(name: &str, files: &[(&str, u64)]) -> DiskEntry {
        DiskEntry {
            name: name.into(),
            is_dir: true,
            files: files.iter().map(|(n, s)| (n.to_string(), *s)).collect(),
            claimed: false,
        }
    }

    fn file(name: &str) -> DiskEntry {
        DiskEntry {
            name: name.into(),
            is_dir: false,
            files: vec![(name.into(), 1)],
            claimed: false,
        }
    }

    fn roms(list: &[(i64, &str)]) -> Vec<(i64, String)> {
        list.iter().map(|(i, n)| (*i, n.to_string())).collect()
    }

    /// The real RomM library of the Deck.
    fn real_romm() -> Vec<(i64, String)> {
        roms(&[
            (1, "Catherine_ Full Body"),
            (2, "Fire Emblem_ Three Houses"),
            (3, "The Legend of Zelda_ Echoes of Wisdom"),
            (4, "Mario Kart 8 Deluxe"),
            (5, "Super Mario Galaxy"),
            (6, "Super Mario Galaxy 2"),
            (7, "Donkey Kong Country Returns HD_ Version 1.1.0"),
            (8, "Pokémon Legends_ Z-A"),
        ])
    }

    /// The real `roms/switch/` listing of the Deck.
    fn real_disk() -> Vec<DiskEntry> {
        vec![
            dir(
                "Catherine Full Body",
                &[("Catherine Full Body [0100BF00112C0000]+[v1.0.1](US-EU).xci", PAD)],
            ),
            dir("Fire Emblem Three Houses", &[("Fire Emblem Three Houses NSW-HR.xci", PAD)]),
            DiskEntry {
                files: vec![("The Legend Of Zelda Echoes Of Wisdom NSW-NiiNTENDO.xci".into(), PAD)],
                ..file("The Legend Of Zelda Echoes Of Wisdom NSW-NiiNTENDO.xci")
            },
            dir("Mario Kart 8 Deluxe", &[("Mario Kart 8 Deluxe [0100152000022000][v0].xci", PAD)]),
            dir("Pokemon Scarlet", &[("Pokemon Scarlet NSW-NiiNTENDO.xci", 100)]),
            file("Pokemon Shining Pearl [010018E011D92000][v0].nsp"),
            file("Super Mario Bros Wonder NSW-LiGHTFORCE.xci"),
            dir("Xenoblade Chronicles 3", &[]),
            file("metadata.txt"),
            file("systeminfo.txt"),
        ]
    }

    #[test]
    fn real_pairs_normalize_equal() {
        for (romm, disk) in [
            ("Catherine_ Full Body", "Catherine Full Body"),
            ("Catherine_ Full Body", "Catherine Full Body [0100BF00112C0000]+[v1.0.1](US-EU).xci"),
            ("Fire Emblem_ Three Houses", "Fire Emblem Three Houses"),
            ("Fire Emblem_ Three Houses", "Fire Emblem Three Houses NSW-HR.xci"),
            (
                "The Legend of Zelda_ Echoes of Wisdom",
                "The Legend Of Zelda Echoes Of Wisdom NSW-NiiNTENDO.xci",
            ),
            ("Mario Kart 8 Deluxe", "Mario Kart 8 Deluxe"),
            ("Mario Kart 8 Deluxe", "Mario Kart 8 Deluxe [0100152000022000][v0].xci"),
        ] {
            assert_eq!(normalize_title(romm), normalize_title(disk), "{romm} / {disk}");
            assert_ne!(normalize_title(romm), "");
        }
    }

    #[test]
    fn exact_values() {
        assert_eq!(normalize_title("Catherine_ Full Body"), "catherine full body");
        assert_eq!(normalize_title("Fire Emblem Three Houses NSW-HR.xci"), "fire emblem three houses");
        assert_eq!(normalize_title("Mario Kart 8 Deluxe [0100152000022000][v0].xci"), "mario kart 8 deluxe");
        assert_eq!(normalize_title("Pokemon Shining Pearl [010018E011D92000][v0].nsp"), "pokemon shining pearl");
        assert_eq!(
            normalize_title("Donkey Kong Country Returns HD_ Version 1.1.0"),
            "donkey kong country returns hd version 1 1 0"
        );
        assert_ne!(normalize_title("Super Mario Galaxy"), normalize_title("Super Mario Galaxy 2"));
    }

    #[test]
    fn unicode_folding() {
        assert_eq!(normalize_title("Pokémon Legends_ Z-A"), "pokemon legends z a");
        assert_eq!(
            normalize_title("Pokémon Legends_ Z-A"),
            normalize_title("Pokemon Legends Z-A NSW-GRP.nsp")
        );
        let nfc = "Pok\u{e9}mon";
        let nfd = "Poke\u{301}mon";
        assert_eq!(normalize_title(nfc), "pokemon");
        assert_eq!(normalize_title(nfd), "pokemon");
    }

    #[test]
    fn extensions_punctuation_and_suffixes() {
        for ext in [".xci", ".NSP", ".nsz", ".xcz"] {
            assert_eq!(normalize_title(&format!("Game{ext}")), "game");
        }
        assert_eq!(normalize_title("metadata.txt"), "metadata txt");
        assert_eq!(normalize_title("A_B:C-D+E'F.G,H&I!J"), "a b c d e f g h i j");
        assert_eq!(normalize_title("NSW-Thing Game"), "nsw thing game");
        assert_eq!(normalize_title("Game nsw-grp"), "game");
        assert_eq!(normalize_title("Game NSW-GRP [v0] (US).xci"), "game");
        assert_eq!(normalize_title("Gamensw-grp"), "gamensw grp");
        for degenerate in ["", "   ", "[0100].xci", "NSW-ABC.xci", "(US) [v0]", "___"] {
            assert_eq!(normalize_title(degenerate), "", "{degenerate:?}");
        }
        assert_eq!(normalize_title("Unclosed [tag"), "unclosed tag");
    }

    #[test]
    fn launch_file_of_entries() {
        assert_eq!(
            entry_launch_file(
                &DiskEntry {
                    files: vec![("A.XCI".into(), 1)],
                    ..file("A.XCI")
                },
                EXTS
            ),
            Some("A.XCI".into())
        );
        assert_eq!(entry_launch_file(&file("metadata.txt"), EXTS), None);
        let g = dir("G", &[("update.nsp", 5), ("base.xci", 50), ("readme.txt", 999)]);
        assert_eq!(entry_launch_file(&g, EXTS), Some("base.xci".into()));
        let tie = dir("G", &[("b.xci", 5), ("A.nsp", 5)]);
        assert_eq!(entry_launch_file(&tie, EXTS), Some("A.nsp".into()));
        assert_eq!(entry_launch_file(&dir("X", &[("notes.txt", 1)]), EXTS), None);
    }

    #[test]
    fn real_device_listing_pairs() {
        let matches = pair_by_title(&real_romm(), &real_disk(), EXTS);
        assert_eq!(
            matches,
            vec![
                TitleMatch {
                    rom_id: 1,
                    entry_name: "Catherine Full Body".into(),
                    is_dir: true,
                    launch_file: "Catherine Full Body [0100BF00112C0000]+[v1.0.1](US-EU).xci".into()
                },
                TitleMatch {
                    rom_id: 2,
                    entry_name: "Fire Emblem Three Houses".into(),
                    is_dir: true,
                    launch_file: "Fire Emblem Three Houses NSW-HR.xci".into()
                },
                TitleMatch {
                    rom_id: 3,
                    entry_name: "The Legend Of Zelda Echoes Of Wisdom NSW-NiiNTENDO.xci".into(),
                    is_dir: false,
                    launch_file: "The Legend Of Zelda Echoes Of Wisdom NSW-NiiNTENDO.xci".into()
                },
                TitleMatch {
                    rom_id: 4,
                    entry_name: "Mario Kart 8 Deluxe".into(),
                    is_dir: true,
                    launch_file: "Mario Kart 8 Deluxe [0100152000022000][v0].xci".into()
                },
            ]
        );
    }

    #[test]
    fn ambiguity_and_claims() {
        let mk = roms(&[(4, "Mario Kart 8 Deluxe")]);
        let two_entries = vec![dir("Mario Kart 8 Deluxe", &[("MK8.xci", 9)]), file("Mario Kart 8 Deluxe NSW-GRP.xci")];
        assert!(pair_by_title(&mk, &two_entries, EXTS).is_empty());
        let two_roms = roms(&[(4, "Mario Kart 8 Deluxe"), (9, "Mario Kart 8_ Deluxe")]);
        assert!(pair_by_title(&two_roms, &[dir("Mario Kart 8 Deluxe", &[("MK8.xci", 9)])], EXTS).is_empty());
        let dup_id = roms(&[(4, "Mario Kart 8 Deluxe"), (4, "Mario Kart 8 Deluxe")]);
        assert_eq!(pair_by_title(&dup_id, &[dir("Mario Kart 8 Deluxe", &[("MK8.xci", 9)])], EXTS).len(), 1);
        let non_qualifying = vec![
            dir("Mario Kart 8 Deluxe", &[("MK8.xci", 9)]),
            dir("Mario Kart 8 Deluxe", &[("notes.txt", 1)]),
        ];
        assert_eq!(pair_by_title(&mk, &non_qualifying, EXTS).len(), 1);
        let mut claimed = dir("Mario Kart 8 Deluxe", &[("MK8.xci", 9)]);
        claimed.claimed = true;
        assert!(pair_by_title(&mk, std::slice::from_ref(&claimed), EXTS).is_empty());
        assert!(pair_by_title(&mk, &[claimed, file("Mario Kart 8 Deluxe.xci")], EXTS).is_empty());
        let empty = roms(&[(1, "[0100]"), (2, "NSW-ABC")]);
        assert!(pair_by_title(&empty, &[file("[0200].xci"), file("NSW-XYZ.nsp")], EXTS).is_empty());
        let galaxy = roms(&[(5, "Super Mario Galaxy"), (6, "Super Mario Galaxy 2")]);
        let got = pair_by_title(&galaxy, &[file("Super Mario Galaxy 2 NSW-GRP.xci")], EXTS);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].rom_id, 6);
        assert!(pair_by_title(&[], &real_disk(), EXTS).is_empty());
        assert!(pair_by_title(&real_romm(), &[], EXTS).is_empty());
    }
}
