"""The plugin's own emulator table — which standalone emulators launch a system, and how.

Replaces the ES-DE ``es_systems.xml`` lookup for the systems it covers (EmuDeck
ships no readable ES-DE config, so that lookup resolves everything to
``needs_setup``). Each covered system lists its emulators in preference order;
each emulator says where EmuDeck installs it (a glob relative to the user's
home) and how to call it (an args template whose last token is the ROM).

Anything that is pure data or pure decision-making over this table belongs
here: the table itself, the per-system launch-file extensions, turning located
emulators into picker options, and rendering the launch-command prefix. Finding
the emulators on disk is I/O and lives in :mod:`adapters.emulator_table`.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

from domain.emulator_commands import EmulatorOption
from domain.shortcut_data import quote_launch_arg

if TYPE_CHECKING:
    from collections.abc import Mapping

# The ROM placeholder in an args template. It must be the template's LAST token:
# :func:`domain.shortcut_data.build_launch_options` appends the quoted ROM path
# after the rendered prefix, the same way every other bake site composes it.
ROM_PLACEHOLDER = "{rom}"

# Steam replaces ``%command%`` with the shortcut's exe (``bin/rom-launcher-ng``,
# a pure ``exec "$@"``), so the env prefix applies to the emulator process.
STEAM_COMMAND_TOKEN = "%command%"

# Mesa: disable vsync throttling for standalone emulators (EmuDeck's own
# launchers do the same for these emulators).
STANDALONE_ENV = ("vblank_mode=0",)

# ``kind`` of an :class:`EmulatorOption` / ``EmulatorInvocation`` that names a
# table emulator. ``command`` carries the emulator id, ``launcher`` the located
# executable path.
TABLE_KIND = "table"


@dataclass(frozen=True)
class TableEmulator:
    """One emulator entry of the table.

    ``emulator_id`` is the stable key; ``label`` is the display label and the
    per-game override pick key (``roms.emulator_override``). ``locate_glob`` is a
    ``/``-separated glob relative to the user's home, matched case-insensitively
    per path component (EmuDeck's ``find -iname``). ``args`` is the argument
    template; ``{rom}`` is the ROM path and must be the final token. ``env`` is
    the environment prefix placed before Steam's ``%command%``.
    """

    emulator_id: str
    label: str
    locate_glob: str
    args: tuple[str, ...]
    env: tuple[str, ...] = STANDALONE_ENV


EMULATOR_TABLE: dict[str, tuple[TableEmulator, ...]] = {
    "switch": (
        TableEmulator("eden", "Eden", "Applications/Eden*.AppImage", ("-f", "-g", ROM_PLACEHOLDER)),
        TableEmulator("ryujinx", "Ryujinx", "Applications/publish/Ryujinx.sh", (ROM_PLACEHOLDER,)),
        TableEmulator("yuzu", "yuzu", "Applications/yuzu*.AppImage", ("-f", "-g", ROM_PLACEHOLDER)),
    ),
}

# Systems whose folder installs (base game + updates/DLC) launch one file chosen
# from these extensions — by default the largest, overridable per game through
# the launch-file pick (``roms.selected_disc``). Lowercase; matched
# case-insensitively.
LAUNCH_FILE_EXTENSIONS: dict[str, frozenset[str]] = {
    "switch": frozenset({".xci", ".nsp"}),
}


def table_for(system: str | None) -> tuple[TableEmulator, ...]:
    """The table entries for *system* in preference order (empty when uncovered)."""
    if not system:
        return ()
    return EMULATOR_TABLE.get(system, ())


def covers(system: str | None) -> bool:
    """Whether *system* resolves through the emulator table."""
    return bool(table_for(system))


def find_emulator(emulator_id: str | None) -> TableEmulator | None:
    """Look up a table entry by its id across every system."""
    if not emulator_id:
        return None
    for entries in EMULATOR_TABLE.values():
        for entry in entries:
            if entry.emulator_id == emulator_id:
                return entry
    return None


def launch_file_extensions(system: str | None) -> frozenset[str] | None:
    """The launch-file extensions for *system*'s folder installs, or ``None`` when uncovered."""
    if not system:
        return None
    return LAUNCH_FILE_EXTENSIONS.get(system)


def table_options(system: str, installed: Mapping[str, str]) -> list[EmulatorOption]:
    """Project *system*'s table into picker options, in table order.

    *installed* maps ``emulator_id`` to its located executable path (from the
    adapter's probe). An installed entry is ``bakeable`` and carries its path as
    ``launcher``; a missing one is ``needs_setup`` with reason ``not_installed``,
    so it is never the default and shows disabled in the picker. The first
    bakeable option is therefore the first installed emulator in table order —
    the system default (:func:`domain.emulator_commands.select_default_option`).
    """
    options: list[EmulatorOption] = []
    for entry in table_for(system):
        path = installed.get(entry.emulator_id)
        options.append(
            EmulatorOption(
                label=entry.label,
                kind=TABLE_KIND,
                core_so=None,
                command=entry.emulator_id,
                status="bakeable" if path else "needs_setup",
                reason=None if path else "not_installed",
                launcher=path or None,
            )
        )
    return options


def render_invocation(entry: TableEmulator, executable: str) -> str:
    """Render the launch-command prefix for *entry* located at *executable*.

    ``<env> %command% "<executable>" <args before {rom}>`` — e.g.
    ``vblank_mode=0 %command% "/home/deck/Applications/Eden.AppImage" -f -g``.
    The caller appends the quoted ROM path via
    :func:`domain.shortcut_data.build_launch_options`. The executable path is
    quoted with the same escaping as the ROM path. Raises ``ValueError`` when
    the template does not end with ``{rom}`` (the table invariant).
    """
    if not entry.args or entry.args[-1] != ROM_PLACEHOLDER or ROM_PLACEHOLDER in entry.args[:-1]:
        raise ValueError(f"args template for {entry.emulator_id} must end with exactly one {ROM_PLACEHOLDER}")
    parts = [*entry.env, STEAM_COMMAND_TOKEN, quote_launch_arg(executable), *entry.args[:-1]]
    return " ".join(parts)
