"""Tests for domain.install_layout — download target names + adoption match decision."""

from __future__ import annotations

from domain.install_layout import (
    AdoptionCandidate,
    MultiFileLayout,
    SingleFileLayout,
    expected_install_layout,
    multi_file_matches,
    pick_one_per_group,
    resolve_download_names,
    single_file_matches,
)
from domain.rom_install import RomInstall


def _single(**over):
    rom = {
        "id": 7,
        "fs_name": "Pokemon (USA).gba",
        "fs_size_bytes": 1024,
        "files": [{"file_name": "Pokemon (USA).gba"}],
    }
    rom.update(over)
    return rom


def _multi(files, **over):
    rom = {
        "id": 9,
        "name": "Final Fantasy VII",
        "fs_path": "psx",
        "fs_name": "Final Fantasy VII (USA)",
        "fs_name_no_ext": "Final Fantasy VII (USA)",
        "has_multiple_files": True,
        "files": files,
    }
    rom.update(over)
    return rom


def _f(rel, size, root="psx/Final Fantasy VII (USA)"):
    path, _, name = f"{root}/{rel}".rpartition("/")
    return {"file_path": path, "file_name": name, "file_size_bytes": size}


class TestResolveDownloadNames:
    def test_plain_single_file(self):
        names = resolve_download_names(_single())
        assert names.file_name == "Pokemon (USA).gba"
        assert not names.file_name_changed
        assert names.is_multi is False

    def test_nested_single_uses_inner_file_name(self):
        names = resolve_download_names(
            {"id": 1, "fs_name": "Zelda (USA)", "has_nested_single_file": True, "files": [{"file_name": "Zelda.nsp"}]}
        )
        assert names.file_name == "Zelda.nsp"

    def test_traversal_in_fs_name_is_coerced_to_basename(self):
        names = resolve_download_names(_single(fs_name="../../etc/passwd"))
        assert names.file_name == "passwd"
        assert names.file_name_changed

    def test_degenerate_fs_name_falls_back_to_synthetic(self):
        names = resolve_download_names(_single(fs_name=".."))
        assert names.file_name == "rom_7"

    def test_multi_extract_dir_name(self):
        names = resolve_download_names(_multi([_f("a.cue", 1), _f("a.bin", 2)]))
        assert names.is_multi
        assert names.extract_dir_name == "Final Fantasy VII (USA)"
        assert not names.extract_dir_name_changed


class TestSingleFileLayout:
    def test_layout_uses_fs_size(self):
        layout = expected_install_layout(_single())
        assert layout == SingleFileLayout(file_name="Pokemon (USA).gba", size=1024)

    def test_match_on_exact_size(self):
        layout = expected_install_layout(_single())
        assert isinstance(layout, SingleFileLayout)
        assert single_file_matches(layout, 1024)

    def test_size_mismatch_does_not_match(self):
        layout = expected_install_layout(_single())
        assert isinstance(layout, SingleFileLayout)
        assert not single_file_matches(layout, 1023)

    def test_missing_file_does_not_match(self):
        layout = expected_install_layout(_single())
        assert isinstance(layout, SingleFileLayout)
        assert not single_file_matches(layout, None)

    def test_missing_romm_size_is_not_adoptable(self):
        assert expected_install_layout(_single(fs_size_bytes=None)) is None
        rom = _single()
        del rom["fs_size_bytes"]
        assert expected_install_layout(rom) is None

    def test_zero_or_bogus_size_is_not_adoptable(self):
        assert expected_install_layout(_single(fs_size_bytes=0)) is None
        assert expected_install_layout(_single(fs_size_bytes="1024")) is None
        assert expected_install_layout(_single(fs_size_bytes=True)) is None

    def test_traversal_name_maps_to_the_download_target(self):
        layout = expected_install_layout(_single(fs_name="../evil.gba"))
        assert layout == SingleFileLayout(file_name="evil.gba", size=1024)


class TestMultiFileLayout:
    def test_all_present_matches(self):
        layout = expected_install_layout(_multi([_f("FF7.cue", 10), _f("FF7.bin", 20)]))
        assert isinstance(layout, MultiFileLayout)
        assert layout.dir_candidates[0] == "Final Fantasy VII (USA)"
        assert multi_file_matches(layout, {"FF7.cue": 10, "FF7.bin": 20})

    def test_one_missing_does_not_match(self):
        layout = expected_install_layout(_multi([_f("FF7.cue", 10), _f("FF7.bin", 20)]))
        assert isinstance(layout, MultiFileLayout)
        assert not multi_file_matches(layout, {"FF7.cue": 10, "FF7.bin": None})
        assert not multi_file_matches(layout, {"FF7.cue": 10})

    def test_one_wrong_size_does_not_match(self):
        layout = expected_install_layout(_multi([_f("FF7.cue", 10), _f("FF7.bin", 20)]))
        assert isinstance(layout, MultiFileLayout)
        assert not multi_file_matches(layout, {"FF7.cue": 10, "FF7.bin": 21})

    def test_nested_paths_are_relative_to_rom_folder(self):
        layout = expected_install_layout(_multi([_f("base.nsp", 5), _f("update/patch.nsp", 6)]))
        assert isinstance(layout, MultiFileLayout)
        assert {f.rel_path for f in layout.files} == {"base.nsp", "update/patch.nsp"}

    def test_names_are_url_decoded_like_the_extraction(self):
        layout = expected_install_layout(_multi([_f("Disc%201.cue", 1), _f("Disc 1.bin", 2)]))
        assert isinstance(layout, MultiFileLayout)
        assert {f.rel_path for f in layout.files} == {"Disc 1.cue", "Disc 1.bin"}

    def test_collapse_dir_candidates_include_top_level_files_and_playlist(self):
        layout = expected_install_layout(_multi([_f("FF7.cue", 10), _f("sub/x.bin", 20)]))
        assert isinstance(layout, MultiFileLayout)
        assert "FF7.cue" in layout.dir_candidates
        assert "Final Fantasy VII (USA).m3u" in layout.dir_candidates
        assert "x.bin" not in layout.dir_candidates

    def test_traversal_in_file_name_is_rejected(self):
        evil = {"file_path": "psx/Final Fantasy VII (USA)", "file_name": "../../evil", "file_size_bytes": 1}
        files = [_f("ok.cue", 1), evil]
        assert expected_install_layout(_multi(files)) is None

    def test_encoded_traversal_is_rejected(self):
        assert expected_install_layout(_multi([_f("ok.cue", 1), _f("%2e%2e", 1)])) is None

    def test_file_outside_rom_folder_is_undeterminable(self):
        files = [_f("ok.cue", 1), {"file_path": "psx/Other Game", "file_name": "x.bin", "file_size_bytes": 1}]
        assert expected_install_layout(_multi(files)) is None

    def test_missing_file_size_is_undeterminable(self):
        files = [_f("ok.cue", 1), {"file_path": "psx/Final Fantasy VII (USA)", "file_name": "x.bin"}]
        assert expected_install_layout(_multi(files)) is None

    def test_unknown_rom_folder_is_undeterminable(self):
        rom = _multi([_f("a.cue", 1), _f("a.bin", 2)])
        del rom["fs_path"]
        assert expected_install_layout(rom) is None

    def test_no_files_listed_is_undeterminable(self):
        assert expected_install_layout(_multi([], has_multiple_files=True)) is None


def _cand(rom_id, group):
    install = RomInstall.mark_installed(
        rom_id=rom_id, file_path=f"/r/{rom_id}", rom_dir=None, platform_slug="gba", system="gba", installed_at="t"
    )
    return AdoptionCandidate(rom_id=rom_id, install=install, bake_path=f"/r/{rom_id}", group_key=group)


class TestPickOnePerGroup:
    def test_lowest_rom_id_wins_within_a_group(self):
        picked = pick_one_per_group([_cand(5, "g"), _cand(3, "g"), _cand(9, "h")])
        assert [c.rom_id for c in picked] == [3, 9]

    def test_solo_roms_are_all_kept(self):
        picked = pick_one_per_group([_cand(2, None), _cand(1, None)])
        assert [c.rom_id for c in picked] == [1, 2]
