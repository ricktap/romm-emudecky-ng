"""EmuDeck launcher-script resolution adapter.

EmuDeck installs per-emulator launcher shell scripts under
``<storagePath>/Emulation/tools/launchers/`` (e.g. ``pcsx2-qt.sh``,
``dolphin-emu.sh``, ``retroarch.sh``). Each launcher exec's its emulator
(flatpak or native) with the right command shape; the plugin's Steam
shortcut ``launch_options`` becomes ``<launcher-path> <emulator-args> "<rom>"``.

The adapter owns the I/O — it probes the launcher directory on disk and
returns a structured resolution (``ok`` / ``launcher_missing`` /
``needs_setup``) so callers can surface the right state to the UI.

Per the EmuDeck port plan §6 the default ``SYSTEM_DEFAULT_LAUNCHERS``
table is **deliberately empty**: launcher script names must be verified
against a real EmuDeck installation before they are committed. The plan
explicitly forbids guessed launcher names (a §21 stop condition). Until
the table is filled from a verified deck session, every system resolves
to ``needs_setup`` — the honest state, not a flatpak/RetroDECK fallback.

The pure launch-string construction lives in
:mod:`domain.emudeck_launch`; this adapter wraps it with disk probes.
"""

from __future__ import annotations

import os
from typing import TYPE_CHECKING, Any, Protocol

from domain.emudeck_launch import (
    RESOLUTION_LAUNCHER_MISSING,
    RESOLUTION_NEEDS_SETUP,
    EmuDeckLaunchResolution,
    resolve_emudeck_invocation,
)

if TYPE_CHECKING:
    import logging

    from domain.shortcut_data import EmulatorInvocation


class _EmuDeckPaths(Protocol):
    """Minimal paths surface the launcher adapter needs from EmuDeckPaths."""

    def launchers_path(self) -> str: ...


# Per-system launcher script names, verified against a real EmuDeck
# installation (port plan §6 hard rule: do not commit guessed launcher
# names). Populate this table from a verified deck session — each entry
# keys an EmuDeck-normalized system folder name (the same identifier
# ``resolve_system`` produces) to the launcher script basename found in
# ``Emulation/tools/launchers/``. Examples the plan calls out (NOT
# committed here until verified):
#     "ps2": "pcsx2-qt.sh",
#     "gc":  "dolphin-emu.sh",
#     "wii": "dolphin-emu.sh",
#     "psp": "ppsspp.sh",
SYSTEM_DEFAULT_LAUNCHERS: dict[str, str] = {}


class EmuDeckLauncherAdapter:
    """Resolve EmuDeck launcher scripts for a system + EmulatorInvocation.

    Holds the host roots (``launchers_path`` from EmuDeckPaths, plus the
    RetroArch cores dir for the libretro branch) and the per-system
    launcher map. Probes the disk so the resolution distinguishes: the
    launcher not being mapped for the system (``needs_setup`` — the
    system is unsupported until the table is filled) vs the launcher
    being mapped but missing on disk (``launcher_missing`` — EmuDeck is
    half-installed or the SD card ejected).
    """

    def __init__(
        self,
        *,
        paths: _EmuDeckPaths,
        retroarch_cores_dir: str,
        system_launcher_map: dict[str, str] | None = None,
        retroarch_launcher_script: str = "retroarch.sh",
        logger: logging.Logger,
    ) -> None:
        self._paths = paths
        self._retroarch_cores_dir = retroarch_cores_dir
        self._system_launcher_map = dict(system_launcher_map or SYSTEM_DEFAULT_LAUNCHERS)
        self._retroarch_launcher_script = retroarch_launcher_script
        self._logger = logger

    def set_system_launcher_map(self, mapping: dict[str, str]) -> None:
        """Replace the per-system launcher map (test/refresh seam)."""
        self._system_launcher_map = dict(mapping)

    def system_launcher_script(self, system: str | None) -> str | None:
        """The mapped launcher script basename for *system*, or ``None``."""
        if not system:
            return None
        return self._system_launcher_map.get(system)

    def _launcher_full_path(self, launcher_script: str) -> str:
        return os.path.join(self._paths.launchers_path(), launcher_script)

    def _launcher_exists(self, launcher_script: str) -> bool:
        path = self._launcher_full_path(launcher_script)
        return os.path.isfile(path) and os.access(path, os.X_OK)

    def resolve_for(
        self,
        rom: dict[str, Any],
        emulator: EmulatorInvocation | None,
        system: str | None,
    ) -> EmuDeckLaunchResolution:
        """Resolve the EmuDeck launch command for *rom* under *emulator*.

        Probes the disk for the chosen launcher script first so the
        ``launcher_missing`` state (mapped but not on disk) is
        distinguished from ``needs_setup`` (no launcher mapped for this
        system). Only delegates to the pure resolver once the launcher is
        confirmed on disk.
        """
        launchers_path = self._paths.launchers_path()
        # For standalone/direct kinds we need a per-system mapped launcher.
        if emulator is not None and emulator.kind in ("standalone", "direct"):
            launcher_script = self.system_launcher_script(system)
            if launcher_script is None:
                return EmuDeckLaunchResolution(status=RESOLUTION_NEEDS_SETUP, reason="no_launcher_mapped")
            if not launchers_path or not self._launcher_exists(launcher_script):
                return EmuDeckLaunchResolution(status=RESOLUTION_LAUNCHER_MISSING, reason=launcher_script)
        # For libretro we need the RetroArch launcher script on disk.
        if (
            emulator is not None
            and emulator.kind == "libretro"
            and (not launchers_path or not self._launcher_exists(self._retroarch_launcher_script))
        ):
            return EmuDeckLaunchResolution(status=RESOLUTION_LAUNCHER_MISSING, reason=self._retroarch_launcher_script)

        # The launcher is mapped AND on disk — delegate to the pure resolver,
        # which always returns ok for this branch.
        return resolve_emudeck_invocation(
            rom,
            emulator,
            launchers_path=launchers_path,
            retroarch_cores_dir=self._retroarch_cores_dir,
            system=system,
            system_launcher_map=self._system_launcher_map,
            retroarch_launcher_script=self._retroarch_launcher_script,
        )
