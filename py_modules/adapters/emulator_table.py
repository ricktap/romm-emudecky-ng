"""Host side of the emulator table — locate table emulators on disk.

The pure table (:mod:`domain.emulator_table`) says *where EmuDeck installs* each
emulator as a glob relative to the user's home; this adapter performs the disk
probe. It also wraps the ES-DE core-info reader so every system the table
covers answers the ``CoreInfoProvider`` questions (default emulator, picker
options) from the table instead of ``es_systems.xml``; uncovered systems pass
straight through to the wrapped reader.
"""

from __future__ import annotations

import fnmatch
import os
from typing import TYPE_CHECKING, Any

from domain.emulator_commands import option_to_invocation, select_default_option
from domain.emulator_table import covers, table_for, table_options

if TYPE_CHECKING:
    import logging

    from domain.emulator_table import TableEmulator
    from domain.shortcut_data import EmulatorInvocation
    from services.protocols import CoreInfoProvider

_GLOB_CHARS = frozenset("*?[")


def _is_executable_file(path: str) -> bool:
    return os.path.isfile(path) and os.access(path, os.X_OK)


class EmulatorLocatorAdapter:
    """Find table emulators under the user's home.

    The home directory comes from the host (``DECKY_USER_HOME``), falling back
    to ``$HOME``; it is never hardcoded. "Installed" means the located path is
    an executable regular file. Each glob component is matched
    case-insensitively (EmuDeck's ``find -iname``); when several files match,
    the lexicographically first executable one wins, so the pick is stable.
    """

    def __init__(self, *, user_home: str | None, logger: logging.Logger) -> None:
        self._user_home = user_home or os.environ.get("HOME", "")
        self._logger = logger

    def locate(self, entry: TableEmulator) -> str | None:
        """Return *entry*'s executable path, or ``None`` when it is not installed."""
        if not self._user_home:
            return None
        candidates = [self._user_home]
        for component in entry.locate_glob.split("/"):
            if not component:
                continue
            candidates = [match for parent in candidates for match in self._match_component(parent, component)]
            if not candidates:
                return None
        for path in candidates:
            if _is_executable_file(path):
                return path
        return None

    def installed(self, system: str) -> dict[str, str]:
        """Map each installed table emulator id of *system* to its executable path."""
        found: dict[str, str] = {}
        for entry in table_for(system):
            path = self.locate(entry)
            if path is not None:
                found[entry.emulator_id] = path
        return found

    def _match_component(self, parent: str, component: str) -> list[str]:
        try:
            names = os.listdir(parent)
        except OSError:
            return []
        wanted = component.lower()
        if _GLOB_CHARS.isdisjoint(component):
            matches = [name for name in names if name.lower() == wanted]
        else:
            matches = [name for name in names if fnmatch.fnmatchcase(name.lower(), wanted)]
        return [os.path.join(parent, name) for name in sorted(matches)]


class TableAwareCoreInfoAdapter:
    """``CoreInfoProvider`` that answers table-covered systems from the emulator table.

    For a covered system the picker options are the table entries (installed
    ones bakeable, the rest ``not_installed``), the default emulator is the first
    installed entry, and there is no libretro active core. Every other system —
    and the sandbox-launcher / cache seams — delegate to *inner* unchanged.
    """

    def __init__(self, *, inner: CoreInfoProvider, locator: EmulatorLocatorAdapter) -> None:
        self._inner = inner
        self._locator = locator

    def get_active_core(self, system_name: str) -> tuple[str | None, str | None]:
        if covers(system_name):
            return (None, None)
        return self._inner.get_active_core(system_name)

    def get_default_emulator(self, system_name: str) -> EmulatorInvocation | None:
        if covers(system_name):
            return option_to_invocation(select_default_option(self._table_options(system_name)))
        return self._inner.get_default_emulator(system_name)

    def get_emulator_options(self, system_name: str) -> dict[str, Any]:
        if covers(system_name):
            return {"available": True, "options": self._table_options(system_name)}
        return self._inner.get_emulator_options(system_name)

    def resolve_sandbox_launcher(self, command: str) -> str | None:
        return self._inner.resolve_sandbox_launcher(command)

    def reset_cache(self) -> None:
        self._inner.reset_cache()

    def _table_options(self, system_name: str) -> list[Any]:
        return table_options(system_name, self._locator.installed(system_name))
