"""Pure emulator-table resolution: table data, options, default/override, rendering."""

from __future__ import annotations

import shlex

import pytest

from domain.emulator_commands import label_to_invocation, option_to_invocation, select_default_option
from domain.emulator_table import (
    EMULATOR_TABLE,
    ROM_PLACEHOLDER,
    TableEmulator,
    covers,
    find_emulator,
    launch_file_extensions,
    render_invocation,
    table_for,
    table_options,
)
from domain.shortcut_data import EmulatorInvocation, build_launch_options

_EDEN = "/home/deck/Applications/Eden.AppImage"
_RYU = "/home/deck/Applications/publish/Ryujinx.sh"
_YUZU = "/home/deck/Applications/yuzu.AppImage"


class TestTableData:
    def test_switch_order_is_eden_ryujinx_yuzu(self):
        assert [e.emulator_id for e in table_for("switch")] == ["eden", "ryujinx", "yuzu"]

    def test_every_template_ends_with_the_rom(self):
        for entries in EMULATOR_TABLE.values():
            for entry in entries:
                assert entry.args[-1] == ROM_PLACEHOLDER
                assert ROM_PLACEHOLDER not in entry.args[:-1]

    def test_globs_are_home_relative(self):
        for entries in EMULATOR_TABLE.values():
            for entry in entries:
                assert not entry.locate_glob.startswith("/")
                assert "/home/deck" not in entry.locate_glob

    def test_uncovered_system(self):
        assert table_for("psx") == ()
        assert table_for(None) == ()
        assert not covers("psx")
        assert covers("switch")

    def test_find_emulator(self):
        assert find_emulator("ryujinx").label == "Ryujinx"
        assert find_emulator("nope") is None
        assert find_emulator(None) is None

    def test_launch_file_extensions(self):
        assert launch_file_extensions("switch") == frozenset({".xci", ".nsp"})
        assert launch_file_extensions("psx") is None
        assert launch_file_extensions(None) is None


class TestOptionsAndSelection:
    def test_all_installed_default_is_eden(self):
        options = table_options("switch", {"eden": _EDEN, "ryujinx": _RYU, "yuzu": _YUZU})
        assert [o.status for o in options] == ["bakeable"] * 3
        default = select_default_option(options)
        assert default is not None and default.label == "Eden"
        assert option_to_invocation(default) == EmulatorInvocation.table("eden", _EDEN, "Eden")

    def test_default_is_first_installed_in_table_order(self):
        options = table_options("switch", {"yuzu": _YUZU, "ryujinx": _RYU})
        default = select_default_option(options)
        assert default is not None and default.label == "Ryujinx"

    def test_missing_emulators_are_not_installed(self):
        options = table_options("switch", {"yuzu": _YUZU})
        eden = options[0]
        assert (eden.status, eden.reason, eden.launcher) == ("needs_setup", "not_installed", None)

    def test_nothing_installed_has_no_default(self):
        options = table_options("switch", {})
        assert select_default_option(options) is None

    def test_override_to_installed_emulator(self):
        options = table_options("switch", {"eden": _EDEN, "ryujinx": _RYU})
        assert label_to_invocation(options, "Ryujinx") == EmulatorInvocation.table("ryujinx", _RYU, "Ryujinx")

    def test_override_to_missing_emulator_does_not_resolve(self):
        # The caller (ActiveCoreResolver) then degrades to the default.
        options = table_options("switch", {"eden": _EDEN})
        assert label_to_invocation(options, "yuzu") is None

    def test_table_option_without_launcher_never_renders(self):
        options = table_options("switch", {})
        assert option_to_invocation(options[0]) is None


class TestRenderInvocation:
    def test_eden(self):
        assert render_invocation(find_emulator("eden"), _EDEN) == f'vblank_mode=0 %command% "{_EDEN}" -f -g'

    def test_ryujinx_has_no_extra_args(self):
        assert render_invocation(find_emulator("ryujinx"), _RYU) == f'vblank_mode=0 %command% "{_RYU}"'

    def test_yuzu(self):
        assert render_invocation(find_emulator("yuzu"), _YUZU) == f'vblank_mode=0 %command% "{_YUZU}" -f -g'

    def test_template_not_ending_with_rom_is_rejected(self):
        bad = TableEmulator("bad", "Bad", "x", (ROM_PLACEHOLDER, "--fullscreen"))
        with pytest.raises(ValueError):
            render_invocation(bad, "/x")

    def test_template_without_rom_is_rejected(self):
        with pytest.raises(ValueError):
            render_invocation(TableEmulator("bad", "Bad", "x", ()), "/x")

    @pytest.mark.parametrize(
        "rom",
        [
            "/run/media/mmcblk0p1/Emulation/roms/switch/Pokemon Shining Pearl [010018E011D92000][v0].nsp",
            "/roms/switch/Zelda's Echoes (v1.0) [DLC].xci",
            '/roms/switch/odd "quoted" name.nsp',
            "/roms/switch/back\\slash.nsp",
        ],
    )
    def test_full_command_tokenizes_to_exact_argv(self, rom):
        exe = "/home/deck/My Apps/Eden v2.AppImage"
        options = build_launch_options(render_invocation(find_emulator("eden"), exe), rom)
        # A POSIX shell (what Steam runs ``%command%`` launch options through)
        # must yield exactly: env, %command%, exe, -f, -g, rom.
        assert shlex.split(options) == ["vblank_mode=0", "%command%", exe, "-f", "-g", rom]
