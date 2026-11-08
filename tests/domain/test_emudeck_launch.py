"""Tests for domain.emudeck_launch — the pure EmuDeck launch resolver."""

from __future__ import annotations

import os

from domain.emudeck_launch import (
    RESOLUTION_NEEDS_SETUP,
    RESOLUTION_OK,
    resolve_emudeck_invocation,
)
from domain.shortcut_data import EmulatorInvocation


class TestLibretroResolution:
    def test_libretro_core_bakes_retroarch_launcher_with_cores_dir(self):
        res = resolve_emudeck_invocation(
            {},
            EmulatorInvocation.libretro("snes9x", label="Snes9x"),
            launchers_path="/home/deck/Emulation/tools/launchers",
            retroarch_cores_dir="/home/deck/.var/app/org.libretro.RetroArch/config/retroarch/cores",
            system="snes",
            system_launcher_map={"snes": "snes9x.sh"},
        )
        assert res.status == RESOLUTION_OK
        assert res.launcher_path == "/home/deck/Emulation/tools/launchers/retroarch.sh"
        assert res.launch_options == (
            "/home/deck/Emulation/tools/launchers/retroarch.sh -L "
            "/home/deck/.var/app/org.libretro.RetroArch/config/retroarch/cores/snes9x.so"
        )

    def test_libretro_uses_custom_retroarch_launcher_script(self):
        res = resolve_emudeck_invocation(
            {},
            EmulatorInvocation.libretro("gambatte"),
            launchers_path="/x/launchers",
            retroarch_cores_dir="/x/cores",
            system="gb",
            system_launcher_map={},
            retroarch_launcher_script="retroarch-emu.sh",
        )
        assert res.status == RESOLUTION_OK
        assert res.launcher_path == "/x/launchers/retroarch-emu.sh"
        assert res.launch_options == "/x/launchers/retroarch-emu.sh -L /x/cores/gambatte.so"

    def test_libretro_with_empty_core_so_is_unrenderable(self):
        # core_so is falsy — the dataclass factory takes bare names, but
        # defensive guard handles None/empty.
        emu = EmulatorInvocation(kind="libretro", core_so=None)
        res = resolve_emudeck_invocation(
            {}, emu, launchers_path="/x", retroarch_cores_dir="/x/c", system="snes", system_launcher_map={}
        )
        assert res.status == RESOLUTION_NEEDS_SETUP
        assert res.launch_options == ""

    def test_libretro_without_host_roots_is_needs_setup(self):
        res = resolve_emudeck_invocation(
            {},
            EmulatorInvocation.libretro("snes9x"),
            launchers_path="",
            retroarch_cores_dir="",
            system="snes",
            system_launcher_map={},
        )
        assert res.status == RESOLUTION_NEEDS_SETUP
        assert res.launch_options == ""
        assert res.reason == "host_roots_unavailable"


class TestStandaloneResolution:
    def test_standalone_with_mapped_launcher_bakes_launcher_and_middle_args(self):
        emu = EmulatorInvocation.standalone("%EMULATOR_RPCS3% --no-gui %ROM%", label="RPCS3")
        res = resolve_emudeck_invocation(
            {},
            emu,
            launchers_path="/x/launchers",
            retroarch_cores_dir="/x/c",
            system="ps3",
            system_launcher_map={"ps3": "rpcs3.sh"},
        )
        assert res.status == RESOLUTION_OK
        assert res.launcher_path == "/x/launchers/rpcs3.sh"
        assert res.launch_options == "/x/launchers/rpcs3.sh --no-gui"

    def test_standalone_with_no_middle_args_yields_bare_launcher(self):
        emu = EmulatorInvocation.standalone("%EMULATOR_RPCS3% %ROM%", label="RPCS3")
        res = resolve_emudeck_invocation(
            {},
            emu,
            launchers_path="/x/launchers",
            retroarch_cores_dir="/x/c",
            system="ps3",
            system_launcher_map={"ps3": "rpcs3.sh"},
        )
        assert res.status == RESOLUTION_OK
        assert res.launch_options == "/x/launchers/rpcs3.sh"

    def test_direct_kind_uses_launcher_and_middle_args(self):
        emu = EmulatorInvocation.direct(
            "%EMULATOR_RPCS3% --no-gui %ROM%",
            launcher="/app/retrodeck/components/rpcs3/component_launcher.sh",
            label="RPCS3",
        )
        res = resolve_emudeck_invocation(
            {},
            emu,
            launchers_path="/x/launchers",
            retroarch_cores_dir="/x/c",
            system="ps3",
            system_launcher_map={"ps3": "rpcs3.sh"},
        )
        assert res.status == RESOLUTION_OK
        # The direct kind's sandbox launcher path is IGNORED on EmuDeck —
        # the new host uses the mapped EmuDeck launcher script, not the
        # RetroDECK sandbox component_launcher path the direct form carried.
        assert res.launcher_path == "/x/launchers/rpcs3.sh"
        assert res.launch_options == "/x/launchers/rpcs3.sh --no-gui"

    def test_standalone_without_mapped_system_returns_needs_setup(self):
        res = resolve_emudeck_invocation(
            {},
            EmulatorInvocation.standalone("%EMULATOR_RPCS3% %ROM%"),
            launchers_path="/x/launchers",
            retroarch_cores_dir="/x/c",
            system="ps3",
            system_launcher_map={"ps2": "pcsx2-qt.sh"},  # ps3 not mapped
        )
        assert res.status == RESOLUTION_NEEDS_SETUP
        assert res.reason == "no_launcher_mapped"
        assert res.launch_options == ""

    def test_standalone_without_system_returns_needs_setup(self):
        res = resolve_emudeck_invocation(
            {},
            EmulatorInvocation.standalone("%EMULATOR_RPCS3% %ROM%"),
            launchers_path="/x/launchers",
            retroarch_cores_dir="/x/c",
            system=None,
            system_launcher_map={"ps3": "rpcs3.sh"},
        )
        assert res.status == RESOLUTION_NEEDS_SETUP
        assert res.reason == "no_launcher_mapped"

    def test_standalone_without_launcher_map_returns_needs_setup(self):
        res = resolve_emudeck_invocation(
            {},
            EmulatorInvocation.standalone("%EMULATOR_RPCS3% %ROM%"),
            launchers_path="/x/launchers",
            retroarch_cores_dir="/x/c",
            system="ps3",
            system_launcher_map=None,
        )
        assert res.status == RESOLUTION_NEEDS_SETUP
        assert res.reason == "no_launcher_mapped"

    def test_standalone_without_launchers_path_returns_needs_setup(self):
        res = resolve_emudeck_invocation(
            {},
            EmulatorInvocation.standalone("%EMULATOR_RPCS3% %ROM%"),
            launchers_path="",
            retroarch_cores_dir="/x/c",
            system="ps3",
            system_launcher_map={"ps3": "rpcs3.sh"},
        )
        assert res.status == RESOLUTION_NEEDS_SETUP
        assert res.reason == "host_roots_unavailable"


class TestNoneEmulator:
    def test_none_emulator_is_needs_setup_never_flatpak(self):
        """Plan §6 hard rule: a missing launcher / unresolved emulator must
        NOT silently fall back to a RetroDECK / flatpak run command. It
        surfaces needs_setup with an empty launch_options string.
        """
        res = resolve_emudeck_invocation(
            {},
            None,
            launchers_path="/x/launchers",
            retroarch_cores_dir="/x/c",
            system="ps3",
            system_launcher_map={"ps3": "rpcs3.sh"},
        )
        assert res.status == RESOLUTION_NEEDS_SETUP
        assert res.reason == "no_emulator_resolved"
        assert res.launch_options == ""
        # Critical: there is NO `flatpak run net.retrodeck.retrodeck` token
        # anywhere in the resolution.
        assert "flatpak" not in res.launch_options
        assert "net.retrodeck" not in res.launch_options


class TestUnrenderableEmulator:
    def test_emulator_with_unrecognized_kind_is_needs_setup(self):
        emu = EmulatorInvocation(kind="unknown_kind", command="whatever")
        res = resolve_emudeck_invocation(
            {},
            emu,
            launchers_path="/x/launchers",
            retroarch_cores_dir="/x/c",
            system="ps3",
            system_launcher_map={"ps3": "rpcs3.sh"},
        )
        assert res.status == RESOLUTION_NEEDS_SETUP
        assert res.reason == "unrenderable_emulator_kind"
        assert res.launch_options == ""

    def test_standalone_with_missing_command_is_needs_setup(self):
        emu = EmulatorInvocation(kind="standalone", command=None)
        res = resolve_emudeck_invocation(
            {},
            emu,
            launchers_path="/x/launchers",
            retroarch_cores_dir="/x/c",
            system="ps3",
            system_launcher_map={"ps3": "rpcs3.sh"},
        )
        assert res.status == RESOLUTION_NEEDS_SETUP
        assert res.reason == "unrenderable_emulator_kind"


class TestNoRetrodeckLeak:
    """Plan §7 task 1: the new renderer must NOT emit any of the RetroDECK
    flatpak / sandbox-core constants. Sweep across every resolved branch."""

    def test_libretro_options_contain_no_retrodeck_or_flatpak_token(self):
        res = resolve_emudeck_invocation(
            {},
            EmulatorInvocation.libretro("snes9x"),
            launchers_path="/x/l",
            retroarch_cores_dir="/x/c",
            system="snes",
            system_launcher_map={"snes": "retroarch.sh"},
        )
        assert res.status == RESOLUTION_OK
        assert "flatpak" not in res.launch_options
        assert "net.retrodeck" not in res.launch_options
        # The RetroDECK sandbox cores literal must not be baked.
        assert "/var/config/retroarch/cores" not in res.launch_options

    def test_standalone_options_contain_no_retrodeck_or_flatpak_token(self):
        res = resolve_emudeck_invocation(
            {},
            EmulatorInvocation.standalone("%EMULATOR_RPCS3% --no-gui %ROM%"),
            launchers_path="/x/l",
            retroarch_cores_dir="/x/c",
            system="ps3",
            system_launcher_map={"ps3": "rpcs3.sh"},
        )
        assert res.status == RESOLUTION_OK
        assert "flatpak" not in res.launch_options
        assert "net.retrodeck" not in res.launch_options
        # The %EMULATOR_*% / %ROM% tokens are stripped — only middle args.
        assert "%EMULATOR_" not in res.launch_options
        assert "%ROM%" not in res.launch_options


class TestPathSeparation:
    def test_launcher_path_is_absolute_under_launchers_path(self):
        res = resolve_emudeck_invocation(
            {},
            EmulatorInvocation.standalone("%EMULATOR_RPCS3% %ROM%"),
            launchers_path="/home/deck/Emulation/tools/launchers",
            retroarch_cores_dir="/x/c",
            system="ps3",
            system_launcher_map={"ps3": "rpcs3.sh"},
        )
        assert res.launcher_path == os.path.join("/home/deck/Emulation/tools/launchers", "rpcs3.sh")
        assert os.path.isabs(res.launcher_path)

    def test_cores_path_is_absolute_under_retroarch_cores_dir(self):
        res = resolve_emudeck_invocation(
            {},
            EmulatorInvocation.libretro("gambatte"),
            launchers_path="/x/l",
            retroarch_cores_dir="/home/deck/.var/app/org.libretro.RetroArch/config/retroarch/cores",
            system="gb",
            system_launcher_map={},
        )
        assert res.status == RESOLUTION_OK
        # The ".so" suffix is appended; the cores dir is preserved as given.
        assert res.launch_options.endswith(
            os.path.join("/home/deck/.var/app/org.libretro.RetroArch/config/retroarch/cores", "gambatte.so")
        )


class TestTableEmulatorBranch:
    def _resolve(self, emulator):
        from domain.emudeck_launch import resolve_emudeck_invocation

        return resolve_emudeck_invocation(
            {"id": 1},
            emulator,
            launchers_path="",
            retroarch_cores_dir="",
            system="switch",
            system_launcher_map=None,
        )

    def test_located_eden_renders(self):
        res = self._resolve(EmulatorInvocation.table("eden", "/h/Applications/Eden.AppImage", "Eden"))
        assert res.status == "ok"
        assert res.launch_options == 'vblank_mode=0 %command% "/h/Applications/Eden.AppImage" -f -g'

    def test_ryujinx_renders_without_flags(self):
        res = self._resolve(EmulatorInvocation.table("ryujinx", "/h/Applications/publish/Ryujinx.sh", "Ryujinx"))
        assert res.launch_options == 'vblank_mode=0 %command% "/h/Applications/publish/Ryujinx.sh"'

    def test_unlocated_emulator_is_needs_setup(self):
        res = self._resolve(EmulatorInvocation.table("eden", None, "Eden"))
        assert (res.status, res.reason, res.launch_options) == ("needs_setup", "emulator_not_installed", "")

    def test_unknown_table_id_is_needs_setup(self):
        res = self._resolve(EmulatorInvocation.table("citra", "/x", "Citra"))
        assert (res.status, res.reason) == ("needs_setup", "unknown_table_emulator")
