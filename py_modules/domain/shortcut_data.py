"""Pure functions for building shortcut data dicts and launch commands.

No I/O, no imports from services, adapters, or lib. Launch-invocation
resolution (the host-specific prefix before the ROM path) is the caller's
responsibility — this module receives pre-resolved invocation strings and
combines them with the ROM path via :func:`build_launch_options`.
"""

from __future__ import annotations

import os
import re
from dataclasses import dataclass
from typing import Any

from domain.sibling_group import compute_sibling_group_key

# The leading ``%EMULATOR_<NAME>%`` binary token and the trailing ``%ROM%`` target
# of an ES-DE ``<command>`` — stripped from a standalone command to recover the
# middle launcher args (e.g. ``--no-gui``) for the folder-boot ``direct`` bake.
_EMULATOR_TOKEN_RE = re.compile(r"%EMULATOR_[A-Z0-9_-]+%")


@dataclass(frozen=True)
class EmulatorInvocation:
    """What a ROM launches with — a libretro core, a standalone emulator, or a direct launch.

    The plugin resolves one of these per ROM and passes it to the host-specific
    launch resolver (the ``LaunchResolverFn`` seam) which turns it into a
    launch-options prefix. The payload carried depends on ``kind``:

    - ``kind == "libretro"`` → ``core_so`` is the BARE core name (no ``.so``); the
      resolver emits the RetroArch ``-L <coresdir>/<so>.so %ROM%`` form.
    - ``kind == "standalone"`` → ``command`` is the full ES-DE ``<command>`` text
      (already ending in ``%ROM%``).
    - ``kind == "direct"`` → the folder-boot form (ADR-0019): ``command`` is the
      same full ES-DE standalone command AND ``launcher`` is the emulator's
      sandbox launcher path.

    ``label`` is the ES-DE display label (diagnostics only). This is the
    standalone-emulator seam (#129); read-path consumers that only understand
    libretro keep reading ``core_so`` (``None`` for a standalone or direct
    emulator) and degrade exactly as they do for a ``(None, None)`` resolution.
    """

    kind: str  # "libretro" | "standalone" | "direct"
    label: str | None = None
    core_so: str | None = None
    command: str | None = None
    launcher: str | None = None

    @classmethod
    def libretro(cls, core_so: str, label: str | None = None) -> EmulatorInvocation:
        """A RetroArch libretro core, identified by its bare ``.so`` name."""
        return cls(kind="libretro", label=label, core_so=core_so)

    @classmethod
    def standalone(cls, command: str, label: str | None = None) -> EmulatorInvocation:
        """A standalone emulator, identified by its full ES-DE ``<command>`` text."""
        return cls(kind="standalone", label=label, command=command)

    @classmethod
    def direct(cls, command: str, launcher: str, label: str | None = None) -> EmulatorInvocation:
        """A standalone emulator launched directly via its sandbox *launcher* (folder-boot form).

        *command* is the full ES-DE standalone ``<command>`` (its middle args are
        recovered at render time); *launcher* is the emulator's sandbox launcher
        path.
        """
        return cls(kind="direct", label=label, command=command, launcher=launcher)


def direct_launch_args(command: str) -> str:
    """Recover a standalone command's middle launcher args for the ``direct`` bake.

    Strips the leading ``%EMULATOR_<NAME>%`` binary token(s) and the ``%ROM%``
    target from an ES-DE ``<command>``, collapsing surrounding whitespace, so
    ``%EMULATOR_RPCS3% --no-gui %ROM%`` yields ``--no-gui`` and
    ``%EMULATOR_RPCS3% %ROM%`` yields ``""``. Pure text.
    """
    stripped = _EMULATOR_TOKEN_RE.sub("", command).replace("%ROM%", "")
    return " ".join(stripped.split())


def build_launch_options(invocation: str, path: str) -> str:
    """Compose the Steam shortcut launch command from *invocation* and ROM *path*.

    The path is double-quoted so paths with spaces survive the launcher's
    ``exec "$@"``. Embedded ``\\`` and ``"`` in the path are backslash-escaped
    (backslash first, then quote) so a server-controlled ROM filename cannot
    break out of the quoted token and inject extra argv elements into the
    emulator invocation. Only the path is escaped — *invocation* is trusted
    build-time text whose own ``-e "..."`` quoting must survive verbatim.
    """
    escaped = path.replace("\\", "\\\\").replace('"', '\\"')
    return f'{invocation} "{escaped}"'


def extract_version_metadata(rom: dict[str, Any]) -> dict[str, Any]:
    """Extract a raw RomM ROM dict's sibling-group identity + version dimensions.

    The server-derived facts (ADR-0021) persisted on the ``Rom`` aggregate: the
    sibling-group key plus the ``regions`` / ``languages`` / ``revision`` /
    ``tags`` / ``is_main_sibling`` dimensions. Shared by the sync shortcut build
    (:func:`build_shortcuts_data`) and the version-switch persist path so both
    derive the group key and ``is_main_sibling`` identically — one source of
    truth for the extraction, no drift between the two call sites.

    Prefers a ``sibling_group_key`` already on the dict (the incremental-skip
    path carries the authoritative key) and recomputes only when absent.
    ``is_main_sibling`` sits under ``rom_user``; a missing or ``null``
    ``rom_user`` degrades to ``False``.
    """
    return {
        "sibling_group_key": rom.get("sibling_group_key") or compute_sibling_group_key(rom),
        "regions": list(rom.get("regions") or []),
        "languages": list(rom.get("languages") or []),
        "revision": rom.get("revision") or "",
        "tags": list(rom.get("tags") or []),
        "is_main_sibling": bool((rom.get("rom_user") or {}).get("is_main_sibling", False)),
    }


def build_shortcuts_data(
    roms: list[dict[str, Any]],
    plugin_dir: str,
    installed_paths: dict[int, str],
    invocations: dict[int, str],
) -> list[dict[str, Any]]:
    """Transform ROM list into shortcut data dicts for frontend AddShortcut calls.

    *installed_paths* maps ``rom_id`` to the resolved on-disk launch path. An
    installed ROM gets a full launch command in ``launch_options``; a ROM absent
    from the map gets ``""`` (empty placeholder) until it is downloaded.

    *invocations* maps ``rom_id`` to the **pre-resolved** launch-invocation prefix
    string (the host-specific command before the ROM path, e.g. the EmuDeck
    launcher script path + args). Only ROMs whose emulator resolved successfully
    appear in the map (the caller pre-resolves each installed ROM through the
    shared ``LaunchResolverFn`` seam); a ROM absent from the map gets an empty
    ``launch_options`` — the launcher was not resolved (``needs_setup`` /
    ``launcher_missing``), not a silent RetroDECK fallback.

    The sibling-group key (ADR-0021) and RomM's version dimensions (``regions`` /
    ``languages`` / ``revision`` / ``tags`` / ``is_main_sibling``) are derived
    from each raw ROM dict here and carried through so the commit persists them
    on the ``Rom`` aggregate. ``is_main_sibling`` sits under ``rom_user``; the
    lookup is guarded so a missing or ``null`` ``rom_user`` degrades to ``False``.
    """
    exe = os.path.join(plugin_dir, "bin", "rom-launcher")
    start_dir = os.path.join(plugin_dir, "bin")
    return [
        {
            "rom_id": rom["id"],
            "name": rom["name"],
            "fs_name": rom.get("fs_name", ""),
            # Alphabetical tie-break key for sibling-group representative
            # resolution (ADR-0021): RomM ships it on the list endpoint; fall
            # back to the filename stem when absent. No DB column — carried only
            # through the sync pipeline, never persisted.
            "fs_name_no_ext": rom.get("fs_name_no_ext") or os.path.splitext(rom.get("fs_name", ""))[0],
            "exe": exe,
            "start_dir": start_dir,
            "launch_options": (
                build_launch_options(
                    invocations.get(rom["id"], ""),
                    installed_paths[rom["id"]],
                )
                if rom["id"] in installed_paths
                else ""
            ),
            "platform_name": rom.get("platform_name", "Unknown"),
            "platform_slug": rom.get("platform_slug", ""),
            "igdb_id": rom.get("igdb_id"),
            "sgdb_id": rom.get("sgdb_id"),
            "ra_id": rom.get("ra_id"),
            "cover_path": "",
            # The sibling-group key + version dimensions (ADR-0021), extracted by
            # the shared helper so the sync build and the version-switch persist
            # path derive them identically. The incremental-skip path reconstructs
            # ROM dicts from persisted rows carrying the authoritative key (real
            # platform_id baked in) but no ``platform_id`` field, so the helper
            # prefers that key over recomputing (which would yield ``…:None`` and
            # split the group's bucket, #1296).
            **extract_version_metadata(rom),
        }
        for rom in roms
    ]
