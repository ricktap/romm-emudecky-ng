//! Switch save facts that need no I/O: title IDs, the emulators' config and metadata formats, profile roles and the
//! "which save folder changed while playing" diff (ADR-0023). Nothing here guesses: every function answers `None` or a
//! "needs a choice" when the input does not say.
use std::collections::BTreeMap;

/// A Switch title ID: 16 hex digits, first byte `01` for applications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TitleId(pub u64);

impl TitleId {
    /// `01008CF01BAAC000`, upper case, the form used by folder names.
    pub fn to_hex(self) -> String {
        format!("{:016X}", self.0)
    }

    /// Exactly 16 hex digits (either case) and an application ID (`01…`); anything else is not a title ID.
    pub fn parse(text: &str) -> Option<TitleId> {
        if text.len() != 16 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let value = u64::from_str_radix(text, 16).ok()?;
        (value >> 56 == 0x01).then_some(TitleId(value))
    }

    /// The base game's ID. An update is `base | 0x800`; DLC is `base + 0x1000 + n` (and owns no saves). A base ID ends in
    /// `000` and is returned unchanged.
    pub fn base(self) -> TitleId {
        let low = self.0 & 0xFFF;
        match low {
            0 => self,
            0x800 => TitleId(self.0 & !0xFFF),
            _ => TitleId((self.0 & !0xFFF).wrapping_sub(0x1000)),
        }
    }
}

/// The title ID tagged in a ROM file name: exactly one `[01XXXXXXXXXXXXXX]` tag, normalised to the base title. Two tags
/// (or none) answer `None` rather than pick one.
pub fn title_id_from_file_name(name: &str) -> Option<TitleId> {
    let mut found: Option<TitleId> = None;
    let mut rest = name;
    while let Some(start) = rest.find('[') {
        let after = &rest[start + 1..];
        let Some(end) = after.find(']') else { break };
        if let Some(id) = TitleId::parse(&after[..end]) {
            if found.is_some() {
                return None;
            }
            found = Some(id.base());
        }
        rest = &after[end + 1..];
    }
    found
}

/// Where an emulator keeps its NAND, from its `qt-config.ini`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NandSetting {
    /// The config says "default" (or has no entry): the emulator's own default location applies.
    Default,
    Custom(String),
}

/// `nand_directory` from a Qt `qt-config.ini`. The key `nand_directory\default=false` marks a custom value; without it,
/// or with `=true`, the default applies. An empty custom value is treated as the default.
pub fn parse_nand_directory(config: &str) -> NandSetting {
    let mut value: Option<String> = None;
    let mut is_default = true;
    for line in config.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("nand_directory\\default=") {
            is_default = v.trim() != "false";
        } else if let Some(v) = line.strip_prefix("nand_directory=") {
            value = Some(v.trim().to_string());
        }
    }
    match value {
        Some(v) if !is_default && !v.is_empty() => NandSetting::Custom(v),
        _ => NandSetting::Default,
    }
}

/// What Ryujinx records in `ExtraData0` of a save index (`SaveDataAttribute`): the title, the user and the save type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RyujinxSave {
    pub title_id: TitleId,
    pub user_id: [u8; 16],
    pub save_type: u8,
}

/// The save type byte of an account save (the only kind that is synced).
pub const RYUJINX_ACCOUNT_SAVE: u8 = 1;

/// Read the header of `ExtraData0`: bytes 0-7 little-endian title ID, 8-23 user ID, byte 0x20 save type. `None` when the
/// file is too short or the title is not an application ID.
pub fn parse_ryujinx_extra_data(bytes: &[u8]) -> Option<RyujinxSave> {
    if bytes.len() < 0x21 {
        return None;
    }
    let raw = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
    let title_id = TitleId::parse(&format!("{raw:016X}"))?;
    let mut user_id = [0u8; 16];
    user_id.copy_from_slice(&bytes[8..24]);
    Some(RyujinxSave { title_id, user_id, save_type: bytes[0x20] })
}

/// The all-zero profile folder of Eden/yuzu (saves not bound to a user).
pub const ZERO_PROFILE: &str = "00000000000000000000000000000000";

fn is_profile_dir(name: &str) -> bool {
    name.len() == 32 && name.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Which user profile of an emulator holds a game's save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileChoice {
    /// Exactly one user profile exists, or the configured one does: use it.
    Use(String),
    /// Several user profiles and none configured: the user must pick; until then the game is unsupported.
    NeedsChoice(Vec<String>),
    /// No user profile folder at all (the game has not saved yet).
    NoneYet,
}

/// Choose the user profile among the profile folders found (the all-zero folder is not a user profile). A configured
/// profile that is not among them is ignored, not trusted.
pub fn choose_user_profile(folders: &[String], configured: Option<&str>) -> ProfileChoice {
    let mut users: Vec<String> = folders
        .iter()
        .filter(|f| is_profile_dir(f) && !f.eq_ignore_ascii_case(ZERO_PROFILE))
        .map(|f| f.to_ascii_uppercase())
        .collect();
    users.sort();
    users.dedup();
    if let Some(c) = configured {
        let c = c.to_ascii_uppercase();
        if users.contains(&c) {
            return ProfileChoice::Use(c);
        }
    }
    match users.len() {
        0 => ProfileChoice::NoneYet,
        1 => ProfileChoice::Use(users.remove(0)),
        _ => ProfileChoice::NeedsChoice(users),
    }
}

/// One entry of Eden's/yuzu's `profiles.dat`: the profile folder name and the name the emulator shows for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileInfo {
    /// The 32-hex folder name under `nand/user/save/0000000000000000/`.
    pub id: String,
    pub name: String,
}

const PROFILES_HEADER: usize = 0x10;
const PROFILE_RECORD: usize = 0xC8;
const PROFILE_SLOTS: usize = 8;
const PROFILE_NAME_AT: usize = 0x28;
const PROFILE_NAME_LEN: usize = 0x20;

/// The profiles in `profiles.dat` (`nand/system/save/8000000000000010/su/avators/profiles.dat`): a 16-byte header and 8
/// records of 200 bytes. A record holds the user UUID twice (16 bytes each), a timestamp and a 32-byte name; the folder name
/// is the UUID with all 16 bytes **reversed**, in upper-case hex (verified against the real save folder on a Deck). Empty
/// slots (an all-zero UUID) are skipped; a file of any other size is not trusted.
pub fn parse_profiles_dat(bytes: &[u8]) -> Option<Vec<ProfileInfo>> {
    if bytes.len() != PROFILES_HEADER + PROFILE_SLOTS * PROFILE_RECORD {
        return None;
    }
    let mut out = Vec::new();
    for slot in 0..PROFILE_SLOTS {
        let record = &bytes[PROFILES_HEADER + slot * PROFILE_RECORD..][..PROFILE_RECORD];
        let uuid = &record[..16];
        if uuid.iter().all(|b| *b == 0) {
            continue;
        }
        let id: String = uuid.iter().rev().map(|b| format!("{b:02X}")).collect();
        let raw = &record[PROFILE_NAME_AT..PROFILE_NAME_AT + PROFILE_NAME_LEN];
        let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
        let name = String::from_utf8_lossy(&raw[..end]).trim().to_string();
        out.push(ProfileInfo { id, name });
    }
    Some(out)
}

/// The shown name of a profile folder, or the folder name itself when `profiles.dat` does not know it.
pub fn profile_label(profiles: &[ProfileInfo], folder: &str) -> String {
    profiles
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(folder))
        .filter(|p| !p.name.is_empty())
        .map_or_else(|| folder.to_string(), |p| p.name.clone())
}

/// What a title's save folder looked like at one moment: enough to see that anything changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FolderStamp {
    pub files: u64,
    pub bytes: u64,
    /// Newest modification time, seconds since the epoch.
    pub newest: i64,
}

/// Title folders whose content differs between two snapshots, taken before and after a play session: new folders and
/// changed ones (a folder that vanished is not a change we learn from).
pub fn changed_titles(before: &BTreeMap<TitleId, FolderStamp>, after: &BTreeMap<TitleId, FolderStamp>) -> Vec<TitleId> {
    after.iter().filter(|(id, stamp)| before.get(id) != Some(stamp)).map(|(id, _)| *id).collect()
}

/// The save folder to propose for a game after a session: only when exactly one folder changed. Several or none: no proposal.
pub fn propose_title(changed: &[TitleId]) -> Option<TitleId> {
    match changed {
        [only] => Some(*only),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tid(s: &str) -> TitleId {
        TitleId::parse(s).unwrap()
    }

    #[test]
    fn a_title_id_is_sixteen_hex_digits_with_an_application_prefix() {
        assert_eq!(tid("01008cf01baac000").to_hex(), "01008CF01BAAC000");
        for bad in ["", "01008CF01BAAC00", "01008CF01BAAC0000", "02008CF01BAAC000", "0100GCF01BAAC000", " 01008CF01BAAC000"] {
            assert_eq!(TitleId::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn updates_and_dlc_normalise_to_the_base_title() {
        assert_eq!(tid("0100152000022000").base(), tid("0100152000022000"), "base");
        assert_eq!(tid("0100152000022800").base(), tid("0100152000022000"), "update");
        assert_eq!(tid("0100152000023001").base(), tid("0100152000022000"), "dlc");
        assert_eq!(tid("0100152000023fff").base(), tid("0100152000022000"), "last dlc of the range");
    }

    #[test]
    fn the_file_name_tag_is_used_only_when_there_is_exactly_one() {
        assert_eq!(title_id_from_file_name("Pokemon Shining Pearl [010018E011D92000][v0].nsp"), Some(tid("010018E011D92000")));
        assert_eq!(title_id_from_file_name("Game [010018E011D92800][v196608].nsp"), Some(tid("010018E011D92000")), "an update tag names its base");
        assert_eq!(title_id_from_file_name("Metroid Dread NSW-VENOM.xci"), None);
        assert_eq!(title_id_from_file_name("A [010018E011D92000] B [0100152000022000].nsp"), None, "two tags: no pick");
        assert_eq!(title_id_from_file_name("A [v0][USA][not an id].nsp"), None);
        assert_eq!(title_id_from_file_name("broken [010018E011D92000"), None);
    }

    #[test]
    fn nand_directory_follows_the_default_flag() {
        let custom = "[Data%20Storage]\nnand_directory\\default=false\nnand_directory=/run/media/mmcblk0p1/Emulation/storage/yuzu/nand\n";
        assert_eq!(parse_nand_directory(custom), NandSetting::Custom("/run/media/mmcblk0p1/Emulation/storage/yuzu/nand".into()));
        // key order does not matter
        let reversed = "nand_directory=/x/nand\nnand_directory\\default=false";
        assert_eq!(parse_nand_directory(reversed), NandSetting::Custom("/x/nand".into()));
        assert_eq!(parse_nand_directory("nand_directory\\default=true\nnand_directory=/ignored"), NandSetting::Default);
        assert_eq!(parse_nand_directory("nand_directory=/no/flag"), NandSetting::Default);
        assert_eq!(parse_nand_directory("nand_directory\\default=false\nnand_directory="), NandSetting::Default);
        assert_eq!(parse_nand_directory(""), NandSetting::Default);
    }

    #[test]
    fn ryujinx_extra_data_is_read_from_the_header_of_the_real_dump() {
        // The first 64 bytes of ExtraData0 of Echoes of Wisdom on the Deck (ADR-0023 §7).
        let mut bytes = vec![0u8; 64];
        bytes[..16].copy_from_slice(&[0x00, 0xc0, 0xaa, 0x1b, 0xf0, 0x8c, 0x00, 0x01, 0x01, 0, 0, 0, 0, 0, 0, 0]);
        bytes[0x20] = 0x01;
        let save = parse_ryujinx_extra_data(&bytes).unwrap();
        assert_eq!(save.title_id, tid("01008CF01BAAC000"));
        assert_eq!(save.user_id[0], 1);
        assert_eq!(save.save_type, RYUJINX_ACCOUNT_SAVE);
    }

    #[test]
    fn ryujinx_extra_data_that_is_short_or_not_an_application_is_not_trusted() {
        assert_eq!(parse_ryujinx_extra_data(&[0u8; 0x20]), None);
        assert_eq!(parse_ryujinx_extra_data(&[0u8; 512]), None, "title 0 is no application");
        let mut system = vec![0u8; 512];
        system[7] = 0x80; // 0x8000...: a system save, not an application
        assert_eq!(parse_ryujinx_extra_data(&system), None);
    }

    fn dirs(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn exactly_one_user_profile_is_used_and_the_zero_profile_is_never_a_user() {
        let deck = dirs(&["7EBDCC305B4640571756A9D7A0BCDB3C", ZERO_PROFILE]);
        assert_eq!(choose_user_profile(&deck, None), ProfileChoice::Use("7EBDCC305B4640571756A9D7A0BCDB3C".into()));
        assert_eq!(choose_user_profile(&dirs(&[ZERO_PROFILE]), None), ProfileChoice::NoneYet);
        assert_eq!(choose_user_profile(&[], None), ProfileChoice::NoneYet);
    }

    #[test]
    fn several_user_profiles_need_a_choice_and_a_configured_one_must_exist() {
        let two = dirs(&["BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", ZERO_PROFILE]);
        assert_eq!(
            choose_user_profile(&two, None),
            ProfileChoice::NeedsChoice(dirs(&["AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB"]))
        );
        assert_eq!(choose_user_profile(&two, Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")), ProfileChoice::Use("BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB".into()));
        assert!(matches!(choose_user_profile(&two, Some("CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC")), ProfileChoice::NeedsChoice(_)), "an unknown configured profile is ignored");
    }

    #[test]
    fn folders_that_are_not_profiles_are_ignored() {
        assert_eq!(choose_user_profile(&dirs(&["readme.txt", "short"]), None), ProfileChoice::NoneYet);
    }

    fn stamp(files: u64, bytes: u64, newest: i64) -> FolderStamp {
        FolderStamp { files, bytes, newest }
    }

    #[test]
    fn the_changed_folder_after_a_session_is_proposed_only_when_it_is_the_only_one() {
        let a = tid("01008CF01BAAC000");
        let b = tid("0100152000022000");
        let before = BTreeMap::from([(a, stamp(3, 100, 10)), (b, stamp(1, 5, 7))]);
        let touched = BTreeMap::from([(a, stamp(3, 120, 99)), (b, stamp(1, 5, 7))]);
        assert_eq!(changed_titles(&before, &touched), vec![a]);
        assert_eq!(propose_title(&changed_titles(&before, &touched)), Some(a));
        let both = BTreeMap::from([(a, stamp(3, 120, 99)), (b, stamp(2, 9, 99))]);
        assert_eq!(propose_title(&changed_titles(&before, &both)), None, "two changed folders: no proposal");
        assert_eq!(propose_title(&changed_titles(&before, &before)), None, "nothing changed");
    }

    #[test]
    fn a_new_folder_counts_as_changed_and_a_vanished_one_does_not() {
        let a = tid("01008CF01BAAC000");
        let b = tid("0100152000022000");
        let before = BTreeMap::from([(b, stamp(1, 5, 7))]);
        let after = BTreeMap::from([(a, stamp(1, 1, 1))]);
        assert_eq!(changed_titles(&before, &after), vec![a]);
    }

    fn profiles_dat_with(uuid: [u8; 16], name: &str) -> Vec<u8> {
        let mut bytes = vec![0u8; 0x10 + 8 * 0xC8];
        let rec = &mut bytes[0x10..0x10 + 0xC8];
        rec[..16].copy_from_slice(&uuid);
        rec[16..32].copy_from_slice(&uuid);
        rec[0x28..0x28 + name.len()].copy_from_slice(name.as_bytes());
        bytes
    }

    const DECK_UUID: [u8; 16] = [0x3c, 0xdb, 0xbc, 0xa0, 0xd7, 0xa9, 0x56, 0x17, 0x57, 0x40, 0x46, 0x5b, 0x30, 0xcc, 0xbd, 0x7e];

    #[test]
    fn profiles_dat_gives_the_folder_name_by_reversing_the_uuid_bytes_and_the_shown_name() {
        // The record of the real Deck (hex dump in ADR-0023): the reversed bytes are the save folder name.
        let profiles = parse_profiles_dat(&profiles_dat_with(DECK_UUID, "Eden")).unwrap();
        assert_eq!(profiles, vec![ProfileInfo { id: "7EBDCC305B4640571756A9D7A0BCDB3C".into(), name: "Eden".into() }]);
        assert_eq!(profile_label(&profiles, "7ebdcc305b4640571756a9d7a0bcdb3c"), "Eden");
        assert_eq!(profile_label(&profiles, ZERO_PROFILE), ZERO_PROFILE, "an unknown folder keeps its folder name");
    }

    #[test]
    fn profiles_dat_skips_empty_slots_and_distrusts_a_file_of_the_wrong_size() {
        assert_eq!(parse_profiles_dat(&vec![0u8; 0x10 + 8 * 0xC8]), Some(vec![]));
        assert_eq!(parse_profiles_dat(&[0u8; 1615]), None);
        assert_eq!(parse_profiles_dat(&[]), None);
    }

    #[test]
    fn a_profile_without_a_name_is_labelled_by_its_folder() {
        let profiles = parse_profiles_dat(&profiles_dat_with(DECK_UUID, "")).unwrap();
        assert_eq!(profile_label(&profiles, "7EBDCC305B4640571756A9D7A0BCDB3C"), "7EBDCC305B4640571756A9D7A0BCDB3C");
    }
}
