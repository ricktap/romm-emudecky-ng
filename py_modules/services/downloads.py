"""DownloadService — ROM download orchestration.

Owns every step between a frontend download request and a ROM
landing on disk: the durable download queue (``DownloadJob`` rows that
survive a plugin restart), the one-at-a-time transfer scheduler,
disk-space pre-flight, single-file and multi-file downloads, ZIP
extraction, failure classification, and partial-download cleanup.
Raw filesystem I/O flows through the ``DownloadFileStore`` Protocol;
HTTP traffic flows through ``RommRomReader``.
"""

from __future__ import annotations

import asyncio
import errno
import os
import re
from dataclasses import dataclass
from functools import partial
from typing import TYPE_CHECKING, Any

from domain.disc_formats import DISC_IMAGE_EXTENSIONS
from domain.download_job import (
    INTERRUPT_AUTH_FAILED,
    INTERRUPT_DISK_FULL,
    INTERRUPT_SERVER_UNREACHABLE,
    DownloadJob,
    InvalidJobTransitionError,
)
from domain.rom_files import (
    build_m3u_content,
    detect_launch_file,
    es_de_collapse_rename,
    folder_boot_layout_root,
    is_multi_file_download,
    needs_m3u,
)
from domain.install_layout import resolve_download_names
from domain.rom_install import RomInstall
from domain.shortcut_data import EmulatorInvocation, build_launch_options
from lib.errors import (
    RommApiError,
    RommAuthError,
    RommForbiddenError,
    RommNotFoundError,
    RommServerError,
    TokenHostMismatchError,
    error_response,
)
from lib.list_result import ErrorCode
from lib.path_safety import PathTraversalError, safe_join_lexical

if TYPE_CHECKING:
    import logging
    from collections.abc import Callable

    from models.state import InstalledRomEntry

    from services.protocols import (
        ActiveCoreReader,
        Clock,
        DiscResolver,
        DownloadFileStore,
        EventEmitter,
        LaunchResolverFn,
        RetroDeckPaths,
        RommRomReader,
        RomRemoverProvider,
        Sleeper,
        SystemM3uSupportFn,
        SystemResolver,
        UnitOfWorkFactory,
    )

_DOWNLOAD_QUEUE_MAX_TERMINAL = 50
_ZIP_TMP_EXT = ".zip.tmp"
_TMP_EXT = ".tmp"
# Stopped-but-unfinished statuses: no transfer task runs, the DownloadJob row and
# the partial transfer are kept, and ``resume_download`` picks the job up again.
_RESUMABLE_STATUSES = ("paused", "interrupted")
# Errno values meaning "the disk is full" — the user can free space and resume.
_DISK_FULL_ERRNOS = frozenset({errno.ENOSPC, errno.EDQUOT})
# The HTTP adapter's completeness check message (``_validate_download``).
_HTTP_STATUS_RE = re.compile(r"HTTP (\d{3})\b")
_INCOMPLETE_RE = re.compile(r"Download incomplete: got (\d+) bytes, expected (\d+)")

_OUTCOME_INTERRUPTED = "interrupted"
_OUTCOME_FAILED = "failed"


def _exception_chain(exc: BaseException) -> list[BaseException]:
    """Return *exc* followed by its ``__cause__`` / ``__context__`` ancestors (cycle-safe)."""
    chain: list[BaseException] = []
    current: BaseException | None = exc
    while current is not None and current not in chain:
        chain.append(current)
        current = current.__cause__ or current.__context__
    return chain


def classify_download_failure(exc: BaseException) -> tuple[str, str | None]:
    """Decide whether a failed download is ``interrupted`` (resumable) or ``failed``.

    Returns ``(outcome, reason)``. ``interrupted`` keeps the partial transfer so
    the user (or startup reconciliation) can resume: a full disk (``ENOSPC`` /
    ``EDQUOT`` anywhere in the cause chain — the HTTP adapter wraps local write
    errors in a ``RommConnectionError``), an auth failure (401/403/token bound to
    another server — fixable by signing in again), or any transport / server
    failure (unreachable, timeout, 5xx, a stream cut short). ``failed`` is only
    for what can never succeed as-is: the ROM is gone (404) or another 4xx, the
    transfer overshot its declared size or produced nothing, or a local
    invariant / path-safety / archive error.
    """
    chain = _exception_chain(exc)
    for link in chain:
        if isinstance(link, OSError) and link.errno in _DISK_FULL_ERRNOS:
            return (_OUTCOME_INTERRUPTED, INTERRUPT_DISK_FULL)
    if isinstance(exc, RommAuthError | RommForbiddenError | TokenHostMismatchError):
        return (_OUTCOME_INTERRUPTED, INTERRUPT_AUTH_FAILED)
    if isinstance(exc, RommNotFoundError):
        return (_OUTCOME_FAILED, None)
    if isinstance(exc, RommServerError):
        return (_OUTCOME_INTERRUPTED, INTERRUPT_SERVER_UNREACHABLE)  # 5xx / 429 rate limit
    if isinstance(exc, RommApiError):
        status = exc.status_code
        if status is None:
            match = _HTTP_STATUS_RE.match(str(exc))
            status = int(match.group(1)) if match is not None else None
        if isinstance(status, int) and 400 <= status < 500 and status not in (408, 429):
            return (_OUTCOME_FAILED, None)
        for link in chain:
            text = str(link)
            if "Download produced 0 bytes" in text:
                return (_OUTCOME_FAILED, None)
            match = _INCOMPLETE_RE.search(text)
            if match is not None and int(match.group(1)) > int(match.group(2)):
                return (_OUTCOME_FAILED, None)
        return (_OUTCOME_INTERRUPTED, INTERRUPT_SERVER_UNREACHABLE)
    return (_OUTCOME_FAILED, None)


class _DownloadControl:
    """Per-download cooperative-control flags. Set on the event-loop thread by
    ``cancel_download`` / ``pause_download``; polled on the executor worker
    thread by the progress callback, which raises ``CancelledError`` to abort
    the in-flight HTTP transfer when EITHER flag is set (#144).

    ``cancelled`` and ``paused`` differ only in the terminal handling: a cancel
    deletes the partial ``.tmp``; a pause keeps it so the transfer can resume
    from where it stopped. ``shutdown`` (plugin unload) keeps the partial AND
    the job row untouched so startup reconciliation resumes it. The abort
    mechanism (raise to unwind the executor transfer) is identical.

    Plain bools — not ``threading.Event`` — because the import-linter
    ``no-stdlib-io-in-services`` contract forbids ``threading`` in services, and
    under the GIL a one-way set-once bool flip needs no synchronisation.
    """

    __slots__ = ("cancelled", "paused", "shutdown")

    def __init__(self) -> None:
        self.cancelled = False
        self.paused = False
        self.shutdown = False


@dataclass(frozen=True)
class DownloadServiceConfig:
    """Frozen wiring bundle handed to ``DownloadService.__init__``.

    Holds the Protocol-typed adapters, runtime infrastructure, time/sleep
    seams, the SQLite Unit-of-Work factory, and path providers
    DownloadService needs at construction time. The shared ``active_core``
    resolver resolves a reinstalled ROM's full active core so
    ``download_complete`` re-bakes the ``-e`` override into ``launch_options``
    (the per-game/per-platform selection survives uninstall → reinstall), and the
    shared ``disc_resolver`` resolves a freshly-downloaded multi-disc ROM's
    selected disc so the same persisted pick survives uninstall → reinstall too.
    """

    romm_api: RommRomReader
    download_file_store: DownloadFileStore
    resolve_system: SystemResolver
    loop: asyncio.AbstractEventLoop
    logger: logging.Logger
    emit: EventEmitter
    clock: Clock
    sleeper: Sleeper
    retrodeck_paths: RetroDeckPaths
    active_core: ActiveCoreReader
    disc_resolver: DiscResolver
    m3u_support: SystemM3uSupportFn
    uow_factory: UnitOfWorkFactory
    # Deferred access to RomRemovalService.remove_rom — the two services form a
    # construction cycle, so the composition root binds it after both exist
    # (#1298 sibling supersede).
    rom_remover: RomRemoverProvider
    launch_resolver: LaunchResolverFn


class DownloadService:
    """ROM download engine: downloads and queue management."""

    def __init__(self, *, config: DownloadServiceConfig) -> None:
        self._romm_api = config.romm_api
        self._download_file_store = config.download_file_store
        self._resolve_system = config.resolve_system
        self._loop = config.loop
        self._logger = config.logger
        self._emit = config.emit
        self._clock = config.clock
        self._sleeper = config.sleeper
        self._retrodeck_paths = config.retrodeck_paths
        self._active_core = config.active_core
        self._disc_resolver = config.disc_resolver
        self._m3u_support = config.m3u_support
        self._uow_factory = config.uow_factory
        self._rom_remover = config.rom_remover
        self._launch_resolver = config.launch_resolver

        # Owned state
        self._download_in_progress: set[int] = set()
        self._download_queue: dict[int, dict[str, Any]] = {}
        self._download_tasks: dict[int, asyncio.Task[None]] = {}
        # One-at-a-time transfer scheduler. ``_active_control`` is the control
        # token of the download holding the single transfer slot; every other
        # download waits in ``_slot_waiters`` and the slot is handed to the
        # waiter with the lowest DownloadJob ``position`` (FIFO on ties).
        self._active_control: _DownloadControl | None = None
        self._slot_waiters: list[tuple[int, _DownloadControl, asyncio.Future[None]]] = []
        # In-memory mirror of each persisted job's queue position, kept in step
        # with every row write so the scheduler never reads SQLite to pick.
        self._job_positions: dict[int, int] = {}
        # rom_ids whose transfer total has been persisted for this attempt, so
        # the first progress tick writes it once (never per tick).
        self._persisted_totals: set[int] = set()
        # Background auto-resume of interrupted jobs after startup reconciliation.
        self._auto_resume_task: asyncio.Task[None] | None = None
        # Reserved bytes per in-flight ROM, so the disk pre-flight accounts for
        # siblings already committed to download but not yet written to disk.
        self._reserved_bytes: dict[int, int] = {}
        # Per-download cooperative-control tokens. The progress callback polls its
        # captured token on the executor thread; ``cancel_download`` /
        # ``pause_download`` flip it on the loop thread to abort the in-flight
        # transfer (#144).
        self._control_tokens: dict[int, _DownloadControl] = {}

    async def shutdown(self) -> None:
        """Cancel in-flight per-ROM download tasks on plugin unload.

        Per-ROM tasks are cancelled fire-and-forget; their ``finally``
        clauses run on the event loop after this method returns, which
        is acceptable on plugin unload.
        """
        # Mark every live attempt as shutting down BEFORE cancelling, so the
        # terminal handler keeps the partial transfer and the job row (the next
        # startup reconciles it) instead of treating the cancel as a user cancel.
        for control in self._control_tokens.values():
            control.shutdown = True
        if self._auto_resume_task is not None:
            self._auto_resume_task.cancel()
            self._auto_resume_task = None
        for task in self._download_tasks.values():
            task.cancel()
        self._download_tasks.clear()

    def _prune_download_queue(self):
        """Remove oldest completed/failed/cancelled items when over the limit.

        Keeps all active (downloading) items. Retains up to
        _DOWNLOAD_QUEUE_MAX_TERMINAL terminal items, removing the oldest
        (by insertion order) when the count exceeds the limit.
        """
        terminal_ids = [
            rid
            for rid, item in self._download_queue.items()
            if item.get("status") in ("completed", "failed", "cancelled")
        ]
        excess = len(terminal_ids) - _DOWNLOAD_QUEUE_MAX_TERMINAL
        if excess <= 0:
            return
        # Dict preserves insertion order (Python 3.7+), so the first
        # entries in terminal_ids are the oldest.
        for rid in terminal_ids[:excess]:
            del self._download_queue[rid]

    def _remove_tmp_files(self, paths: list[str]) -> int:
        """Remove each path in *paths*, logging a warning on per-file failure.

        Returns the count of successful removals. Mirrors the
        SteamGridService cache-prune pattern: service owns the loop +
        ``try``/``except`` + ``logger.warning`` so the operational
        signal on each failure is preserved instead of being swallowed
        inside the adapter.
        """
        removed = 0
        for path in paths:
            try:
                self._download_file_store.remove_file(path)
                removed += 1
            except OSError as e:
                self._logger.warning(f"Failed to remove tmp file {path}: {e}")
        return removed

    def _clean_bios_tmp_files(self):
        """Remove leftover .tmp files from BIOS directory (firmware has no durable queue yet)."""
        bios_base = self._retrodeck_paths.bios_path()
        if not bios_base:
            return 0
        paths = self._download_file_store.walk_files_matching_suffixes(bios_base, (_TMP_EXT,))
        return self._remove_tmp_files(paths)

    def _clean_orphan_rom_tmp_files(self, keep: set[str]) -> int:
        """Remove ``.tmp`` / ``.zip.tmp`` files under the ROM tree that belong to no job."""
        roms_base = self._retrodeck_paths.roms_path()
        if not roms_base:
            return 0
        paths = self._download_file_store.walk_files_matching_suffixes(roms_base, (_TMP_EXT, _ZIP_TMP_EXT))
        return self._remove_tmp_files([path for path in paths if path not in keep])

    # ── Durable queue (DownloadJob rows) ─────────────────────────────

    def _now_iso(self) -> str:
        return self._clock.now().isoformat()

    def _load_job(self, rom_id: int) -> DownloadJob | None:
        """Read the persisted job for *rom_id* in a short UoW (``None`` on miss or DB error)."""
        try:
            with self._uow_factory() as uow:
                return uow.download_jobs.get(rom_id)
        except Exception as e:
            self._logger.warning(f"Failed to read download job for ROM {rom_id}: {e}")
            return None

    def _mutate_job(self, rom_id: int, mutate: Callable[[DownloadJob], None]) -> DownloadJob | None:
        """Apply one verb to the persisted job in a short write UoW.

        A missing row (a download driven without a job, or one already evicted)
        is a no-op. An illegal transition or a DB failure is logged and swallowed
        — persistence must never break the transfer itself. Never called while
        another UoW is open.
        """
        try:
            with self._uow_factory() as uow:
                job = uow.download_jobs.get(rom_id)
                if job is None:
                    return None
                mutate(job)
                uow.download_jobs.save(job)
        except InvalidJobTransitionError as e:
            self._logger.warning(f"Download job {rom_id}: {e}")
            return None
        except Exception as e:
            self._logger.warning(f"Failed to persist download job for ROM {rom_id}: {e}")
            return None
        self._job_positions[rom_id] = job.position
        return job

    def _enqueue_job(
        self,
        *,
        rom_id: int,
        rom_name: str,
        platform_name: str,
        file_name: str,
        target_path: str,
        tmp_path: str,
        is_multi: bool,
        total_bytes: int | None,
    ) -> DownloadJob | None:
        """Persist the job as ``queued``: requeue a paused/interrupted row in place, else append.

        A paused or interrupted job keeps its queue position (resume never jumps
        the line). A fresh download takes ``max(position) + 1``.
        """
        now = self._now_iso()
        try:
            with self._uow_factory() as uow:
                existing = uow.download_jobs.get(rom_id)
                if existing is not None and existing.status in _RESUMABLE_STATUSES:
                    existing.requeue(now)
                    job = existing
                else:
                    position = existing.position if existing is not None else uow.download_jobs.next_position()
                    job = DownloadJob.enqueue(
                        rom_id=rom_id,
                        rom_name=rom_name,
                        platform_name=platform_name,
                        file_name=file_name,
                        target_path=target_path,
                        tmp_path=tmp_path,
                        is_multi=is_multi,
                        position=position,
                        now=now,
                        total_bytes=total_bytes,
                    )
                uow.download_jobs.save(job)
        except Exception as e:
            self._logger.warning(f"Failed to persist queued download job for ROM {rom_id}: {e}")
            return None
        self._job_positions[rom_id] = job.position
        return job

    def _drop_job(self, rom_id: int) -> None:
        """Delete the job row for *rom_id* (completed / cancelled / failed / evicted)."""
        self._job_positions.pop(rom_id, None)
        self._persisted_totals.discard(rom_id)
        try:
            with self._uow_factory() as uow:
                uow.download_jobs.delete(rom_id)
        except Exception as e:
            self._logger.warning(f"Failed to delete download job for ROM {rom_id}: {e}")

    def _persist_transfer_observation(self, rom_id: int, total: int) -> None:
        """Record the transfer's full size + Range verdict once per attempt (first progress tick)."""
        if rom_id in self._persisted_totals or not total:
            return
        self._persisted_totals.add(rom_id)
        entry = self._download_queue.get(rom_id, {})
        resumable = entry.get("resumable")
        now = self._now_iso()
        self._mutate_job(
            rom_id,
            lambda job: job.observe_transfer(
                total_bytes=total,
                range_supported=None if resumable is None else bool(resumable),
                now=now,
            ),
        )

    def move_to_front(self, rom_id: int | str) -> dict[str, Any]:
        """Move an unfinished download to the head of the queue.

        The job takes a position below every other job's, so the scheduler
        hands it the transfer slot next (it does not pre-empt a transfer that
        is already running).
        """
        rom_id = int(rom_id)
        now = self._now_iso()
        try:
            with self._uow_factory() as uow:
                job = uow.download_jobs.get(rom_id)
                if job is None:
                    return {"success": False, "reason": "not_queued", "message": "No queued download for this ROM"}
                lowest = uow.download_jobs.min_position()
                if lowest is not None and lowest < job.position:
                    job.move_to_front(lowest - 1, now)
                    uow.download_jobs.save(job)
        except Exception as e:
            self._logger.warning(f"Failed to move download {rom_id} to the front: {e}")
            return {"success": False, "reason": ErrorCode.UNKNOWN.value, "message": "Failed to reorder the queue"}
        self._job_positions[rom_id] = job.position
        return {"success": True, "message": "Moved to front"}

    # ── Transfer scheduler ───────────────────────────────────────────

    async def _acquire_transfer_slot(self, rom_id: int, control: _DownloadControl) -> None:
        """Wait until *control*'s download holds the single transfer slot.

        Free slot → taken immediately. Otherwise the caller waits; on release
        the slot goes to the waiter with the lowest job position. A waiter that
        is cancelled after being granted the slot releases it again so the next
        job is not starved.
        """
        if self._active_control is None:
            self._active_control = control
            return
        future: asyncio.Future[None] = self._loop.create_future()
        waiter = (rom_id, control, future)
        self._slot_waiters.append(waiter)
        try:
            await future
        except BaseException:
            if waiter in self._slot_waiters:
                self._slot_waiters.remove(waiter)
            self._release_transfer_slot(control)
            raise

    def _release_transfer_slot(self, control: _DownloadControl) -> None:
        """Release the slot if *control* holds it and hand it to the next waiter by position."""
        if self._active_control is not control:
            return
        self._active_control = None
        live = [(index, w) for index, w in enumerate(self._slot_waiters) if not w[2].done()]
        if not live:
            self._slot_waiters = []
            return
        _index, chosen = min(live, key=lambda item: (self._job_positions.get(item[1][0], float("inf")), item[0]))
        self._slot_waiters.remove(chosen)
        self._active_control = chosen[1]
        chosen[2].set_result(None)

    # ── Startup reconciliation ───────────────────────────────────────

    def reconcile_persisted_downloads(self) -> None:
        """Restore the durable download queue after a plugin (re)start.

        Every persisted job keeps its partial transfer — the file's real size is
        the resume point. Jobs that were queued, transferring, extracting, or
        interrupted become ``interrupted`` (reason ``restart`` unless already
        set); a user-paused job stays paused and is never auto-resumed. The
        in-memory queue is rebuilt so ``get_download_queue`` shows the jobs
        immediately. ``.tmp`` / ``.zip.tmp`` files that belong to no job (and
        every BIOS ``.tmp``) are deleted. Finally the interrupted jobs are
        auto-resumed in queue order in the background.
        """
        now = self._now_iso()
        jobs_loaded = True
        try:
            with self._uow_factory() as uow:
                jobs = list(uow.download_jobs.iter_all())
                for job in jobs:
                    job.reconcile_after_restart(now)
                    uow.download_jobs.save(job)
        except Exception as e:
            self._logger.error(f"Failed to load persisted downloads: {e}")
            jobs = []
            jobs_loaded = False

        keep: set[str] = set()
        for job in jobs:
            keep.add(job.tmp_path)
            self._job_positions[job.rom_id] = job.position
            on_disk = self._download_file_store.file_size(job.tmp_path)
            total = job.total_bytes or 0
            entry: dict[str, Any] = {
                "rom_id": job.rom_id,
                "rom_name": job.rom_name,
                "platform_name": job.platform_name,
                "file_name": job.file_name,
                "status": job.status,
                "progress": min(on_disk / total, 1.0) if total else 0,
                "bytes_downloaded": on_disk,
                "total_bytes": total,
                "resumable": bool(job.range_supported),
            }
            if job.interrupt_reason:
                entry["reason"] = job.interrupt_reason
            self._download_queue[job.rom_id] = entry

        # Without the job list we cannot tell orphans from resumable partials, so a
        # failed load must never fall through to deleting every ROM .tmp.
        rom_cleaned = self._clean_orphan_rom_tmp_files(keep) if jobs_loaded else 0
        cleaned = rom_cleaned + self._clean_bios_tmp_files()
        if cleaned:
            self._logger.info(f"Cleaned {cleaned} orphan tmp file(s)")
        if jobs:
            self._logger.info(f"Restored {len(jobs)} unfinished download(s)")

        to_resume = [job.rom_id for job in jobs if job.status == "interrupted"]
        if to_resume:
            self._auto_resume_task = self._loop.create_task(self._auto_resume(to_resume))

    async def _auto_resume(self, rom_ids: list[int]) -> None:
        """Resume interrupted jobs in queue order; stop at the first unreachable server.

        Each resume re-fetches the ROM from RomM. When that fetch fails because
        the server is unreachable (or the login is rejected), the job stays
        interrupted with that reason, and every remaining job is marked the same
        way without another attempt — no retry loop; the user resumes later.
        """
        for index, rom_id in enumerate(rom_ids):
            result = await self.resume_download(rom_id)
            if result.get("success"):
                continue
            reason = result.get("reason")
            if reason in (INTERRUPT_SERVER_UNREACHABLE, INTERRUPT_AUTH_FAILED):
                for remaining in rom_ids[index + 1 :]:
                    await self._mark_interrupted(remaining, reason)
                return
            self._logger.info(f"Auto-resume of ROM {rom_id} skipped: {result.get('message')}")

    async def start_download(self, rom_id):
        rom_id = int(rom_id)
        if rom_id in self._download_in_progress:
            return {"success": False, "reason": "already_downloading", "message": "Already downloading"}
        # Claim the in-progress slot BEFORE the supersede await so a second
        # start_download for this same rom during that await is rejected by the
        # guard above instead of racing past it (B1). ``_begin_download`` re-adds
        # idempotently; every early exit here (removal abort, exception) releases
        # the claim, and on success the download task's ``finally`` owns it.
        self._download_in_progress.add(rom_id)
        # At most one downloaded version per shortcut binding (#1298): strip a
        # sibling install bound to this shortcut (or unbound) before this one
        # lands — a grandfathered sibling with its own shortcut is exempt. A
        # removal failure aborts the download so the invariant stays honest.
        try:
            cleanup_failure = await self._remove_conflicting_sibling_installs(rom_id)
        except Exception:
            self._download_in_progress.discard(rom_id)
            raise
        if cleanup_failure is not None:
            self._download_in_progress.discard(rom_id)
            return cleanup_failure
        return await self._begin_download(rom_id, resume=False)

    async def _remove_conflicting_sibling_installs(self, rom_id: int) -> dict[str, Any] | None:
        """Strip any other downloaded version of ``rom_id``'s sibling group (#1298 T7).

        Shared by ``start_download`` and ``resume_download``. The group's members
        are snapshotted in one short read UoW (closed before the removal seam runs
        — the removal opens its own UoW, which must not nest; ADR-0006). The
        membership read is a single indexed lookup done directly (like
        ``get_installed_rom``); each superseded install is then removed through the
        canonical ``RomRemovalService.remove_rom`` (files + ``rom_installs`` row;
        saves untouched per ADR-0007) rather than duplicating its deletion logic.
        Every removal attempt is logged with both rom ids so a failure is
        attributable (S7). A removal that reports ``not_installed`` raced clean and
        is skipped; any other failure is returned so the caller aborts with that
        shape. A superseded sibling's *paused* queue entry is evicted so the queue
        stays coherent with disk (S1). Returns ``None`` when the group is clean /
        all removals succeeded.
        """
        sibling_ids = self._conflicting_sibling_install_ids(rom_id)
        if not sibling_ids:
            return None
        remove_rom = self._rom_remover()
        for sibling_id in sibling_ids:
            result = await remove_rom(sibling_id)
            if result.get("success"):
                self._logger.info(f"Superseding install of rom {sibling_id} (group of rom {rom_id})")
                self._evict_if_paused(sibling_id)
                continue
            if result.get("reason") == "not_installed":
                continue  # raced clean — nothing to supersede
            self._logger.error(
                f"Superseding install of rom {sibling_id} (group of rom {rom_id}) failed: {result.get('message')}"
            )
            return result
        return None

    def _evict_if_paused(self, rom_id: int) -> None:
        """Drop a superseded sibling's queue entry when it is paused (#1298 S1).

        A paused (or interrupted) download whose install this supersede just
        removed is stale — its row (and partial ``.tmp``) would otherwise linger
        and could be resumed into a second installed version. Only a paused or
        interrupted row is evicted; a live or terminal row for the sibling is left
        to its own lifecycle.
        """
        entry = self._download_queue.get(rom_id)
        if entry is not None and entry.get("status") in _RESUMABLE_STATUSES:
            self.evict(rom_id)

    def _conflicting_sibling_install_ids(self, rom_id: int) -> list[int]:
        """Sibling members with a ``rom_installs`` row that this download supersedes.

        Read in one short UoW (closed before the removal seam runs). A member is
        superseded when it is installed and either unbound or bound to the *same*
        shortcut as ``rom_id`` — a member bound to a **different** shortcut is a
        grandfathered duplicate (ADR-0021 §5) with its own Steam entry and is
        never removed. A ROM with no sibling group key (solo / unbackfilled) has
        no siblings, so the list is empty.
        """
        with self._uow_factory() as uow:
            rom = uow.roms.get(rom_id)
            if rom is None or rom.sibling_group_key is None:
                return []
            x_app_id = rom.shortcut_app_id
            superseded: list[int] = []
            for member in uow.roms.iter_by_group_key(rom.sibling_group_key):
                if member.rom_id == rom_id:
                    continue
                if uow.rom_installs.get(member.rom_id) is None:
                    continue
                if member.shortcut_app_id is not None and member.shortcut_app_id != x_app_id:
                    continue  # grandfathered separate shortcut — never removed
                superseded.append(member.rom_id)
            return superseded

    async def _begin_download(self, rom_id, *, resume: bool):
        """Shared core of ``start_download`` and ``resume_download``.

        Fetches ROM detail, resolves the platform path, runs the disk pre-flight,
        then registers the queue entry, task, byte reservation, control token,
        and the persisted ``queued`` job. On ``resume=True`` the disk pre-flight
        discounts the bytes already on the existing ``.tmp`` (only the remainder
        is still needed) and ``_do_download`` is started with ``resume=True`` so
        the transfer appends rather than restarts.

        A resume that cannot proceed keeps the partial whenever it could succeed
        later: an unreachable server / rejected login / full disk leaves the job
        ``interrupted`` with that reason; only a can-never-succeed failure (the
        ROM is gone, an unsafe path) drops the job and its partial.

        The ``already_downloading`` guard stays with ``start_download``;
        ``resume_download`` validates the paused/interrupted entry before calling here.
        """
        self._download_in_progress.add(rom_id)
        try:
            rom_detail = await self._loop.run_in_executor(None, self._romm_api.get_rom, rom_id)
        except Exception as e:
            self._download_in_progress.discard(rom_id)
            self._logger.error(f"Failed to fetch ROM {rom_id}: {e}")
            response = error_response(e)
            if resume:
                outcome, reason = classify_download_failure(e)
                if outcome == _OUTCOME_INTERRUPTED and reason is not None:
                    await self._mark_interrupted(rom_id, reason)
                    response["reason"] = reason
                else:
                    await self._discard_resumable(rom_id, status="failed", error=response["message"])
            return response

        platform_slug = rom_detail.get("platform_slug", "")
        platform_fs_slug = rom_detail.get("platform_fs_slug")
        system = self._resolve_system(platform_slug, platform_fs_slug)

        # Path building, directory creation, and the disk pre-flight can raise
        # (SD card unmounted → OSError; ``roms_path()`` returning None → TypeError
        # in the join). Any raise here must release the in-progress flag so the
        # ROM isn't stuck "Already downloading" until a plugin reload (#1048).
        # The explicit early-return guards inside still ``return`` (not raise)
        # and discard the flag themselves; a ``return`` does not trip the except.
        try:
            roms_path = self._retrodeck_paths.roms_path()
            try:
                # ``system`` may be an unmapped server slug passed through verbatim
                # (ADR-0010). Validate it stays under roms_path BEFORE any make_dirs
                # so a slug like "../../etc" cannot create or write outside roms.
                # Lexical join: the recorded install path stays under the
                # configured roms root, not its symlink-resolved mount point.
                roms_dir = safe_join_lexical(roms_path, system)
            except PathTraversalError as e:
                self._download_in_progress.discard(rom_id)
                self._logger.error(f"Rejected download for ROM {rom_id}: unsafe platform slug {system!r}: {e}")
                message = "Server sent an unsafe platform path — download aborted"
                if resume:
                    await self._discard_resumable(rom_id, status="failed", error=message)
                await self._emit(
                    "download_failed",
                    {
                        "rom_id": rom_id,
                        "rom_name": rom_detail.get("name", ""),
                        "platform_name": rom_detail.get("platform_name", platform_slug),
                        "error_message": message,
                    },
                )
                return {"success": False, "reason": "path_traversal", "message": message}
            # The shared name resolution (``resolve_download_names``) is the one
            # adoption of pre-existing files uses too, so the two never diverge.
            # It coerces the server-supplied name to a single safe component:
            # any directory portion is stripped and a degenerate result (``..``
            # would escape the platform dir, ``.``/``""`` would resolve to it)
            # falls back to the synthetic rom_<id> identity.
            names = resolve_download_names(rom_detail)
            if names.files_missing:
                self._logger.warning(
                    "has_nested_single_file=true but files list is empty; "
                    f"falling back to fs_name='{names.raw_file_name}'"
                )
            if names.file_name_changed:
                self._logger.warning(f"Sanitized fs_name from '{names.raw_file_name}' to '{names.file_name}'")
            file_name = names.file_name
            file_size = rom_detail.get("fs_size_bytes", 0)
            target_path = os.path.join(roms_dir, file_name)
            is_multi = is_multi_file_download(rom_detail)
            partial_bytes = self._partial_tmp_size(target_path, rom_detail) if resume else 0

            # Check disk space: multi-file ROMs need space for ZIP + extracted contents
            self._download_file_store.make_dirs(roms_dir)
            free_space = self._download_file_store.disk_free(roms_dir)
            buffer = 100 * 1024 * 1024
            required = file_size * 2 + buffer if is_multi else file_size + buffer
            # On resume, the partial ``.tmp`` already holds some of the bytes, so
            # only the remainder still needs free space — discount what's on disk
            # so a near-complete resume isn't rejected for the full size.
            if resume:
                required = max(required - partial_bytes, 0)
            # Account for siblings already reserved but not yet written to disk,
            # so two queued downloads can't each pass a pre-flight that only one
            # of them actually fits (#1053).
            reserved_total = sum(self._reserved_bytes.values())
            if file_size and free_space - reserved_total < required:
                self._download_in_progress.discard(rom_id)
                free_mb = max(free_space - reserved_total, 0) // (1024 * 1024)
                need_mb = required // (1024 * 1024)
                if resume:
                    # The partial is kept: free some space and resume.
                    await self._mark_interrupted(rom_id, INTERRUPT_DISK_FULL)
                return {
                    "success": False,
                    "reason": "insufficient_space",
                    "message": f"Not enough disk space ({free_mb}MB free, need {need_mb}MB)",
                }
        except Exception as e:
            self._download_in_progress.discard(rom_id)
            self._logger.error(f"Failed to prepare download for ROM {rom_id}: {e}")
            return {"success": False, "reason": ErrorCode.UNKNOWN.value, "message": "Failed to start download"}

        rom_name = rom_detail.get("name", file_name)
        platform_name = rom_detail.get("platform_name", platform_slug)
        # Carry the prior resumability verdict across a resume so the UI keeps
        # showing Pause before the resumed transfer's headers re-confirm it.
        prior = self._download_queue.get(rom_id, {})
        resumable = bool(prior.get("resumable", False))

        # Create the control token BEFORE the task so the closure captured inside
        # ``_do_download``'s progress callback polls this exact object. A later
        # re-download installs a fresh token, leaving the zombie's callback bound
        # to the cancelled one (#144).
        control = _DownloadControl()
        try:
            task = self._loop.create_task(
                self._do_download(rom_id, rom_detail, target_path, system, file_name, control, resume=resume)
            )
        except Exception as e:
            self._download_in_progress.discard(rom_id)
            self._logger.error(f"Failed to start download task for ROM {rom_id}: {e}")
            return {"success": False, "reason": ErrorCode.UNKNOWN.value, "message": "Failed to start download"}

        tmp_path = target_path + (_ZIP_TMP_EXT if is_multi else _TMP_EXT)
        job = self._enqueue_job(
            rom_id=rom_id,
            rom_name=rom_name,
            platform_name=platform_name,
            file_name=file_name,
            target_path=target_path,
            tmp_path=tmp_path,
            is_multi=is_multi,
            total_bytes=None if is_multi else file_size,
        )
        self._persisted_totals.discard(rom_id)
        total_bytes = (job.total_bytes if job is not None and job.total_bytes else None) or file_size

        self._download_queue[rom_id] = {
            "rom_id": rom_id,
            "rom_name": rom_name,
            "platform_name": platform_name,
            "file_name": file_name,
            # Honest initial status: the task hasn't been handed the transfer
            # slot yet. ``_do_download`` flips this to "downloading" once the
            # scheduler grants it (#1053).
            "status": "queued",
            "progress": min(partial_bytes / total_bytes, 1.0) if total_bytes else 0,
            "bytes_downloaded": partial_bytes,
            "total_bytes": total_bytes,
            # Whether the server proved byte-range resume support for this ROM.
            # Re-confirmed live by the ``on_meta`` callback once headers arrive.
            "resumable": resumable,
        }
        self._download_tasks[rom_id] = task
        # Reserve this download's required bytes so a queued sibling's
        # pre-flight sees the outstanding claim (released in ``_do_download``'s
        # ``finally``).
        self._reserved_bytes[rom_id] = required
        self._control_tokens[rom_id] = control
        return {"success": True, "message": "Download started"}

    async def _mark_interrupted(self, rom_id: int, reason: str) -> None:
        """Flip a job to ``interrupted`` (row + queue entry) and emit the frame. Partial kept."""
        now = self._now_iso()
        self._mutate_job(rom_id, lambda job: job.interrupt(reason, now))
        entry = self._download_queue.get(rom_id)
        if entry is None:
            return
        entry["status"] = "interrupted"
        entry["reason"] = reason
        await self._emit_entry_frame(rom_id, entry)

    async def _discard_resumable(self, rom_id: int, *, status: str, error: str | None = None) -> None:
        """Drop a paused/interrupted job for good: delete its row and partial, finish its entry."""
        job = self._load_job(rom_id)
        if job is not None:
            self._remove_tmp_files([job.tmp_path])
        self._drop_job(rom_id)
        entry = self._download_queue.get(rom_id)
        if entry is None:
            return
        entry["status"] = status
        entry.pop("reason", None)
        if error is not None:
            entry["error"] = error
        if status == "failed":
            await self._emit(
                "download_failed",
                {
                    "rom_id": rom_id,
                    "rom_name": entry.get("rom_name", ""),
                    "platform_name": entry.get("platform_name", ""),
                    "error_message": error or "Download failed",
                },
            )
        else:
            await self._emit_entry_frame(rom_id, entry)
        self._prune_download_queue()

    async def _emit_entry_frame(self, rom_id: int, entry: dict[str, Any]) -> None:
        """Emit a ``download_progress`` frame mirroring the queue entry's current state."""
        frame: dict[str, Any] = {
            "rom_id": rom_id,
            "rom_name": entry.get("rom_name", ""),
            "platform_name": entry.get("platform_name", ""),
            "file_name": entry.get("file_name", ""),
            "status": entry.get("status", ""),
            "progress": entry.get("progress", 0),
            "bytes_downloaded": entry.get("bytes_downloaded", 0),
            "total_bytes": entry.get("total_bytes", 0),
            "resumable": entry.get("resumable", False),
        }
        if entry.get("reason"):
            frame["reason"] = entry["reason"]
        await self._emit("download_progress", frame)

    def _partial_tmp_size(self, target_path, rom_detail) -> int:
        """Bytes already on disk in the partial ``.tmp`` for *target_path*.

        Single-file ROMs stream to ``target_path + .tmp``; multi-file ROMs to
        ``target_path + .zip.tmp``. Returns 0 when no partial exists (the file
        store reports a missing path as size 0).
        """
        tmp_ext = _ZIP_TMP_EXT if is_multi_file_download(rom_detail) else _TMP_EXT
        return self._download_file_store.file_size(target_path + tmp_ext)

    def _record_install_io(self, *, rom_id, rom_detail, file_path, rom_dir, system, cleanup):
        """Build the ``RomInstall`` aggregate and persist it in a short write UoW.

        The filesystem work (rename, extraction, M3U detection) has already run
        outside any transaction; only the ``RomInstall`` upsert is wrapped here
        (ADR-0006). ``rom_dir`` is the dedicated extract directory for a
        multi-file ROM, or ``None`` for a single-file ROM (which owns no folder).
        If the RomM data fails the aggregate's invariant (non-positive
        ``rom_id``), nothing is persisted, *cleanup* removes the just-installed
        artifact, and a failure message is returned.

        Returns ``(file_path, None)`` on success or ``(None, error)`` when the
        invariant rejects the data.
        """
        try:
            install = RomInstall.mark_installed(
                rom_id=int(rom_id),
                file_path=file_path,
                rom_dir=rom_dir,
                platform_slug=rom_detail.get("platform_slug", ""),
                system=system,
                installed_at=self._clock.now().isoformat(),
            )
        except ValueError as e:
            cleanup()
            return None, f"Invalid install metadata: {e}"

        with self._uow_factory() as uow:
            uow.rom_installs.save(install)
        return file_path, None

    def _resolve_safe_extract_dir_name(self, rom_detail: dict[str, Any]) -> str:
        """Resolve the sanitized base name for a multi-file ROM's extract dir.

        Names the directory after the ROM's identity via
        ``resolve_extract_dir_name`` (never ``files[0]``), then coerces the
        server-supplied value to a single safe path component
        (``coerce_safe_component``): a directory portion (``../evil``, an
        absolute path) is stripped to its basename, AND a degenerate result
        (``..``/``.``/empty/whitespace — which would resolve to the roms root
        or the platform dir and turn a later ``remove_tree`` into a
        library-wide delete) falls back to the synthetic ``rom_<id>`` identity.
        Mirrors the ``file_name`` guard in ``_begin_download``.
        """
        names = resolve_download_names(rom_detail)
        if names.extract_dir_name_changed:
            self._logger.warning(
                f"Sanitized extract dir name from '{names.raw_extract_dir_name}' to '{names.extract_dir_name}'"
            )
        return names.extract_dir_name

    def _post_download_multi_io(
        self, rom_id, rom_detail, target_path, file_name, system, extract_dir_name, *, fresh_extract=False
    ):
        """Sync helper for _do_download multi-file — extraction + renames in executor.

        *extract_dir_name* is the ROM-identity-derived, sanitized base name for
        the extract directory (``_resolve_safe_extract_dir_name``) — never a
        name derived from ``files[0]``. *fresh_extract* is set when the archive
        was already complete on disk (an earlier attempt died mid-extraction):
        the half-extracted staging dir is removed first so extraction starts
        clean. Returns ``(launch_file, error)``. ``error`` is a string when the
        RomM data fails the ``RomInstall`` invariant — the extracted directory
        is removed and nothing is persisted — otherwise ``None``.
        """
        extract_dir = os.path.join(os.path.dirname(target_path), extract_dir_name)
        if fresh_extract:
            self._download_file_store.remove_tree(extract_dir)
        self._download_file_store.make_dirs(extract_dir)
        roms_base = self._retrodeck_paths.roms_path()
        tmp_zip = target_path + _ZIP_TMP_EXT
        # ZIP-slip protection: adapter validates members resolve within extract_dir
        # AND that extract_dir itself resolves within roms_base.
        rom_name = rom_detail.get("name", file_name)
        platform_name = rom_detail.get("platform_name", rom_detail.get("platform_slug", ""))
        extract_cb = self._make_extract_callback(rom_id, rom_name, platform_name, file_name)
        self._download_file_store.extract_zip(tmp_zip, extract_dir, roms_base, progress_callback=extract_cb)
        self._download_file_store.remove_file(tmp_zip)
        self._download_file_store.decode_url_encoded_names(extract_dir)
        # Heal a folder-boot disc dump whose PS3_DISC.SFB ships .txt-suffixed,
        # before launch detection reads the layout (ADR-0019 / #1212).
        self._maybe_heal_ps3_sfb_io(extract_dir)
        # Whether ES-DE lists .m3u for this system (gates both M3U generation and
        # launch-file selection so a RomM-bundled .m3u is ignored on Switch/Xbox).
        m3u_supported = self._m3u_support(system)
        # Auto-generate M3U if missing and multiple disc files exist
        self._maybe_generate_m3u_io(extract_dir, rom_detail, m3u_supported)
        # Detect launch file: prefer M3U > CUE > largest file
        launch_file = self._collect_and_detect_launch_file(extract_dir, m3u_supported)
        # ES-DE collapses a multi-file dir into one game entry only when the
        # dir is named after the launch file *including* the extension. The
        # launch file is only known after extraction (the M3U may be
        # auto-generated above), so the rename happens here, last of all the
        # filesystem work, so a later failure cleans up the renamed dir.
        extract_dir, launch_file = self._maybe_es_de_collapse_io(extract_dir, launch_file)

        return self._record_install_io(
            rom_id=rom_id,
            rom_detail=rom_detail,
            file_path=launch_file,
            rom_dir=extract_dir,
            system=system,
            cleanup=lambda: self._download_file_store.remove_tree(extract_dir),
        )

    def _maybe_es_de_collapse_io(self, extract_dir: str, launch_file: str) -> tuple[str, str]:
        """Rename *extract_dir* after the launch file so ES-DE collapses it to one entry.

        Returns ``(rom_dir, launch_file)`` — the renamed pair when the move
        applied, or the originals unchanged. Moves the *whole* directory
        (never just the launch file — ADR-0008). Skips the move when
        ``es_de_collapse_rename`` reports no rename is needed, and on
        collision: if the target already exists the staging dir is kept and a
        warning is logged rather than clobbering or merging an existing dir.
        """
        rename = es_de_collapse_rename(extract_dir, launch_file)
        if rename is None:
            return (extract_dir, launch_file)
        new_rom_dir, new_launch_file = rename
        if self._download_file_store.exists(new_rom_dir):
            self._logger.warning(
                "ES-DE collapse rename skipped: target '%s' already exists; keeping staging dir '%s'",
                new_rom_dir,
                extract_dir,
            )
            return (extract_dir, launch_file)
        self._download_file_store.move_dir(extract_dir, new_rom_dir)
        return (new_rom_dir, new_launch_file)

    def _post_download_single_io(self, rom_id, rom_detail, target_path, system):
        """Sync helper for _do_download single-file — rename + DB persist in executor.

        Returns ``(target_path, error)``. ``error`` is a string when the RomM
        data fails the ``RomInstall`` invariant — the renamed file is removed
        and nothing is persisted — otherwise ``None``.
        """
        tmp_path = target_path + _TMP_EXT
        self._download_file_store.rename(tmp_path, target_path)

        return self._record_install_io(
            rom_id=rom_id,
            rom_detail=rom_detail,
            file_path=target_path,
            rom_dir=None,
            system=system,
            cleanup=lambda: self._download_file_store.remove_file(target_path),
        )

    def _resolve_bound_app_id(self, rom_id: int, file_path: str) -> tuple[int | None, EmulatorInvocation | None, str]:
        """Return the ROM's ``(shortcut_app_id, emulator, bake_path)`` for the re-bake.

        Reads the ROM + its fresh install record in a short read UoW, then
        resolves the ROM's FULL active emulator through the shared ``active_core``
        resolver and the multi-disc launch path through the shared
        ``disc_resolver`` so ``download_complete`` re-bakes the right launch
        command. ``app_id`` is ``None`` when the ROM has no Steam shortcut yet
        (not synced) — the frontend no-ops and the next sync writes the launch
        command. ``emulator`` is the resolved :class:`EmulatorInvocation` (libretro
        core or standalone) when the ROM's per-game/per-platform/system resolution
        yields one (bake the ``-e`` form), ``None`` when it resolves to nothing — a
        genuinely unresolvable platform (bake the plain launch). ``bake_path`` is
        the selected disc's path for a multi-disc ROM (the persisted pick survives
        uninstall → reinstall, just like the core override), or *file_path*
        unchanged for a single-disc ROM. The resolver already warns + degrades on
        a stale label/pin, so no bogus invocation or missing-disc path ever
        reaches the bake. This is the load-bearing site: the per-game override and
        the disc pin both live on ``roms`` so they survive uninstall → reinstall,
        and reinstall goes through here.
        """
        with self._uow_factory() as uow:
            rom = uow.roms.get(int(rom_id))
            install = uow.rom_installs.get(int(rom_id))
            selected_disc = rom.selected_disc if rom is not None else None
        if rom is None:
            return (None, None, file_path)
        emulator = self._active_core.active_emulator_for_rom(int(rom_id))
        # The install record was committed just before this read, so it is
        # present in the normal flow; guard for the rare race where it is not and
        # fall back to the raw download path (no multi-disc resolution possible).
        bake_path = (
            self._disc_resolver.resolve_for_install(install, selected_disc) if install is not None else file_path
        )
        return (rom.shortcut_app_id, emulator, bake_path)

    def _make_progress_callback(self, rom_id, rom_name, platform_name, file_name, control=None):
        """Build a throttled progress callback for a download."""
        if control is None:
            control = _DownloadControl()
        last_emit = [0.0]  # mutable container for closure
        last_log = [0.0]

        def progress_callback(downloaded, total):
            if control.cancelled or control.paused or control.shutdown:
                # Abort the in-flight transfer thread (#144). CancelledError is a
                # BaseException, so it propagates untouched through the adapter's
                # Exception-only retry/translate — no retry, no error translation.
                # Both cancel and pause unwind through here; the terminal handling
                # in ``_do_download`` branches on which flag was set.
                raise asyncio.CancelledError()
            now = self._clock.monotonic()
            if now - last_log[0] >= 30.0:
                last_log[0] = now
                self._log_download_progress(rom_name, downloaded, total)
            if now - last_emit[0] < 0.5 and downloaded < total:
                return
            last_emit[0] = now
            progress = downloaded / total if total else 0

            # This callback runs on a ``run_in_executor`` worker thread. Both the
            # queue-dict mutation and the emit-scheduling must happen on the loop
            # thread, so marshal them across via ``call_soon_threadsafe`` (#973).
            self._loop.call_soon_threadsafe(
                self._apply_download_progress,
                rom_id,
                rom_name,
                platform_name,
                file_name,
                progress,
                downloaded,
                total,
            )

        return progress_callback

    def _log_download_progress(self, rom_name, downloaded, total):
        """Log a throttled one-line human-readable progress summary (MB + %)."""
        mb_dl = downloaded / (1024 * 1024)
        mb_total = total / (1024 * 1024) if total else 0
        pct = (downloaded / total * 100) if total else 0
        self._logger.info(f"Download progress: {rom_name} — {mb_dl:.1f}/{mb_total:.1f} MB ({pct:.0f}%)")

    def _apply_download_progress(self, rom_id, rom_name, platform_name, file_name, progress, downloaded, total):
        """Update the live queue entry and schedule a ``download_progress`` emit.

        Runs on the loop thread (marshaled from the executor worker via
        ``call_soon_threadsafe``). Guarded by ``.get`` — if the entry was evicted
        between ticks we must not resurrect it or raise KeyError off-thread (#973).
        """
        entry = self._download_queue.get(rom_id)
        if entry is None:
            return  # evicted mid-download — do not resurrect or emit
        entry.update(
            {
                "progress": progress,
                "bytes_downloaded": downloaded,
                "total_bytes": total,
            }
        )
        # Once per attempt: persist the server's full size so a restart can tell
        # a complete partial (skip the transfer) from an incomplete one.
        self._persist_transfer_observation(rom_id, total)
        self._loop.create_task(
            self._emit(
                "download_progress",
                {
                    "rom_id": rom_id,
                    "rom_name": rom_name,
                    "platform_name": platform_name,
                    "file_name": file_name,
                    "status": "downloading",
                    "progress": progress,
                    "bytes_downloaded": downloaded,
                    "total_bytes": total,
                    "resumable": entry.get("resumable", False),
                },
            )
        )

    def _make_extract_callback(self, rom_id, rom_name, platform_name, file_name):
        """Build a throttled extraction-progress callback for a multi-file ROM.

        Mirrors ``_make_progress_callback`` but for the post-transfer ZIP
        extraction. ``last_emit`` starts at 0.0 so the FIRST tick emits
        immediately — the UI switches to the "extracting" phase promptly once
        the byte transfer finishes. Emits are then throttled to 0.5s and a
        human-readable log line to ~30s. Unlike the download callback this does
        NOT poll the cancel/pause token: extraction is not cancellable this
        iteration. Runs on the executor worker thread, so the queue mutation
        and emit-scheduling are marshaled to the loop thread via
        ``call_soon_threadsafe`` (#973).
        """
        last_emit = [0.0]  # mutable container for closure
        last_log = [0.0]

        def extract_callback(extracted, total):
            now = self._clock.monotonic()
            if now - last_log[0] >= 30.0:
                last_log[0] = now
                self._log_extract_progress(rom_name, extracted, total)
            if now - last_emit[0] < 0.5 and extracted < total:
                return
            last_emit[0] = now
            progress = extracted / total if total else 0
            self._loop.call_soon_threadsafe(
                self._apply_extract_progress,
                rom_id,
                rom_name,
                platform_name,
                file_name,
                progress,
                extracted,
                total,
            )

        return extract_callback

    def _log_extract_progress(self, rom_name, extracted, total):
        """Log a throttled one-line human-readable extraction summary (MB + %)."""
        mb_done = extracted / (1024 * 1024)
        mb_total = total / (1024 * 1024) if total else 0
        pct = (extracted / total * 100) if total else 0
        self._logger.info(f"Extract progress: {rom_name} — {mb_done:.1f}/{mb_total:.1f} MB ({pct:.0f}%)")

    def _apply_extract_progress(self, rom_id, rom_name, platform_name, file_name, progress, extracted, total):
        """Update the live queue entry and schedule an ``extracting`` ``download_progress`` emit.

        Runs on the loop thread (marshaled from the executor worker via
        ``call_soon_threadsafe``). Guarded by ``.get`` — if the entry was evicted
        between ticks we must not resurrect it or raise KeyError off-thread (#973).
        Mirrors ``_apply_download_progress`` but flips the entry to the
        "extracting" phase and reports ``resumable: False`` (extraction is never
        resumable).
        """
        entry = self._download_queue.get(rom_id)
        if entry is None:
            return  # evicted mid-extraction — do not resurrect or emit
        entry.update(
            {
                "status": "extracting",
                "progress": progress,
                "bytes_downloaded": extracted,
                "total_bytes": total,
            }
        )
        self._loop.create_task(
            self._emit(
                "download_progress",
                {
                    "rom_id": rom_id,
                    "rom_name": rom_name,
                    "platform_name": platform_name,
                    "file_name": file_name,
                    "status": "extracting",
                    "progress": progress,
                    "bytes_downloaded": extracted,
                    "total_bytes": total,
                    "resumable": False,
                },
            )
        )

    async def _finalize_download_complete(self, rom_id, rom_detail, final_path, rom_name, platform_name):
        """Mark the queue entry completed and emit ``download_complete``.

        Resolves the bound Steam ``shortcut_app_id`` for this rom_id (or ``None``
        when the ROM hasn't been synced yet) plus the ROM's full active core
        (resolved ``.so`` or ``None``) and the multi-disc launch path so the
        frontend confirm-sets launch options on the exact shortcut without a
        full-library scan to re-resolve rom_id→app_id, and the re-bake keeps the
        per-game/per-platform core AND the selected disc across uninstall →
        reinstall. Called from the normal success path and from the cancel handler
        when the install committed before the cancel landed (#1049).
        """
        self._drop_job(rom_id)
        entry = self._download_queue[rom_id]
        entry["status"] = "completed"
        entry["progress"] = 1.0
        entry.pop("reason", None)
        app_id, emulator, bake_path = await self._loop.run_in_executor(
            None, self._resolve_bound_app_id, rom_id, final_path
        )
        resolution = self._launch_resolver.resolve_for(rom_detail, emulator, rom_detail.get("platform_slug"))
        await self._emit(
            "download_complete",
            {
                "rom_id": rom_id,
                "rom_name": rom_name,
                "platform_name": platform_name,
                "file_path": final_path,
                "app_id": app_id,
                "launch_options": build_launch_options(resolution.launch_options, bake_path),
                "resumable": entry.get("resumable", False),
            },
        )
        self._logger.info(f"Download complete: {rom_name} -> {final_path}")

    async def _reconcile_post_io(self, post_io_future):
        """After a cancel, settle an in-flight post-IO commit and report whether
        the install committed. Executor threads run to completion regardless of
        cancellation, so letting the future settle here lets a race-committed
        install be honored instead of torn down. Returns (final_path, committed).
        """
        if post_io_future is None:
            return (None, False)  # cancel landed before the post-IO phase started
        # Let the (shielded) executor future settle WITHOUT awaiting it directly:
        # ``asyncio.wait`` reports completion through the future's own state, so a
        # cancelled or failed commit is inspected here, never swallowed — the
        # caller's ``except asyncio.CancelledError`` keeps ownership of the re-raise.
        await asyncio.wait({post_io_future})
        if post_io_future.cancelled() or post_io_future.exception() is not None:
            # Executor work was cancelled before it ran, or the commit raised.
            return (None, False)
        final_path, post_io_error = post_io_future.result()
        if post_io_error is None and final_path is not None:
            return (final_path, True)
        return (None, False)

    def _make_on_meta(self, rom_id, rom_name, platform_name, file_name):
        """Build the one-shot resumability callback the adapter fires when the
        download's response headers arrive (before the body streams).

        It records the server's ``range_supported`` verdict on the queue entry
        and emits a ``download_progress`` frame carrying it, so the frontend can
        flip Pause/Cancel live DURING the transfer instead of only learning the
        verdict at the end. Runs on the executor transfer thread, so it hops back
        to the loop via ``call_soon_threadsafe`` like the progress callback (#973).
        """

        def on_meta(range_supported: bool) -> None:
            def _apply() -> None:
                entry = self._download_queue.get(rom_id)
                if entry is None:
                    return  # evicted mid-download — do not resurrect or emit
                entry["resumable"] = range_supported
                self._loop.create_task(
                    self._emit(
                        "download_progress",
                        {
                            "rom_id": rom_id,
                            "rom_name": rom_name,
                            "platform_name": platform_name,
                            "file_name": file_name,
                            "status": entry.get("status", "downloading"),
                            "progress": entry.get("progress", 0),
                            "bytes_downloaded": entry.get("bytes_downloaded", 0),
                            "total_bytes": entry.get("total_bytes", 0),
                            "resumable": range_supported,
                        },
                    )
                )

            self._loop.call_soon_threadsafe(_apply)

        return on_meta

    def _expected_transfer_total(self, rom_id: int, rom_detail: dict[str, Any], has_multiple: bool) -> int:
        """Full size of the transfer file, or 0 when unknown.

        The server-reported total persisted on the job wins; a single-file ROM
        falls back to RomM's ``fs_size_bytes`` (the file IS the transfer). A
        multi-file ZIP's size is only known once the server has reported it.
        """
        job = self._load_job(rom_id)
        if job is not None and job.total_bytes:
            return job.total_bytes
        if has_multiple:
            return 0
        return int(rom_detail.get("fs_size_bytes") or 0)

    async def _transfer(self, rom_id, rom_detail, tmp_path, file_name, progress_callback, on_meta, *, resume, has_multiple):
        """Run the HTTP transfer into *tmp_path* unless the partial is already complete.

        Returns ``True`` when the transfer was skipped because the ``.tmp``
        already holds the full file (a resume after a crash between transfer
        and install — re-requesting would draw a 416). A partial LARGER than
        the expected size is stale: it is deleted and the transfer restarts.
        """
        if resume:
            expected = self._expected_transfer_total(rom_id, rom_detail, has_multiple)
            on_disk = self._download_file_store.file_size(tmp_path)
            if expected and on_disk == expected:
                self._logger.info(f"Partial for ROM {rom_id} is already complete ({on_disk} bytes); skipping transfer")
                self._apply_download_progress_entry(rom_id, on_disk, expected)
                return True
            if expected and on_disk > expected:
                self._logger.warning(
                    f"Partial for ROM {rom_id} exceeds the expected size ({on_disk} > {expected}); restarting"
                )
                self._remove_tmp_files([tmp_path])
                resume = False
        await self._loop.run_in_executor(
            None,
            partial(
                self._romm_api.download_rom_content,
                rom_id,
                file_name,
                tmp_path,
                progress_callback,
                resume=resume,
                on_meta=on_meta,
            ),
        )
        return False

    def _apply_download_progress_entry(self, rom_id: int, downloaded: int, total: int) -> None:
        entry = self._download_queue.get(rom_id)
        if entry is None:
            return
        entry.update({"progress": downloaded / total if total else 0, "bytes_downloaded": downloaded, "total_bytes": total})

    async def _do_download(self, rom_id, rom_detail, target_path, system, file_name, control=None, *, resume=False):
        if control is None:
            # Direct invocation (no ``_begin_download``): own + register a control
            # so the ``finally``'s identity-gated cleanup releases this task's
            # registrations like the real path does.
            control = _DownloadControl()
            self._control_tokens[rom_id] = control
        rom_name = rom_detail.get("name", file_name)
        platform_name = rom_detail.get("platform_name", rom_detail.get("platform_slug", ""))
        has_multiple = is_multi_file_download(rom_detail)
        # Name the extract dir from the ROM's identity, not files[0] (which may be
        # an arbitrary inner asset for a nested-single folder game). Computed once
        # here and threaded to both the extraction and the cleanup so the dir a
        # failure tears down matches the dir extraction created. Only multi-file
        # ROMs own a dedicated dir, so single-file skips the derivation.
        extract_dir_name = self._resolve_safe_extract_dir_name(rom_detail) if has_multiple else ""
        progress_callback = self._make_progress_callback(rom_id, rom_name, platform_name, file_name, control)
        on_meta = self._make_on_meta(rom_id, rom_name, platform_name, file_name)
        # Tracks the resolved launch path once extraction returns it, so a
        # failure AFTER the ES-DE collapse rename cleans up the *renamed* dir
        # (``os.path.dirname(final_path)``) — not just the staging name.
        final_path: str | None = None
        # The post-IO commit future, declared before the try so the cancel
        # handler can reconcile a race-committed install (#1049). Stays ``None``
        # while waiting for the transfer slot or during the transfer.
        post_io_future: asyncio.Future[tuple[str | None, str | None]] | None = None

        try:
            self._logger.info(f"Download starting: {rom_name} (rom_id={rom_id}, multi={has_multiple}) -> {target_path}")

            # One transfer at a time (D5). If the slot is taken, surface an honest
            # "queued" frame so the UI shows the wait instead of a stalled
            # "downloading" bar.
            if self._active_control is not None and self._active_control is not control:
                entry = self._download_queue[rom_id]
                entry["status"] = "queued"
                await self._emit(
                    "download_progress",
                    {
                        "rom_id": rom_id,
                        "rom_name": rom_name,
                        "platform_name": platform_name,
                        "file_name": file_name,
                        "status": "queued",
                        "progress": entry.get("progress", 0),
                        "bytes_downloaded": entry.get("bytes_downloaded", 0),
                        "total_bytes": entry.get("total_bytes", rom_detail.get("fs_size_bytes", 0)),
                        "resumable": entry.get("resumable", False),
                    },
                )

            await self._acquire_transfer_slot(rom_id, control)
            self._download_queue[rom_id]["status"] = "downloading"
            self._download_queue[rom_id].pop("reason", None)
            started_at = self._now_iso()
            self._mutate_job(rom_id, lambda job: job.start(started_at))

            if has_multiple:
                # Multi-file ROM: API returns ZIP, download to temp then extract
                tmp_zip = target_path + _ZIP_TMP_EXT
                skipped = await self._transfer(
                    rom_id,
                    rom_detail,
                    tmp_zip,
                    file_name,
                    progress_callback,
                    on_meta,
                    resume=resume,
                    has_multiple=True,
                )
                extract_at = self._now_iso()
                self._mutate_job(rom_id, lambda job: job.begin_extract(extract_at))
                # A complete archive left by an attempt that died mid-extraction
                # re-extracts into a clean staging dir.
                post_io = (
                    partial(self._post_download_multi_io, fresh_extract=True)
                    if skipped
                    else self._post_download_multi_io
                )
                post_io_future = self._loop.run_in_executor(
                    None, post_io, rom_id, rom_detail, target_path, file_name, system, extract_dir_name
                )
                # Shield the commit await: a cancel here must propagate to this
                # coroutine WITHOUT cancelling the underlying future, so
                # ``_reconcile_post_io`` can re-await it for the real result. A
                # bare ``await`` cancels the asyncio future (the executor thread
                # still commits), so the re-await would raise CancelledError and
                # the committed install would be mis-reported as not committed
                # → torn down (#1049).
                final_path, post_io_error = await asyncio.shield(post_io_future)
            else:
                tmp_path = target_path + _TMP_EXT
                await self._transfer(
                    rom_id,
                    rom_detail,
                    tmp_path,
                    file_name,
                    progress_callback,
                    on_meta,
                    resume=resume,
                    has_multiple=False,
                )
                post_io_future = self._loop.run_in_executor(
                    None, self._post_download_single_io, rom_id, rom_detail, target_path, system
                )
                # Shielded so a racing cancel leaves the future intact for
                # ``_reconcile_post_io`` to re-await (see the multi-file branch).
                final_path, post_io_error = await asyncio.shield(post_io_future)

            if post_io_error is not None or final_path is None:
                # The download succeeded but the install record failed its
                # invariant; the artifact was already cleaned up by the worker.
                # ``final_path is None`` always coincides with a non-None error
                # — the guard narrows the type for the launch-command build below.
                raise ValueError(post_io_error or "install record produced no launch path")

            await self._finalize_download_complete(rom_id, rom_detail, final_path, rom_name, platform_name)

        except asyncio.CancelledError:
            # The cancel/pause may have raced a committing install. Executor
            # threads run to completion, so wait for the post-IO future before
            # deciding (#1049).
            committed_path, committed = await self._reconcile_post_io(post_io_future)
            if committed:
                # The ROM IS installed — surface completed, don't tear it down.
                # This also bakes launch_options for the just-committed install.
                await self._finalize_download_complete(rom_id, rom_detail, committed_path, rom_name, platform_name)
                self._logger.info(f"Download cancelled after install committed; surfaced as complete: {rom_name}")
            elif control.shutdown:
                # PLUGIN UNLOAD: keep the partial AND the job row exactly as they
                # are — the next startup reconciles the job to "interrupted" and
                # resumes it. Nothing is emitted (the frontend is going away too).
                self._logger.info(f"Download stopped by plugin shutdown; will resume on restart: {rom_name}")
            elif control.paused:
                # PAUSE: keep the partial ``.tmp`` so the transfer can resume from
                # where it stopped. NOTHING is cleaned up. The entry stays "paused"
                # in the queue (``_prune_download_queue`` only prunes terminal
                # completed/failed/cancelled, so "paused" is retained) and the job
                # row survives a restart without auto-resuming.
                paused_at = self._now_iso()
                self._mutate_job(rom_id, lambda job: job.pause(paused_at))
                entry = self._download_queue[rom_id]
                entry["status"] = "paused"
                entry.pop("reason", None)
                await self._emit_entry_frame(rom_id, entry)
                self._logger.info(f"Download paused: {rom_name}")
            else:
                entry = self._download_queue[rom_id]
                entry["status"] = "cancelled"
                entry.pop("reason", None)
                self._drop_job(rom_id)
                self._cleanup_partial_download(target_path, has_multiple, extract_dir_name, final_path)
                # #1017: emit a terminal frame so the frontend resets the button
                # out of its downloading state (the global cancel path was silent).
                await self._emit_entry_frame(rom_id, entry)
                self._logger.info(f"Download cancelled: {rom_name}")
            raise

        except Exception as e:
            outcome, reason = classify_download_failure(e)
            if outcome == _OUTCOME_INTERRUPTED and reason is not None:
                # Transient: keep the partial so the user (or the next startup)
                # resumes from where it stopped.
                self._logger.warning(f"Download interrupted for {rom_name} ({reason}): {e}")
                await self._mark_interrupted(rom_id, reason)
            else:
                self._download_queue[rom_id]["status"] = "failed"
                self._download_queue[rom_id]["error"] = str(e)
                self._download_queue[rom_id].pop("reason", None)
                self._drop_job(rom_id)
                self._cleanup_partial_download(target_path, has_multiple, extract_dir_name, final_path)
                self._logger.error(f"Download failed for {rom_name}: {e}")
                await self._emit(
                    "download_failed",
                    {
                        "rom_id": rom_id,
                        "rom_name": rom_name,
                        "platform_name": platform_name,
                        "error_message": str(e),
                    },
                )

        finally:
            # Hand the transfer slot to the next queued job (no-op when this
            # attempt never held it).
            self._release_transfer_slot(control)
            # A re-download (or resume) can overwrite these per-download
            # registrations with a fresh attempt's BEFORE this (older/superseded)
            # task's finally runs. Gate ALL of them on the control-token identity
            # so a zombie/superseded task never evicts the newer attempt's task,
            # in-progress flag, reservation, or token (#144). The control is
            # registered by ``_begin_download``; a direct-call test that never
            # registered it simply skips these no-op pops.
            if self._control_tokens.get(rom_id) is control:
                self._download_tasks.pop(rom_id, None)
                self._download_in_progress.discard(rom_id)
                self._reserved_bytes.pop(rom_id, None)
                del self._control_tokens[rom_id]
            self._prune_download_queue()

    def _maybe_heal_ps3_sfb_io(self, extract_dir: str) -> None:
        """Copy a folder-boot dump's ``PS3_DISC.SFB.txt`` back to ``PS3_DISC.SFB``.

        RPCS3 identifies a disc-dump folder by a ``PS3_DISC.SFB`` at the game
        root; some RomM dumps ship it renamed ``PS3_DISC.SFB.txt`` and never
        boot until it is restored (verified on-device, #1212). Only for a
        folder-boot layout (:func:`folder_boot_layout_root` locates the game
        root) and only the exact ``.txt`` → real-name case: when the ``.txt``
        exists and the real name does not, a copy is written (the original is
        kept). Any other shape — real SFB already present, no ``.txt``, no
        folder-boot layout — is left untouched. Scoped to this one known dump
        quirk, not a general renamer.
        """
        all_files = self._download_file_store.scan_files_with_sizes(extract_dir)
        root = folder_boot_layout_root([path for path, _size in all_files])
        if root is None:
            return
        sfb = os.path.join(root, "PS3_DISC.SFB")
        sfb_txt = os.path.join(root, "PS3_DISC.SFB.txt")
        if self._download_file_store.exists(sfb_txt) and not self._download_file_store.exists(sfb):
            self._download_file_store.copy_file(sfb_txt, sfb)
            self._logger.info("healed PS3_DISC.SFB from .txt-suffixed dump: %s", sfb)

    def _maybe_generate_m3u_io(self, extract_dir: str, rom_detail: dict[str, Any], m3u_supported: bool) -> None:
        """Auto-generate a game-named M3U playlist when one is warranted (see ``needs_m3u``).

        Writes ``<fs_name_no_ext>.m3u`` when the platform supports ``.m3u``, no
        M3U already exists, and the disc files warrant one: multi-disc ROMs (any
        of cue/chd/iso) for disc switching, or a single-disc bin/cue ROM so the
        extract dir collapses to a game-named entry in ES-DE. When the platform
        does not support ``.m3u`` (per ES-DE's ``es_systems.xml``), no playlist
        is generated.
        """
        if not m3u_supported:
            return

        all_files = self._download_file_store.scan_files_with_sizes(extract_dir)
        # A folder-boot dump (PS3 — …/PS3_GAME/USRDIR/EBOOT.BIN) launches the game
        # directory directly (ADR-0019), never a playlist — and its many payload
        # files must never be misread as "discs". Suppress the M3U outright even
        # though ES-DE lists .m3u for the platform. (#1212)
        if folder_boot_layout_root([path for path, _size in all_files]) is not None:
            return
        # Check if an M3U already exists (search recursively)
        if any(path.lower().endswith(".m3u") for path, _size in all_files):
            return

        # Collect disc files (the DISC_IMAGE_EXTENSIONS set; search recursively).
        disc_suffixes = tuple(DISC_IMAGE_EXTENSIONS)
        disc_files = [
            os.path.relpath(path, extract_dir) for path, _size in all_files if path.lower().endswith(disc_suffixes)
        ]

        if not needs_m3u(disc_files, m3u_supported):
            return

        rom_name = rom_detail.get("fs_name_no_ext", rom_detail.get("name", "playlist"))
        m3u_path = os.path.join(extract_dir, f"{rom_name}.m3u")
        self._download_file_store.write_text_atomic(m3u_path, build_m3u_content(disc_files))
        self._logger.info(f"Auto-generated M3U playlist: {m3u_path}")

    def _collect_and_detect_launch_file(self, extract_dir: str, m3u_supported: bool) -> str:
        """Find the best launch file in an extracted multi-file ROM directory.

        *m3u_supported* gates whether a ``.m3u`` (including a RomM-bundled one)
        may be chosen as the launch file — see ``detect_launch_file``.
        """
        all_files = self._download_file_store.scan_files_with_sizes(extract_dir)
        result = detect_launch_file(all_files, m3u_supported)
        return result if result is not None else extract_dir

    def _cleanup_partial_download(self, target_path, has_multiple, extract_dir_name, final_path=None):
        """Clean up partial download files. Each step is independent so one failure doesn't block others.

        Only ever called for a download that did NOT commit an install (the
        failure path and the cancel-without-commit path); a cancel that loses the
        race to a committed install routes to ``_finalize_download_complete``
        instead and never reaches here.

        Removes ONLY the transient transfer artifacts (``.zip.tmp`` / ``.tmp``)
        and, for a multi-file ROM, the extract dir(s) this download created. The
        bare ``target_path`` is NEVER removed: a single-file transfer writes to
        ``target_path + .tmp`` and only renames to ``target_path`` on success, so
        deleting the bare path would destroy a PRE-EXISTING install (a re-download
        that fails mid-stream) or a just-committed one (a cancel race) — the #1049
        data-loss bug.

        For a multi-file ROM the extract dir may have been renamed for ES-DE
        collapse after extraction. *final_path* (the resolved launch file,
        ``None`` until extraction returns it) lets cleanup tear down whichever
        of the two dir names exists — the staging name *and* the renamed dir
        (``os.path.dirname(final_path)``) — so no failure path orphans a dir.

        *extract_dir_name* is the same ROM-identity-derived base name extraction
        created the staging dir under (``_resolve_safe_extract_dir_name``), so
        cleanup targets exactly that dir rather than re-deriving it from a stale
        source (unused for single-file ROMs, which own no dir).
        """
        paths_to_remove = [
            target_path + _ZIP_TMP_EXT,
            target_path + _TMP_EXT,
        ]
        for path in paths_to_remove:
            try:
                self._download_file_store.remove_file(path)
            except Exception as e:
                self._logger.warning(f"Cleanup failed for {path}: {e}")
        if has_multiple:
            staging_dir = os.path.join(os.path.dirname(target_path), extract_dir_name)
            dirs_to_remove = {staging_dir}
            if final_path:
                dirs_to_remove.add(os.path.dirname(final_path))
            for extract_dir in dirs_to_remove:
                try:
                    self._download_file_store.remove_tree(extract_dir)
                except Exception as e:
                    self._logger.warning(f"Cleanup failed for directory {extract_dir}: {e}")

    def cancel_download(self, rom_id):
        """Cancel a download: abort a live one, or discard a paused/interrupted one.

        A live (queued or transferring) download is aborted through its control
        token + task; ``_do_download``'s cancel branch deletes the partial and the
        job row. A paused or interrupted download has no task — it is discarded
        here directly (row + partial deleted, entry marked ``cancelled``).
        """
        rom_id = int(rom_id)
        task = self._download_tasks.get(rom_id)
        if not task:
            entry = self._download_queue.get(rom_id)
            if entry is not None and entry.get("status") in _RESUMABLE_STATUSES:
                self._loop.create_task(self._discard_resumable(rom_id, status="cancelled"))
                return {"success": True, "message": "Download cancelled"}
            return {"success": False, "reason": "no_active_download", "message": "No active download for this ROM"}
        token = self._control_tokens.get(rom_id)
        if token is not None:
            token.cancelled = True  # stop the executor transfer thread, not just the asyncio wrapper (#144)
        task.cancel()
        return {"success": True, "message": "Download cancelled"}

    def pause_download(self, rom_id):
        """Pause an in-flight download, keeping the partial ``.tmp`` for resume.

        Mirrors ``cancel_download`` but flips ``control.paused`` instead of
        ``cancelled`` before cancelling the task, so ``_do_download``'s terminal
        handler routes to the pause branch (status "paused", no cleanup) rather
        than the cancel branch (status "cancelled", ``.tmp`` deleted). Kept
        defensive — the frontend only offers Pause when a download is resumable.
        """
        rom_id = int(rom_id)
        task = self._download_tasks.get(rom_id)
        if not task:
            return {"success": False, "reason": "no_active_download", "message": "No active download for this ROM"}
        token = self._control_tokens.get(rom_id)
        if token is not None:
            token.paused = True  # stop the executor transfer thread, keeping the .tmp (#144)
        task.cancel()
        return {"success": True, "message": "Download paused"}

    async def resume_download(self, rom_id):
        """Resume a paused or interrupted download from its partial ``.tmp``.

        Requires a queue entry in status "paused" or "interrupted"; otherwise
        returns the ``not_paused`` failure shape. A switch may have moved the group's shortcut
        binding to a sibling while this download was paused (#1298 S1): if another
        member now owns the shortcut, this target is stale — the resume is refused
        (``superseded``) and its queue entry dropped rather than re-downloading a
        version the picker has already moved away from. Otherwise the same sibling
        supersede that guards ``start_download`` runs first (abort on removal
        failure), then the download re-begins with ``resume=True`` so the transfer
        appends onto the existing bytes (when the server honoured the original
        ``Range`` probe) instead of restarting. The in-progress claim is taken
        before the supersede await and released on every early exit (B1).
        """
        rom_id = int(rom_id)
        entry = self._download_queue.get(rom_id)
        if entry is None or entry.get("status") not in _RESUMABLE_STATUSES:
            return {"success": False, "reason": "not_paused", "message": "No paused download for this ROM"}
        if rom_id in self._download_in_progress:
            return {"success": False, "reason": "already_downloading", "message": "Already downloading"}
        if self._resume_target_superseded(rom_id):
            self.evict(rom_id)
            return {"success": False, "reason": "superseded", "message": "Another version is now active"}
        self._download_in_progress.add(rom_id)
        try:
            cleanup_failure = await self._remove_conflicting_sibling_installs(rom_id)
        except Exception:
            self._download_in_progress.discard(rom_id)
            raise
        if cleanup_failure is not None:
            self._download_in_progress.discard(rom_id)
            return cleanup_failure
        return await self._begin_download(rom_id, resume=True)

    def _resume_target_superseded(self, rom_id: int) -> bool:
        """True when a sibling now owns the group's shortcut, stranding this resume.

        Reads the target's sibling group in one short UoW. A solo ROM (no group
        key), a target that is itself a bound member, or a group with no bound
        member at all is never superseded — those resume normally. Only a group
        whose shortcut binding has moved to a DIFFERENT member (a version switch
        landed while this download was paused) strands this target: resuming it
        would re-create a second installed version the supersede predicate can't
        reach (it protects installs bound to a different app_id — #1298 S1).
        """
        with self._uow_factory() as uow:
            rom = uow.roms.get(rom_id)
            if rom is None or rom.sibling_group_key is None:
                return False
            if rom.shortcut_app_id is not None:
                return False  # target is (a) bound member — resume normally
            return any(
                member.rom_id != rom_id and member.shortcut_app_id is not None
                for member in uow.roms.iter_by_group_key(rom.sibling_group_key)
            )

    def get_download_queue(self):
        return {"downloads": list(self._download_queue.values())}

    def active_download_rom_ids(self) -> set[int]:
        """Snapshot the rom ids with an in-flight or queued download (#1298 F1).

        VersionSwitchService consults this to refuse a switch while any member of
        the target's sibling group is actively downloading (the user cancels
        first). A *paused* download is not in this set — its task's ``finally``
        already released the in-progress claim — so a switch is allowed while a
        sibling is paused, and a later resume of a superseded target is refused by
        ``resume_download`` instead. An *interrupted* download (transient failure
        or plugin restart, not yet auto-resumed) is excluded the same way. Returns a copy so the caller cannot mutate the
        live control set.
        """
        return set(self._download_in_progress)

    def get_installed_rom(self, rom_id: int) -> InstalledRomEntry | None:
        """Return the install record for *rom_id* as a frontend-shaped dict, or ``None``.

        Reads the ``RomInstall`` aggregate via the Unit of Work and projects it
        onto the ``InstalledRom`` shape the QAM panel + launch gate consume.
        ``file_name`` is derived from the launch ``file_path`` (the aggregate
        stores the launch file, not the original archive name).
        """
        with self._uow_factory() as uow:
            install = uow.rom_installs.get(int(rom_id))
        if install is None:
            return None
        entry: InstalledRomEntry = {
            "rom_id": install.rom_id,
            "file_name": os.path.basename(install.file_path),
            "file_path": install.file_path,
            "system": install.system,
            "platform_slug": install.platform_slug,
            "installed_at": install.installed_at,
        }
        return entry

    # ── DownloadQueueCleanup Protocol ──────────────────────────────

    def evict(self, rom_id: int) -> None:
        """Remove the queue entry and job row for *rom_id* if present. Idempotent.

        A paused or interrupted download's partial is deleted with it (nothing
        will resume it any more); a live transfer's partial is left to that
        transfer's own terminal handling.
        """
        rom_id = int(rom_id)
        entry = self._download_queue.pop(rom_id, None)
        if entry is not None and entry.get("status") in _RESUMABLE_STATUSES and rom_id not in self._download_tasks:
            job = self._load_job(rom_id)
            if job is not None:
                self._remove_tmp_files([job.tmp_path])
        self._drop_job(rom_id)

    def clear(self) -> None:
        """Remove all entries from the download queue, with their job rows."""
        for rom_id in list(self._download_queue):
            self.evict(rom_id)
        self._download_queue.clear()
