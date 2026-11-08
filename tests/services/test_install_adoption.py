"""Tests for InstallAdoptionService — adopting ROM files already on disk.

Real files under ``tmp_path`` through the real ``DownloadFileAdapter`` (the
probe's realpath containment needs a real filesystem); relational state in the
in-memory fake Unit of Work.
"""

from __future__ import annotations

import asyncio
import logging
from datetime import UTC, datetime
from unittest.mock import AsyncMock, MagicMock

import pytest
from fakes.fake_disc_resolver import FakeDiscResolver
from fakes.fake_retrodeck_paths import FakeRetroDeckPaths
from fakes.fake_romm_api import FakeRommApi
from fakes.fake_unit_of_work import FakeUnitOfWorkFactory
from fakes.system_time import FakeClock

from adapters.download_file import DownloadFileAdapter
from domain.download_job import DownloadJob
from domain.rom import Rom
from domain.rom_install import RomInstall
from services.install_adoption import InstallAdoptionService, InstallAdoptionServiceConfig

_GROUP = "igdb:100:1"


class _Env:
    def __init__(self, tmp_path, loop):
        self.roms_root = tmp_path / "roms"
        self.roms_root.mkdir()
        self.factory = FakeUnitOfWorkFactory()
        self.uow = self.factory.uow
        self.romm = FakeRommApi()
        self.active: set[int] = set()
        self.emit = AsyncMock()
        self.relaunch = MagicMock()
        self.relaunch.relaunch_item_for_rom.return_value = {"app_id": 4242, "launch_options": "LAUNCH"}
        self.clock = FakeClock(now=datetime(2026, 11, 8, tzinfo=UTC))
        self.service = InstallAdoptionService(
            config=InstallAdoptionServiceConfig(
                romm_api=self.romm,
                file_reader=DownloadFileAdapter(),
                resolve_system=lambda slug, fs_slug=None: slug,
                retrodeck_paths=FakeRetroDeckPaths(roms=str(self.roms_root)),
                m3u_support=lambda system: True,
                disc_resolver=FakeDiscResolver(),
                relaunch_items=self.relaunch,
                active_downloads=lambda: set(self.active),
                uow_factory=self.factory,
                loop=loop,
                logger=logging.getLogger("test_install_adoption"),
                clock=self.clock,
                emit=self.emit,
            )
        )

    def seed_rom(self, rom_id, *, group=None, app_id=None, platform="gba"):
        with self.factory() as uow:
            uow.roms.save(
                Rom.synced(
                    rom_id=rom_id,
                    platform_slug=platform,
                    name=f"rom-{rom_id}",
                    fs_name=f"rom-{rom_id}",
                    shortcut_app_id=app_id,
                    synced_at="2026-01-01T00:00:00",
                    sibling_group_key=group,
                )
            )

    def seed_install(self, rom_id, path="/elsewhere/x.gba"):
        with self.factory() as uow:
            uow.rom_installs.save(
                RomInstall.mark_installed(
                    rom_id=rom_id, file_path=path, rom_dir=None, platform_slug="gba", system="gba", installed_at="t"
                )
            )

    def seed_job(self, rom_id):
        with self.factory() as uow:
            uow.download_jobs.save(
                DownloadJob.enqueue(
                    rom_id=rom_id,
                    rom_name="x",
                    platform_name="GBA",
                    file_name="x.gba",
                    target_path="/r/x.gba",
                    tmp_path="/r/x.gba.tmp",
                    is_multi=False,
                    position=1,
                    now="t",
                )
            )

    def put(self, rel, size):
        path = self.roms_root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"x" * size)
        return path


def _single(rom_id=1, *, fs_name="game.gba", size=8, platform="gba", **over):
    rom = {
        "id": rom_id,
        "name": f"Game {rom_id}",
        "fs_name": fs_name,
        "fs_size_bytes": size,
        "platform_slug": platform,
        "platform_name": "GBA",
        "files": [{"file_name": fs_name}],
    }
    rom.update(over)
    return rom


def _multi(rom_id=2, files=(("FF7.cue", 3), ("FF7.bin", 9))):
    root = "psx/Final Fantasy VII (USA)"
    return {
        "id": rom_id,
        "name": "Final Fantasy VII",
        "fs_path": "psx",
        "fs_name": "Final Fantasy VII (USA)",
        "fs_name_no_ext": "Final Fantasy VII (USA)",
        "platform_slug": "psx",
        "has_multiple_files": True,
        "files": [
            {
                "file_path": f"{root}/{rel}".rpartition("/")[0],
                "file_name": rel.rpartition("/")[2],
                "file_size_bytes": size,
            }
            for rel, size in files
        ],
    }


@pytest.fixture
async def env(tmp_path):
    # Async so the service binds the test's running loop (run_in_executor/create_task).
    return _Env(tmp_path, asyncio.get_running_loop())


# ── plan_adoptions ───────────────────────────────────────────────


class TestPlanSingleFile:
    def test_matching_file_is_planned_with_download_install_record(self, env):
        path = env.put("gba/game.gba", 8)
        env.seed_rom(1)
        [cand] = env.service.plan_adoptions([_single()])
        assert cand.rom_id == 1
        assert cand.install.file_path == str(path.resolve())
        assert cand.install.rom_dir is None
        assert cand.install.system == "gba"
        assert cand.install.platform_slug == "gba"
        assert cand.install.installed_at == "2026-11-08T00:00:00+00:00"
        assert cand.bake_path == cand.install.file_path

    def test_size_mismatch_is_not_planned(self, env):
        env.put("gba/game.gba", 7)
        assert env.service.plan_adoptions([_single()]) == []

    def test_missing_file_is_not_planned(self, env):
        assert env.service.plan_adoptions([_single()]) == []

    def test_missing_romm_size_is_not_planned(self, env):
        env.put("gba/game.gba", 8)
        assert env.service.plan_adoptions([_single(size=None)]) == []

    def test_renamed_copy_is_not_planned(self, env):
        env.put("gba/Game (renamed).gba", 8)
        assert env.service.plan_adoptions([_single()]) == []

    def test_directory_at_target_is_not_planned(self, env):
        (env.roms_root / "gba" / "game.gba").mkdir(parents=True)
        assert env.service.plan_adoptions([_single()]) == []

    def test_unsafe_platform_slug_is_rejected(self, env, tmp_path):
        (tmp_path / "game.gba").write_bytes(b"x" * 8)
        assert env.service.plan_adoptions([_single(platform="..")]) == []

    def test_traversal_fs_name_only_checks_the_download_target(self, env, tmp_path):
        # The download would write "../game.gba" to roms/gba/game.gba (basename);
        # a file outside the platform dir is never matched.
        (env.roms_root / "game.gba").write_bytes(b"x" * 8)
        assert env.service.plan_adoptions([_single(fs_name="../game.gba")]) == []

    def test_symlink_escaping_roms_dir_is_rejected(self, env, tmp_path):
        outside = tmp_path / "outside.gba"
        outside.write_bytes(b"x" * 8)
        (env.roms_root / "gba").mkdir()
        (env.roms_root / "gba" / "game.gba").symlink_to(outside)
        assert env.service.plan_adoptions([_single()]) == []


class TestPlanGuards:
    def test_existing_install_blocks(self, env):
        env.put("gba/game.gba", 8)
        env.seed_rom(1)
        env.seed_install(1)
        assert env.service.plan_adoptions([_single()]) == []

    def test_unfinished_download_job_blocks(self, env):
        env.put("gba/game.gba", 8)
        env.seed_rom(1)
        env.seed_job(1)
        assert env.service.plan_adoptions([_single()]) == []

    def test_active_download_blocks(self, env):
        env.put("gba/game.gba", 8)
        env.active.add(1)
        assert env.service.plan_adoptions([_single()]) == []

    def test_installed_sibling_blocks(self, env):
        env.put("gba/game.gba", 8)
        env.seed_rom(1, group=_GROUP)
        env.seed_rom(5, group=_GROUP)
        env.seed_install(5)
        assert env.service.plan_adoptions([_single()]) == []

    def test_installed_sibling_blocks_via_stamped_group_key(self, env):
        env.put("gba/game.gba", 8)
        env.seed_rom(5, group=_GROUP)
        env.seed_install(5)
        assert env.service.plan_adoptions([_single(sibling_group_key=_GROUP)]) == []

    def test_two_siblings_on_disk_adopts_only_one(self, env):
        env.put("gba/a.gba", 8)
        env.put("gba/b.gba", 8)
        roms = [
            _single(3, fs_name="b.gba", sibling_group_key=_GROUP),
            _single(2, fs_name="a.gba", sibling_group_key=_GROUP),
        ]
        assert [c.rom_id for c in env.service.plan_adoptions(roms)] == [2]

    def test_empty_input(self, env):
        assert env.service.plan_adoptions([]) == []


class TestPlanMultiFile:
    def test_all_files_present_in_extract_dir(self, env):
        d = env.roms_root / "psx" / "Final Fantasy VII (USA)"
        env.put("psx/Final Fantasy VII (USA)/FF7.cue", 3)
        env.put("psx/Final Fantasy VII (USA)/FF7.bin", 9)
        [cand] = env.service.plan_adoptions([_multi()])
        assert cand.install.rom_dir == str(d.resolve())
        assert cand.install.file_path == str((d / "FF7.cue").resolve())  # CUE preferred, like the download

    def test_es_de_collapsed_dir_is_found(self, env):
        env.put("psx/FF7.cue/FF7.cue", 3)
        env.put("psx/FF7.cue/FF7.bin", 9)
        [cand] = env.service.plan_adoptions([_multi()])
        assert cand.install.rom_dir is not None
        assert cand.install.rom_dir.endswith("/psx/FF7.cue")

    def test_one_file_missing_is_not_planned(self, env):
        env.put("psx/Final Fantasy VII (USA)/FF7.cue", 3)
        assert env.service.plan_adoptions([_multi()]) == []

    def test_one_file_wrong_size_is_not_planned(self, env):
        env.put("psx/Final Fantasy VII (USA)/FF7.cue", 3)
        env.put("psx/Final Fantasy VII (USA)/FF7.bin", 8)
        assert env.service.plan_adoptions([_multi()]) == []

    def test_nested_files(self, env):
        rom = _multi(files=(("base.nsp", 4), ("update/patch.nsp", 5)))
        env.put("psx/Final Fantasy VII (USA)/base.nsp", 4)
        env.put("psx/Final Fantasy VII (USA)/update/patch.nsp", 5)
        [cand] = env.service.plan_adoptions([rom])
        assert cand.install.rom_dir is not None


# ── commit_adoptions ─────────────────────────────────────────────


class TestCommit:
    def test_writes_rom_install(self, env):
        env.put("gba/game.gba", 8)
        env.seed_rom(1)
        cands = env.service.plan_adoptions([_single()])
        assert env.service.commit_adoptions(cands) == [1]
        with env.factory() as uow:
            install = uow.rom_installs.get(1)
        assert install == cands[0].install

    def test_skips_rom_without_row(self, env):
        env.put("gba/game.gba", 8)
        cands = env.service.plan_adoptions([_single()])
        assert len(cands) == 1
        assert env.service.commit_adoptions(cands) == []

    def test_skips_when_a_download_landed_since_planning(self, env):
        env.put("gba/game.gba", 8)
        env.seed_rom(1)
        cands = env.service.plan_adoptions([_single()])
        env.seed_install(1, path="/downloaded.gba")
        assert env.service.commit_adoptions(cands) == []
        with env.factory() as uow:
            assert uow.rom_installs.get(1).file_path == "/downloaded.gba"

    def test_skips_when_a_job_appeared_since_planning(self, env):
        env.put("gba/game.gba", 8)
        env.seed_rom(1)
        cands = env.service.plan_adoptions([_single()])
        env.seed_job(1)
        assert env.service.commit_adoptions(cands) == []

    def test_empty(self, env):
        assert env.service.commit_adoptions([]) == []


# ── adopt_rom (game page) ────────────────────────────────────────


class TestAdoptRom:
    @pytest.mark.asyncio
    async def test_adopts_and_emits_download_complete(self, env):
        path = env.put("gba/game.gba", 8)
        env.seed_rom(1, app_id=4242)
        env.romm.roms[1] = _single()

        assert await env.service.adopt_rom(1) is True

        with env.factory() as uow:
            install = uow.rom_installs.get(1)
        assert install is not None
        assert install.file_path == str(path.resolve())
        env.relaunch.relaunch_item_for_rom.assert_called_once_with(1)
        env.emit.assert_awaited_once_with(
            "download_complete",
            {
                "rom_id": 1,
                "rom_name": "Game 1",
                "platform_name": "GBA",
                "file_path": str(path.resolve()),
                "app_id": 4242,
                "launch_options": "LAUNCH",
                "resumable": False,
                "adopted": True,
            },
        )

    @pytest.mark.asyncio
    async def test_unbound_rom_emits_null_app_id(self, env):
        env.put("gba/game.gba", 8)
        env.seed_rom(1)
        env.romm.roms[1] = _single()
        env.relaunch.relaunch_item_for_rom.return_value = None
        assert await env.service.adopt_rom(1) is True
        payload = env.emit.await_args.args[1]
        assert payload["app_id"] is None
        assert payload["launch_options"] == ""

    @pytest.mark.asyncio
    async def test_existing_install_skips_without_network(self, env):
        env.put("gba/game.gba", 8)
        env.seed_rom(1)
        env.seed_install(1)
        env.romm.roms[1] = _single()
        assert await env.service.adopt_rom(1) is False
        assert not [c for c in env.romm.call_log if c[0] == "get_rom"]
        env.emit.assert_not_awaited()

    @pytest.mark.asyncio
    async def test_unfinished_download_job_skips(self, env):
        env.put("gba/game.gba", 8)
        env.seed_rom(1)
        env.seed_job(1)
        env.romm.roms[1] = _single()
        assert await env.service.adopt_rom(1) is False
        with env.factory() as uow:
            assert uow.rom_installs.get(1) is None

    @pytest.mark.asyncio
    async def test_active_download_skips(self, env):
        env.put("gba/game.gba", 8)
        env.seed_rom(1)
        env.active.add(1)
        env.romm.roms[1] = _single()
        assert await env.service.adopt_rom(1) is False

    @pytest.mark.asyncio
    async def test_installed_sibling_skips(self, env):
        env.put("gba/game.gba", 8)
        env.seed_rom(1, group=_GROUP)
        env.seed_rom(5, group=_GROUP)
        env.seed_install(5)
        env.romm.roms[1] = _single()
        assert await env.service.adopt_rom(1) is False
        with env.factory() as uow:
            assert uow.rom_installs.get(1) is None

    @pytest.mark.asyncio
    async def test_unsynced_rom_skips(self, env):
        env.put("gba/game.gba", 8)
        env.romm.roms[1] = _single()
        assert await env.service.adopt_rom(1) is False

    @pytest.mark.asyncio
    async def test_no_match_returns_false(self, env):
        env.put("gba/game.gba", 5)
        env.seed_rom(1)
        env.romm.roms[1] = _single()
        assert await env.service.adopt_rom(1) is False
        env.emit.assert_not_awaited()

    @pytest.mark.asyncio
    async def test_server_failure_returns_false_without_raising(self, env):
        env.put("gba/game.gba", 8)
        env.seed_rom(1)
        env.romm.get_rom_side_effect = ConnectionError("offline")
        assert await env.service.adopt_rom(1) is False
        env.emit.assert_not_awaited()


class TestScheduleAdoptionCheck:
    @pytest.mark.asyncio
    async def test_runs_in_background_and_dedupes(self, env):
        env.put("gba/game.gba", 8)
        env.seed_rom(1)
        env.romm.roms[1] = _single()

        env.service.schedule_adoption_check(1)
        env.service.schedule_adoption_check(1)  # in flight → ignored
        tasks = list(env.service._in_flight.values())
        assert len(tasks) == 1
        await asyncio.gather(*tasks)

        assert [c[0] for c in env.romm.call_log].count("get_rom") == 1
        with env.factory() as uow:
            assert uow.rom_installs.get(1) is not None

    @pytest.mark.asyncio
    async def test_cooldown_then_rechecks(self, env):
        env.seed_rom(1)
        env.romm.roms[1] = _single()
        env.service.schedule_adoption_check(1)
        await asyncio.gather(*env.service._in_flight.values())
        env.service.schedule_adoption_check(1)  # within cooldown
        assert env.service._in_flight == {}
        env.clock.advance(31)
        env.service.schedule_adoption_check(1)
        assert len(env.service._in_flight) == 1
        await asyncio.gather(*env.service._in_flight.values())
        assert [c[0] for c in env.romm.call_log].count("get_rom") == 2
