"""Contract tests for adopting ROM files already on disk (Phase 1.5).

A ROM whose file already sits in the ROM folder exactly where a download would
put it (``roms/<system>/<RomM file name>``) with RomM's exact size counts as
installed: the real ``RomInstall`` row is written and the launch command goes
through the same paths a finished download takes.

Driven frontend-shaped through the real callables over the real wired plugin
(real SQLite, real file adapters under ``tmp_path``); only RomM is the fake.

* Game page: ``getCachedGameDetail = callable<[number], CachedGameDetail>``
  reports ``installed: false`` and starts a background check; on adoption the
  backend emits ``download_complete`` (``adopted: true``) and the next read
  reports ``installed: true``; ``getInstalledRom`` returns the record.
* Library sync: the adopted ROM's shortcut carries its launch command in the
  same sync, and the ``RomInstall`` row is written.
"""

from __future__ import annotations

import asyncio
import os

from ._harness import background_tasks
from ._seed import seed_rom

_APP_ID = 4242


async def _drain_background_tasks(harness) -> None:
    for _ in range(6):
        pending = background_tasks()
        if not pending:
            break
        await asyncio.gather(*pending, return_exceptions=True)
    # Events emitted by the drained work cross the socket; a round-trip makes
    # sure the shim has relayed them to ``harness.emit``.
    await harness.flush_events()


def _server_rom(harness, rom_id: int, *, fs_name: str, size: int, platform_id: int = 1) -> None:
    harness.romm.roms[rom_id] = {
        "id": rom_id,
        "name": f"Game {rom_id}",
        "fs_name": fs_name,
        "fs_size_bytes": size,
        "platform_id": platform_id,
        "platform_slug": "gba",
        "platform_fs_slug": "gba",
        "platform_name": "Game Boy Advance",
        "files": [{"file_name": fs_name, "file_size_bytes": size}],
    }


def _place_file(harness, system: str, name: str, size: int) -> str:
    platform_dir = os.path.join(harness.retrodeck_paths.roms_path(), system)
    os.makedirs(platform_dir, exist_ok=True)
    path = os.path.join(platform_dir, name)
    with open(path, "wb") as f:
        f.write(b"\0" * size)
    return os.path.realpath(path)


def _events(harness, name: str) -> list[dict]:
    return [c.args[1] for c in harness.emit.call_args_list if c.args and c.args[0] == name]


async def test_game_page_adopts_file_already_on_disk(harness):
    seed_rom(harness, 7, platform_slug="gba", shortcut_app_id=_APP_ID)
    _server_rom(harness, 7, fs_name="Pokemon Emerald (USA).gba", size=64)
    path = _place_file(harness, "gba", "Pokemon Emerald (USA).gba", 64)

    first = await harness.plugin.get_cached_game_detail(_APP_ID)
    assert first["found"] is True
    assert first["installed"] is False
    await _drain_background_tasks(harness)

    # Same event a finished download emits — the frontend flips the page to Play
    # and confirm-sets the shortcut's launch options from this payload.
    [event] = _events(harness, "download_complete")
    assert event["rom_id"] == 7
    assert event["adopted"] is True
    assert event["app_id"] == _APP_ID
    assert event["file_path"] == path
    assert path in event["launch_options"]

    again = await harness.plugin.get_cached_game_detail(_APP_ID)
    assert again["installed"] is True
    installed = await harness.plugin.get_installed_rom(7)
    assert installed is not None
    assert installed["file_path"] == path
    assert installed["file_name"] == "Pokemon Emerald (USA).gba"
    with harness.uow_factory() as uow:
        install = uow.rom_installs.get(7)
    assert install is not None
    assert install.rom_dir is None
    assert install.system == "gba"


async def test_game_page_does_not_adopt_wrong_size(harness):
    seed_rom(harness, 7, platform_slug="gba", shortcut_app_id=_APP_ID)
    _server_rom(harness, 7, fs_name="Pokemon Emerald (USA).gba", size=64)
    _place_file(harness, "gba", "Pokemon Emerald (USA).gba", 63)

    await harness.plugin.get_cached_game_detail(_APP_ID)
    await _drain_background_tasks(harness)

    assert _events(harness, "download_complete") == []
    assert (await harness.plugin.get_cached_game_detail(_APP_ID))["installed"] is False
    assert await harness.plugin.get_installed_rom(7) is None


async def test_game_page_server_unreachable_stays_not_installed(harness):
    seed_rom(harness, 7, platform_slug="gba", shortcut_app_id=_APP_ID)
    _place_file(harness, "gba", "Pokemon Emerald (USA).gba", 64)
    harness.romm.get_rom_side_effect = ConnectionError("offline")

    detail = await harness.plugin.get_cached_game_detail(_APP_ID)
    await _drain_background_tasks(harness)

    assert detail["installed"] is False
    assert _events(harness, "download_complete") == []
    assert await harness.plugin.get_installed_rom(7) is None


async def _ack_immediately(_unit, event):
    event.set()
    return {}


async def test_library_sync_adopts_and_bakes_launch_options(harness):
    harness.romm.platforms = [{"id": 1, "name": "Game Boy Advance", "slug": "gba", "rom_count": 2}]
    _server_rom(harness, 10, fs_name="On Disk.gba", size=32)
    _server_rom(harness, 11, fs_name="Not On Disk.gba", size=32)
    harness.backend.settings["enabled_platforms"] = {"1": True}
    path = _place_file(harness, "gba", "On Disk.gba", 32)

    orch = harness.backend._sync_service._orchestrator
    orch._wait_for_unit_complete = _ack_immediately
    # start_sync runs the per-unit pipeline as a background task.
    assert (await harness.plugin.start_sync())["success"] is True
    await _drain_background_tasks(harness)

    [unit] = _events(harness, "sync_apply_unit")
    by_rom = {s["rom_id"]: s for s in unit["shortcuts"]}
    assert path in by_rom[10]["launch_options"]
    assert by_rom[11]["launch_options"] == ""
    with harness.uow_factory() as uow:
        adopted = uow.rom_installs.get(10)
        assert uow.rom_installs.get(11) is None
    assert adopted is not None
    assert adopted.file_path == path
    installed = await harness.plugin.get_installed_rom(10)
    assert installed is not None


# ── Switch: normalized-title fallback ────────────────────────────

_CATHERINE_XCI = "Catherine Full Body [0100BF00112C0000]+[v1.0.1](US-EU).xci"
_FE_XCI = "Fire Emblem Three Houses NSW-HR.xci"
_ZELDA_XCI = "The Legend Of Zelda Echoes Of Wisdom NSW-NiiNTENDO.xci"


def _eden(harness) -> str:
    path = harness.tmp_path / "home" / "Applications" / "Eden.AppImage"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("#!/bin/sh\n")
    path.chmod(0o755)
    return str(path)


def _eden_cmd(exe: str, rom: str) -> str:
    return f'vblank_mode=0 %command% "{exe}" -f -g "{rom}"'


def _server_switch_folder_rom(harness, rom_id: int, fs_name: str) -> None:
    """A RomM Switch game stored as a folder whose contents differ from the copy on disk."""
    harness.romm.roms[rom_id] = {
        "id": rom_id,
        "name": fs_name.replace("_", ":"),
        "fs_path": "switch",
        "fs_name": fs_name,
        "fs_name_no_ext": fs_name,
        "fs_size_bytes": 7985954816,
        "platform_id": 9,
        "platform_slug": "switch",
        "platform_fs_slug": "switch",
        "platform_name": "Nintendo Switch",
        "has_multiple_files": True,
        "files": [{"file_path": f"switch/{fs_name}", "file_name": "base.xci", "file_size_bytes": 7985954816}],
    }


def _switch_dir(harness) -> str:
    return os.path.join(harness.retrodeck_paths.roms_path(), "switch")


def _place_switch(harness, rel: str, size: int = 8) -> str:
    path = os.path.join(_switch_dir(harness), rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(b"\0" * size)
    return path


async def test_game_page_adopts_switch_folder_by_title(harness):
    exe = _eden(harness)
    seed_rom(harness, 21, platform_slug="switch", shortcut_app_id=_APP_ID)
    _server_switch_folder_rom(harness, 21, "Fire Emblem_ Three Houses")
    launch = _place_switch(harness, f"Fire Emblem Three Houses/{_FE_XCI}")
    rom_dir = os.path.join(_switch_dir(harness), "Fire Emblem Three Houses")

    assert (await harness.plugin.get_cached_game_detail(_APP_ID))["installed"] is False
    await _drain_background_tasks(harness)

    [event] = _events(harness, "download_complete")
    assert event["rom_id"] == 21
    assert event["adopted"] is True
    assert event["file_path"] == launch
    assert event["launch_options"] == _eden_cmd(exe, launch)
    assert (await harness.plugin.get_cached_game_detail(_APP_ID))["installed"] is True
    installed = await harness.plugin.get_installed_rom(21)
    assert installed is not None
    assert installed["file_path"] == launch
    with harness.uow_factory() as uow:
        install = uow.rom_installs.get(21)
    assert install is not None
    assert install.rom_dir == rom_dir
    assert install.system == "switch"
    assert (await harness.plugin.get_rom_relaunch_options(21))["launch_options"] == _eden_cmd(exe, launch)


async def test_library_sync_adopts_switch_copies_by_title(harness):
    exe = _eden(harness)
    harness.romm.platforms = [{"id": 9, "name": "Nintendo Switch", "slug": "switch", "rom_count": 4}]
    _server_switch_folder_rom(harness, 31, "Catherine_ Full Body")
    _server_switch_folder_rom(harness, 32, "The Legend of Zelda_ Echoes of Wisdom")
    _server_switch_folder_rom(harness, 33, "Super Mario Galaxy")
    _server_switch_folder_rom(harness, 34, "Super Mario Galaxy 2")
    # A re-sync of an already-synced library (the on-device case): the table
    # invocation is resolved from the existing ROM rows.
    for rom_id in (31, 32, 33, 34):
        seed_rom(harness, rom_id, platform_slug="switch")
    harness.backend.settings["enabled_platforms"] = {"9": True}
    catherine = _place_switch(harness, f"Catherine Full Body/{_CATHERINE_XCI}")
    zelda = _place_switch(harness, _ZELDA_XCI)
    # Not in RomM: never adopted, never a crash.
    _place_switch(harness, "Pokemon Scarlet/Pokemon Scarlet NSW-NiiNTENDO.xci")
    _place_switch(harness, "Pokemon Shining Pearl [010018E011D92000][v0].nsp")
    _place_switch(harness, "metadata.txt")

    orch = harness.backend._sync_service._orchestrator
    orch._wait_for_unit_complete = _ack_immediately
    assert (await harness.plugin.start_sync())["success"] is True
    await _drain_background_tasks(harness)

    [unit] = _events(harness, "sync_apply_unit")
    by_rom = {s["rom_id"]: s for s in unit["shortcuts"]}
    assert by_rom[31]["launch_options"] == _eden_cmd(exe, catherine)
    assert by_rom[32]["launch_options"] == _eden_cmd(exe, zelda)
    assert by_rom[33]["launch_options"] == ""
    assert by_rom[34]["launch_options"] == ""
    with harness.uow_factory() as uow:
        cat = uow.rom_installs.get(31)
        zel = uow.rom_installs.get(32)
        assert uow.rom_installs.get(33) is None
        assert uow.rom_installs.get(34) is None
    assert cat is not None
    assert cat.rom_dir == os.path.join(_switch_dir(harness), "Catherine Full Body")
    assert cat.file_path == catherine
    assert zel is not None
    assert zel.rom_dir is None
    assert zel.file_path == zelda


async def test_first_sync_bakes_table_emulator_for_switch_copy(harness):
    """Fresh install: no ROM rows exist before the first sync, yet the adopted
    Switch copy must get the full emulator command in that same sync."""
    exe = _eden(harness)
    harness.romm.platforms = [{"id": 9, "name": "Nintendo Switch", "slug": "switch", "rom_count": 1}]
    _server_switch_folder_rom(harness, 41, "Catherine_ Full Body")
    harness.backend.settings["enabled_platforms"] = {"9": True}
    catherine = _place_switch(harness, f"Catherine Full Body/{_CATHERINE_XCI}")
    with harness.uow_factory() as uow:
        assert uow.roms.get(41) is None

    orch = harness.backend._sync_service._orchestrator
    orch._wait_for_unit_complete = _ack_immediately
    assert (await harness.plugin.start_sync())["success"] is True
    await _drain_background_tasks(harness)

    [unit] = _events(harness, "sync_apply_unit")
    [shortcut] = [s for s in unit["shortcuts"] if s["rom_id"] == 41]
    assert shortcut["launch_options"] == _eden_cmd(exe, catherine)
    with harness.uow_factory() as uow:
        install = uow.rom_installs.get(41)
    assert install is not None
    assert install.file_path == catherine
