"""Tests for the ``DownloadJob`` aggregate (durable download queue)."""

from __future__ import annotations

import pytest

from domain.download_job import (
    INTERRUPT_DISK_FULL,
    INTERRUPT_RESTART,
    INTERRUPT_SERVER_UNREACHABLE,
    DownloadJob,
    InvalidJobTransitionError,
)

T0 = "2026-01-01T00:00:00+00:00"
T1 = "2026-01-01T00:01:00+00:00"


def _job(**overrides) -> DownloadJob:
    kwargs = {
        "rom_id": 7,
        "rom_name": "Zelda",
        "platform_name": "Nintendo 64",
        "file_name": "zelda.z64",
        "target_path": "/roms/n64/zelda.z64",
        "tmp_path": "/roms/n64/zelda.z64.tmp",
        "is_multi": False,
        "position": 3,
        "now": T0,
        "total_bytes": 1024,
    }
    kwargs.update(overrides)
    return DownloadJob.enqueue(**kwargs)


def _in_status(status: str) -> DownloadJob:
    job = _job()
    if status == "queued":
        return job
    if status == "paused":
        job.pause(T0)
        return job
    if status == "interrupted":
        job.interrupt(INTERRUPT_SERVER_UNREACHABLE, T0)
        return job
    job.start(T0)
    if status == "extracting":
        job.begin_extract(T0)
    return job


class TestEnqueue:
    def test_new_job_is_queued_at_position(self):
        job = _job()
        assert job.status == "queued"
        assert job.position == 3
        assert job.created_at == T0
        assert job.updated_at == T0
        assert job.interrupt_reason is None
        assert job.range_supported is None
        assert job.total_bytes == 1024

    def test_zero_total_is_unknown(self):
        assert _job(total_bytes=0).total_bytes is None

    @pytest.mark.parametrize("rom_id", [0, -1])
    def test_rejects_non_positive_rom_id(self, rom_id):
        with pytest.raises(ValueError, match="rom_id"):
            _job(rom_id=rom_id)

    def test_rejects_empty_paths(self):
        with pytest.raises(ValueError):
            _job(tmp_path="")

    def test_rejects_tmp_equal_to_target(self):
        with pytest.raises(ValueError):
            _job(tmp_path="/roms/n64/zelda.z64")


class TestTransitions:
    def test_start_from_queued(self):
        job = _job()
        job.start(T1)
        assert job.status == "downloading"
        assert job.updated_at == T1

    @pytest.mark.parametrize("status", ["downloading", "extracting", "paused", "interrupted"])
    def test_start_requires_queued(self, status):
        with pytest.raises(InvalidJobTransitionError):
            _in_status(status).start(T1)

    def test_begin_extract_from_downloading(self):
        job = _in_status("downloading")
        job.begin_extract(T1)
        assert job.status == "extracting"

    @pytest.mark.parametrize("status", ["queued", "extracting", "paused", "interrupted"])
    def test_begin_extract_requires_downloading(self, status):
        with pytest.raises(InvalidJobTransitionError):
            _in_status(status).begin_extract(T1)

    @pytest.mark.parametrize("status", ["queued", "downloading"])
    def test_pause_from_live(self, status):
        job = _in_status(status)
        job.pause(T1)
        assert job.status == "paused"
        assert job.interrupt_reason is None

    @pytest.mark.parametrize("status", ["extracting", "paused", "interrupted"])
    def test_pause_rejected_otherwise(self, status):
        with pytest.raises(InvalidJobTransitionError):
            _in_status(status).pause(T1)

    @pytest.mark.parametrize("status", ["queued", "downloading", "extracting", "paused", "interrupted"])
    def test_interrupt_from_any_status(self, status):
        job = _in_status(status)
        job.interrupt(INTERRUPT_DISK_FULL, T1)
        assert job.status == "interrupted"
        assert job.interrupt_reason == INTERRUPT_DISK_FULL
        assert job.updated_at == T1

    def test_interrupt_requires_reason(self):
        with pytest.raises(ValueError):
            _job().interrupt("", T1)

    @pytest.mark.parametrize("status", ["paused", "interrupted"])
    def test_requeue_keeps_position_and_clears_reason(self, status):
        job = _in_status(status)
        job.requeue(T1)
        assert job.status == "queued"
        assert job.position == 3
        assert job.interrupt_reason is None

    @pytest.mark.parametrize("status", ["queued", "downloading", "extracting"])
    def test_requeue_rejected_for_live_jobs(self, status):
        with pytest.raises(InvalidJobTransitionError):
            _in_status(status).requeue(T1)

    def test_start_clears_a_stale_reason(self):
        job = _in_status("interrupted")
        job.requeue(T1)
        job.start(T1)
        assert job.interrupt_reason is None

    def test_move_to_front(self):
        job = _job()
        job.move_to_front(-5, T1)
        assert job.position == -5
        assert job.updated_at == T1


class TestObserveTransfer:
    def test_records_total_and_range_support(self):
        job = _job(total_bytes=None)
        job.observe_transfer(total_bytes=4096, range_supported=True, now=T1)
        assert job.total_bytes == 4096
        assert job.range_supported is True
        assert job.updated_at == T1

    def test_unknown_values_do_not_clobber(self):
        job = _job()
        job.observe_transfer(total_bytes=2048, range_supported=False, now=T0)
        job.observe_transfer(total_bytes=0, range_supported=None, now=T1)
        assert job.total_bytes == 2048
        assert job.range_supported is False


class TestReconcileAfterRestart:
    @pytest.mark.parametrize("status", ["queued", "downloading", "extracting"])
    def test_live_jobs_become_interrupted_by_restart(self, status):
        job = _in_status(status)
        job.reconcile_after_restart(T1)
        assert job.status == "interrupted"
        assert job.interrupt_reason == INTERRUPT_RESTART

    def test_interrupted_keeps_original_reason(self):
        job = _in_status("interrupted")
        job.reconcile_after_restart(T1)
        assert job.status == "interrupted"
        assert job.interrupt_reason == INTERRUPT_SERVER_UNREACHABLE

    def test_user_pause_is_untouched(self):
        job = _in_status("paused")
        job.reconcile_after_restart(T1)
        assert job.status == "paused"
        assert job.interrupt_reason is None
        assert job.updated_at == T0
