"""EmuDeck launch resolution — build a host-neutral launch command from a
resolved :class:`domain.shortcut_data.EmulatorInvocation` and the host's
launcher / RetroArch cores roots.

EmuDeck does not boot games through a single sandbox app the way RetroDECK
does. Each emulator has its own launcher script under
``<storagePath>/Emulation/tools/launchers/`` (e.g. ``pcsx2-qt.sh``,
``dolphin-emu.sh``, ``retroarch.sh``), and the launch command is the
launcher script path followed by the emulator's middle args and the ROM
path.

This module is pure: it takes the resolved roots (``launchers_path``,
``retroarch_cores_dir``) and the per-system launcher-script map as inputs
and emits a structured resolution. The adapter
(:mod:`adapters.emudeck_launchers`) supplies the roots; the existing
shortcut renderer (:func:`domain.shortcut_data.resolve_emulator_invocation`)
delegates here so every ``launch_options`` bake site uses one shared
resolver (the invariant of EmuDeck port plan §7).

The resolution carries a structured status so callers that need a
user-visible state (game-detail panel, launcher-missing banners) get
``needs_setup`` / ``launcher_missing`` instead of a silently-empty string.
The shortcut-bake path consumes ``launch_options`` only.
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from typing import Any

from domain.emulator_table import find_emulator, render_invocation
from domain.shortcut_data import EmulatorInvocation, direct_launch_args

# Resolution states surfaced to the UI / game-detail panel. The shortcut bake
# path only consumes ``launch_options``; the structured status lets the UI
# distinguish "no emulator resolved for this platform" (needs_setup — the user
# must configure an emulator) from "emulator resolved but its launcher script
# is missing on disk" (launcher_missing — EmuDeck is half-installed or the SD
# card ejected). See EmuDeck port plan §6 fallback order.
RESOLUTION_OK = "ok"
RESOLUTION_NEEDS_SETUP = "needs_setup"
RESOLUTION_LAUNCHER_MISSING = "launcher_missing"


@dataclass(frozen=True)
class EmuDeckLaunchResolution:
    """Outcome of resolving a ROM's launch command on EmuDeck.

    Attributes:
        status: One of ``ok`` / ``needs_setup`` / ``launcher_missing``.
        launch_options: The host-neutral launch command prefix to bake into
            the Steam shortcut's ``launch_options`` (the ROM path is
            appended separately by
            :func:`domain.shortcut_data.build_launch_options`). Empty when
            status is not ``ok`` — never a flatpak/RetroDECK fallback.
        launcher_path: The resolved on-disk launcher script path when
            status is ``ok``; diagnostic only (the UI can show which
            launcher was chosen).
        reason: Short machine-readable slug for the non-ok cases; for
            ``launcher_missing`` it carries the missing launcher script
            basename so the user knows what to install.
    """

    status: str
    launch_options: str = ""
    launcher_path: str = ""
    reason: str = ""


def _retroarch_invocation(
    *,
    launchers_path: str,
    retroarch_cores_dir: str,
    core_so: str,
    retroarch_launcher_script: str,
) -> EmuDeckLaunchResolution:
    """Build the RetroArch (libretro) EmuDeck invocation.

    EmuDeck ships a ``retroarch.sh`` launcher at ``<launchers>/retroarch.sh``
    that exec's RetroArch with the right flatpak/native wrapper. The libretro
    ``-L <coresdir>/<core>.so`` form is preserved verbatim — EmuDeck's
    RetroArch reads cores from a flatpak-scoped dir (``retroarch_cores_dir``),
    not the RetroDECK sandbox literal the old renderer baked in.
    """
    launcher = os.path.join(launchers_path, retroarch_launcher_script)
    # The %ROM% placeholder stays — :func:`build_launch_options` appends the
    # actual ROM path after the invocation, and rom-launcher-ng exec's "$@" so
    # the trailing ROM argument lands in RetroArch's %ROM% slot. We do NOT
    # bake %ROM% here because build_launch_options composes
    # ``<invocation> "<rom-path>"`` — adding %ROM% would double the ROM.
    invocation = f"{launcher} -L {os.path.join(retroarch_cores_dir, core_so + '.so')}"
    return EmuDeckLaunchResolution(
        status=RESOLUTION_OK,
        launch_options=invocation,
        launcher_path=launcher,
    )


def _table_invocation(emulator: EmulatorInvocation) -> EmuDeckLaunchResolution:
    """Render an emulator-table invocation (see :func:`resolve_emudeck_invocation`)."""
    entry = find_emulator(emulator.command)
    if entry is None:
        return EmuDeckLaunchResolution(status=RESOLUTION_NEEDS_SETUP, reason="unknown_table_emulator")
    if not emulator.launcher:
        return EmuDeckLaunchResolution(status=RESOLUTION_NEEDS_SETUP, reason="emulator_not_installed")
    return EmuDeckLaunchResolution(
        status=RESOLUTION_OK,
        launch_options=render_invocation(entry, emulator.launcher),
        launcher_path=emulator.launcher,
    )


def resolve_emudeck_invocation(
    rom: dict[str, Any],
    emulator: EmulatorInvocation | None,
    *,
    launchers_path: str,
    retroarch_cores_dir: str,
    system: str | None,
    system_launcher_map: dict[str, str] | None,
    retroarch_launcher_script: str = "retroarch.sh",
) -> EmuDeckLaunchResolution:
    """Resolve the EmuDeck launch command for *rom* under *emulator*.

    Branches on ``emulator.kind`` (the existing EmulatorInvocation shape,
    kept for transitional compatibility — port plan §12):

    - ``None`` (no emulator resolved for the platform) → ``needs_setup``.
      The plan's hard rule (§6): never fall back to a RetroDECK command.
    - ``libretro`` with a ``core_so`` → the RetroArch launcher script with
      ``-L <coresdir>/<core>.so``. The retroarch launcher script name is
      ``retroarch.sh`` by default (overridable for installations that ship a
      differently-named RetroArch launcher).
    - ``table`` (an emulator-table entry, :mod:`domain.emulator_table`) → the
      entry's args template over its located executable, behind the
      ``vblank_mode=0 %command%`` prefix (EmuDeck plan D4: the emulator is
      invoked directly through ``rom-launcher-ng``, never through EmuDeck's
      launcher scripts). No executable located → ``needs_setup``
      (``emulator_not_installed``).
    - ``standalone`` or ``direct`` with a ``command`` → the per-system
      launcher script (looked up in ``system_launcher_map`` by ``system``);
      the standalone command's middle args (``%EMULATOR_*%`` and ``%ROM%``
      stripped) ride between the launcher and the ROM.

    Returns the structured resolution; the shortcut-bake path consumes
    ``launch_options`` only. Caller passes ``launchers_path`` + the
    launcher script name → the function returns the absolute launcher path
    in ``launcher_path`` (diagnostic). The function does NOT probe the disk
    — I/O lives in :class:`adapters.emudeck_launchers.EmuDeckLauncherAdapter`,
    which calls this after verifying the launcher exists.
    """
    del rom  # reserved for future per-ROM branching; symmetric with legacy
    if emulator is None:
        return EmuDeckLaunchResolution(status=RESOLUTION_NEEDS_SETUP, reason="no_emulator_resolved")

    if emulator.kind == "table":
        return _table_invocation(emulator)

    if emulator.kind == "libretro" and emulator.core_so:
        if not launchers_path or not retroarch_cores_dir:
            return EmuDeckLaunchResolution(status=RESOLUTION_NEEDS_SETUP, reason="host_roots_unavailable")
        return _retroarch_invocation(
            launchers_path=launchers_path,
            retroarch_cores_dir=retroarch_cores_dir,
            core_so=emulator.core_so,
            retroarch_launcher_script=retroarch_launcher_script,
        )

    if emulator.kind in ("standalone", "direct") and emulator.command:
        if not launchers_path:
            return EmuDeckLaunchResolution(status=RESOLUTION_NEEDS_SETUP, reason="host_roots_unavailable")
        if not system or not system_launcher_map:
            return EmuDeckLaunchResolution(status=RESOLUTION_NEEDS_SETUP, reason="no_launcher_mapped")
        launcher_script = system_launcher_map.get(system)
        if not launcher_script:
            return EmuDeckLaunchResolution(status=RESOLUTION_NEEDS_SETUP, reason="no_launcher_mapped")
        launcher = os.path.join(launchers_path, launcher_script)
        args = direct_launch_args(emulator.command)
        invocation = f"{launcher} {args}" if args else launcher
        return EmuDeckLaunchResolution(
            status=RESOLUTION_OK,
            launch_options=invocation,
            launcher_path=launcher,
        )

    return EmuDeckLaunchResolution(status=RESOLUTION_NEEDS_SETUP, reason="unrenderable_emulator_kind")
