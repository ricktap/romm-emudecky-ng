"""The Decky shim (``main.py``): forwarder generation, launch, and unload.

The shim must carry no callable logic — it generates one forwarder per name the
backend lists and relays everything over the socket.
"""

from __future__ import annotations

import inspect
import logging
from unittest.mock import AsyncMock, patch

import main
from rpc import protocol
from rpc.server import BackendServer

_LOG = logging.getLogger("shim-test")


def test_install_forwarders_adds_async_methods():
    cls = type("Shim", (), {})
    main.install_forwarders(cls, ["get_settings", "start_download"])
    assert inspect.iscoroutinefunction(cls.get_settings)
    assert cls.start_download.__name__ == "start_download"


def test_install_forwarders_refuses_private_names():
    cls = type("Shim", (), {})
    main.install_forwarders(cls, ["_main", "__shutdown__"])
    assert "_main" not in cls.__dict__
    assert "__shutdown__" not in cls.__dict__


def test_install_forwarders_never_overwrites_real_methods():
    async def own(self):
        return "mine"

    cls = type("Shim", (), {"get_settings": own})
    main.install_forwarders(cls, ["get_settings"])
    assert cls.get_settings is own


async def _serve(registry):
    server = BackendServer(logger=_LOG, registry=registry, on_shutdown=AsyncMock())
    await server.start(protocol.socket_path_for(None))
    return server


async def test_attach_generates_forwarders_that_round_trip():
    async def add(a, b):
        return a + b

    server = await _serve({"add": add})
    plugin = main.Plugin()
    relay = AsyncMock()
    await plugin._attach(server.socket_path, relay)
    assert await plugin.add(2, 5) == 7  # type: ignore[attr-defined]  # generated forwarder
    await plugin._client.close()
    await server.close()


async def test_main_launches_backend_and_unload_shuts_it_down():
    async def ping():
        return "pong"

    holder: dict[str, BackendServer] = {}

    async def fake_launch(socket_path):
        server = BackendServer(logger=_LOG, registry={"ping": ping}, on_shutdown=AsyncMock())
        await server.start(socket_path)
        holder["server"] = server
        return server

    with (
        patch.object(main, "_launch_backend", fake_launch),
        patch.object(main, "socket_path_for", lambda _rt: protocol.socket_path_for(None)),
    ):
        plugin = main.Plugin()
        await plugin._main()
    assert await plugin.ping() == "pong"  # type: ignore[attr-defined]  # generated forwarder

    await plugin._unload()
    assert plugin._client is None
    holder["server"]._on_shutdown.assert_awaited_once()


async def test_main_stays_inert_when_backend_fails_to_start():
    async def failing_launch(_socket_path):
        raise RuntimeError("sqlite migration failed")

    with patch.object(main, "_launch_backend", failing_launch):
        plugin = main.Plugin()
        await plugin._main()
    assert plugin._client is None
    await plugin._unload()  # no client, no backend — must not raise


def test_forwarders_exist_at_import_for_every_backend_callable():
    """Decky serves frontend calls in parallel with ``_main`` — the methods must already exist."""
    names = main.static_callable_names()
    assert "get_settings" in names
    assert "start_download" in names
    for name in names:
        assert inspect.iscoroutinefunction(getattr(main.Plugin, name)), name


async def test_call_made_during_startup_waits_for_the_backend():
    """A frontend call that lands before ``_main`` attaches waits instead of failing."""
    import asyncio

    async def ping():
        return "pong"

    started = asyncio.Event()

    async def slow_launch(socket_path):
        await started.wait()
        server = BackendServer(logger=_LOG, registry={"ping": ping}, on_shutdown=AsyncMock())
        await server.start(socket_path)
        return server

    main.install_forwarders(main.Plugin, ["ping"])
    with (
        patch.object(main, "_launch_backend", slow_launch),
        patch.object(main, "socket_path_for", lambda _rt: protocol.socket_path_for(None)),
    ):
        plugin = main.Plugin()
        startup = asyncio.ensure_future(plugin._main())
        early_call = asyncio.ensure_future(plugin.ping())  # type: ignore[attr-defined]
        await asyncio.sleep(0.05)
        assert not early_call.done()  # waiting on the readiness gate, not failed
        started.set()
        await startup
        assert await early_call == "pong"
    await plugin._unload()


async def test_call_after_failed_startup_raises_backend_unavailable():
    import pytest

    async def failing_launch(_socket_path):
        raise RuntimeError("sqlite migration failed")

    with patch.object(main, "_launch_backend", failing_launch):
        plugin = main.Plugin()
        await plugin._main()
    with pytest.raises(RuntimeError, match="Backend unavailable"):
        await plugin.get_settings()  # type: ignore[attr-defined]
