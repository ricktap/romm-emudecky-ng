"""The Decky shim (``main.py``): forwarder generation, launch, and unload.

The shim must carry no callable logic — it generates one forwarder per name the
backend lists and relays everything over the socket. The spawn path is driven
with a stand-in backend executable (a Python script speaking the protocol), and
— when it has been built — against the real Rust binary.
"""

from __future__ import annotations

import asyncio
import inspect
import logging
import os
import stat
import sys
from pathlib import Path
from unittest.mock import AsyncMock, patch

import pytest

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


_PY_MODULES = Path(__file__).resolve().parent.parent / "py_modules"
_REPO = Path(__file__).resolve().parent.parent


def _fake_backend(tmp_path: Path, body: str) -> str:
    """Write an executable stand-in for ``bin/romm-emudecky-ng-backend``."""
    script = tmp_path / "fake-backend"
    script.write_text(f"#!{sys.executable}\nimport sys\nsys.path.insert(0, {str(_PY_MODULES)!r})\n{body}")
    script.chmod(script.stat().st_mode | stat.S_IXUSR)
    return str(script)


_SERVING_BACKEND = """
import asyncio, json, logging, os
from rpc.server import BackendServer

if sys.argv[1] == "--list-methods":
    print(json.dumps(["ping", "settings_dir"]))
    sys.exit(0)

async def ping():
    return "pong"

async def settings_dir():
    return os.environ["DECKY_PLUGIN_SETTINGS_DIR"]

async def main():
    server = BackendServer(logger=logging.getLogger("fake"), registry={"ping": ping, "settings_dir": settings_dir})
    await server.start(sys.argv[2])
    await server.wait_closed()

asyncio.run(main())
"""


def test_static_callable_names_reads_the_binary(tmp_path):
    binary = _fake_backend(tmp_path, _SERVING_BACKEND)
    assert main.static_callable_names(binary) == ["ping", "settings_dir"]


def test_static_callable_names_falls_back_to_empty(tmp_path):
    assert main.static_callable_names(str(tmp_path / "missing")) == []
    assert main.static_callable_names(_fake_backend(tmp_path, "print('not json')")) == []
    assert main.static_callable_names(_fake_backend(tmp_path, "print('[1, 2]')")) == []
    assert main.static_callable_names(_fake_backend(tmp_path, "sys.exit(4)")) == []


async def test_spawned_backend_round_trip_and_unload_reaps_it(tmp_path):
    binary = _fake_backend(tmp_path, _SERVING_BACKEND)
    with (
        patch.object(main, "BACKEND_BINARY", binary),
        patch.object(main, "socket_path_for", lambda _rt: protocol.socket_path_for(None)),
    ):
        plugin = main.Plugin()
        await plugin._main()
    assert await plugin.ping() == "pong"  # type: ignore[attr-defined]  # generated forwarder
    import decky

    assert await plugin.settings_dir() == decky.DECKY_PLUGIN_SETTINGS_DIR  # type: ignore[attr-defined]
    process = plugin._backend.process
    await plugin._unload()
    assert process.returncode == 0
    assert plugin._backend is None


async def test_backend_exiting_during_startup_leaves_plugin_inert(tmp_path):
    binary = _fake_backend(tmp_path, "sys.exit(3)")
    with (
        patch.object(main, "BACKEND_BINARY", binary),
        patch.object(main, "socket_path_for", lambda _rt: protocol.socket_path_for(None)),
    ):
        plugin = main.Plugin()
        await asyncio.wait_for(plugin._main(), timeout=10)
    assert plugin._start_failed
    assert plugin._client is None
    assert plugin._backend is None
    await plugin._unload()


async def test_missing_binary_leaves_plugin_inert(tmp_path):
    with patch.object(main, "BACKEND_BINARY", str(tmp_path / "missing")):
        plugin = main.Plugin()
        await plugin._main()
    assert plugin._start_failed


def _built_rust_backend() -> str | None:
    for candidate in (
        os.environ.get("ROMM_RUST_BACKEND_BIN", ""),
        str(_REPO / "backend" / "target" / "release" / "romm-emudecky-ng-backend"),
        str(_REPO / "backend" / "target" / "debug" / "romm-emudecky-ng-backend"),
    ):
        if candidate and os.access(candidate, os.X_OK):
            return candidate
    return None


@pytest.mark.skipif(_built_rust_backend() is None, reason="Rust backend not built (cargo build in backend/)")
async def test_shim_against_the_real_rust_backend(tmp_path):
    """End to end: the real shim spawns the real binary and forwards over the socket."""
    import decky

    binary = _built_rust_backend()
    names = main.static_callable_names(binary)
    assert len(names) == 121
    with (
        patch.object(main, "BACKEND_BINARY", binary),
        patch.object(main, "socket_path_for", lambda _rt: protocol.socket_path_for(None)),
    ):
        plugin = main.Plugin()
        await plugin._main()
    settings = await plugin.get_settings()  # type: ignore[attr-defined]
    assert settings["romm_url"] == ""
    assert settings["has_token"] is False
    assert await plugin.get_settings_reset_notice() == {"pending": False, "backed_up_to": None}  # type: ignore[attr-defined]
    from rpc.client import BackendCallError

    assert await plugin.get_download_queue() == {"downloads": []}  # type: ignore[attr-defined]
    with pytest.raises(BackendCallError, match="not implemented in the Rust backend yet: sync_preview"):
        await plugin.sync_preview()  # type: ignore[attr-defined]
    process = plugin._backend.process
    await plugin._unload()
    assert process.returncode == 0
    assert os.path.exists(os.path.join(decky.DECKY_PLUGIN_SETTINGS_DIR, "settings.json"))


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
    async def failing_launch(_socket_path):
        raise RuntimeError("sqlite migration failed")

    main.install_forwarders(main.Plugin, ["get_settings"])
    with patch.object(main, "_launch_backend", failing_launch):
        plugin = main.Plugin()
        await plugin._main()
    with pytest.raises(RuntimeError, match="Backend unavailable"):
        await plugin.get_settings()  # type: ignore[attr-defined]
