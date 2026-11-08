"""Tests for domain.title_match — normalized-title pairing of hand-copied games.

Fixtures are the real names observed on a Steam Deck (``roms/switch/``) and in
the RomM library it syncs from.
"""

from __future__ import annotations

import pytest

from domain.title_match import DiskEntry, TitleMatch, entry_launch_file, normalize_title, pair_by_title

_EXTS = frozenset({".xci", ".nsp"})
_PAD = 7985954816  # XCI cart padding: four different games share this exact size

# RomM fs_name -> on-disk name that must normalize to the same title.
_REAL_PAIRS = [
    ("Catherine_ Full Body", "Catherine Full Body"),
    ("Catherine_ Full Body", "Catherine Full Body [0100BF00112C0000]+[v1.0.1](US-EU).xci"),
    ("Fire Emblem_ Three Houses", "Fire Emblem Three Houses"),
    ("Fire Emblem_ Three Houses", "Fire Emblem Three Houses NSW-HR.xci"),
    ("The Legend of Zelda_ Echoes of Wisdom", "The Legend Of Zelda Echoes Of Wisdom NSW-NiiNTENDO.xci"),
    ("Mario Kart 8 Deluxe", "Mario Kart 8 Deluxe"),
    ("Mario Kart 8 Deluxe", "Mario Kart 8 Deluxe [0100152000022000][v0].xci"),
]

# RomM library (fs_name), real on-device.
_ROMM = [
    (1, "Catherine_ Full Body"),
    (2, "Fire Emblem_ Three Houses"),
    (3, "The Legend of Zelda_ Echoes of Wisdom"),
    (4, "Mario Kart 8 Deluxe"),
    (5, "Super Mario Galaxy"),
    (6, "Super Mario Galaxy 2"),
    (7, "Donkey Kong Country Returns HD_ Version 1.1.0"),
    (8, "Pokémon Legends_ Z-A"),
]

# roms/switch/ listing, real on-device.
_DISK = [
    DiskEntry("Catherine Full Body", True, (("Catherine Full Body [0100BF00112C0000]+[v1.0.1](US-EU).xci", _PAD),)),
    DiskEntry("Fire Emblem Three Houses", True, (("Fire Emblem Three Houses NSW-HR.xci", _PAD),)),
    DiskEntry(
        "The Legend Of Zelda Echoes Of Wisdom NSW-NiiNTENDO.xci",
        False,
        (("The Legend Of Zelda Echoes Of Wisdom NSW-NiiNTENDO.xci", _PAD),),
    ),
    DiskEntry("Mario Kart 8 Deluxe", True, (("Mario Kart 8 Deluxe [0100152000022000][v0].xci", _PAD),)),
    DiskEntry("Pokemon Scarlet", True, (("Pokemon Scarlet NSW-NiiNTENDO.xci", 100),)),
    DiskEntry("Pokemon Shining Pearl [010018E011D92000][v0].nsp", False, (("x", 1),)),
    DiskEntry("Super Mario Bros Wonder NSW-LiGHTFORCE.xci", False, (("x", 1),)),
    DiskEntry("Xenoblade Chronicles 3", True, ()),
    DiskEntry("metadata.txt", False, (("metadata.txt", 10),)),
    DiskEntry("systeminfo.txt", False, (("systeminfo.txt", 10),)),
]


class TestNormalizeTitle:
    @pytest.mark.parametrize(("romm", "disk"), _REAL_PAIRS)
    def test_real_pairs_normalize_equal(self, romm, disk):
        assert normalize_title(romm) == normalize_title(disk)
        assert normalize_title(romm) != ""

    def test_exact_values(self):
        assert normalize_title("Catherine_ Full Body") == "catherine full body"
        assert normalize_title("Fire Emblem Three Houses NSW-HR.xci") == "fire emblem three houses"
        assert normalize_title("Mario Kart 8 Deluxe [0100152000022000][v0].xci") == "mario kart 8 deluxe"
        assert normalize_title("Pokemon Shining Pearl [010018E011D92000][v0].nsp") == "pokemon shining pearl"
        assert normalize_title("Donkey Kong Country Returns HD_ Version 1.1.0") == (
            "donkey kong country returns hd version 1 1 0"
        )

    def test_galaxy_and_galaxy_2_stay_distinct(self):
        assert normalize_title("Super Mario Galaxy") != normalize_title("Super Mario Galaxy 2")

    def test_non_ascii_accent_is_folded(self):
        assert normalize_title("Pokémon Legends_ Z-A") == "pokemon legends z a"
        assert normalize_title("Pokémon Legends_ Z-A") == normalize_title("Pokemon Legends Z-A NSW-GRP.nsp")

    def test_nfd_input_equals_nfc_input(self):
        assert normalize_title("Pokémon") == normalize_title("Pokémon") == "pokemon"

    @pytest.mark.parametrize("ext", [".xci", ".NSP", ".nsz", ".xcz"])
    def test_known_extensions_dropped(self, ext):
        assert normalize_title(f"Game{ext}") == "game"

    def test_unknown_extension_kept_as_word(self):
        assert normalize_title("metadata.txt") == "metadata txt"

    def test_punctuation_becomes_space(self):
        assert normalize_title("A_B:C-D+E'F.G,H&I!J") == "a b c d e f g h i j"

    def test_scene_suffix_only_at_end(self):
        assert normalize_title("NSW-Thing Game") == "nsw thing game"
        assert normalize_title("Game nsw-grp") == "game"

    def test_scene_suffix_after_tags(self):
        assert normalize_title("Game NSW-GRP [v0] (US).xci") == "game"

    @pytest.mark.parametrize("name", ["", "   ", "[0100].xci", "NSW-ABC.xci", "(US) [v0]", "___"])
    def test_degenerate_names_are_empty(self, name):
        assert normalize_title(name) == ""


class TestEntryLaunchFile:
    def test_file_with_switch_extension(self):
        assert entry_launch_file(DiskEntry("A.XCI", False, (("A.XCI", 1),)), _EXTS) == "A.XCI"

    def test_file_with_other_extension(self):
        assert entry_launch_file(DiskEntry("metadata.txt", False), _EXTS) is None

    def test_dir_picks_largest_launch_file(self):
        entry = DiskEntry("G", True, (("update.nsp", 5), ("base.xci", 50), ("readme.txt", 999)))
        assert entry_launch_file(entry, _EXTS) == "base.xci"

    def test_dir_ties_break_by_lowercase_name(self):
        entry = DiskEntry("G", True, (("b.xci", 5), ("A.nsp", 5)))
        assert entry_launch_file(entry, _EXTS) == "A.nsp"

    def test_dir_without_launch_file(self):
        assert entry_launch_file(DiskEntry("Xenoblade Chronicles 3", True, (("notes.txt", 1),)), _EXTS) is None


class TestPairByTitle:
    def test_real_device_listing(self):
        matches = pair_by_title(_ROMM, _DISK, _EXTS)
        assert matches == [
            TitleMatch(1, "Catherine Full Body", True, "Catherine Full Body [0100BF00112C0000]+[v1.0.1](US-EU).xci"),
            TitleMatch(2, "Fire Emblem Three Houses", True, "Fire Emblem Three Houses NSW-HR.xci"),
            TitleMatch(
                3,
                "The Legend Of Zelda Echoes Of Wisdom NSW-NiiNTENDO.xci",
                False,
                "The Legend Of Zelda Echoes Of Wisdom NSW-NiiNTENDO.xci",
            ),
            TitleMatch(4, "Mario Kart 8 Deluxe", True, "Mario Kart 8 Deluxe [0100152000022000][v0].xci"),
        ]

    def test_two_disk_entries_same_title_adopt_none(self):
        disk = [
            DiskEntry("Mario Kart 8 Deluxe", True, (("MK8.xci", 9),)),
            DiskEntry("Mario Kart 8 Deluxe NSW-GRP.xci", False),
        ]
        assert pair_by_title([(4, "Mario Kart 8 Deluxe")], disk, _EXTS) == []

    def test_two_roms_same_title_adopt_none(self):
        roms = [(4, "Mario Kart 8 Deluxe"), (9, "Mario Kart 8_ Deluxe")]
        disk = [DiskEntry("Mario Kart 8 Deluxe", True, (("MK8.xci", 9),))]
        assert pair_by_title(roms, disk, _EXTS) == []

    def test_duplicate_rom_id_is_not_ambiguous(self):
        roms = [(4, "Mario Kart 8 Deluxe"), (4, "Mario Kart 8 Deluxe")]
        disk = [DiskEntry("Mario Kart 8 Deluxe", True, (("MK8.xci", 9),))]
        assert [m.rom_id for m in pair_by_title(roms, disk, _EXTS)] == [4]

    def test_non_qualifying_entry_does_not_count_for_ambiguity(self):
        disk = [
            DiskEntry("Mario Kart 8 Deluxe", True, (("MK8.xci", 9),)),
            DiskEntry("Mario Kart 8 Deluxe", True, (("notes.txt", 1),)),
        ]
        assert [m.rom_id for m in pair_by_title([(4, "Mario Kart 8 Deluxe")], disk, _EXTS)] == [4]

    def test_claimed_entry_is_never_adopted(self):
        disk = [DiskEntry("Mario Kart 8 Deluxe", True, (("MK8.xci", 9),), claimed=True)]
        assert pair_by_title([(4, "Mario Kart 8 Deluxe")], disk, _EXTS) == []

    def test_claimed_entry_still_makes_title_ambiguous(self):
        disk = [
            DiskEntry("Mario Kart 8 Deluxe", True, (("MK8.xci", 9),), claimed=True),
            DiskEntry("Mario Kart 8 Deluxe.xci", False),
        ]
        assert pair_by_title([(4, "Mario Kart 8 Deluxe")], disk, _EXTS) == []

    def test_empty_titles_never_pair(self):
        roms = [(1, "[0100]"), (2, "NSW-ABC")]
        disk = [DiskEntry("[0200].xci", False), DiskEntry("NSW-XYZ.nsp", False)]
        assert pair_by_title(roms, disk, _EXTS) == []

    def test_galaxy_does_not_take_galaxy_2(self):
        roms = [(5, "Super Mario Galaxy"), (6, "Super Mario Galaxy 2")]
        disk = [DiskEntry("Super Mario Galaxy 2 NSW-GRP.xci", False)]
        assert pair_by_title(roms, disk, _EXTS) == [
            TitleMatch(6, "Super Mario Galaxy 2 NSW-GRP.xci", False, "Super Mario Galaxy 2 NSW-GRP.xci")
        ]

    def test_empty_inputs(self):
        assert pair_by_title([], _DISK, _EXTS) == []
        assert pair_by_title(_ROMM, [], _EXTS) == []
