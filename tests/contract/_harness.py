"""Contract-test harness — the real shim over the real backend socket.

This tier drives the callable surface the way the frontend does: every
callable goes through the real Decky shim (:class:`main.Plugin`), whose
generated forwarders send newline-delimited JSON requests over a real Unix
socket to the real backend protocol server (:class:`rpc.server.BackendServer`)
fronting the real :class:`backend.Backend`. Arguments and results therefore
cross a JSON round-trip exactly as they will for a native backend, and events
reach ``harness.emit`` only via protocol event frames relayed by the shim.

Anything that builds, wires, or seeds the real plugin for
a contract test belongs here; the per-callable contract assertions live
in the sibling ``test_*.py`` modules.

What stays real vs. what is faked
=================================

The harness calls the **real** :func:`bootstrap` (real settings dict,
real SQLite database + migrations, real file-store adapters) rooted under
a pytest ``tmp_path`` and wires the **real** services via the real
:func:`wire_services`. Only the outermost edges are swapped:

* ``romm_api`` → :class:`FakeRommApi` (the network transport — so tests
  seed library/saves/server state and inject failures without HTTP).
* ``sgdb_adapter`` → :class:`FakeSteamGridDbApi` (the SteamGridDB
  network transport).
* ``release_api`` → :class:`FakeReleaseFeedApi` (the GitHub release feed).
* ``clock`` / ``uuid_gen`` / ``sleeper`` → the deterministic fakes from
  ``tests.fakes.system_time`` so timestamped responses are assertable.
* ``http_adapter.with_retry`` → a single-attempt pass-through so a
  failure-injection test does not pay the real exponential backoff
  ``time.sleep`` (1s, 3s …). Everything else on the real
  ``RommHttpAdapter`` (``resolve_system``, settings binding) stays real —
  it is a pure settings/file read with no network on the read paths.
* the shim's event relay (``decky.emit`` in production) → an ``AsyncMock``
  (``harness.emit``) so tests assert the emissions that crossed the socket.
* the launch resolver → :class:`FakeLaunchResolver` for systems the
  emulator table does not cover; table systems (Switch) resolve through the
  real :class:`EmuDeckLauncherAdapter` so their launch commands are real.

``harness.plugin`` is the shim — call callables on it. ``harness.backend`` is
the in-process :class:`backend.Backend`, for tests that seed or inspect
service internals directly (never for driving a callable).

The SQLite database, the file stores, and the settings file all write
into ``tmp_path``; that is intended and isolated per test (pytest hands
each test a fresh ``tmp_path``).
"""

from __future__ import annotations

import asyncio
import dataclasses
import logging
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any
from unittest.mock import AsyncMock

from bootstrap import (
    RuntimeBundle,
    WiringConfig,
    bootstrap,
    wire_services,
)
from fakes.fake_release_feed_api import FakeReleaseFeedApi
from fakes.fake_romm_api import FakeRommApi
from fakes.fake_steamgrid_db_api import FakeSteamGridDbApi
from fakes.system_time import FakeClock, FakeSleeper, FakeUuidGen
from rpc.protocol import TASK_NAME_PREFIX, socket_path_for
from rpc.server import BackendServer

from domain.emulator_table import EMULATOR_TABLE
from tests.fakes.fake_launch_resolver import FakeLaunchResolver

if TYPE_CHECKING:
    from collections.abc import Callable

# Imported lazily inside the builder so ``conftest`` import order (which sets
# up ``sys.path`` and the mock ``decky`` module) is respected before ``main``
# is touched.

# The service attributes ``Backend._main`` binds. The harness
# binds the same set; the loud-failure assert below checks every one is present
# so a wiring drift (a renamed/added service key) fails the fixture instead of
# surfacing as a confusing ``AttributeError`` mid-test.
_BOUND_SERVICE_ATTRS = {
    "_save_sync_service": "save_sync_service",
    "_playtime_service": "playtime_service",
    "_sync_service": "sync_service",
    "_download_service": "download_service",
    "_rom_removal_service": "rom_removal_service",
    "_firmware_service": "firmware_service",
    "_sgdb_service": "sgdb_service",
    "_metadata_service": "metadata_service",
    "_achievements_service": "achievements_service",
    "_migration_service": "migration_service",
    "_game_detail_service": "game_detail_service",
    "_artwork_service": "artwork_service",
    "_shortcut_removal_service": "shortcut_removal_service",
    "_settings_service": "settings_service",
    "_core_service": "core_service",
    "_disc_service": "disc_service",
    "_version_switch_service": "version_switch_service",
    "_connection_service": "connection_service",
    "_startup_healing_service": "startup_healing_service",
    "_launch_gate_service": "launch_gate_service",
    "_session_lifecycle_service": "session_lifecycle_service",
    "_relaunch_options_resolver": "relaunch_options_resolver",
    "_update_service": "update_service",
}


@dataclass
class ContractHarness:
    """What a contract test reaches for: the shim, the backend, and the fake edges.

    ``plugin`` is the real :class:`main.Plugin` shim connected to the backend
    socket — every callable on it is a JSON round-trip. ``backend`` is the real
    :class:`backend.Backend` with every service wired (for seeding/inspection).
    ``romm`` is the :class:`FakeRommApi` the plugin's services talk to —
    tests seed library/saves/server state on it and arm its failure seams.
    ``sgdb`` is the :class:`FakeSteamGridDbApi`. ``emit`` is the
    ``AsyncMock`` the runtime emits through. ``clock`` is the deterministic
    :class:`FakeClock`. ``tmp_path`` is the per-test root every real adapter
    writes under.
    """

    plugin: Any
    backend: Any
    romm: FakeRommApi
    sgdb: FakeSteamGridDbApi
    emit: AsyncMock
    clock: FakeClock
    tmp_path: Any
    # The real SQLite Unit-of-Work factory the wired services use. Tests open it
    # to seed relational state (roms / rom_installs / rom_save_states / kv_config)
    # exactly as the services read it — same database, same connection contract.
    uow_factory: Any
    # The real RetroDECK paths provider bound onto the plugin. A migration test
    # swaps a controllable fake onto ``plugin._migration_service._retrodeck_paths``
    # to drive RetroDECK-home changes through detection.
    retrodeck_paths: Any
    # The fake GitHub release feed the update service reads (outer network edge).
    release_feed: Any
    # The protocol server fronting ``backend`` (socket path, close()).
    server: Any = None

    async def flush_events(self) -> None:
        """Round-trip a request so every event emitted so far has been relayed.

        Events and responses share one ordered stream, so once any response
        arrives every earlier event frame has already reached ``emit``. Use after
        draining background work whose events were emitted outside a callable.
        """
        await self.plugin._client.list_methods()

    async def close(self) -> None:
        """Disconnect the shim and stop the server (no backend shutdown hooks)."""
        if self.plugin._client is not None:
            await self.plugin._client.close()
        if self.server is not None:
            await self.server.close()


def background_tasks() -> list[asyncio.Task[Any]]:
    """Pending tasks other than the caller and the protocol's long-lived connection tasks.

    The shim's reader loop and the server's connection handler never finish
    while the harness is up; draining helpers must not wait on them.
    """
    current = asyncio.current_task()
    return [
        t
        for t in asyncio.all_tasks()
        if t is not current and not t.done() and not t.get_name().startswith(TASK_NAME_PREFIX)
    ]


class _HarnessLaunchResolver:
    """Route emulator-table systems to the real resolver, everything else to the fake.

    The fake keeps the pre-table contract tests' RetroDECK-shaped expectations;
    the real :class:`EmuDeckLauncherAdapter` renders table emulators (Switch) so
    their contract tests assert the actual baked command.
    """

    def __init__(self, real: Any, fake: FakeLaunchResolver) -> None:
        self._real = real
        self._fake = fake

    def resolve_for(self, rom: Any, emulator: Any, system: Any) -> Any:
        if (emulator is not None and emulator.kind == "table") or system in EMULATOR_TABLE:
            return self._real.resolve_for(rom, emulator, system)
        return self._fake.resolve_for(rom, emulator, system)


def _single_attempt_pass_through(fn: Callable[..., Any], *args: Any, **kwargs: Any) -> Any:
    """Run ``fn`` exactly once, no retry/backoff — replaces ``http_adapter.with_retry``.

    The real ``with_retry`` sleeps with exponential backoff on retryable
    transport errors; a failure-injection contract test would otherwise pay
    real wall-clock seconds. The single-attempt pass-through preserves the
    contract (the wrapped call's value or exception propagates verbatim).
    """
    return fn(*args, **kwargs)


async def build_contract_harness(tmp_path: Any) -> ContractHarness:
    """Build the real backend over the real ``bootstrap()`` and connect the real shim.

    Mirrors ``Backend._main`` bindings exactly (see ``_BOUND_SERVICE_ATTRS``)
    so the wired backend behaves as it does in production, then asserts every
    expected service attribute is bound — a wiring drift fails loudly here.
    The backend is served on a fresh socket and the shim attaches to it.
    """
    from backend import Backend
    from main import Plugin

    logger = logging.getLogger("contract")

    # 1. Real bootstrap — real settings dict, real SQLite + migrations, real
    #    file-store adapters, all rooted under tmp_path.
    result = bootstrap(
        settings_dir=str(tmp_path / "settings"),
        runtime_dir=str(tmp_path / "runtime"),
        plugin_dir=str(tmp_path / "plugin"),
        user_home=str(tmp_path / "home"),
        logger=logger,
    )

    # 2. Swap only the network edges in the returned AdapterBundle. The bundle
    #    is frozen, so rebuild it with dataclasses.replace. The real
    #    http_adapter is kept (resolve_system is a pure read) but its with_retry
    #    is neutralised so failure-injection tests don't sleep.
    fake_romm = FakeRommApi()
    fake_sgdb = FakeSteamGridDbApi()
    fake_release_feed = FakeReleaseFeedApi()
    real_http = result.adapters.http_adapter
    real_http.with_retry = _single_attempt_pass_through  # type: ignore[method-assign]
    patched_adapters = dataclasses.replace(
        result.adapters,
        romm_api=fake_romm,
        sgdb_adapter=fake_sgdb,
        release_api=fake_release_feed,
    )

    # Deterministic time/uuid/sleep seams so timestamped responses assert cleanly.
    fake_clock = FakeClock()
    fake_uuid = FakeUuidGen()
    fake_sleeper = FakeSleeper()

    emit = AsyncMock()
    # The protocol server whose ``emit`` the services publish through; events
    # reach ``emit`` (above) only after crossing the socket via the shim relay.
    server = BackendServer(logger=logger)
    # The running loop the test executes in. The harness is built from inside an
    # async fixture so this is the *same* loop the callables will await on —
    # binding the loop captured at module import (a different loop) would make
    # ``run_in_executor`` raise "got Future attached to a different loop".
    loop = asyncio.get_event_loop()

    # 3. Wire the real services with the patched bundle.
    # Swap the launch resolver with a fake so the contract tests exercise the
    # callable orchestration (the real EmuDeckLauncherAdapter can't resolve
    # launchers on a non-EmuDeck host; the adapter has its own unit tests in
    # tests/adapters/test_emudeck_launchers.py).
    launch_resolver = _HarnessLaunchResolver(result.callbacks.launch_resolver, FakeLaunchResolver())
    patched_callbacks = dataclasses.replace(
        result.callbacks,
        launch_resolver=launch_resolver,
    )
    cfg = WiringConfig(
        adapters=patched_adapters,
        stores=result.stores,
        runtime=RuntimeBundle(
            loop=loop,
            logger=logger,
            plugin_dir=str(tmp_path / "plugin"),
            runtime_dir=str(tmp_path / "runtime"),
            emit=server.emit,
            clock=fake_clock,
            uuid_gen=fake_uuid,
            sleeper=fake_sleeper,
            hostname_provider=result.runtime_adapters.hostname_provider,
            machine_id_provider=result.runtime_adapters.machine_id_provider,
        ),
        callbacks=patched_callbacks,
        min_required_version=Backend._MIN_REQUIRED_VERSION,
    )
    services = wire_services(cfg)

    # 4. Construct the real Backend and bind exactly as Backend._main does.
    backend = Backend()
    backend.loop = loop
    backend.settings = result.stores.settings
    backend._logger = logger
    backend._debug_logger = result.handles.debug_logger
    backend._persistence = result.handles.persistence
    backend._retrodeck_paths = result.callbacks.retrodeck_paths
    for attr, key in _BOUND_SERVICE_ATTRS.items():
        setattr(backend, attr, services[key])

    # Loud-failure guard: a wiring drift (renamed/added service) must fail the
    # fixture here, not as a confusing AttributeError deep in a contract test.
    missing = [attr for attr in _BOUND_SERVICE_ATTRS if getattr(backend, attr, None) is None]
    assert not missing, f"contract harness wiring drift — unbound service attrs: {missing}"

    # 5. Serve the backend on a real socket and attach the real shim to it.
    server.set_registry(backend.callables())
    server.set_shutdown_hook(backend._unload)
    await server.start(socket_path_for(None))
    plugin = Plugin()
    assert server.socket_path is not None
    await plugin._attach(server.socket_path, emit)

    return ContractHarness(
        plugin=plugin,
        backend=backend,
        romm=fake_romm,
        sgdb=fake_sgdb,
        emit=emit,
        clock=fake_clock,
        tmp_path=tmp_path,
        uow_factory=result.callbacks.uow_factory,
        retrodeck_paths=result.callbacks.retrodeck_paths,
        release_feed=fake_release_feed,
        server=server,
    )
