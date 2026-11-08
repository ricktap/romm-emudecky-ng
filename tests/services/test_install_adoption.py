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
    def __init__(self, tmp_path, loop, *, roms_root=None):
        self.roms_root = roms_root or tmp_path / "roms"
        self.roms_root.mkdir(exist_ok=True)
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

    def seed_rom(self, rom_id, *, group=None, app_id=None, platform="gba", fs_name=None):
        with self.factory() as uow:
            uow.roms.save(
                Rom.synced(
                    rom_id=rom_id,
                    platform_slug=platform,
                    name=f"rom-{rom_id}",
                    fs_name=fs_name or f"rom-{rom_id}",
                    shortcut_app_id=app_id,
                    synced_at="2026-01-01T00:00:00",
                    sibling_group_key=group,
                )
            )

    def seed_install(self, rom_id, path="/elsewhere/x.gba", *, rom_dir=None, system="gba"):
        with self.factory() as uow:
            uow.rom_installs.save(
                RomInstall.mark_installed(
                    rom_id=rom_id,
                    file_path=path,
                    rom_dir=rom_dir,
                    platform_slug=system,
                    system=system,
                    installed_at="t",
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


# ── Path consistency (symlinked roms root) ───────────────────────


@pytest.fixture
async def linked_env(tmp_path):
    # SteamOS: the configured root /run/media/mmcblk0p1/... is a symlink to the
    # SD card's mount /run/media/deck/<uuid>/...
    (tmp_path / "deck" / "uuid" / "roms").mkdir(parents=True)
    (tmp_path / "mmcblk0p1").symlink_to(tmp_path / "deck" / "uuid")
    return _Env(tmp_path, asyncio.get_running_loop(), roms_root=tmp_path / "mmcblk0p1" / "roms")


class TestRecordedPathUsesConfiguredRoot:
    def test_exact_single_file_keeps_configured_root(self, linked_env):
        linked_env.put("gba/game.gba", 8)
        [cand] = linked_env.service.plan_adoptions([_single()])
        assert cand.install.file_path == str(linked_env.roms_root / "gba" / "game.gba")
        assert "/deck/uuid/" not in cand.install.file_path

    def test_exact_multi_file_keeps_configured_root(self, linked_env):
        linked_env.put("psx/Final Fantasy VII (USA)/FF7.cue", 3)
        linked_env.put("psx/Final Fantasy VII (USA)/FF7.bin", 9)
        [cand] = linked_env.service.plan_adoptions([_multi()])
        d = linked_env.roms_root / "psx" / "Final Fantasy VII (USA)"
        assert cand.install.rom_dir == str(d)
        assert cand.install.file_path == str(d / "FF7.cue")

    def test_title_match_keeps_configured_root(self, linked_env):
        linked_env.put("switch/Catherine Full Body/Catherine Full Body [0100BF00112C0000]+[v1.0.1](US-EU).xci", 4)
        [cand] = linked_env.service.plan_adoptions([_switch(1, "Catherine_ Full Body")])
        d = linked_env.roms_root / "switch" / "Catherine Full Body"
        assert cand.install.rom_dir == str(d)
        assert cand.install.file_path == str(d / "Catherine Full Body [0100BF00112C0000]+[v1.0.1](US-EU).xci")


# ── Normalized-title fallback (Switch) ───────────────────────────


def _switch(rom_id, fs_name, *, inner=("game.xci", 100), **over):
    """A RomM Switch ROM stored as a folder (never matches the exact rule on its own)."""
    root = f"switch/{fs_name}"
    rom = {
        "id": rom_id,
        "name": fs_name.replace("_", ":"),
        "fs_path": "switch",
        "fs_name": fs_name,
        "fs_name_no_ext": fs_name,
        "platform_slug": "switch",
        "platform_name": "Nintendo Switch",
        "has_multiple_files": True,
        "files": [{"file_path": root, "file_name": inner[0], "file_size_bytes": inner[1]}],
    }
    rom.update(over)
    return rom


_CATHERINE_XCI = "Catherine Full Body [0100BF00112C0000]+[v1.0.1](US-EU).xci"
_FE_XCI = "Fire Emblem Three Houses NSW-HR.xci"
_ZELDA_XCI = "The Legend Of Zelda Echoes Of Wisdom NSW-NiiNTENDO.xci"
_MK8_XCI = "Mario Kart 8 Deluxe [0100152000022000][v0].xci"

_ROMM_SWITCH = [
    (1, "Catherine_ Full Body"),
    (2, "Fire Emblem_ Three Houses"),
    (3, "The Legend of Zelda_ Echoes of Wisdom"),
    (4, "Mario Kart 8 Deluxe"),
    (5, "Super Mario Galaxy"),
    (6, "Super Mario Galaxy 2"),
    (7, "Donkey Kong Country Returns HD_ Version 1.1.0"),
    (8, "Pokémon Legends_ Z-A"),
]


def _real_switch_folder(env):
    """The roms/switch/ layout observed on the Deck (tiny files; sizes irrelevant)."""
    env.put(f"switch/Catherine Full Body/{_CATHERINE_XCI}", 4)
    env.put(f"switch/Fire Emblem Three Houses/{_FE_XCI}", 4)
    env.put(f"switch/{_ZELDA_XCI}", 4)
    env.put(f"switch/Mario Kart 8 Deluxe/{_MK8_XCI}", 4)
    env.put("switch/Pokemon Scarlet/Pokemon Scarlet NSW-NiiNTENDO.xci", 4)
    env.put("switch/Pokemon Shining Pearl [010018E011D92000][v0].nsp", 4)
    env.put("switch/Super Mario Bros Wonder NSW-LiGHTFORCE.xci", 4)
    (env.roms_root / "switch" / "Xenoblade Chronicles 3").mkdir()
    env.put("switch/metadata.txt", 4)
    env.put("switch/systeminfo.txt", 4)


class TestTitleFallback:
    def _plan(self, env):
        return {c.rom_id: c for c in env.service.plan_adoptions([_switch(i, n) for i, n in _ROMM_SWITCH])}

    def test_real_device_layout(self, env):
        _real_switch_folder(env)
        sw = env.roms_root / "switch"
        by_id = self._plan(env)
        assert sorted(by_id) == [1, 2, 3, 4]
        assert by_id[1].install.rom_dir == str(sw / "Catherine Full Body")
        assert by_id[1].install.file_path == str(sw / "Catherine Full Body" / _CATHERINE_XCI)
        assert by_id[2].install.rom_dir == str(sw / "Fire Emblem Three Houses")
        assert by_id[2].install.file_path == str(sw / "Fire Emblem Three Houses" / _FE_XCI)
        assert by_id[3].install.rom_dir is None
        assert by_id[3].install.file_path == str(sw / _ZELDA_XCI)
        assert by_id[4].install.rom_dir == str(sw / "Mario Kart 8 Deluxe")
        assert by_id[4].install.file_path == str(sw / "Mario Kart 8 Deluxe" / _MK8_XCI)
        for cand in by_id.values():
            assert cand.install.system == "switch"
            assert cand.install.platform_slug == "switch"
            assert cand.bake_path == cand.install.file_path

    def test_folder_launch_file_is_largest_switch_file(self, env):
        env.put("switch/Mario Kart 8 Deluxe/update.nsp", 50)
        env.put(f"switch/Mario Kart 8 Deluxe/{_MK8_XCI}", 90)
        env.put("switch/Mario Kart 8 Deluxe/huge.txt", 500)
        [cand] = env.service.plan_adoptions([_switch(4, "Mario Kart 8 Deluxe")])
        assert cand.install.file_path.endswith(f"/Mario Kart 8 Deluxe/{_MK8_XCI}")

    def test_nested_only_launch_file_does_not_qualify(self, env):
        env.put(f"switch/Mario Kart 8 Deluxe/sub/{_MK8_XCI}", 4)
        assert env.service.plan_adoptions([_switch(4, "Mario Kart 8 Deluxe")]) == []

    def test_two_disk_entries_for_one_title_adopt_none(self, env):
        env.put(f"switch/Mario Kart 8 Deluxe/{_MK8_XCI}", 4)
        env.put("switch/Mario Kart 8 Deluxe NSW-GRP.xci", 4)
        assert env.service.plan_adoptions([_switch(4, "Mario Kart 8 Deluxe")]) == []

    def test_two_roms_in_batch_with_one_title_adopt_none(self, env):
        env.put(f"switch/Mario Kart 8 Deluxe/{_MK8_XCI}", 4)
        roms = [_switch(4, "Mario Kart 8 Deluxe"), _switch(9, "Mario Kart 8_ Deluxe")]
        assert env.service.plan_adoptions(roms) == []

    def test_synced_rom_sharing_the_title_makes_it_ambiguous(self, env):
        # The single-ROM hook only sees one ROM; the synced library supplies the rest.
        env.put(f"switch/Mario Kart 8 Deluxe/{_MK8_XCI}", 4)
        env.seed_rom(9, platform="switch", fs_name="Mario Kart 8_ Deluxe")
        assert env.service.plan_adoptions([_switch(4, "Mario Kart 8 Deluxe")]) == []

    def test_entry_used_by_an_existing_install_is_not_adopted(self, env):
        env.put(f"switch/Mario Kart 8 Deluxe/{_MK8_XCI}", 4)
        mk8 = env.roms_root / "switch" / "Mario Kart 8 Deluxe"
        env.seed_rom(9, platform="switch", fs_name="MK8")
        env.seed_install(9, str(mk8 / _MK8_XCI), rom_dir=str(mk8), system="switch")
        assert env.service.plan_adoptions([_switch(4, "Mario Kart 8 Deluxe")]) == []

    def test_existing_install_blocks(self, env):
        env.put(f"switch/{_ZELDA_XCI}", 4)
        env.seed_rom(3, platform="switch")
        env.seed_install(3, system="switch")
        assert env.service.plan_adoptions([_switch(3, "The Legend of Zelda_ Echoes of Wisdom")]) == []

    def test_download_job_blocks(self, env):
        env.put(f"switch/{_ZELDA_XCI}", 4)
        env.seed_rom(3, platform="switch")
        env.seed_job(3)
        assert env.service.plan_adoptions([_switch(3, "The Legend of Zelda_ Echoes of Wisdom")]) == []

    def test_active_download_blocks(self, env):
        env.put(f"switch/{_ZELDA_XCI}", 4)
        env.active.add(3)
        assert env.service.plan_adoptions([_switch(3, "The Legend of Zelda_ Echoes of Wisdom")]) == []

    def test_installed_sibling_blocks(self, env):
        env.put(f"switch/{_ZELDA_XCI}", 4)
        env.seed_rom(3, platform="switch", group=_GROUP)
        env.seed_rom(30, platform="switch", group=_GROUP)
        env.seed_install(30, system="switch")
        assert env.service.plan_adoptions([_switch(3, "The Legend of Zelda_ Echoes of Wisdom")]) == []

    def test_exact_rule_wins_and_claims_its_entry(self, env):
        # RomM file "Mario Kart 8 Deluxe.xci" sits exactly where a download puts it.
        exact = env.put("switch/Mario Kart 8 Deluxe.xci", 8)
        env.put(f"switch/Mario Kart 8 Deluxe/{_MK8_XCI}", 4)
        rom = {
            "id": 4,
            "name": "Mario Kart 8 Deluxe",
            "fs_name": "Mario Kart 8 Deluxe.xci",
            "fs_size_bytes": 8,
            "platform_slug": "switch",
            "files": [{"file_name": "Mario Kart 8 Deluxe.xci"}],
        }
        [cand] = env.service.plan_adoptions([rom])
        assert cand.install.file_path == str(exact)
        assert cand.install.rom_dir is None

    def test_systems_without_launch_file_extensions_never_use_titles(self, env):
        env.put("gba/Game X.gba", 8)
        assert env.service.plan_adoptions([_single(fs_name="Game_ X.gba")]) == []

    def test_missing_switch_folder(self, env):
        assert env.service.plan_adoptions([_switch(4, "Mario Kart 8 Deluxe")]) == []

    def test_unsafe_system_is_skipped(self, env):
        assert env.service.plan_adoptions([_switch(4, "Mario Kart 8 Deluxe", platform_slug="..")]) == []

    def test_symlink_entry_escaping_roms_root_is_rejected(self, env, tmp_path):
        outside = tmp_path / "outside" / "Mario Kart 8 Deluxe"
        outside.mkdir(parents=True)
        (outside / _MK8_XCI).write_bytes(b"x")
        (env.roms_root / "switch").mkdir()
        (env.roms_root / "switch" / "Mario Kart 8 Deluxe").symlink_to(outside)
        assert env.service.plan_adoptions([_switch(4, "Mario Kart 8 Deluxe")]) == []

    def test_listing_failure_is_logged_not_raised(self, env, monkeypatch):
        env.put(f"switch/{_ZELDA_XCI}", 4)

        def boom(_directory):
            raise OSError("io")

        monkeypatch.setattr(env.service._file_reader, "list_entries", boom)
        assert env.service.plan_adoptions([_switch(3, "The Legend of Zelda_ Echoes of Wisdom")]) == []

    def test_commit_writes_title_match(self, env):
        env.put(f"switch/{_ZELDA_XCI}", 4)
        env.seed_rom(3, platform="switch")
        cands = env.service.plan_adoptions([_switch(3, "The Legend of Zelda_ Echoes of Wisdom")])
        assert env.service.commit_adoptions(cands) == [3]
        with env.factory() as uow:
            assert uow.rom_installs.get(3).file_path == str(env.roms_root / "switch" / _ZELDA_XCI)

    @pytest.mark.asyncio
    async def test_game_page_hook_adopts_by_title(self, env):
        env.put(f"switch/Fire Emblem Three Houses/{_FE_XCI}", 4)
        env.seed_rom(2, platform="switch", app_id=4242, fs_name="Fire Emblem_ Three Houses")
        env.romm.roms[2] = _switch(2, "Fire Emblem_ Three Houses")

        assert await env.service.adopt_rom(2) is True

        d = env.roms_root / "switch" / "Fire Emblem Three Houses"
        with env.factory() as uow:
            install = uow.rom_installs.get(2)
        assert install.rom_dir == str(d)
        assert install.file_path == str(d / _FE_XCI)
        payload = env.emit.await_args.args[1]
        assert payload["adopted"] is True
        assert payload["file_path"] == str(d / _FE_XCI)
