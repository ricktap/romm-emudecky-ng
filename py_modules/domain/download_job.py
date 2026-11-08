"""DownloadJob — one ROM download that has not finished yet.

The durable half of the download queue: a row exists exactly while a ROM's
download is queued, transferring, extracting, paused by the user, or interrupted
by a transient failure (server unreachable, auth, disk full, plugin restart).
Finished downloads (completed, cancelled, failed) have no job — their row is
deleted. The partial transfer file at ``tmp_path`` is the progress record; the
job only changes on status transitions, never per progress tick.
"""

from __future__ import annotations

from typing import Literal

from domain._aggregate import cosmic_aggregate

DownloadJobStatus = Literal["queued", "downloading", "extracting", "paused", "interrupted"]

JOB_STATUSES: tuple[str, ...] = ("queued", "downloading", "extracting", "paused", "interrupted")

# Short slugs explaining why a job is ``interrupted``.
INTERRUPT_SERVER_UNREACHABLE = "server_unreachable"
INTERRUPT_AUTH_FAILED = "auth_failed"
INTERRUPT_DISK_FULL = "disk_full"
INTERRUPT_RESTART = "restart"


class InvalidJobTransitionError(ValueError):
    """Raised when a verb is applied to a job in a status that does not allow it."""


@cosmic_aggregate
class DownloadJob:
    """One unfinished ROM download, keyed by ``rom_id``; ordered by ``position``."""

    rom_id: int
    rom_name: str
    platform_name: str
    file_name: str
    target_path: str
    tmp_path: str
    is_multi: bool
    status: str
    position: int
    created_at: str
    updated_at: str
    total_bytes: int | None = None
    interrupt_reason: str | None = None
    range_supported: bool | None = None

    @classmethod
    def enqueue(
        cls,
        *,
        rom_id: int,
        rom_name: str,
        platform_name: str,
        file_name: str,
        target_path: str,
        tmp_path: str,
        is_multi: bool,
        position: int,
        now: str,
        total_bytes: int | None = None,
    ) -> DownloadJob:
        """Register a new download waiting for its turn at queue ``position``."""
        if rom_id <= 0:
            raise ValueError(f"rom_id must be positive, got {rom_id}")
        if not target_path or not tmp_path:
            raise ValueError("target_path and tmp_path are required")
        if tmp_path == target_path:
            raise ValueError("tmp_path must differ from target_path")
        return cls(
            rom_id=rom_id,
            rom_name=rom_name,
            platform_name=platform_name,
            file_name=file_name,
            target_path=target_path,
            tmp_path=tmp_path,
            is_multi=is_multi,
            status="queued",
            position=position,
            created_at=now,
            updated_at=now,
            total_bytes=total_bytes if total_bytes else None,
        )

    def _require(self, verb: str, allowed: tuple[str, ...]) -> None:
        if self.status not in allowed:
            raise InvalidJobTransitionError(f"cannot {verb} a download job in status {self.status!r}")

    def start(self, now: str) -> None:
        """The scheduler handed this job the transfer slot."""
        self._require("start", ("queued",))
        self.status = "downloading"
        self.interrupt_reason = None
        self.updated_at = now

    def begin_extract(self, now: str) -> None:
        """The archive transfer finished; extraction of the multi-file ROM began."""
        self._require("begin extraction of", ("downloading",))
        self.status = "extracting"
        self.updated_at = now

    def pause(self, now: str) -> None:
        """The user paused the job; it never resumes on its own."""
        self._require("pause", ("queued", "downloading"))
        self.status = "paused"
        self.interrupt_reason = None
        self.updated_at = now

    def interrupt(self, reason: str, now: str) -> None:
        """A transient failure stopped the job; the partial transfer is kept."""
        if not reason:
            raise ValueError("an interrupt needs a reason")
        self.status = "interrupted"
        self.interrupt_reason = reason
        self.updated_at = now

    def requeue(self, now: str) -> None:
        """A paused or interrupted job goes back in line, keeping its position."""
        self._require("requeue", ("paused", "interrupted"))
        self.status = "queued"
        self.interrupt_reason = None
        self.updated_at = now

    def move_to_front(self, position: int, now: str) -> None:
        """Reorder the job to ``position`` (a value below every other job's)."""
        self.position = position
        self.updated_at = now

    def observe_transfer(self, *, total_bytes: int | None, range_supported: bool | None, now: str) -> None:
        """Record what the server revealed about the transfer (full size, Range support)."""
        if total_bytes:
            self.total_bytes = total_bytes
        if range_supported is not None:
            self.range_supported = range_supported
        self.updated_at = now

    def reconcile_after_restart(self, now: str) -> None:
        """Startup verdict: anything not user-paused becomes ``interrupted``.

        A job that was queued, transferring, or extracting when the plugin
        stopped is interrupted with reason ``restart``; an already-interrupted
        job keeps its original reason. A user pause is left untouched — it is
        never auto-resumed.
        """
        if self.status == "paused":
            return
        if self.status == "interrupted" and self.interrupt_reason:
            return
        self.interrupt(INTERRUPT_RESTART, now)
