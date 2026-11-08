"""Decky shim — the only file Decky Loader talks to.

Holds no callable logic. ``_main`` launches the backend, connects to it over the
backend protocol socket (``docs/architecture/backend-protocol.md``), asks it for
its callable names, and generates one ``async def`` forwarder per name on
:class:`Plugin` (Decky resolves a frontend ``callable("name")`` as a method on
this class). Backend events are relayed to ``decky.emit``. ``_unload`` asks the
backend to shut down.

Today the backend is the Python :mod:`backend` module, run in this process
(Decky is a frozen executable with no separate interpreter). Moving to a
separate binary only replaces :func:`_launch_backend`.
"""

import asyncio
import inspect
import os
import sys
from typing import Any

plugin_dir = os.path.dirname(__file__)
sys.path.insert(0, os.path.join(plugin_dir, "py_modules"))
sys.path.insert(0, plugin_dir)

import decky

from rpc.client import BackendClient
from rpc.protocol import socket_path_for


async def _launch_backend(socket_path: str) -> Any:
    """Start the backend serving *socket_path*; return a handle with ``wait_closed()``.

    The single swap point for the backend implementation. In-process Python
    today; a native backend becomes "spawn ``bin/<binary>`` with the Decky env
    and *socket_path*, return the process handle".
    """
    from backend import serve_in_process

    return await serve_in_process(
        socket_path,
        settings_dir=decky.DECKY_PLUGIN_SETTINGS_DIR,
        runtime_dir=decky.DECKY_PLUGIN_RUNTIME_DIR,
        plugin_dir=decky.DECKY_PLUGIN_DIR,
        user_home=decky.DECKY_USER_HOME,
        logger=decky.logger,
    )


# How long a frontend call made during plugin startup waits for the backend.
_READY_TIMEOUT_S = 60.0


def static_callable_names() -> list[str]:
    """The callable surface, known before the backend runs.

    Decky starts ``_main`` as a fire-and-forget task and serves frontend calls in
    parallel, so the forwarders must exist at import time — a call that arrives
    while the backend is still starting then waits for it instead of failing with
    "no such method". Today the surface is read from the Python ``Backend``
    class; a native backend ships the same list as data.
    """
    from backend import Backend

    return sorted(
        name
        for name, fn in inspect.getmembers(Backend, inspect.iscoroutinefunction)
        if not name.startswith("_")
    )


def _make_forwarder(name: str):
    async def forward(self, *args):
        await self._wait_ready()
        return await self._client.call(name, args)

    forward.__name__ = name
    forward.__qualname__ = f"Plugin.{name}"
    forward.__doc__ = f"Forward the ``{name}`` callable to the backend."
    return forward


def install_forwarders(cls: type, names: list[str]) -> None:
    """Add one ``async def`` forwarder per backend callable name to *cls*.

    Private names (leading ``_``) are refused — they could shadow the Decky
    lifecycle hooks. Re-installing an already-forwarded name is a no-op.
    """
    for name in names:
        if name.startswith("_"):
            continue
        existing = cls.__dict__.get(name)
        if existing is not None and getattr(existing, "__name__", None) != name:
            continue
        setattr(cls, name, _make_forwarder(name))


class Plugin:
    _client: Any = None
    _backend: Any = None
    _ready_event: asyncio.Event | None = None
    _start_failed: bool = False

    def _ready(self) -> asyncio.Event:
        if self._ready_event is None:
            self._ready_event = asyncio.Event()
        return self._ready_event

    async def _wait_ready(self) -> None:
        """Block a forwarded call until the backend is attached (or failed).

        Once ready this returns without yielding, so steady-state calls pay nothing.
        """
        ready = self._ready()
        if not ready.is_set():
            try:
                await asyncio.wait_for(ready.wait(), timeout=_READY_TIMEOUT_S)
            except TimeoutError:
                raise RuntimeError("Backend is still starting — try again in a moment") from None
        if self._start_failed or self._client is None:
            raise RuntimeError("Backend unavailable — see the plugin log")

    async def _main(self):  # Decky lifecycle — must be async
        socket_path = socket_path_for(decky.DECKY_PLUGIN_RUNTIME_DIR)
        try:
            self._backend = await _launch_backend(socket_path)
            await self._attach(socket_path, decky.emit)
        except Exception:
            # Same posture as the old in-process startup failure: log and stay inert.
            decky.logger.exception("Backend failed to start; plugin stays inert")
            self._start_failed = True
            return
        finally:
            self._ready().set()
        decky.logger.info("EmuDecky RomM Manager plugin loaded")

    async def _attach(self, socket_path: str, relay: Any) -> None:
        """Connect to the backend at *socket_path* and generate the callable forwarders."""
        self._client = await BackendClient.connect(socket_path, relay, decky.logger)
        install_forwarders(type(self), await self._client.list_methods())
        self._ready().set()

    async def _unload(self):  # Decky lifecycle — must be async
        client, self._client = self._client, None
        if client is not None:
            await client.shutdown_backend()
            await client.close()
        backend, self._backend = self._backend, None
        if backend is not None:
            await backend.wait_closed()
        decky.logger.info("EmuDecky RomM Manager plugin unloaded")


# Forwarders exist from import time on (see static_callable_names); _attach only
# adds names the running backend reports beyond the static list.
install_forwarders(Plugin, static_callable_names())
