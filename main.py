"""Decky shim — the only file Decky Loader talks to.

Holds no callable logic. ``_main`` spawns the native backend
(``bin/romm-emudecky-ng-backend``), connects to it over the backend protocol
socket (``docs/architecture/backend-protocol.md``), and relays its events to
``decky.emit``. Every frontend callable is an ``async def`` forwarder generated
on :class:`Plugin` (Decky resolves a frontend ``callable("name")`` as a method
on this class). ``_unload`` asks the backend to shut down and reaps the process.
"""

import asyncio
import json
import os
import subprocess
import sys
from typing import Any

plugin_dir = os.path.dirname(__file__)
sys.path.insert(0, os.path.join(plugin_dir, "py_modules"))
sys.path.insert(0, plugin_dir)

import decky

from rpc.client import BackendClient
from rpc.protocol import socket_path_for

# The native backend. The Decky CLI builds it from ``backend/`` and ships it in ``bin/``.
BACKEND_BINARY = os.path.join(plugin_dir, "bin", "romm-emudecky-ng-backend")

# How long a frontend call made during plugin startup waits for the backend.
_READY_TIMEOUT_S = 60.0
# How long the backend may take to bind its socket (migrations, legacy token mint).
_CONNECT_TIMEOUT_S = 60.0
# How long ``_unload`` waits for the process to exit after ``__shutdown__``.
_EXIT_TIMEOUT_S = 10.0


class BackendProcess:
    """The spawned backend process; ``wait_closed`` reaps it (killing it if it hangs)."""

    def __init__(self, process: asyncio.subprocess.Process) -> None:
        self.process = process

    async def wait_closed(self) -> None:
        try:
            await asyncio.wait_for(self.process.wait(), timeout=_EXIT_TIMEOUT_S)
        except TimeoutError:
            decky.logger.warning("Backend did not exit after shutdown; killing it")
            self.process.kill()
            await self.process.wait()


def backend_env() -> dict[str, str]:
    """The environment the backend runs with: ours plus the Decky directories."""
    env = dict(os.environ)
    env.update(
        {
            "DECKY_PLUGIN_SETTINGS_DIR": decky.DECKY_PLUGIN_SETTINGS_DIR,
            "DECKY_PLUGIN_RUNTIME_DIR": decky.DECKY_PLUGIN_RUNTIME_DIR,
            "DECKY_PLUGIN_LOG_DIR": decky.DECKY_PLUGIN_LOG_DIR,
            "DECKY_PLUGIN_DIR": decky.DECKY_PLUGIN_DIR,
            "DECKY_USER_HOME": decky.DECKY_USER_HOME,
        }
    )
    return env


async def _launch_backend(socket_path: str) -> BackendProcess:
    """Spawn the native backend serving *socket_path*; return its process handle."""
    process = await asyncio.create_subprocess_exec(
        BACKEND_BINARY,
        "--socket",
        socket_path,
        env=backend_env(),
        stdin=asyncio.subprocess.DEVNULL,
    )
    return BackendProcess(process)


def static_callable_names(binary: str = BACKEND_BINARY) -> list[str]:
    """The callable surface, known before the backend runs.

    Decky starts ``_main`` as a fire-and-forget task and serves frontend calls in
    parallel, so the forwarders must exist at import time — a call that arrives
    while the backend is still starting then waits for it instead of failing with
    "no such method". The binary prints its surface with ``--list-methods``; when
    it is missing or broken the list is empty (logged) and ``_attach`` installs
    whatever the running backend reports.
    """
    try:
        out = subprocess.run([binary, "--list-methods"], capture_output=True, timeout=10, check=True)
        names = json.loads(out.stdout)
    except (OSError, subprocess.SubprocessError, ValueError) as exc:
        decky.logger.error("Could not read the backend callable list from %s: %s", binary, exc)
        return []
    if not isinstance(names, list) or not all(isinstance(n, str) for n in names):
        decky.logger.error("Backend %s printed a malformed callable list", binary)
        return []
    return sorted(names)


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
            # Log and stay inert; a half-started process is not left behind.
            decky.logger.exception("Backend failed to start; plugin stays inert")
            self._start_failed = True
            await self._kill_backend()
            return
        finally:
            self._ready().set()
        decky.logger.info("EmuDecky RomM Manager plugin loaded")

    async def _attach(self, socket_path: str, relay: Any) -> None:
        """Connect to the backend at *socket_path* and generate the callable forwarders.

        Fails fast when the spawned backend process exits before its socket accepts.
        """
        connect = asyncio.ensure_future(
            BackendClient.connect(socket_path, relay, decky.logger, timeout=_CONNECT_TIMEOUT_S)
        )
        process = getattr(self._backend, "process", None)
        if process is not None:
            exited = asyncio.ensure_future(process.wait())
            done, _ = await asyncio.wait({connect, exited}, return_when=asyncio.FIRST_COMPLETED)
            if connect not in done:
                connect.cancel()
                raise RuntimeError(f"Backend exited during startup (code {process.returncode})")
            exited.cancel()
        self._client = await connect
        install_forwarders(type(self), await self._client.list_methods())
        self._ready().set()

    async def _kill_backend(self) -> None:
        backend, self._backend = self._backend, None
        process = getattr(backend, "process", None)
        if process is not None and process.returncode is None:
            process.kill()
            await process.wait()

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
