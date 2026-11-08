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
