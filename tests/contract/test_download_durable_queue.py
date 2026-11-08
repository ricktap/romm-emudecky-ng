"""Contract test for the durable download queue surviving a plugin restart.

Driven frontend-shaped (``startDownload`` / ``getDownloadQueue`` /
``getInstalledRom``). A download is started, its transfer writes part of the
file and the plugin is torn down mid-transfer. A NEW ``Plugin`` is then built
over the same ``tmp_path`` (same real SQLite + the same partial on disk), its
startup reconciliation runs, and the job must reappear in ``get_download_queue``
and resume from the existing partial — the Range offset equals the partial's
size — ending installed.
"""

from __future__ import annotations

import asyncio
import os
import threading
from typing import Any

from tests.contract._harness import build_contract_harness

from ._seed import seed_rom

_ROM_ID = 7
_FS_NAME = "metroid.gba"
_PAYLOAD = bytes(i % 251 for i in range(4096))
_PARTIAL = 1500


def _seed_server_rom(harness) -> None:
    harness.romm.roms[_ROM_ID] = {
        "id": _ROM_ID,
        "name": "Metroid",
        "fs_name": _FS_NAME,
        "fs_size_bytes": len(_PAYLOAD),
        "platform_slug": "gba",
        "platform_fs_slug": "gba",
        "platform_name": "GBA",
    }


async def _drain(service) -> None:
    for _ in range(20):
        pending = list(service._download_tasks.values())
        auto = service._auto_resume_task
        if auto is not None and not auto.done():
            pending.append(auto)
        if not pending:
            return
        await asyncio.gather(*pending, return_exceptions=True)


async def test_interrupted_download_resumes_from_partial_after_restart(harness, tmp_path):
    seed_rom(harness, _ROM_ID, platform_slug="gba")
    _seed_server_rom(harness)

    # First plugin instance: the transfer writes a partial, then stalls until the
    # plugin goes away (the progress callback aborts it on shutdown).
    stalled = threading.Event()
    release = threading.Event()

    def stalling_transfer(rom_id, filename, dest, progress_callback=None, *, resume=False, on_meta=None):
        if on_meta is not None:
            on_meta(True)
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        with open(dest, "wb") as f:
            f.write(_PAYLOAD[:_PARTIAL])
        progress_callback(_PARTIAL, len(_PAYLOAD))
        stalled.set()
        release.wait(5)
        progress_callback(_PARTIAL, len(_PAYLOAD))  # raises once the plugin is shutting down

    harness.romm.download_rom_content = stalling_transfer  # type: ignore[method-assign]

    result = await harness.plugin.start_download(_ROM_ID)
    assert result == {"success": True, "message": "Download started"}
    while not stalled.is_set():
        await asyncio.sleep(0.01)

    first_service = harness.backend._download_service
    tasks = list(first_service._download_tasks.values())
    await first_service.shutdown()  # what _unload does
    release.set()
    await asyncio.gather(*tasks, return_exceptions=True)

    tmp_file = next(
        os.path.join(root, name) for root, _dirs, files in os.walk(tmp_path) for name in files if name.endswith(".tmp")
    )
    assert os.path.getsize(tmp_file) == _PARTIAL

    # Second plugin instance over the SAME tmp_path: real SQLite + the partial.
    restarted = await build_contract_harness(tmp_path)
    _seed_server_rom(restarted)
    calls: list[dict[str, Any]] = []

    def ranged_transfer(rom_id, filename, dest, progress_callback=None, *, resume=False, on_meta=None):
        offset = os.path.getsize(dest) if resume and os.path.exists(dest) else 0
        calls.append({"rom_id": rom_id, "resume": resume, "offset": offset})
        if on_meta is not None:
            on_meta(True)
        with open(dest, "ab" if offset else "wb") as f:
            f.write(_PAYLOAD[offset:])
        if progress_callback is not None:
            progress_callback(len(_PAYLOAD), len(_PAYLOAD))

    restarted.romm.download_rom_content = ranged_transfer  # type: ignore[method-assign]

    # Startup reconciliation (main._main) restores the queue before anything resumes.
    # Read the reconciled state synchronously, before the auto-resume task it
    # schedules gets a turn on the loop (that resume is asserted below).
    restarted.backend._download_service.reconcile_persisted_downloads()
    queue = restarted.backend._download_service.get_download_queue()
    entry = next(d for d in queue["downloads"] if d["rom_id"] == _ROM_ID)
    assert entry["status"] == "interrupted"
    assert entry["reason"] == "restart"
    assert entry["bytes_downloaded"] == _PARTIAL
    assert entry["total_bytes"] == len(_PAYLOAD)

    await _drain(restarted.backend._download_service)

    assert calls == [{"rom_id": _ROM_ID, "resume": True, "offset": _PARTIAL}]
    installed = await restarted.plugin.get_installed_rom(_ROM_ID)
    assert installed is not None
    with open(installed["file_path"], "rb") as f:
        assert f.read() == _PAYLOAD
    assert not os.path.exists(tmp_file)

    queue = await restarted.plugin.get_download_queue()
    entry = next(d for d in queue["downloads"] if d["rom_id"] == _ROM_ID)
    assert entry["status"] == "completed"
    with restarted.uow_factory() as uow:
        assert uow.download_jobs.get(_ROM_ID) is None
    await restarted.close()
