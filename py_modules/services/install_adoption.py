"""InstallAdoptionService — treat ROM files already on disk as installed.

A ROM counts as installed only when a ``RomInstall`` row exists. Files that
reached the ROM folders some other way (copied by hand, or downloaded by an
older plugin) are adopted here: when every file sits exactly where this
plugin's download would put it, with exactly RomM's size, the same
``RomInstall`` a finished download writes is recorded. Anything that decides
whether on-disk files may be adopted, or records an adoption, belongs here;
the path/match decision itself is the pure kernel in
``domain.install_layout`` (shared with the download path).

Adoption never touches the files (no rename, no extraction, no playlist
generation) and never holds a Unit of Work while probing the filesystem.
"""

from __future__ import annotations

import asyncio
import os
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any

from domain.install_layout import (
    AdoptionCandidate,
    MultiFileLayout,
    SingleFileLayout,
    expected_install_layout,
    multi_file_matches,
    pick_one_per_group,
    single_file_matches,
)
from domain.rom_files import detect_launch_file
from domain.rom_install import RomInstall
from lib.path_safety import PathTraversalError, safe_join

if TYPE_CHECKING:
    import logging

    from services.protocols import (
        ActiveDownloadRomIdsFn,
        Clock,
        DiscResolver,
        EventEmitter,
        InstalledFileReader,
        RetroDeckPaths,
        RommRomReader,
        RomRelaunchItemReader,
        SystemM3uSupportFn,
        SystemResolver,
        UnitOfWork,
        UnitOfWorkFactory,
    )

# A game page asks for its cached detail several times while it mounts; one
# RomM round-trip per ROM per window is plenty.
_CHECK_COOLDOWN_SEC = 30.0


@dataclass(frozen=True)
class InstallAdoptionServiceConfig:
    """Frozen wiring bundle handed to ``InstallAdoptionService.__init__``.

    Holds the RomM ROM reader (the single-ROM check fetches the ROM's file
    names and sizes), the read-only installed-file probe, the platform-dir
    resolution seams the downloader uses (``retrodeck_paths`` /
    ``resolve_system`` / ``m3u_support``), the shared ``disc_resolver`` (bake
    path for a multi-disc adoption at sync time), the relaunch-item reader (the
    launch command a page-time adoption hands the frontend), the downloader's
    active-download snapshot, the Unit-of-Work factory, and runtime
    infrastructure.
    """

    romm_api: RommRomReader
    file_reader: InstalledFileReader
    resolve_system: SystemResolver
    retrodeck_paths: RetroDeckPaths
    m3u_support: SystemM3uSupportFn
    disc_resolver: DiscResolver
    relaunch_items: RomRelaunchItemReader
    active_downloads: ActiveDownloadRomIdsFn
    uow_factory: UnitOfWorkFactory
    loop: asyncio.AbstractEventLoop
    logger: logging.Logger
    clock: Clock
    emit: EventEmitter


@dataclass(frozen=True)
class _Eligibility:
    """Per-ROM DB facts read in one short UoW before any filesystem probe."""

    blocked: bool
    group_key: str | None
    selected_disc: str | None


class InstallAdoptionService:
    """Adopt pre-existing ROM files as installs (library sync + game page)."""

    def __init__(self, *, config: InstallAdoptionServiceConfig) -> None:
        self._romm_api = config.romm_api
        self._file_reader = config.file_reader
        self._resolve_system = config.resolve_system
        self._retrodeck_paths = config.retrodeck_paths
        self._m3u_support = config.m3u_support
        self._disc_resolver = config.disc_resolver
        self._relaunch_items = config.relaunch_items
        self._active_downloads = config.active_downloads
        self._uow_factory = config.uow_factory
        self._loop = config.loop
        self._logger = config.logger
        self._clock = config.clock
        self._emit = config.emit
        self._in_flight: dict[int, asyncio.Task[bool]] = {}
        self._last_check: dict[int, float] = {}

    # ── Eligibility (DB) ─────────────────────────────────────────

    @staticmethod
    def _sibling_installed(uow: UnitOfWork, rom_id: int, group_key: str | None) -> bool:
        if group_key is None:
            return False
        return any(
            member.rom_id != rom_id and uow.rom_installs.get(member.rom_id) is not None
            for member in uow.roms.iter_by_group_key(group_key)
        )

    def _read_eligibility(self, roms: list[dict[str, Any]]) -> dict[int, _Eligibility]:
        """Snapshot, per ROM, whether adoption is blocked plus its group key and disc pin.

        Blocked when the ROM already has a ``RomInstall``, has a persisted
        download job (queued / downloading / extracting / paused / interrupted),
        or another version of its sibling group is installed. One short read
        UoW, closed before any filesystem probe.
        """
        out: dict[int, _Eligibility] = {}
        with self._uow_factory() as uow:
            for rom in roms:
                rom_id = int(rom["id"])
                row = uow.roms.get(rom_id)
                group_key = rom.get("sibling_group_key") or (row.sibling_group_key if row is not None else None)
                blocked = (
                    uow.rom_installs.get(rom_id) is not None
                    or uow.download_jobs.get(rom_id) is not None
                    or self._sibling_installed(uow, rom_id, group_key)
                )
                out[rom_id] = _Eligibility(
                    blocked=blocked,
                    group_key=group_key,
                    selected_disc=row.selected_disc if row is not None else None,
                )
        return out

    # ── Filesystem probe ─────────────────────────────────────────

    def _probe_install(self, rom: dict[str, Any]) -> RomInstall | None:
        """Return the ``RomInstall`` a download would have written, if the files match.

        Reads only (one ``stat`` per expected file). ``None`` when the layout
        is undeterminable, the platform path is unsafe, or any file is missing
        or has a different size.
        """
        layout = expected_install_layout(rom)
        if layout is None:
            return None
        platform_slug = rom.get("platform_slug", "") or ""
        system = self._resolve_system(platform_slug, rom.get("platform_fs_slug"))
        try:
            roms_dir = safe_join(self._retrodeck_paths.roms_path(), system)
        except (PathTraversalError, TypeError):
            return None
        if isinstance(layout, SingleFileLayout):
            return self._probe_single(rom, layout, roms_dir, platform_slug, system)
        return self._probe_multi(rom, layout, roms_dir, platform_slug, system)

    def _probe_single(
        self, rom: dict[str, Any], layout: SingleFileLayout, roms_dir: str, platform_slug: str, system: str
    ) -> RomInstall | None:
        try:
            safe_join(roms_dir, layout.file_name)
        except PathTraversalError:
            return None
        file_path = os.path.join(roms_dir, layout.file_name)
        if not single_file_matches(layout, self._file_reader.regular_file_size(file_path)):
            return None
        return self._build_install(rom, file_path=file_path, rom_dir=None, platform_slug=platform_slug, system=system)

    def _probe_multi(
        self, rom: dict[str, Any], layout: MultiFileLayout, roms_dir: str, platform_slug: str, system: str
    ) -> RomInstall | None:
        for dir_name in layout.dir_candidates:
            rom_dir = os.path.join(roms_dir, dir_name)
            if not self._file_reader.is_dir(rom_dir):
                continue
            observed: dict[str, int | None] = {}
            for expected in layout.files:
                try:
                    path = safe_join(rom_dir, *expected.parts)
                except PathTraversalError:
                    observed[expected.rel_path] = None
                    continue
                observed[expected.rel_path] = self._file_reader.regular_file_size(path)
            if not multi_file_matches(layout, observed):
                continue
            # Same launch-file pick the download makes after extraction.
            files = self._file_reader.scan_files_with_sizes(rom_dir)
            launch_file = detect_launch_file(files, self._m3u_support(system)) or rom_dir
            return self._build_install(
                rom, file_path=launch_file, rom_dir=rom_dir, platform_slug=platform_slug, system=system
            )
        return None

    def _build_install(
        self, rom: dict[str, Any], *, file_path: str, rom_dir: str | None, platform_slug: str, system: str
    ) -> RomInstall | None:
        """Build the same ``RomInstall`` a finished download records (``_record_install_io``)."""
        try:
            return RomInstall.mark_installed(
                rom_id=int(rom["id"]),
                file_path=file_path,
                rom_dir=rom_dir,
                platform_slug=platform_slug,
                system=system,
                installed_at=self._clock.now().isoformat(),
            )
        except ValueError:
            return None

    # ── InstallAdopter Protocol (library sync) ───────────────────

    def plan_adoptions(self, roms: list[dict[str, Any]]) -> list[AdoptionCandidate]:
        """Return the ROMs among *roms* whose files are already on disk, unadopted.

        Skips ROMs that are installed, have a download job or an active
        download, or whose sibling group already has an installed version; at
        most one version per sibling group is returned. Probes the filesystem
        with no Unit of Work open and writes nothing. Each candidate carries the
        disc-resolved bake path for the shortcut's launch command.
        """
        if not roms:
            return []
        eligibility = self._read_eligibility(roms)
        active = self._active_downloads()
        candidates: list[AdoptionCandidate] = []
        for rom in roms:
            rom_id = int(rom["id"])
            facts = eligibility[rom_id]
            if facts.blocked or rom_id in active:
                continue
            try:
                install = self._probe_install(rom)
            except OSError as e:
                self._logger.warning(f"Install adoption probe failed for rom {rom_id}: {e}")
                continue
            if install is None:
                continue
            bake_path = self._disc_resolver.resolve_for_install(install, facts.selected_disc)
            candidates.append(
                AdoptionCandidate(rom_id=rom_id, install=install, bake_path=bake_path, group_key=facts.group_key)
            )
        return pick_one_per_group(candidates)

    def commit_adoptions(self, candidates: list[AdoptionCandidate]) -> list[int]:
        """Persist the candidates' ``RomInstall`` rows in one short write UoW.

        Re-checks every guard inside the transaction (the ROM row exists, no
        install, no download job, no installed sibling) so a download or a
        sibling adoption that landed since the probe wins. Returns the adopted
        rom ids.
        """
        if not candidates:
            return []
        active = self._active_downloads()
        adopted: list[int] = []
        with self._uow_factory() as uow:
            for cand in candidates:
                if cand.rom_id in active or uow.roms.get(cand.rom_id) is None:
                    continue
                if uow.rom_installs.get(cand.rom_id) is not None or uow.download_jobs.get(cand.rom_id) is not None:
                    continue
                if self._sibling_installed(uow, cand.rom_id, cand.group_key):
                    continue
                uow.rom_installs.save(cand.install)
                adopted.append(cand.rom_id)
        for rom_id in adopted:
            self._logger.info(f"Adopted existing files as installed: rom {rom_id}")
        return adopted

    # ── Single ROM (game page) ───────────────────────────────────

    def schedule_adoption_check(self, rom_id: int) -> None:
        """Start a background adoption check for *rom_id* (deduped, rate-limited). Never raises."""
        try:
            rom_id = int(rom_id)
            if rom_id in self._in_flight:
                return
            now = self._clock.monotonic()
            last = self._last_check.get(rom_id)
            if last is not None and now - last < _CHECK_COOLDOWN_SEC:
                return
            self._last_check[rom_id] = now
            task = self._loop.create_task(self.adopt_rom(rom_id))
            self._in_flight[rom_id] = task
            task.add_done_callback(lambda _t, rid=rom_id: self._in_flight.pop(rid, None))
        except Exception as e:
            self._logger.warning(f"Install adoption check could not start for rom {rom_id}: {e}")

    def _quick_blocked(self, rom_id: int) -> bool:
        if rom_id in self._active_downloads():
            return True
        with self._uow_factory() as uow:
            row = uow.roms.get(rom_id)
            if row is None:
                return True
            return (
                uow.rom_installs.get(rom_id) is not None
                or uow.download_jobs.get(rom_id) is not None
                or self._sibling_installed(uow, rom_id, row.sibling_group_key)
            )

    async def adopt_rom(self, rom_id: int) -> bool:
        """Adopt one synced ROM's on-disk files, then announce it like a finished download.

        Cheap DB guards run first (no network when the ROM is installed, being
        downloaded, or has an installed sibling). Otherwise RomM's file names
        and sizes are fetched, the files probed, and on a match the
        ``RomInstall`` is written and ``download_complete`` is emitted with
        ``adopted: true`` and the resolved launch command — the same event a
        finished download emits, so the page flips to Play and the frontend
        confirm-sets the shortcut's launch options. Returns whether it adopted.
        Never raises (a RomM or filesystem failure just means "not adopted").
        """
        try:
            if await self._loop.run_in_executor(None, self._quick_blocked, rom_id):
                return False
            rom = await self._loop.run_in_executor(None, self._romm_api.get_rom, rom_id)
            if not isinstance(rom, dict) or int(rom.get("id", -1)) != rom_id:
                return False
            candidates = await self._loop.run_in_executor(None, self.plan_adoptions, [rom])
            if not candidates:
                return False
            adopted = await self._loop.run_in_executor(None, self.commit_adoptions, candidates)
            if rom_id not in adopted:
                return False
            item = await self._loop.run_in_executor(None, self._relaunch_items.relaunch_item_for_rom, rom_id)
        except Exception as e:
            self._logger.warning(f"Install adoption check failed for rom {rom_id}: {e}")
            return False
        install = candidates[0].install
        await self._emit(
            "download_complete",
            {
                "rom_id": rom_id,
                "rom_name": rom.get("name", ""),
                "platform_name": rom.get("platform_name", rom.get("platform_slug", "")),
                "file_path": install.file_path,
                "app_id": item["app_id"] if item else None,
                "launch_options": item["launch_options"] if item else "",
                "resumable": False,
                "adopted": True,
            },
        )
        return True
