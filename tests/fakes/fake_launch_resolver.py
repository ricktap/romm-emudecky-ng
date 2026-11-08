"""In-memory ``LaunchResolverFn`` implementation for service tests.

The production resolver is
:class:`adapters.emudeck_launchers.EmuDeckLauncherAdapter`; service tests
that don't exercise the host-launch seam use this fake to control the
resolved ``launch_options`` per (rom, emulator, system) tuple.

The default behaviour (no ``responses`` override) emulates the old
``resolve_emulator_invocation`` logic so existing service-test assertions
that expect ``flatpak run net.retrodeck.retrodeck`` continue to pass
unchanged during the EmuDeck port transition. Tests that need a different
invocation string set ``responses`` or override ``launch_options``.
"""

from __future__ import annotations

import re

from domain.emudeck_launch import (
    RESOLUTION_OK,
    EmuDeckLaunchResolution,
)

# The leading ``%EMULATOR_<NAME>%`` binary token — stripped from a standalone
# command to recover the middle launcher args for the ``direct`` form.
_EMULATOR_TOKEN_RE = re.compile(r"%EMULATOR_[A-Z0-9_-]+%")
_RETRODECK_APP_ID = "net.retrodeck.retrodeck"
_RETRODECK_INVOCATION = f"flatpak run {_RETRODECK_APP_ID}"
_RETROARCH_CORES_DIR = "/var/config/retroarch/cores"


class FakeLaunchResolver:
    """Programmable ``LaunchResolverFn`` for tests.

    Defaults to producing an ``ok`` resolution with ``launch_options``
    matching the old ``resolve_emulator_invocation`` behaviour (tests that
    don't care about launch behavior keep their assertions). Set
    ``launch_options`` to a non-empty string to override the default
    per-emulator resolution for the ``no emulator`` / ``no response match``
    case. Per-tuple overrides come via the ``responses`` map keyed by
    ``(rom_id, system)``; a wildcard key of ``None`` matches every tuple
    when no specific entry is found.
    """

    def __init__(
        self,
        *,
        launch_options: str = "",
        status: str = RESOLUTION_OK,
        responses: dict[tuple[int | None, str | None], EmuDeckLaunchResolution] | None = None,
    ) -> None:
        self._default_options = launch_options
        self._default_status = status
        self._responses = dict(responses or {})

    def _emulate_old_resolver(self, emulator) -> str:
        """Replicate the old ``resolve_emulator_invocation`` logic for test compatibility.

        Produces ``flatpak run net.retrodeck.retrodeck`` (with optional
        ``-e`` override) so existing service-test assertions continue to
        pass during the EmuDeck port transition.
        """
        if emulator is None:
            return _RETRODECK_INVOCATION
        if emulator.kind in ("direct",) and emulator.launcher and emulator.command:
            stripped = _EMULATOR_TOKEN_RE.sub("", emulator.command).replace("%ROM%", "")
            mid_args = " ".join(stripped.split())
            base = f"flatpak run --command={emulator.launcher} {_RETRODECK_APP_ID}"
            return f"{base} {mid_args}" if mid_args else base
        if emulator.kind == "standalone" and emulator.command:
            return f'{_RETRODECK_INVOCATION} -e "{emulator.command}"'
        if emulator.kind == "libretro" and emulator.core_so:
            core_path = f"{_RETROARCH_CORES_DIR}/{emulator.core_so}.so"
            return f'{_RETRODECK_INVOCATION} -e "%EMULATOR_RETROARCH% -L {core_path} %ROM%"'
        return _RETRODECK_INVOCATION

    def resolve_for(self, rom, emulator, system):
        rom_id = rom.get("id") if isinstance(rom, dict) else None
        for key, res in self._responses.items():
            wildcard_id, wildcard_system = key
            id_match = wildcard_id is None or wildcard_id == rom_id
            sys_match = wildcard_system is None or wildcard_system == system
            if id_match and sys_match:
                return res
        if self._default_options:
            return EmuDeckLaunchResolution(
                status=self._default_status,
                launch_options=self._default_options,
            )
        return EmuDeckLaunchResolution(
            status=RESOLUTION_OK,
            launch_options=self._emulate_old_resolver(emulator),
        )
