"""Backend side of the protocol — serves a callable registry on a Unix socket.

Hosts any name → coroutine-function registry behind the wire format in
:mod:`rpc.protocol`: dispatches requests (each in its own task, so a
long-running callable never blocks a concurrent one), answers the reserved
``__list_methods__`` / ``__shutdown__`` requests, and turns backend events into
``{"event", "args"}`` frames for every connected client. Knows nothing about
what the callables do.
"""

from __future__ import annotations

import asyncio
import contextlib
import os
from typing import TYPE_CHECKING, Any

from rpc import protocol

if TYPE_CHECKING:
    import logging
    from collections.abc import Awaitable, Callable, Mapping

    Registry = Mapping[str, Callable[..., Awaitable[Any]]]

# Events emitted while no client is connected (startup work runs before the
# shim connects) are held and flushed to the first client. Bounded so a backend
# that never gets a client cannot grow without limit.
_MAX_BUFFERED_EVENTS = 1000


class BackendServer:
    """Serve *registry* over newline-delimited JSON on a Unix socket.

    ``registry`` maps each callable's wire name to an async function taking the
    request's positional ``args``. ``on_shutdown`` (optional) runs when a client
    sends ``__shutdown__``; the server answers that request, then stops
    accepting and closes every connection.
    """

    def __init__(
        self,
        *,
        registry: Registry | None = None,
        logger: logging.Logger,
        on_shutdown: Callable[[], Awaitable[None]] | None = None,
    ) -> None:
        self._registry: dict[str, Callable[..., Awaitable[Any]]] = dict(registry or {})
        self._logger = logger
        self._on_shutdown = on_shutdown
        self._server: asyncio.AbstractServer | None = None
        self._socket_path: str | None = None
        self._writers: set[asyncio.StreamWriter] = set()
        self._tasks: set[asyncio.Task[Any]] = set()
        self._buffered: list[bytes] = []
        self._closed = asyncio.Event()

    # ── registry ───────────────────────────────────────────────────────

    def set_registry(self, registry: Registry) -> None:
        """Replace the callable registry (bound once the backend has started)."""
        self._registry = dict(registry)

    def set_shutdown_hook(self, on_shutdown: Callable[[], Awaitable[None]]) -> None:
        """Replace the coroutine run on ``__shutdown__`` before the server closes."""
        self._on_shutdown = on_shutdown

    def method_names(self) -> list[str]:
        """The callable names this server answers, sorted."""
        return sorted(self._registry)

    @property
    def socket_path(self) -> str | None:
        return self._socket_path

    # ── events ─────────────────────────────────────────────────────────

    async def emit(self, event: str, /, *args: object) -> None:
        """Send ``{"event": event, "args": [...]}`` to every connected client.

        Satisfies the services' ``EventEmitter`` Protocol, so it is wired in
        place of ``decky.emit``. With no client connected the frame is buffered
        and flushed to the next client that connects.
        """
        try:
            frame = protocol.encode(protocol.event(event, list(args)))
        except (TypeError, ValueError):
            self._logger.exception("backend event '%s' carries a non-JSON payload; dropped", event)
            return
        if not self._writers:
            if len(self._buffered) < _MAX_BUFFERED_EVENTS:
                self._buffered.append(frame)
            return
        for writer in list(self._writers):
            await self._write(writer, frame)

    # ── lifecycle ──────────────────────────────────────────────────────

    async def start(self, socket_path: str) -> None:
        """Bind and listen on *socket_path* (a stale socket file is replaced)."""
        with contextlib.suppress(FileNotFoundError):
            os.unlink(socket_path)
        os.makedirs(os.path.dirname(socket_path), exist_ok=True)
        self._server = await asyncio.start_unix_server(
            self._handle_client, path=socket_path, limit=protocol.MAX_LINE_BYTES
        )
        self._socket_path = socket_path
        self._closed.clear()

    async def close(self) -> None:
        """Stop accepting, close every connection, cancel in-flight requests."""
        server = self._server
        self._server = None
        if server is not None:
            server.close()
        for writer in list(self._writers):
            writer.close()
        self._writers.clear()
        current = asyncio.current_task()
        pending = [t for t in self._tasks if t is not current]
        for task in pending:
            task.cancel()
        if pending:
            await asyncio.gather(*pending, return_exceptions=True)
        if server is not None:
            with contextlib.suppress(Exception):
                await server.wait_closed()
        if self._socket_path:
            with contextlib.suppress(FileNotFoundError):
                os.unlink(self._socket_path)
        self._closed.set()

    async def wait_closed(self) -> None:
        """Block until :meth:`close` has run (e.g. after a ``__shutdown__``)."""
        await self._closed.wait()

    # ── connection handling ────────────────────────────────────────────

    async def _handle_client(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        task = asyncio.current_task()
        if task is not None:
            task.set_name(f"{protocol.TASK_NAME_PREFIX}connection")
        self._writers.add(writer)
        buffered, self._buffered = self._buffered, []
        for frame in buffered:
            await self._write(writer, frame)
        try:
            while True:
                try:
                    line = await reader.readline()
                except (ValueError, asyncio.LimitOverrunError):
                    await self._write(writer, protocol.encode(protocol.error_response(None, "frame too large")))
                    break
                except ConnectionError:
                    break
                if not line:
                    break
                if not line.strip():
                    continue
                self._spawn(self._dispatch(line, writer))
        finally:
            self._writers.discard(writer)
            writer.close()

    def _spawn(self, coro: Awaitable[None]) -> None:
        task = asyncio.ensure_future(coro)
        self._tasks.add(task)
        task.add_done_callback(self._tasks.discard)

    async def _dispatch(self, line: bytes, writer: asyncio.StreamWriter) -> None:
        try:
            message = protocol.decode(line)
        except protocol.ProtocolError as exc:
            await self._write(writer, protocol.encode(protocol.error_response(None, str(exc))))
            return
        request_id = message.get("id")
        method = message.get("method")
        args = message.get("args", [])
        if not isinstance(method, str) or not isinstance(args, list):
            await self._respond(writer, protocol.error_response(request_id, "request needs a string 'method' and list 'args'"))
            return

        if method == protocol.LIST_METHODS:
            await self._respond(writer, protocol.result_response(request_id, self.method_names()))
            return
        if method == protocol.SHUTDOWN:
            await self._shutdown(request_id, writer)
            return

        fn = self._registry.get(method)
        if fn is None:
            await self._respond(writer, protocol.error_response(request_id, f"unknown method: {method}"))
            return
        try:
            result = await fn(*args)
        except asyncio.CancelledError:
            raise
        except Exception as exc:  # every callable failure crosses the wire as an error frame
            self._logger.exception("backend callable '%s' raised", method)
            await self._respond(writer, protocol.error_response(request_id, str(exc) or type(exc).__name__))
            return
        await self._respond(writer, protocol.result_response(request_id, result))

    async def _shutdown(self, request_id: Any, writer: asyncio.StreamWriter) -> None:
        error: str | None = None
        if self._on_shutdown is not None:
            try:
                await self._on_shutdown()
            except Exception as exc:
                self._logger.exception("backend shutdown hook raised")
                error = str(exc) or type(exc).__name__
        response = (
            protocol.error_response(request_id, error)
            if error is not None
            else protocol.result_response(request_id, None)
        )
        await self._respond(writer, response)
        await self.close()

    async def _respond(self, writer: asyncio.StreamWriter, message: dict[str, Any]) -> None:
        try:
            frame = protocol.encode(message)
        except (TypeError, ValueError) as exc:
            self._logger.exception("backend response is not JSON-serializable")
            frame = protocol.encode(protocol.error_response(message.get("id"), f"result is not JSON-serializable: {exc}"))
        await self._write(writer, frame)

    async def _write(self, writer: asyncio.StreamWriter, frame: bytes) -> None:
        if writer.is_closing():
            return
        try:
            writer.write(frame)
            await writer.drain()
        except (ConnectionError, RuntimeError):
            self._writers.discard(writer)
