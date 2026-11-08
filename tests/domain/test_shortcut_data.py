"""Tests for domain/shortcut_data.py pure functions."""

import os
import shlex

from domain.shortcut_data import (
    build_launch_options,
    build_shortcuts_data,
)

# Shared test invocation string — replaces the removed RETRODECK_INVOCATION.
# The actual value is arbitrary; the tests verify composition, not host detail.
_TEST_INVOCATION = "flatpak run net.retrodeck.retrodeck"


class TestBuildLaunchOptions:
    """Tests for build_launch_options()."""

    def test_quotes_path(self):
        assert build_launch_options(_TEST_INVOCATION, "/roms/n64/zelda.z64") == (
            'flatpak run net.retrodeck.retrodeck "/roms/n64/zelda.z64"'
        )

    def test_quotes_path_with_spaces(self):
        result = build_launch_options(_TEST_INVOCATION, "/roms/dc/My Game.chd")
        assert result == 'flatpak run net.retrodeck.retrodeck "/roms/dc/My Game.chd"'

    def test_empty_path_still_quoted(self):
        assert build_launch_options(_TEST_INVOCATION, "") == 'flatpak run net.retrodeck.retrodeck ""'

    # The tests below use shlex.split(posix=True) as a reference POSIX /
    # ``\"``-honoring tokenizer: it proves the escaping is internally correct (a
    # server-controlled ROM filename round-trips to exactly ONE final argv token).
    # Parity with Steam's actual (closed-source) launch-time tokenizer is verified
    # on-device, not here.

    def test_quote_in_path_round_trips_to_one_arg(self):
        path = '/roms/gba/Game".gba'
        result = build_launch_options(_TEST_INVOCATION, path)
        assert shlex.split(result, posix=True) == [
            "flatpak",
            "run",
            "net.retrodeck.retrodeck",
            path,
        ]

    def test_argv_injection_attempt_is_neutralized(self):
        path = '/roms/gba/evil" --inject-flag --foo.gba'
        result = build_launch_options(_TEST_INVOCATION, path)
        tokens = shlex.split(result, posix=True)
        assert tokens == ["flatpak", "run", "net.retrodeck.retrodeck", path]

    def test_backslash_in_path_round_trips(self):
        path = "/roms/gba/a\\b.gba"
        result = build_launch_options(_TEST_INVOCATION, path)
        tokens = shlex.split(result, posix=True)
        assert tokens[-1] == path
        assert tokens == ["flatpak", "run", "net.retrodeck.retrodeck", path]

    def test_trailing_backslash_does_not_eat_closing_quote(self):
        path = "/roms/gba/dir\\"
        result = build_launch_options(_TEST_INVOCATION, path)
        tokens = shlex.split(result, posix=True)
        assert tokens[-1] == path
        assert tokens == ["flatpak", "run", "net.retrodeck.retrodeck", path]

    def test_combined_backslash_and_quote_round_trips(self):
        path = '/roms/gba/a\\"b.gba'
        result = build_launch_options(_TEST_INVOCATION, path)
        tokens = shlex.split(result, posix=True)
        assert tokens[-1] == path
        assert tokens == ["flatpak", "run", "net.retrodeck.retrodeck", path]

    def test_path_is_just_a_quote(self):
        path = '"'
        result = build_launch_options(_TEST_INVOCATION, path)
        assert shlex.split(result, posix=True) == [
            "flatpak",
            "run",
            "net.retrodeck.retrodeck",
            path,
        ]

    def test_override_invocation_is_preserved_unescaped(self):
        invocation = (
            'flatpak run net.retrodeck.retrodeck -e "%EMULATOR_RETROARCH% -L /var/config/retroarch/cores/mgba.so %ROM%"'
        )
        path = '/roms/gba/Game".gba'
        result = build_launch_options(invocation, path)
        assert '-e "%EMULATOR_RETROARCH% -L /var/config/retroarch/cores/mgba.so %ROM%"' in result
        assert shlex.split(result, posix=True)[-1] == path


class TestBuildShortcutsData:
    """Tests for build_shortcuts_data()."""

    def test_builds_correct_format(self):
        plugin_dir = "/home/deck/homebrew/plugins/decky-romm-sync"
        roms = [
            {
                "id": 1,
                "name": "Game A",
                "fs_name": "gamea.z64",
                "platform_name": "N64",
                "platform_slug": "n64",
                "igdb_id": 100,
                "sgdb_id": 200,
                "ra_id": 300,
            },
            {"id": 2, "name": "Game B", "platform_name": "SNES", "platform_slug": "snes"},
        ]
        result = build_shortcuts_data(roms, plugin_dir, {1: "/roms/n64/gamea.z64"}, {})
        assert len(result) == 2
        assert result[0]["rom_id"] == 1
        assert result[0]["name"] == "Game A"
        assert result[0]["fs_name"] == "gamea.z64"
        assert result[0]["platform_name"] == "N64"
        assert result[0]["platform_slug"] == "n64"
        assert result[0]["igdb_id"] == 100
        assert result[0]["sgdb_id"] == 200
        assert result[0]["ra_id"] == 300
        assert result[0]["cover_path"] == ""
        assert result[0]["exe"] == os.path.join(plugin_dir, "bin", "rom-launcher")
        assert result[0]["start_dir"] == os.path.join(plugin_dir, "bin")
        assert result[1]["fs_name"] == ""

    def test_installed_rom_with_invocation_gets_launch_command(self):
        roms = [{"id": 1, "name": "Game A"}]
        result = build_shortcuts_data(roms, "/plugin", {1: "/roms/n64/gamea.z64"}, {1: _TEST_INVOCATION})
        assert result[0]["launch_options"] == 'flatpak run net.retrodeck.retrodeck "/roms/n64/gamea.z64"'

    def test_installed_rom_without_invocation_gets_empty_launch_options(self):
        roms = [{"id": 1, "name": "Game A"}]
        result = build_shortcuts_data(roms, "/plugin", {1: "/roms/n64/gamea.z64"}, {})
        # Installed but no invocation resolved (needs_setup / launcher_missing) —
        # the path is still baked (build_launch_options("", path)) so the shortcut
        # exists; the empty invocation prefix means rom-launcher exec's the path
        # directly (not useful without an emulator, but the shortcut survives).
        assert result[0]["launch_options"] == ' "/roms/n64/gamea.z64"'

    def test_installed_rom_path_with_spaces_is_quoted(self):
        roms = [{"id": 7, "name": "Spacey"}]
        result = build_shortcuts_data(roms, "/plugin", {7: "/roms/dc/My Game.chd"}, {7: _TEST_INVOCATION})
        assert result[0]["launch_options"] == 'flatpak run net.retrodeck.retrodeck "/roms/dc/My Game.chd"'

    def test_uninstalled_rom_gets_empty_launch_options(self):
        roms = [{"id": 2, "name": "Game B"}]
        result = build_shortcuts_data(roms, "/plugin", {}, {})
        assert result[0]["launch_options"] == ""

    def test_mixed_installed_and_uninstalled(self):
        roms = [
            {"id": 1, "name": "Installed"},
            {"id": 2, "name": "NotInstalled"},
        ]
        result = build_shortcuts_data(roms, "/plugin", {1: "/roms/snes/installed.sfc"}, {1: _TEST_INVOCATION})
        assert result[0]["launch_options"] == 'flatpak run net.retrodeck.retrodeck "/roms/snes/installed.sfc"'
        assert result[1]["launch_options"] == ""

    def test_installed_rom_with_core_override_bakes_pre_resolved_invocation(self):
        roms = [{"id": 1, "name": "PSX Game"}]
        override = (
            "flatpak run net.retrodeck.retrodeck "
            '-e "%EMULATOR_RETROARCH% -L /var/config/retroarch/cores/pcsx_rearmed_libretro.so %ROM%"'
        )
        result = build_shortcuts_data(roms, "/plugin", {1: "/roms/psx/game.chd"}, {1: override})
        assert result[0]["launch_options"] == (
            "flatpak run net.retrodeck.retrodeck "
            '-e "%EMULATOR_RETROARCH% -L /var/config/retroarch/cores/pcsx_rearmed_libretro.so %ROM%" '
            '"/roms/psx/game.chd"'
        )

    def test_installed_rom_with_standalone_override_bakes_pre_resolved_invocation(self):
        roms = [{"id": 1, "name": "PS3 Game"}]
        override = 'flatpak run net.retrodeck.retrodeck -e "%EMULATOR_RPCS3% --no-gui %ROM%"'
        result = build_shortcuts_data(
            roms,
            "/plugin",
            {1: "/roms/ps3/game/PS3_GAME/USRDIR/EBOOT.BIN"},
            {1: override},
        )
        assert result[0]["launch_options"] == (
            'flatpak run net.retrodeck.retrodeck -e "%EMULATOR_RPCS3% --no-gui %ROM%" '
            '"/roms/ps3/game/PS3_GAME/USRDIR/EBOOT.BIN"'
        )

    def test_installed_rom_absent_from_invocations_is_plain(self):
        roms = [{"id": 1, "name": "Plain"}]
        result = build_shortcuts_data(roms, "/plugin", {1: "/roms/n64/g.z64"}, {2: _TEST_INVOCATION})
        # ROM 1 is installed but absent from invocations (only ROM 2 has one) —
        # same as test_installed_rom_without_invocation: the path is baked with
        # an empty invocation prefix. No "-e" override (no invocation at all).
        assert result[0]["launch_options"] == ' "/roms/n64/g.z64"'
        assert "-e" not in result[0]["launch_options"]

    def test_uninstalled_rom_with_override_still_empty(self):
        roms = [{"id": 1, "name": "NotDownloaded"}]
        result = build_shortcuts_data(roms, "/plugin", {}, {1: "some-invocation"})
        assert result[0]["launch_options"] == ""

    def test_empty_roms(self):
        result = build_shortcuts_data([], "/some/dir", {}, {})
        assert result == []

    def test_missing_optional_fields(self):
        roms = [{"id": 5, "name": "Minimal"}]
        result = build_shortcuts_data(roms, "/plugin", {}, {})
        assert result[0]["rom_id"] == 5
        assert result[0]["platform_name"] == "Unknown"
        assert result[0]["platform_slug"] == ""
        assert result[0]["igdb_id"] is None
        assert result[0]["sgdb_id"] is None

    def test_exe_path_contains_rom_launcher(self):
        plugin_dir = "/home/deck/homebrew/plugins/decky-romm-sync"
        roms = [{"id": 1, "name": "Game"}]
        result = build_shortcuts_data(roms, plugin_dir, {}, {})
        assert result[0]["exe"].endswith("/bin/rom-launcher")

    def test_start_dir_is_parent_of_exe(self):
        plugin_dir = "/home/deck/homebrew/plugins/decky-romm-sync"
        roms = [{"id": 1, "name": "Game"}]
        result = build_shortcuts_data(roms, plugin_dir, {}, {})
        assert result[0]["start_dir"] == os.path.dirname(result[0]["exe"])

    def test_multiple_roms_each_has_required_fields(self):
        required_fields = {"rom_id", "name", "exe", "start_dir", "launch_options", "platform_name", "platform_slug"}
        roms = [{"id": i, "name": f"Game {i}"} for i in range(5)]
        result = build_shortcuts_data(roms, "/plugin", {}, {})
        for item in result:
            for field in required_fields:
                assert field in item, f"Missing field '{field}' in shortcut data"


class TestBuildShortcutsDataVersionMetadata:
    """build_shortcuts_data derives the sibling-group key + version dimensions (#1295)."""

    def test_carries_group_key_and_version_dimensions(self):
        roms = [
            {
                "id": 1,
                "name": "Game A",
                "platform_id": 57,
                "igdb_id": 3404,
                "regions": ["USA", "Europe"],
                "languages": ["En", "Fr"],
                "revision": "1",
                "tags": ["Demo"],
                "rom_user": {"is_main_sibling": True},
            }
        ]
        result = build_shortcuts_data(roms, "/plugin", {}, {})
        assert result[0]["sibling_group_key"] == "igdb:3404:57"
        assert result[0]["regions"] == ["USA", "Europe"]
        assert result[0]["languages"] == ["En", "Fr"]
        assert result[0]["revision"] == "1"
        assert result[0]["tags"] == ["Demo"]
        assert result[0]["is_main_sibling"] is True

    def test_unmatched_rom_gets_fallback_group_key(self):
        roms = [{"id": 4409, "name": "Solo", "platform_id": 57}]
        result = build_shortcuts_data(roms, "/plugin", {}, {})
        assert result[0]["sibling_group_key"] == "romm:4409:57"

    def test_missing_version_fields_default_empty(self):
        roms = [{"id": 5, "name": "Minimal", "platform_id": 9}]
        result = build_shortcuts_data(roms, "/plugin", {}, {})
        assert result[0]["regions"] == []
        assert result[0]["languages"] == []
        assert result[0]["revision"] == ""
        assert result[0]["tags"] == []
        assert result[0]["is_main_sibling"] is False

    def test_null_rom_user_is_not_main_sibling(self):
        roms = [{"id": 6, "name": "Untouched", "platform_id": 9, "rom_user": None}]
        result = build_shortcuts_data(roms, "/plugin", {}, {})
        assert result[0]["is_main_sibling"] is False

    def test_null_version_arrays_default_empty(self):
        roms = [
            {
                "id": 7,
                "name": "Nulls",
                "platform_id": 9,
                "regions": None,
                "languages": None,
                "tags": None,
                "revision": None,
            }
        ]
        result = build_shortcuts_data(roms, "/plugin", {}, {})
        assert result[0]["regions"] == []
        assert result[0]["languages"] == []
        assert result[0]["tags"] == []
        assert result[0]["revision"] == ""

    def test_persisted_group_key_is_authoritative_over_recompute(self):
        reconstructed = {"id": 10, "name": "Zelda (USA)", "igdb_id": 100, "sibling_group_key": "igdb:100:57"}
        result = build_shortcuts_data([reconstructed], "/plugin", {}, {})
        assert result[0]["sibling_group_key"] == "igdb:100:57"

    def test_reconstructed_and_fetched_sibling_land_in_one_bucket(self):
        from domain.sync_diff import collapse_sibling_groups

        reconstructed = {"id": 10, "name": "Zelda (USA)", "sibling_group_key": "igdb:100:57"}
        fetched = {"id": 11, "name": "Zelda (JP)", "igdb_id": 100, "platform_id": 57}
        result = build_shortcuts_data([reconstructed, fetched], "/plugin", {}, {})
        assert {sd["sibling_group_key"] for sd in result} == {"igdb:100:57"}

        emitted = collapse_sibling_groups(result, registry={}, installed_rom_ids=set(), complete_group_view=True)
        assert len(emitted) == 1
