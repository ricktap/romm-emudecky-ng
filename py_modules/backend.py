"""The Python backend: every frontend callable plus the plugin lifecycle work.

Sits behind the backend protocol socket (``docs/architecture/backend-protocol.md``).
The Decky shim (``main.py``) never calls into this module directly except to launch
it in-process; it reaches the callables below only through
:class:`rpc.server.BackendServer`. Each public ``async def`` on :class:`Backend` is
one wire callable, registered under its exact name — the callable surface the
frontend's ``callable("name")`` declarations must match (enforced by
``scripts/check_callable_manifest.py``). Startup (``_main``) and teardown
(``_unload``) work lives here too, so a replacement backend owns the whole
lifecycle. Service wiring stays in ``bootstrap.py``.
"""

from __future__ import annotations

import asyncio
import inspect
import logging
from dataclasses import asdict, dataclass
from typing import TYPE_CHECKING, Any

from bootstrap import (
    RuntimeBundle,
    WiringConfig,
    bootstrap,
    wire_services,
)

from lib.migration_gate import migration_blocked
from rpc.server import BackendServer

if TYPE_CHECKING:
    from collections.abc import Awaitable, Callable

    from services.protocols import EventEmitter


@dataclass(frozen=True)
class BackendEnv:
    """What the backend needs from its host to start.

    The Decky directories (``DECKY_PLUGIN_SETTINGS_DIR`` / ``…RUNTIME_DIR`` /
    ``…PLUGIN_DIR`` / ``DECKY_USER_HOME``), the logger, and the event emitter the
    services publish through (the protocol server's ``emit`` — every event
    leaves as a ``{"event", "args"}`` frame).
    """

    settings_dir: str
    runtime_dir: str
    plugin_dir: str
    user_home: str
    logger: logging.Logger
    emit: EventEmitter


class Backend:
    """Every wire callable, delegating to the services ``bootstrap`` wires."""

    settings: dict[str, Any]
    loop: asyncio.AbstractEventLoop

    # Test-only attribute slots — production ``Backend`` does not read
    # these after ``_main`` (the wired services own them), but the
    # test suite constructs ``Backend()`` bare and pokes the same handles
    # the production wiring would set. Annotated as ``Any`` because
    # tests pass real adapters, ``MagicMock``s, or fakes interchangeably.
    # Annotations alone do not create the attribute, so bare access still
    # raises ``AttributeError`` (the ``TestPersistenceAttributeIsLoud``
    # regression remains green).
    _persistence: Any
    _settings_persister: Any
    _http_adapter: Any
    _romm_api: Any
    _steam_config: Any
    _retrodeck_paths: Any

    # Strong refs to the fire-and-forget play-session flush tasks. ``create_task``
    # alone is not enough — without a strong ref the loop is free to GC the task
    # before it completes. ``add_done_callback`` prunes finished entries and
    # ``_unload`` cancels any still in-flight (mirrors SessionLifecycleService).
    # Lazily created on first schedule so a bare ``Backend()`` (test/harness that
    # skips ``_main``) still tracks tasks.
    _playtime_flush_tasks: set[asyncio.Task[None]]

    _MIN_REQUIRED_VERSION = (4, 9, 0)

    # -- logging ---------------------------------------------------------------
    #
    # ``_debug_logger`` is wired by ``_main()`` to the
    # ``SettingsAwareDebugLogger`` adapter built in ``bootstrap``. The
    # class-level default is a no-op so a bare ``Backend()`` (used in test
    # fixtures that don't reach ``_main``) does not raise on
    # ``_log_debug`` — production always replaces it before any service
    # consumes the callback.
    _debug_logger = staticmethod(lambda _msg: None)

    def _log_debug(self, msg):
        """Forward a debug message through the wired ``DebugLogger`` adapter.

        Thin compatibility shim: production wiring sets ``_debug_logger``
        from bootstrap; tests construct ``Backend`` bare and may patch in
        their own logger. The actual filtering logic lives in
        :class:`adapters.debug_logger.SettingsAwareDebugLogger`.
        """
        self._debug_logger(msg)

    # Fallback logger for a bare ``Backend()`` (tests); ``_main`` binds the host's.
    _logger: logging.Logger = logging.getLogger("romm_backend")

    @classmethod
    def callable_names(cls) -> list[str]:
        """The wire callable names: every public ``async def`` on the class."""
        return sorted(
            name
            for name, member in inspect.getmembers(cls)
            if not name.startswith("_") and inspect.iscoroutinefunction(member)
        )

    def callables(self) -> dict[str, Callable[..., Awaitable[Any]]]:
        """Map each wire callable name to its bound method (the server registry)."""
        return {name: getattr(self, name) for name in self.callable_names()}

    async def _main(self, env: BackendEnv):
        """Wire adapters + services and run the startup work (was Decky's ``_main``)."""
        self.loop = asyncio.get_event_loop()
        self._logger = env.logger

        # ── 1. Wire adapters ────────────────────────────────────────────────
        # Bootstrap loads + migrates settings as part of adapter construction
        # so RommHttpAdapter binds the live, migrated dict in one pass.
        result = bootstrap(
            settings_dir=env.settings_dir,
            runtime_dir=env.runtime_dir,
            plugin_dir=env.plugin_dir,
            user_home=env.user_home,
            logger=env.logger,
        )
        self.settings = result.stores.settings
        self._debug_logger = result.handles.debug_logger
        # Persistence adapter — held directly for the disk-touching callable
        # paths that read/write settings without routing through a service.
        self._persistence = result.handles.persistence
        # RetroDECK path resolver — held directly so the get_retrodeck_status
        # callable can read the resolution health without routing through a
        # service (it's a pure adapter read, no orchestration).
        self._retrodeck_paths = result.callbacks.retrodeck_paths

        # ── 4. Wire services ────────────────────────────────────────────────
        services = wire_services(
            WiringConfig(
                adapters=result.adapters,
                stores=result.stores,
                runtime=RuntimeBundle(
                    loop=self.loop,
                    logger=env.logger,
                    plugin_dir=env.plugin_dir,
                    runtime_dir=env.runtime_dir,
                    emit=env.emit,
                    clock=result.runtime_adapters.clock,
                    uuid_gen=result.runtime_adapters.uuid_gen,
                    sleeper=result.runtime_adapters.sleeper,
                    hostname_provider=result.runtime_adapters.hostname_provider,
                    machine_id_provider=result.runtime_adapters.machine_id_provider,
                ),
                callbacks=result.callbacks,
                min_required_version=self._MIN_REQUIRED_VERSION,
            )
        )
        self._save_sync_service = services["save_sync_service"]
        self._playtime_service = services["playtime_service"]
        self._sync_service = services["sync_service"]
        self._download_service = services["download_service"]
        self._rom_removal_service = services["rom_removal_service"]
        self._firmware_service = services["firmware_service"]
        self._sgdb_service = services["sgdb_service"]
        self._metadata_service = services["metadata_service"]
        self._achievements_service = services["achievements_service"]
        self._migration_service = services["migration_service"]
        self._game_detail_service = services["game_detail_service"]
        self._artwork_service = services["artwork_service"]
        self._shortcut_removal_service = services["shortcut_removal_service"]
        self._settings_service = services["settings_service"]
        self._core_service = services["core_service"]
        self._disc_service = services["disc_service"]
        self._version_switch_service = services["version_switch_service"]
        self._connection_service = services["connection_service"]
        self._startup_healing_service = services["startup_healing_service"]
        self._launch_gate_service = services["launch_gate_service"]
        self._session_lifecycle_service = services["session_lifecycle_service"]
        self._relaunch_options_resolver = services["relaunch_options_resolver"]
        self._update_service = services["update_service"]

        # ── 4b. Legacy credential migration ─────────────────────────────────
        # Upgrade a stored-password install to a Client API Token. The
        # method swallows every failure (plugin stays inert, no Basic-auth
        # fallback), so a mint failure here never blocks startup.
        await self._connection_service.migrate_legacy_credentials()

        # ── 5. Startup healing ──────────────────────────────────────────────
        # Detect retrodeck path changes BEFORE pruning so the prune can skip
        # entries living under a pending migration's previous home.
        self._migration_service.detect_retrodeck_path_change()
        self._startup_healing_service.prune_stale_installed_roms()
        self._startup_healing_service.reconcile_orphaned_sync_runs()
        # No save-sync orphan prune: roms rows are permanent identity anchors
        # and saves/playtime survive a ROM leaving RomM (ADR-0007).
        self._sgdb_service.prune_orphaned_artwork_cache()
        self._artwork_service.prune_orphaned_staging_artwork()
        self._artwork_service.prune_orphaned_cover_cache()
        # Restore the durable download queue: keep job partials, drop orphan
        # .tmp files, and auto-resume interrupted jobs in the background.
        self._download_service.reconcile_persisted_downloads()

        # ── 6. Background tasks ─────────────────────────────────────────────
        self._migration_service.detect_save_sort_change()
        # One throttled update check in the background — never blocks startup.
        self._update_service.schedule_background_check()
        self._logger.info("EmuDecky RomM Manager backend started")

    async def _unload(self):
        """Stop background work (was Decky's ``_unload``); run on ``__shutdown__``."""
        self._sync_service.shutdown()
        await self._download_service.shutdown()
        await self._migration_service.shutdown()
        await self._session_lifecycle_service.shutdown()
        await self._cancel_playtime_flush_tasks()
        await self._update_service.shutdown()
        self._logger.info("EmuDecky RomM Manager backend stopped")

    async def _cancel_playtime_flush_tasks(self):
        """Cancel and await any in-flight play-session flush tasks on unload.

        Mirrors ``SessionLifecycleService.shutdown`` so a detached flush
        coroutine does not leak across the plugin-unload boundary. No-op when
        none are pending (or the set was never created).
        """
        tasks = getattr(self, "_playtime_flush_tasks", None)
        if not tasks:
            return
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)

    # ── Callables ──────────────────────────────────────────────────────
    # Every public ``async def`` below is one wire callable (see
    # ``callable_names``), served over the backend socket under its exact name.

    async def test_connection(self):
        return await self._connection_service.test_connection()

    async def connect_with_credentials(self, romm_url, username, password, allow_insecure_ssl=None):
        return await self._connection_service.establish_token(romm_url, username, password, allow_insecure_ssl)

    async def connect_with_token(self, romm_url, token, allow_insecure_ssl=None):
        return await self._connection_service.establish_user_token(romm_url, token, allow_insecure_ssl)

    async def connect_with_pairing_code(self, romm_url, code, allow_insecure_ssl=None):
        return await self._connection_service.establish_paired_token(romm_url, code, allow_insecure_ssl)

    async def sign_out(self):
        return self._connection_service.sign_out()

    async def save_server_url(self, romm_url, allow_insecure_ssl=None):
        return self._settings_service.save_server_url(romm_url, allow_insecure_ssl)

    async def frontend_log(self, level, message):
        self._settings_service.frontend_log(level, message)

    async def debug_log(self, message):
        self._settings_service.frontend_log("debug", message)

    async def save_log_level(self, level):
        return self._settings_service.save_log_level(level)

    async def save_steam_input_setting(self, mode):
        return self._settings_service.save_steam_input_setting(mode)

    async def save_preferred_region(self, region):
        return self._settings_service.save_preferred_region(region)

    async def get_known_regions(self):
        return self._settings_service.get_known_regions()

    async def apply_steam_input_setting(self):
        return self._settings_service.apply_steam_input_setting()

    async def fix_retroarch_input_driver(self):
        return self._settings_service.fix_retroarch_input_driver()

    async def get_settings(self):
        return self._settings_service.get_settings()

    async def check_for_update(self, force):
        return await self._update_service.check_for_update(bool(force))

    async def get_retrodeck_status(self):
        """Report RetroDECK path-resolution health for the frontend banner.

        Discriminated-status union (Callable response shapes carve-out):
        ``status`` carries one of ``ok`` / ``absent`` / ``unreadable`` /
        ``root_missing``. The frontend owns the human-readable copy; the
        backend returns the discriminant plus the probed paths.
        """
        return {
            "status": self._retrodeck_paths.config_health().value,
            "config_path": self._retrodeck_paths.config_path(),
            "resolved_home": self._retrodeck_paths.retrodeck_home(),
        }

    async def get_whitelist_settings(self):
        return self._settings_service.get_whitelist_settings()

    async def update_whitelist_settings(self, disabled_defaults, custom_names):
        return self._settings_service.update_whitelist_settings(disabled_defaults, custom_names)

    async def get_cached_game_detail(self, app_id):
        return self._game_detail_service.get_cached_game_detail(app_id)

    @migration_blocked
    async def set_system_core(self, platform_slug, core_label):
        return await self._core_service.set_system_core(platform_slug, core_label)

    @migration_blocked
    async def set_game_core(self, rom_id, label):
        return await self._core_service.set_game_core(rom_id, label)

    @migration_blocked
    async def clear_game_core(self, rom_id):
        return await self._core_service.clear_game_core(rom_id)

    async def get_platform_core_info(self, rom_id):
        return await self._core_service.get_platform_core_info(rom_id)

    # ── Disc picker delegation to DiscService ──────────────

    async def get_disc_selection(self, rom_id):
        return await self._disc_service.get_disc_selection(rom_id)

    @migration_blocked
    async def select_disc(self, rom_id, filename):
        return await self._disc_service.select_disc(rom_id, filename)

    # ── Version picker delegation to VersionSwitchService ──────────────

    async def get_version_list(self, app_id):
        return await self._version_switch_service.get_version_list(app_id)

    @migration_blocked
    async def switch_version(self, app_id, target_rom_id, allow_stranded):
        return await self._version_switch_service.switch_version(app_id, target_rom_id, allow_stranded)

    # ── Firmware delegation to FirmwareService ──────────────

    async def get_firmware_status(self):
        return await self._firmware_service.get_firmware_status()

    @migration_blocked
    async def download_all_firmware(self, platform_slug):
        return await self._firmware_service.download_all_firmware(platform_slug)

    @migration_blocked
    async def download_required_firmware(self, platform_slug):
        return await self._firmware_service.download_required_firmware(platform_slug)

    async def check_platform_bios(self, platform_slug):
        # Platform-level BIOS check (the frontend callable sends only the slug);
        # no per-game core to thread, so the system default drives the filter.
        return await self._firmware_service.check_platform_bios(platform_slug)

    async def get_bios_status(self, rom_id):
        return await self._game_detail_service.get_bios_status(rom_id)

    @migration_blocked
    async def delete_platform_bios(self, platform_slug):
        return await self._firmware_service.delete_platform_bios(platform_slug)

    # ── Sync delegation to LibraryService ─────────────────────

    async def get_platforms(self):
        return await self._sync_service.get_platforms()

    @migration_blocked
    async def save_platform_sync(self, platform_id, enabled):
        return self._sync_service.save_platform_sync(platform_id, enabled)

    @migration_blocked
    async def set_all_platforms_sync(self, enabled):
        return await self._sync_service.set_all_platforms_sync(enabled)

    async def get_collections(self):
        return await self._sync_service.get_collections()

    @migration_blocked
    async def save_collection_sync(self, collection_id, kind, enabled):
        return self._sync_service.save_collection_sync(collection_id, kind, enabled)

    @migration_blocked
    async def set_all_collections_sync(self, enabled, scope=None):
        return await self._sync_service.set_all_collections_sync(enabled, scope)

    async def save_collection_platform_groups(self, enabled):
        return self._settings_service.save_collection_platform_groups(enabled)

    @migration_blocked
    async def start_sync(self):
        return self._sync_service.start_sync()

    async def cancel_sync(self, run_id):
        return self._sync_service.cancel_sync(run_id)

    async def sync_heartbeat(self):
        return self._sync_service.sync_heartbeat()

    @migration_blocked
    async def sync_preview(self):
        return await self._sync_service.sync_preview()

    @migration_blocked
    async def sync_apply_delta(self, preview_id):
        return await self._sync_service.sync_apply_delta(preview_id)

    async def sync_cancel_preview(self):
        return self._sync_service.sync_cancel_preview()

    async def get_sync_status(self):
        return self._sync_service.get_sync_status()

    async def report_unit_results(self, rom_id_to_app_id, run_id, unit_id):
        return await self._sync_service.report_unit_results(rom_id_to_app_id, run_id, unit_id)

    async def get_registry_platforms(self):
        return self._sync_service.get_registry_platforms()

    @migration_blocked
    async def remove_platform_shortcuts(self, platform_slug):
        return await self._shortcut_removal_service.remove_platform_shortcuts(platform_slug)

    @migration_blocked
    async def remove_all_shortcuts(self):
        return self._shortcut_removal_service.remove_all_shortcuts()

    async def report_removal_results(self, removed_rom_ids):
        return await self._shortcut_removal_service.report_removal_results(removed_rom_ids)

    async def reconcile_shortcuts(self, live_app_ids):
        return await self._shortcut_removal_service.reconcile_live_shortcuts(live_app_ids)

    async def get_artwork_base64(self, rom_id):
        return await self._artwork_service.get_artwork_base64(rom_id)

    async def fetch_cover_base64(self, rom_id):
        return await self._artwork_service.fetch_cover_base64(rom_id)

    @migration_blocked
    async def refresh_cover_artwork(self, rom_id):
        return await self._artwork_service.refresh_cover(int(rom_id))

    @migration_blocked
    async def clear_sync_cache(self):
        return self._sync_service.clear_sync_cache()

    async def get_sync_stats(self):
        return self._sync_service.get_sync_stats()

    async def evaluate_launch(self, steam_app_id):
        verdict = await self._launch_gate_service.evaluate(steam_app_id)
        return asdict(verdict)

    async def check_local_drift(self, rom_id):
        return await self._launch_gate_service.check_local_drift(rom_id)

    async def get_rom_relaunch_options(self, rom_id):
        """Return ``{app_id, launch_options}`` for one installed+bound ROM, or None.

        The Play-button funnel re-confirms the shortcut's launch command from
        this just before launch to heal mid-session ``launch_options`` drift
        (#1150). Read-only — no migration gate, no canonical failure shape.
        """
        return await self.loop.run_in_executor(None, self._relaunch_options_resolver.relaunch_item_for_rom, int(rom_id))

    async def probe_reachability(self):
        return await self._connection_service.probe_reachability()

    async def refresh_save_status(self, rom_id):
        # Fire-and-forget: schedule the background status check (which re-reads
        # the conflict state and emits ``save_status_updated``) and return
        # immediately so the frontend never blocks on the round-trip. Mirrors the
        # create_task pattern in services/saves/slots/switching.py (same call,
        # same target); check_save_status_background owns its own error handling.
        self.loop.create_task(self._save_sync_service.check_save_status_background(int(rom_id)))
        return {"success": True}

    async def finalize_game_session(self, rom_id):
        result = await self._session_lifecycle_service.finalize(rom_id)
        return asdict(result)

    # ── Download delegation to DownloadService ──────────────

    @migration_blocked
    async def start_download(self, rom_id):
        return await self._download_service.start_download(rom_id)

    async def cancel_download(self, rom_id):
        return self._download_service.cancel_download(rom_id)

    async def pause_download(self, rom_id):
        return self._download_service.pause_download(rom_id)

    @migration_blocked
    async def resume_download(self, rom_id):
        return await self._download_service.resume_download(rom_id)

    async def get_download_queue(self):
        return self._download_service.get_download_queue()

    async def get_installed_rom(self, rom_id):
        return self._download_service.get_installed_rom(rom_id)

    @migration_blocked
    async def remove_rom(self, rom_id):
        return await self._rom_removal_service.remove_rom(rom_id)

    @migration_blocked
    async def uninstall_all_roms(self):
        return await self._rom_removal_service.uninstall_all_roms()

    # ── Save Sync / Playtime delegation to services ──────────

    async def ensure_device_registered(self):
        return await self._save_sync_service.ensure_device_registered()

    async def list_devices(self):
        return await self._save_sync_service.list_devices()

    async def get_save_status(self, rom_id):
        return await self._save_sync_service.get_save_status(rom_id)

    async def check_core_change(self, rom_id):
        return self._save_sync_service.check_core_change(rom_id)

    @migration_blocked
    async def pre_launch_sync(self, rom_id):
        return await self._save_sync_service.pre_launch_sync(rom_id)

    @migration_blocked
    async def sync_rom_saves(self, rom_id):
        return await self._save_sync_service.sync_rom_saves(rom_id)

    async def get_save_slots(self, rom_id):
        return await self._save_sync_service.get_save_slots(rom_id)

    async def get_slot_saves(self, rom_id, slot):
        return await self._save_sync_service.get_slot_saves(rom_id, slot)

    @migration_blocked
    async def switch_slot(self, rom_id, new_slot):
        return await self._save_sync_service.switch_slot(rom_id, new_slot)

    async def get_slot_delete_info(self, rom_id, slot):
        return await self._save_sync_service.get_slot_delete_info(rom_id, slot)

    @migration_blocked
    async def delete_slot(self, rom_id, slot):
        return await self._save_sync_service.delete_slot(rom_id, slot)

    async def is_save_tracking_configured(self, rom_id):
        return self._save_sync_service.is_save_tracking_configured(rom_id)

    async def get_save_setup_info(self, rom_id):
        return await self._save_sync_service.get_save_setup_info(rom_id)

    @migration_blocked
    async def confirm_slot_choice(self, rom_id, chosen_slot, migrate=False, migrate_from_slot=None):
        return await self._save_sync_service.confirm_slot_choice(rom_id, chosen_slot, migrate, migrate_from_slot)

    @migration_blocked
    async def sync_all_saves(self):
        return await self._save_sync_service.sync_all_saves()

    @migration_blocked
    async def resolve_sync_conflict(self, rom_id, filename, server_save_id, action):
        return await self._save_sync_service.resolve_sync_conflict(rom_id, filename, server_save_id, action)

    async def get_save_sync_settings(self):
        return self._save_sync_service.get_save_sync_settings()

    @migration_blocked
    async def update_save_sync_settings(self, settings):
        return self._save_sync_service.update_save_sync_settings(settings)

    @migration_blocked
    async def delete_local_saves(self, rom_id):
        return self._save_sync_service.delete_local_saves(rom_id)

    @migration_blocked
    async def delete_platform_saves(self, platform_slug):
        return self._save_sync_service.delete_platform_saves(platform_slug)

    async def saves_list_file_versions(self, rom_id, slot, filename):
        return await self._save_sync_service.list_file_versions(rom_id, slot, filename)

    @migration_blocked
    async def saves_rollback_to_version(self, rom_id, slot, save_id):
        return await self._save_sync_service.rollback_to_version(rom_id, slot, save_id)

    async def record_session_start(self, rom_id):
        result = self._playtime_service.record_session_start(rom_id)
        # Fire-and-forget: drain any offline play-session backlog into RomM's
        # native ingest on the next launch; returns immediately so the launch is
        # never blocked on the round-trip. flush_pending_sessions owns its own
        # error handling (best-effort, offline-safe).
        self._schedule_playtime_flush()
        return result

    def _schedule_playtime_flush(self):
        """Kick off the offline play-session flush as a tracked background task.

        Keeps a strong ref in ``_playtime_flush_tasks`` so the loop cannot GC the
        task mid-flush, prunes it on completion, and lets ``_unload`` cancel any
        still pending. The set is created lazily so a bare ``Backend()`` works too.
        """
        tasks = getattr(self, "_playtime_flush_tasks", None)
        if tasks is None:
            tasks = set()
            self._playtime_flush_tasks = tasks
        task = self.loop.create_task(self._playtime_service.flush_pending_sessions())
        tasks.add(task)
        task.add_done_callback(tasks.discard)

    async def get_all_playtime(self):
        return self._playtime_service.get_all_playtime()

    async def reconcile_playtime(self, rom_id):
        return await self._playtime_service.reconcile_playtime(int(rom_id))

    async def get_playtime_scope_notice(self):
        """Report whether the token lacks the play-session read scope.

        Returns ``{"pending": bool}``. ``pending`` is set when a reconcile GET
        403'd because the stored token predates the ``roms.user.read`` scope
        (#1280) — the frontend surfaces a persistent "sign in again to enable
        cross-device playtime" banner. Non-consuming (mirrors
        ``get_settings_reset_notice``): the durable flag is cleared only by a
        later successful reconcile GET or a fresh sign-in, so the banner stays up
        across reloads until the user re-authenticates.
        """
        return self._playtime_service.get_scope_notice()

    # ── SGDB delegation to SteamGridService ───────────────────────

    async def get_sgdb_artwork_base64(self, rom_id, asset_type_num):
        return await self._sgdb_service.get_sgdb_artwork_base64(rom_id, asset_type_num)

    async def verify_sgdb_api_key(self, api_key=None):
        return await self._sgdb_service.verify_sgdb_api_key(api_key)

    async def save_sgdb_api_key(self, api_key):
        return self._sgdb_service.save_sgdb_api_key(api_key)

    async def save_shortcut_icon(self, app_id, icon_base64):
        return await self._sgdb_service.save_shortcut_icon(app_id, icon_base64)

    async def get_sgdb_resolution(self, rom_id):
        return await self._sgdb_service.get_sgdb_resolution(rom_id)

    async def search_sgdb_games(self, term):
        return await self._sgdb_service.search_sgdb_games(term)

    async def apply_sgdb_game_id(self, rom_id, sgdb_id):
        return await self._sgdb_service.apply_sgdb_game_id(rom_id, sgdb_id)

    # ── Metadata delegation to MetadataService ────────────────

    async def get_rom_metadata(self, rom_id):
        return self._metadata_service.get_rom_metadata(rom_id)

    async def get_all_metadata_cache(self):
        return self._metadata_service.get_all_metadata_cache()

    async def get_app_id_rom_id_map(self):
        return self._metadata_service.get_app_id_rom_id_map()

    async def get_installed_relaunch_options(self):
        """Return [{app_id, launch_options}] for every installed+bound ROM so the
        frontend can re-confirm drifted Steam-shortcut launch commands at startup
        (#1043). Read-only — not migration-gated."""
        return await self.loop.run_in_executor(None, self._startup_healing_service.get_installed_relaunch_options)

    # ── Achievements delegation to AchievementsService ───────

    async def get_achievements(self, rom_id):
        return await self._achievements_service.get_achievements(rom_id)

    async def get_achievement_progress(self, rom_id):
        return await self._achievements_service.get_achievement_progress(rom_id)

    # ── Migration delegation to MigrationService ──────────────

    async def migrate_retrodeck_files(self, conflict_strategy=None):
        return await self._migration_service.migrate_retrodeck_files(conflict_strategy)

    async def get_migration_status(self):
        return await self._migration_service.get_migration_status()

    async def get_save_sort_migration_status(self):
        return await self._migration_service.get_save_sort_migration_status()

    async def migrate_save_sort_files(self, conflict_strategy=None):
        return await self._migration_service.migrate_save_sort_files(conflict_strategy)

    async def dismiss_save_sort_migration(self):
        return self._migration_service.dismiss_save_sort_migration()

    async def dismiss_retrodeck_migration(self):
        return self._migration_service.dismiss_retrodeck_migration()

    async def refresh_migration_state(self):
        return await self._migration_service.refresh_state()

    async def get_settings_reset_notice(self):
        """Report whether a corrupt ``settings.json`` was reset at boot.

        Reads the persistent ``_settings_reset_notice`` marker from the live
        settings dict (written by bootstrap when ``load_settings`` quarantined an
        unparseable file). Returns ``{"pending": bool, "backed_up_to": str |
        None}``. Non-consuming — the marker survives a plugin reload and is
        cleared only by an explicit user acknowledgement in the QAM
        (``dismiss_settings_reset_notice``), so the frontend banner + game-detail
        cards stay up until the user dismisses. A clean boot returns
        ``{"pending": False, "backed_up_to": None}``.
        """
        notice = self.settings.get("_settings_reset_notice")
        return {"pending": notice is not None, "backed_up_to": (notice or {}).get("backed_up_to")}

    async def dismiss_settings_reset_notice(self):
        """Acknowledge the corrupt-settings reset, clearing the persistent marker.

        The user's explicit ack in the QAM — pops ``_settings_reset_notice`` and
        persists, so the banner and game-detail cards stay down across reloads.
        Returns ``{"success": True}``.
        """
        return self._settings_service.dismiss_settings_reset_notice()


async def serve_in_process(
    socket_path: str,
    *,
    settings_dir: str,
    runtime_dir: str,
    plugin_dir: str,
    user_home: str,
    logger: logging.Logger,
) -> BackendServer:
    """Start the Python backend inside the current process and serve it on *socket_path*.

    Runs the backend's startup work first (``Backend._main``), then binds the
    socket, so a client that connects finds every callable ready. Events the
    startup emits before any client connects are buffered by the server and
    flushed to the first client. A ``__shutdown__`` request runs
    ``Backend._unload`` and closes the server; the returned server's
    ``wait_closed`` resolves once that has happened.
    """
    backend = Backend()
    server = BackendServer(logger=logger, on_shutdown=backend._unload)
    env = BackendEnv(
        settings_dir=settings_dir,
        runtime_dir=runtime_dir,
        plugin_dir=plugin_dir,
        user_home=user_home,
        logger=logger,
        emit=server.emit,
    )
    await backend._main(env)
    server.set_registry(backend.callables())
    await server.start(socket_path)
    return server
