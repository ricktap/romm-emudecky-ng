"""Tests for adapters.emudeck_launchers.EmuDeckLauncherAdapter."""

from __future__ import annotations

import logging
import os
from typing import Any

from adapters.emudeck_launchers import EmuDeckLauncherAdapter
from domain.emudeck_launch import RESOLUTION_LAUNCHER_MISSING, RESOLUTION_NEEDS_SETUP, RESOLUTION_OK
from domain.shortcut_data import EmulatorInvocation


class _FakePaths:
    """Minimal RetroDeckPaths-shaped fake — only ``launchers_path`` is used."""

    def __init__(self, *, launchers: str, exists: bool = True) -> None:
        self._launchers = launchers
        self._exists = exists

    def launchers_path(self) -> str:
        return self._launchers

    def roms_path(self) -> str:
        return ""

    def bios_path(self) -> str:
        return ""

    def saves_path(self) -> str:
        return ""

    def retrodeck_home(self) -> str:
        return ""

    def config_path(self) -> str:
        return ""

    def config_health(self) -> Any:
        from lib.emudeck_health import EmuDeckConfigHealth

        return EmuDeckConfigHealth.OK


def _make_launcher(tmp_path, name: str) -> str:
    launchers_dir = tmp_path / "Emulation" / "tools" / "launchers"
    launchers_dir.mkdir(parents=True, exist_ok=True)
    launcher = launchers_dir / name
    launcher.write_text('#!/bin/sh\nexec "$@"\n')
    launcher.chmod(0o755)
    return str(launchers_dir)


class TestStandaloneResolutionWithDiskProbe:
    def test_mapped_launcher_on_disk_resolves_ok(self, tmp_path):
        launchers = _make_launcher(tmp_path, "rpcs3.sh")
        adapter = EmuDeckLauncherAdapter(
            paths=_FakePaths(launchers=launchers),
            retroarch_cores_dir="/x/c",
            system_launcher_map={"ps3": "rpcs3.sh"},
            logger=logging.getLogger("test"),
        )
        res = adapter.resolve_for({}, EmulatorInvocation.standalone("%EMULATOR_RPCS3% --no-gui %ROM%"), "ps3")
        assert res.status == RESOLUTION_OK
        assert res.launcher_path == os.path.join(launchers, "rpcs3.sh")
        assert res.launch_options == f"{os.path.join(launchers, 'rpcs3.sh')} --no-gui"

    def test_mapped_launcher_missing_on_disk_is_launcher_missing(self, tmp_path):
        # The launchers dir exists but rpcs3.sh is not present — EmuDeck is
        # half-installed or the SD card ejected the launcher tree.
        launchers_dir = tmp_path / "Emulation" / "tools" / "launchers"
        launchers_dir.mkdir(parents=True)
        adapter = EmuDeckLauncherAdapter(
            paths=_FakePaths(launchers=str(launchers_dir)),
            retroarch_cores_dir="/x/c",
            system_launcher_map={"ps3": "rpcs3.sh"},
            logger=logging.getLogger("test"),
        )
        res = adapter.resolve_for({}, EmulatorInvocation.standalone("%EMULATOR_RPCS3% %ROM%"), "ps3")
        assert res.status == RESOLUTION_LAUNCHER_MISSING
        assert res.reason == "rpcs3.sh"
        assert res.launch_options == ""

    def test_system_not_mapped_is_needs_setup(self, tmp_path):
        launchers = _make_launcher(tmp_path, "rpcs3.sh")
        adapter = EmuDeckLauncherAdapter(
            paths=_FakePaths(launchers=launchers),
            retroarch_cores_dir="/x/c",
            system_launcher_map={"ps2": "pcsx2-qt.sh"},  # ps3 absent
            logger=logging.getLogger("test"),
        )
        res = adapter.resolve_for({}, EmulatorInvocation.standalone("%EMULATOR_RPCS3% %ROM%"), "ps3")
        assert res.status == RESOLUTION_NEEDS_SETUP
        assert res.reason == "no_launcher_mapped"

    def test_non_executable_launcher_is_launcher_missing(self, tmp_path):
        launchers_dir = tmp_path / "Emulation" / "tools" / "launchers"
        launchers_dir.mkdir(parents=True)
        launcher = launchers_dir / "rpcs3.sh"
        launcher.write_text("#!/bin/sh\n")
        launcher.chmod(0o644)  # NOT executable
        adapter = EmuDeckLauncherAdapter(
            paths=_FakePaths(launchers=str(launchers_dir)),
            retroarch_cores_dir="/x/c",
            system_launcher_map={"ps3": "rpcs3.sh"},
            logger=logging.getLogger("test"),
        )
        res = adapter.resolve_for({}, EmulatorInvocation.standalone("%EMULATOR_RPCS3% %ROM%"), "ps3")
        assert res.status == RESOLUTION_LAUNCHER_MISSING
        assert res.reason == "rpcs3.sh"


class TestLibretroResolutionWithDiskProbe:
    def test_libretro_with_retroarch_launcher_on_disk_resolves_ok(self, tmp_path):
        launchers = _make_launcher(tmp_path, "retroarch.sh")
        adapter = EmuDeckLauncherAdapter(
            paths=_FakePaths(launchers=launchers),
            retroarch_cores_dir="/home/deck/.var/app/org.libretro.RetroArch/config/retroarch/cores",
            logger=logging.getLogger("test"),
        )
        res = adapter.resolve_for({}, EmulatorInvocation.libretro("snes9x"), "snes")
        assert res.status == RESOLUTION_OK
        assert res.launcher_path == os.path.join(launchers, "retroarch.sh")
        assert res.launch_options.endswith("snes9x.so")

    def test_libretro_with_retroarch_launcher_missing_is_launcher_missing(self, tmp_path):
        # No retroarch.sh in the launchers dir.
        launchers_dir = tmp_path / "Emulation" / "tools" / "launchers"
        launchers_dir.mkdir(parents=True)
        adapter = EmuDeckLauncherAdapter(
            paths=_FakePaths(launchers=str(launchers_dir)),
            retroarch_cores_dir="/x/c",
            logger=logging.getLogger("test"),
        )
        res = adapter.resolve_for({}, EmulatorInvocation.libretro("snes9x"), "snes")
        assert res.status == RESOLUTION_LAUNCHER_MISSING
        assert res.reason == "retroarch.sh"


class TestNoneEmulator:
    def test_none_emulator_is_needs_setup_never_flatpak(self, tmp_path):
        launchers = _make_launcher(tmp_path, "rpcs3.sh")
        adapter = EmuDeckLauncherAdapter(
            paths=_FakePaths(launchers=launchers),
            retroarch_cores_dir="/x/c",
            system_launcher_map={"ps3": "rpcs3.sh"},
            logger=logging.getLogger("test"),
        )
        res = adapter.resolve_for({}, None, "ps3")
        assert res.status == RESOLUTION_NEEDS_SETUP
        assert res.reason == "no_emulator_resolved"
        assert res.launch_options == ""
        assert "flatpak" not in res.launch_options
        assert "net.retrodeck" not in res.launch_options


class TestEmptyDefaultMap:
    """The shipped default launcher table is empty per port plan §6.

    Until the table is populated from a verified EmuDeck installation,
    EVERY standalone/direct system resolves to needs_setup — never a
    guessed launcher name and never a flatpak/RetroDECK fallback.
    """

    def test_default_empty_map_yields_needs_setup_for_standalone(self, tmp_path):
        launchers = _make_launcher(tmp_path, "rpcs3.sh")
        adapter = EmuDeckLauncherAdapter(
            paths=_FakePaths(launchers=launchers),
            retroarch_cores_dir="/x/c",
            logger=logging.getLogger("test"),
        )
        res = adapter.resolve_for({}, EmulatorInvocation.standalone("%EMULATOR_RPCS3% %ROM%"), "ps3")
        assert res.status == RESOLUTION_NEEDS_SETUP
        assert res.reason == "no_launcher_mapped"

    def test_libretro_does_not_require_system_map(self, tmp_path):
        """The libretro branch uses the retroarch launcher script directly,
        not the per-system map — so an empty map does NOT block libretro."""
        launchers = _make_launcher(tmp_path, "retroarch.sh")
        adapter = EmuDeckLauncherAdapter(
            paths=_FakePaths(launchers=launchers),
            retroarch_cores_dir="/x/c",
            logger=logging.getLogger("test"),
        )
        res = adapter.resolve_for({}, EmulatorInvocation.libretro("gambatte"), "gb")
        assert res.status == RESOLUTION_OK

    def test_set_system_launcher_map_overrides_default(self, tmp_path):
        launchers = _make_launcher(tmp_path, "rpcs3.sh")
        adapter = EmuDeckLauncherAdapter(
            paths=_FakePaths(launchers=launchers),
            retroarch_cores_dir="/x/c",
            logger=logging.getLogger("test"),
        )
        # Default map empty — ps3 resolves to needs_setup.
        assert (
            adapter.resolve_for({}, EmulatorInvocation.standalone("%EMULATOR_RPCS3% %ROM%"), "ps3").status
            == RESOLUTION_NEEDS_SETUP
        )
        # Override the map at runtime (test/refresh seam).
        adapter.set_system_launcher_map({"ps3": "rpcs3.sh"})
        assert (
            adapter.resolve_for({}, EmulatorInvocation.standalone("%EMULATOR_RPCS3% %ROM%"), "ps3").status
            == RESOLUTION_OK
        )


class TestNoRetrodeckLeak:
    def test_ok_resolution_carries_no_retrodeck_or_flatpak_token(self, tmp_path):
        launchers = _make_launcher(tmp_path, "rpcs3.sh")
        adapter = EmuDeckLauncherAdapter(
            paths=_FakePaths(launchers=launchers),
            retroarch_cores_dir="/x/c",
            system_launcher_map={"ps3": "rpcs3.sh"},
            logger=logging.getLogger("test"),
        )
        res = adapter.resolve_for({}, EmulatorInvocation.standalone("%EMULATOR_RPCS3% --no-gui %ROM%"), "ps3")
        assert res.status == RESOLUTION_OK
        assert "flatpak" not in res.launch_options
        assert "net.retrodeck" not in res.launch_options
        assert "/var/config/retroarch/cores" not in res.launch_options
        assert "%EMULATOR_" not in res.launch_options
        assert "%ROM%" not in res.launch_options


class TestTableEmulatorResolution:
    """Table emulators render directly — no EmuDeck launcher scripts involved (plan D4)."""

    def _adapter(self) -> EmuDeckLauncherAdapter:
        return EmuDeckLauncherAdapter(
            paths=_FakePaths(launchers=""), retroarch_cores_dir="", logger=logging.getLogger("t")
        )

    def test_located_executable_renders_eden_command(self, tmp_path):
        exe = tmp_path / "Eden.AppImage"
        exe.write_text("#!/bin/sh\n")
        exe.chmod(0o755)
        res = self._adapter().resolve_for({"id": 1}, EmulatorInvocation.table("eden", str(exe), "Eden"), "switch")
        assert res.status == RESOLUTION_OK
        assert res.launch_options == f'vblank_mode=0 %command% "{exe}" -f -g'
        assert res.launcher_path == str(exe)

    def test_vanished_executable_is_launcher_missing(self, tmp_path):
        exe = tmp_path / "Eden.AppImage"
        res = self._adapter().resolve_for({"id": 1}, EmulatorInvocation.table("eden", str(exe), "Eden"), "switch")
        assert res.status == RESOLUTION_LAUNCHER_MISSING
        assert res.reason == "Eden.AppImage"

    def test_non_executable_file_is_launcher_missing(self, tmp_path):
        exe = tmp_path / "Eden.AppImage"
        exe.write_text("x")
        exe.chmod(0o644)
        res = self._adapter().resolve_for({"id": 1}, EmulatorInvocation.table("eden", str(exe), "Eden"), "switch")
        assert res.status == RESOLUTION_LAUNCHER_MISSING

    def test_no_emulator_for_switch_is_needs_setup(self):
        res = self._adapter().resolve_for({"id": 1}, None, "switch")
        assert res.status == RESOLUTION_NEEDS_SETUP
        assert res.launch_options == ""
