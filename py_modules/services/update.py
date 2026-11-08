"""UpdateService — tells the frontend whether a newer plugin release exists.

Anything that decides whether the installed plugin is out of date belongs
here: fetching the latest release through the ``ReleaseFeedApi`` Protocol,
validating it (``domain.self_update``), throttling checks, and shaping the
callable response. It never installs anything — the frontend hands the
validated asset to Decky's own installer, which asks the user to confirm.
"""

from __future__ import annotations

import asyncio
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any

from domain.self_update import RELEASES_PAGE_URL, is_newer, parse_release, release_version
from lib.errors import ReleaseFeedError
from lib.list_result import ErrorCode

if TYPE_CHECKING:
    import logging

    from services.protocols import Clock, PluginMetadataReader, ReleaseFeedApi

# A successful check is reused for this long unless the user forces a check.
CHECK_INTERVAL_S = 6 * 60 * 60
# A failed check (offline at boot, rate limit) is retried sooner than that.
FAILURE_RETRY_S = 15 * 60

_RATE_LIMITED = "rate_limited"


@dataclass(frozen=True)
class UpdateServiceConfig:
    """Dependencies for :class:`UpdateService`.

    Parameters
    ----------
    release_api:
        Reads the latest GitHub release JSON.
    plugin_metadata:
        Reads the installed version (``package.json``) and the Decky plugin
        name (``plugin.json`` ``"name"``) Decky's installer matches on.
    plugin_dir:
        ``decky.DECKY_PLUGIN_DIR`` — where the metadata files live.
    clock:
        Wall-clock source for throttling and ``checked_at``.
    loop:
        Event loop used to run the blocking fetch in an executor.
    logger:
        Logger instance.
    """

    release_api: ReleaseFeedApi
    plugin_metadata: PluginMetadataReader
    plugin_dir: str
    clock: Clock
    loop: asyncio.AbstractEventLoop
    logger: logging.Logger


class UpdateService:
    """Checks GitHub for a newer plugin release, throttled and cached."""

    def __init__(self, *, config: UpdateServiceConfig) -> None:
        self._release_api = config.release_api
        self._plugin_metadata = config.plugin_metadata
        self._plugin_dir = config.plugin_dir
        self._clock = config.clock
        self._loop = config.loop
        self._logger = config.logger
        self._lock = asyncio.Lock()
        self._cached: dict[str, Any] | None = None
        self._cached_at: float | None = None
        self._background_tasks: set[asyncio.Task[Any]] = set()

    async def check_for_update(self, force: bool = False) -> dict[str, Any]:
        """Return the update status, re-checking GitHub only when due.

        A cached success is reused for :data:`CHECK_INTERVAL_S`, a cached
        failure for :data:`FAILURE_RETRY_S`; ``force=True`` always re-checks.
        Never raises: failures use the canonical ``{success: False, reason,
        message}`` shape with ``reason`` ``server_unreachable`` /
        ``rate_limited`` / ``unknown`` (plus ``current_version`` and
        ``checked_at``).
        """
        async with self._lock:
            if not force and self._cache_is_fresh():
                assert self._cached is not None
                return dict(self._cached)
            result = await self._run_check()
            self._cached = result
            self._cached_at = self._clock.time()
            return dict(result)

    def schedule_background_check(self) -> None:
        """Fire one non-forced check without blocking the caller (plugin start)."""
        task = self._loop.create_task(self.check_for_update(False))
        self._background_tasks.add(task)
        task.add_done_callback(self._background_tasks.discard)

    async def shutdown(self) -> None:
        """Cancel any in-flight background check."""
        tasks = list(self._background_tasks)
        for task in tasks:
            task.cancel()
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)

    def _cache_is_fresh(self) -> bool:
        if self._cached is None or self._cached_at is None:
            return False
        ttl = CHECK_INTERVAL_S if self._cached.get("success") else FAILURE_RETRY_S
        return (self._clock.time() - self._cached_at) < ttl

    async def _run_check(self) -> dict[str, Any]:
        try:
            current_version, plugin_name = await self._loop.run_in_executor(None, self._read_metadata_io)
        except Exception as e:
            self._logger.warning(f"Update check could not read plugin metadata: {e}")
            return self._failure(ErrorCode.UNKNOWN, "Could not read the installed plugin version", "0.0.0")
        try:
            data = await self._loop.run_in_executor(None, self._release_api.fetch_latest_release)
        except ReleaseFeedError as e:
            self._logger.warning(f"Update check failed ({e.kind}): {e}")
            return self._failure(_failure_reason(e), _failure_message(e), current_version)
        except Exception as e:
            self._logger.warning(f"Update check failed unexpectedly: {e}")
            return self._failure(ErrorCode.UNKNOWN, "Could not check for updates", current_version)

        checked_at = self._clock.now().isoformat()
        release = parse_release(data)
        if release is None:
            # No installable asset (missing zip / untrusted URL / no sha256) or a
            # pre-release tag: report what is out there, but offer nothing.
            return {
                "success": True,
                "current_version": current_version,
                "latest_version": release_version(data),
                "update_available": False,
                "notes": "",
                "asset_url": "",
                "sha256": "",
                "html_url": RELEASES_PAGE_URL,
                "plugin_name": plugin_name,
                "checked_at": checked_at,
            }
        return {
            "success": True,
            "current_version": current_version,
            "latest_version": release.version,
            "update_available": is_newer(release.version, current_version),
            "notes": release.notes,
            "asset_url": release.asset_url,
            "sha256": release.sha256,
            "html_url": release.html_url,
            "plugin_name": plugin_name,
            "checked_at": checked_at,
        }

    def _read_metadata_io(self) -> tuple[str, str]:
        """Blocking worker: installed version + Decky plugin name."""
        return (
            self._plugin_metadata.read_version(self._plugin_dir),
            self._plugin_metadata.read_plugin_name(self._plugin_dir),
        )

    def _failure(self, reason: str, message: str, current_version: str) -> dict[str, Any]:
        return {
            "success": False,
            "reason": str(reason),
            "message": message,
            "current_version": current_version,
            "checked_at": self._clock.now().isoformat(),
        }


def _failure_reason(error: ReleaseFeedError) -> str:
    if error.kind == _RATE_LIMITED:
        return _RATE_LIMITED
    if error.kind == "bad_response":
        return ErrorCode.UNKNOWN
    return ErrorCode.SERVER_UNREACHABLE


def _failure_message(error: ReleaseFeedError) -> str:
    if error.kind == _RATE_LIMITED:
        return "GitHub rate limit reached — try again later"
    if error.kind == "bad_response":
        return "GitHub returned an unreadable response"
    return "Could not reach GitHub — check your internet connection"
