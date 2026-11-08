"""Tests for ``SqliteDownloadJobRepository`` over the ``download_jobs`` table."""

from __future__ import annotations

import sqlite3


import pytest

from adapters.repositories.unit_of_work import SqliteUnitOfWork
from domain.download_job import DownloadJob


T0 = "2026-01-01T00:00:00+00:00"


def _job(rom_id: int, *, position: int, is_multi: bool = False, total: int | None = 1024) -> DownloadJob:
    ext = ".zip.tmp" if is_multi else ".tmp"
    return DownloadJob.enqueue(
        rom_id=rom_id,
        rom_name=f"Game {rom_id}",
        platform_name="PlayStation",
        file_name=f"game{rom_id}.bin",
        target_path=f"/roms/psx/game{rom_id}.bin",
        tmp_path=f"/roms/psx/game{rom_id}.bin{ext}",
        is_multi=is_multi,
        position=position,
        now=T0,
        total_bytes=total,
    )


class TestRoundTrip:
    def test_all_fields_preserved(self, uow: SqliteUnitOfWork):
        job = _job(1, position=4, is_multi=True, total=None)
        job.interrupt("disk_full", T0)
        job.observe_transfer(total_bytes=9000, range_supported=True, now=T0)
        uow.download_jobs.save(job)
        assert uow.download_jobs.get(1) == job

    def test_nullable_fields_round_trip_as_none(self, uow: SqliteUnitOfWork):
        job = _job(2, position=1, total=None)
        uow.download_jobs.save(job)
        loaded = uow.download_jobs.get(2)
        assert loaded is not None
        assert loaded.total_bytes is None
        assert loaded.range_supported is None
        assert loaded.interrupt_reason is None
        assert loaded.is_multi is False

    def test_range_supported_false_is_not_none(self, uow: SqliteUnitOfWork):
        job = _job(3, position=1)
        job.observe_transfer(total_bytes=None, range_supported=False, now=T0)
        uow.download_jobs.save(job)
        loaded = uow.download_jobs.get(3)
        assert loaded is not None
        assert loaded.range_supported is False

    def test_save_upserts(self, uow: SqliteUnitOfWork):
        job = _job(1, position=1)
        uow.download_jobs.save(job)
        job.start(T0)
        uow.download_jobs.save(job)
        loaded = uow.download_jobs.get(1)
        assert loaded is not None
        assert loaded.status == "downloading"

    def test_survives_commit(self, db: str):
        with SqliteUnitOfWork(db) as unit:
            unit.download_jobs.save(_job(5, position=1))
        with SqliteUnitOfWork(db) as unit:
            assert unit.download_jobs.get(5) is not None


class TestMissAndDelete:
    def test_get_absent_returns_none(self, uow: SqliteUnitOfWork):
        assert uow.download_jobs.get(99) is None

    def test_delete_is_idempotent(self, uow: SqliteUnitOfWork):
        uow.download_jobs.save(_job(1, position=1))
        uow.download_jobs.delete(1)
        uow.download_jobs.delete(1)
        assert uow.download_jobs.get(1) is None


class TestOrdering:
    def test_iter_all_orders_by_position(self, uow: SqliteUnitOfWork):
        uow.download_jobs.save(_job(1, position=3))
        uow.download_jobs.save(_job(2, position=-1))
        uow.download_jobs.save(_job(3, position=2))
        assert [j.rom_id for j in uow.download_jobs.iter_all()] == [2, 3, 1]

    def test_next_position_empty_is_one(self, uow: SqliteUnitOfWork):
        assert uow.download_jobs.next_position() == 1
        assert uow.download_jobs.min_position() is None

    def test_next_and_min_position(self, uow: SqliteUnitOfWork):
        uow.download_jobs.save(_job(1, position=5))
        uow.download_jobs.save(_job(2, position=-2))
        assert uow.download_jobs.next_position() == 6
        assert uow.download_jobs.min_position() == -2


class TestSchemaGuards:
    def test_rejects_unknown_status(self, uow: SqliteUnitOfWork):
        job = _job(1, position=1)
        object.__setattr__(job, "status", "completed")
        with pytest.raises(sqlite3.IntegrityError):
            uow.download_jobs.save(job)
