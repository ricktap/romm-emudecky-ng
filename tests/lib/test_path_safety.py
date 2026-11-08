"""Tests for lib.path_safety — path containment predicates with realpath resolution."""

import os

import pytest

from lib.path_safety import (
    PathTraversalError,
    coerce_safe_component,
    is_safe_rom_path,
    safe_join,
    safe_join_lexical,
    safe_path_component,
)


class TestSafePathComponent:
    def test_accepts_clean_name(self):
        assert safe_path_component("game.z64") == "game.z64"

    def test_accepts_name_with_spaces_and_parens(self):
        # Decoded ZIP basenames commonly carry these — must stay valid.
        assert safe_path_component("Final Fantasy VII (USA).cue") == "Final Fantasy VII (USA).cue"

    def test_rejects_parent_dir(self):
        with pytest.raises(PathTraversalError):
            safe_path_component("..")

    def test_rejects_current_dir(self):
        with pytest.raises(PathTraversalError):
            safe_path_component(".")

    def test_rejects_empty(self):
        with pytest.raises(PathTraversalError):
            safe_path_component("")

    def test_rejects_nul_byte(self):
        with pytest.raises(PathTraversalError):
            safe_path_component("evil\x00.sh")

    def test_rejects_absolute_path(self):
        with pytest.raises(PathTraversalError):
            safe_path_component("/etc/passwd")

    def test_rejects_separator(self):
        # A single component must stay a single component.
        with pytest.raises(PathTraversalError):
            safe_path_component("a/b")

    def test_rejects_decoded_traversal(self):
        # ``%2e%2e%2fevil.sh`` decodes to this — the #968 attack vector.
        with pytest.raises(PathTraversalError):
            safe_path_component("../evil.sh")

    def test_rejects_embedded_traversal_segment(self):
        with pytest.raises(PathTraversalError):
            safe_path_component("sub/../../evil")


class TestCoerceSafeComponent:
    _FALLBACK = "rom_42"

    def test_legit_name_is_byte_identical_and_unchanged(self):
        assert coerce_safe_component("Metal Gear Solid 4", self._FALLBACK) == ("Metal Gear Solid 4", False)

    def test_legit_name_with_spaces_preserved_byte_identical(self):
        # Leading/trailing spaces in a legit name are NOT stripped (only tested
        # for degeneracy) — the component stays byte-identical.
        assert coerce_safe_component("  My Game  ", self._FALLBACK) == ("  My Game  ", False)

    def test_directory_portion_stripped_to_basename(self):
        assert coerce_safe_component("../../etc/pwned", self._FALLBACK) == ("pwned", True)

    def test_absolute_path_stripped_to_basename(self):
        assert coerce_safe_component("/etc/passwd", self._FALLBACK) == ("passwd", True)

    def test_dotdot_falls_back(self):
        # os.path.basename("..") == ".." — the load-bearing degenerate case.
        assert coerce_safe_component("..", self._FALLBACK) == (self._FALLBACK, True)

    def test_dot_falls_back(self):
        assert coerce_safe_component(".", self._FALLBACK) == (self._FALLBACK, True)

    def test_empty_falls_back(self):
        assert coerce_safe_component("", self._FALLBACK) == (self._FALLBACK, True)

    def test_trailing_slash_basenames_to_empty_falls_back(self):
        # os.path.basename("foo/") == "" — a directory-shaped name.
        assert coerce_safe_component("foo/", self._FALLBACK) == (self._FALLBACK, True)

    def test_whitespace_only_falls_back(self):
        assert coerce_safe_component("   ", self._FALLBACK) == (self._FALLBACK, True)

    def test_dotdot_wrapped_in_dirs_falls_back(self):
        # os.path.basename("foo/..") == ".." → still degenerate.
        assert coerce_safe_component("foo/..", self._FALLBACK) == (self._FALLBACK, True)

    def test_leading_dots_name_is_not_degenerate(self):
        # "..foo" is a real (if unusual) child name, not a traversal.
        assert coerce_safe_component("..foo", self._FALLBACK) == ("..foo", False)


class TestSafeJoin:
    def test_accepts_legit_single_component(self, tmp_path):
        base = str(tmp_path / "bios")
        os.makedirs(base)
        result = safe_join(base, "scph5501.bin")
        assert result == os.path.realpath(os.path.join(base, "scph5501.bin"))

    def test_accepts_legit_multi_component(self, tmp_path):
        # The registry ``dc/dc_boot.bin`` shape must pass.
        base = str(tmp_path / "bios")
        os.makedirs(base)
        result = safe_join(base, "dc/dc_boot.bin")
        assert result == os.path.realpath(os.path.join(base, "dc", "dc_boot.bin"))

    def test_rejects_parent_traversal(self, tmp_path):
        base = str(tmp_path / "bios")
        os.makedirs(base)
        with pytest.raises(PathTraversalError):
            safe_join(base, "../evil.desktop")

    def test_rejects_deep_traversal(self, tmp_path):
        base = str(tmp_path / "retrodeck" / "roms")
        os.makedirs(base)
        with pytest.raises(PathTraversalError):
            safe_join(base, "../../etc/passwd")

    def test_rejects_absolute_second_arg(self, tmp_path):
        # os.path.join resets to an absolute part — must be caught.
        base = str(tmp_path / "bios")
        os.makedirs(base)
        with pytest.raises(PathTraversalError):
            safe_join(base, "/home/deck/.config/autostart/evil.desktop")

    def test_rejects_equality_with_base(self, tmp_path):
        # Strictly-below semantics: resolving exactly to base is rejected.
        base = str(tmp_path / "bios")
        os.makedirs(base)
        with pytest.raises(PathTraversalError):
            safe_join(base, ".")

    def test_rejects_symlink_escape(self, tmp_path):
        # A symlink planted UNDER base pointing OUTSIDE must not be a usable
        # escape — realpath resolves it, lexical normalization would not.
        base = tmp_path / "bios"
        base.mkdir()
        outside = tmp_path / "outside"
        outside.mkdir()
        (outside / "secret.bin").write_bytes(b"x")
        # base/link -> ../outside
        (base / "link").symlink_to(outside)
        with pytest.raises(PathTraversalError):
            safe_join(str(base), "link/secret.bin")


class TestSafeJoinLexical:
    """Same guard as safe_join, but the result keeps the caller's base spelling."""

    def test_keeps_symlinked_base_unresolved(self, tmp_path):
        # SteamOS: /run/media/mmcblk0p1 -> /run/media/deck/<uuid>.
        real = tmp_path / "deck" / "uuid" / "roms"
        real.mkdir(parents=True)
        link = tmp_path / "mmcblk0p1"
        link.symlink_to(tmp_path / "deck" / "uuid")
        base = str(link / "roms")
        assert safe_join_lexical(base, "switch") == os.path.join(base, "switch")
        assert safe_join(base, "switch") == os.path.realpath(os.path.join(base, "switch"))

    def test_normalises_trailing_slash(self, tmp_path):
        base = str(tmp_path / "roms")
        os.makedirs(base)
        assert safe_join_lexical(base + "/", "gba") == os.path.join(base, "gba")

    def test_rejects_parent_traversal(self, tmp_path):
        base = str(tmp_path / "roms")
        os.makedirs(base)
        with pytest.raises(PathTraversalError):
            safe_join_lexical(base, "../etc")

    def test_rejects_equality_with_base(self, tmp_path):
        base = str(tmp_path / "roms")
        os.makedirs(base)
        with pytest.raises(PathTraversalError):
            safe_join_lexical(base, ".")

    def test_rejects_symlink_escaping_base(self, tmp_path):
        base = tmp_path / "roms"
        base.mkdir()
        (base / "evil").symlink_to(tmp_path)
        with pytest.raises(PathTraversalError):
            safe_join_lexical(str(base), "evil", "..", "..", "x")

    def test_rejects_lexical_escape_that_resolves_inside(self, tmp_path):
        # A ".." that stays under base lexically is fine; one that climbs above
        # base lexically is rejected even when the symlink-resolved path
        # (roms/deep -> roms/a/b, so "deep/../../a/x" -> roms/a/x) is inside.
        base = tmp_path / "roms"
        (base / "a" / "b").mkdir(parents=True)
        (base / "inner").symlink_to(base / "a" / "b")
        assert safe_join_lexical(str(base), "inner/../x") == os.path.join(str(base), "x")
        (base / "deep").symlink_to(base / "a" / "b")
        with pytest.raises(PathTraversalError):
            safe_join_lexical(str(base), "deep/../../a/x")


class TestIsSafeRomPath:
    def test_path_inside_roms_dir_is_safe(self, tmp_path):
        roms_base = str(tmp_path / "retrodeck" / "roms")
        safe = str(tmp_path / "retrodeck" / "roms" / "n64" / "game.z64")
        assert is_safe_rom_path(safe, roms_base) is True

    def test_path_outside_roms_dir_is_not_safe(self, tmp_path):
        roms_base = str(tmp_path / "retrodeck" / "roms")
        outside = str(tmp_path / "evil" / "game.z64")
        assert is_safe_rom_path(outside, roms_base) is False

    def test_roms_base_itself_is_not_safe(self, tmp_path):
        roms_base = str(tmp_path / "retrodeck" / "roms")
        # Only 1 level deep — must be at least 2
        base = str(tmp_path / "retrodeck" / "roms" / "n64")
        assert is_safe_rom_path(base, roms_base) is False

    def test_etc_passwd_is_not_safe(self, tmp_path):
        roms_base = str(tmp_path / "retrodeck" / "roms")
        assert is_safe_rom_path("/etc/passwd", roms_base) is False

    def test_deeper_than_two_levels_is_safe(self, tmp_path):
        roms_base = str(tmp_path / "retrodeck" / "roms")
        deep = str(tmp_path / "retrodeck" / "roms" / "gb" / "sub" / "file.zip")
        assert is_safe_rom_path(deep, roms_base) is True

    def test_exactly_two_levels_is_safe(self, tmp_path):
        roms_base = str(tmp_path / "retrodeck" / "roms")
        two_levels = str(tmp_path / "retrodeck" / "roms" / "gb" / "file.zip")
        assert is_safe_rom_path(two_levels, roms_base) is True

    def test_one_level_deep_is_not_safe(self, tmp_path):
        roms_base = str(tmp_path / "retrodeck" / "roms")
        one_level = str(tmp_path / "retrodeck" / "roms" / "file.zip")
        assert is_safe_rom_path(one_level, roms_base) is False

    def test_empty_roms_base_returns_quirky_cwd_match(self):
        # Preserved quirk: empty roms_base resolves to cwd via os.path.realpath("").
        cwd = os.path.realpath(os.getcwd())
        path_two_levels_in_cwd = os.path.join(cwd, "a", "b")
        assert is_safe_rom_path(path_two_levels_in_cwd, "") is True
