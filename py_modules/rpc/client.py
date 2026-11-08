"""Shim side of the protocol — calls backend methods and relays backend events.

The client the Decky shim (``main.py``) uses to reach whatever backend serves
the socket. It correlates responses to requests by ``id``, delivers event
frames to a relay callback in arrival order (so an event a callable emits is
relayed before that callable's response resolves), and reconnects lazily on the
next call after the connection drops.
"""

from __future__ import annotations

import asyncio
import contextlib
import itertools
from typing import TYPE_CHECKING, Any

from rpc import protocol

if TYPE_CHECKING:
    import logging
    from collections.abc import Awaitable, Callable

    EventRelay = Callable[..., Awaitable[Any]]


class BackendCallError(Exception):
    """The backend answered a request with an ``error`` frame."""


class BackendConnectionError(ConnectionError):
    """The backend socket is unreachable or the connection dropped mid-request."""


class BackendClient:
    """One connection to the backend socket.

    Build with :meth:`connect`. ``on_event(name, *args)`` is awaited for every
    event frame, in order, on the reader task.
    """

    def __init__(self, socket_path: str, on_event: EventRelay, logger: logging.Logger) -> None:
        self._socket_path = socket_path
        self._on_event = on_event
        self._logger = logger
        self._ids = itertools.count(1)
        self._pending: dict[int, asyncio.Future[Any]] = {}
        self._reader: asyncio.StreamReader | None = None
        self._writer: asyncio.StreamWriter | None = None
        self._reader_task: asyncio.Task[None] | None = None
        self._connect_lock = asyncio.Lock()
        self._closed = False

    @classmethod
    async def connect(
        cls,
        socket_path: str,
        on_event: EventRelay,
        logger: logging.Logger,
        *,
        timeout: float = 10.0,
    ) -> BackendClient:
        """Connect to *socket_path*, retrying until it accepts or *timeout* elapses.

        The retry covers a backend that is still binding its socket (a spawned
        process, later). Raises :class:`BackendConnectionError` on timeout.
        """
        client = cls(socket_path, on_event, logger)
        await client._open(timeout=timeout)
        return client

    @property
    def connected(self) -> bool:
        return self._writer is not None and not self._writer.is_closing()

    async def _open(self, *, timeout: float) -> None:
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout
        while True:
            try:
                reader, writer = await asyncio.open_unix_connection(self._socket_path, limit=protocol.MAX_LINE_BYTES)
                break
            except (FileNotFoundError, ConnectionRefusedError) as exc:
                if loop.time() >= deadline:
                    raise BackendConnectionError(f"backend socket {self._socket_path} not reachable") from exc
                await asyncio.sleep(0.05)
        self._reader, self._writer = reader, writer
        self._reader_task = asyncio.get_running_loop().create_task(
            self._read_loop(reader), name=f"{protocol.TASK_NAME_PREFIX}client-reader"
        )

    async def _ensure_connected(self) -> None:
        if self._closed:
            raise BackendConnectionError("backend client is closed")
        if self.connected:
            return
        async with self._connect_lock:
            if not self.connected:
                await self._open(timeout=2.0)

    async def call(self, method: str, args: list[Any] | tuple[Any, ...] = ()) -> Any:
        """Invoke *method* with positional *args*; return its JSON result.

        Raises :class:`BackendCallError` for an error frame and
        :class:`BackendConnectionError` when the backend cannot be reached or the
        connection drops before the response arrives.
        """
        await self._ensure_connected()
        request_id = next(self._ids)
        future: asyncio.Future[Any] = asyncio.get_running_loop().create_future()
        self._pending[request_id] = future
        try:
            frame = protocol.encode(protocol.request(request_id, method, list(args)))
        except (TypeError, ValueError) as exc:
            self._pending.pop(request_id, None)
            raise BackendCallError(f"arguments for {method} are not JSON-serializable: {exc}") from exc
        writer = self._writer
        assert writer is not None
        try:
            writer.write(frame)
            await writer.drain()
        except (ConnectionError, RuntimeError) as exc:
            self._pending.pop(request_id, None)
            if future.done():
                future.exception()  # the reader already failed it; mark retrieved
            else:
                future.cancel()
            raise BackendConnectionError(f"backend connection lost while calling {method}") from exc
        try:
            return await future
        finally:
            self._pending.pop(request_id, None)

    async def list_methods(self) -> list[str]:
        """Return the callable names the backend serves."""
        return list(await self.call(protocol.LIST_METHODS, []))

    async def shutdown_backend(self) -> None:
        """Ask the backend to run its shutdown and stop serving."""
        with contextlib.suppress(BackendConnectionError):
            await self.call(protocol.SHUTDOWN, [])

    async def close(self) -> None:
        """Close the connection; pending calls fail with a connection error."""
        self._closed = True
        writer, self._writer = self._writer, None
        if writer is not None:
            writer.close()
            with contextlib.suppress(Exception):
                await writer.wait_closed()
        task = self._reader_task
        if task is not None and not task.done():
            task.cancel()
            with contextlib.suppress(asyncio.CancelledError, Exception):
                await task
        self._fail_pending("backend client closed")

    async def _read_loop(self, reader: asyncio.StreamReader) -> None:
        try:
            while True:
                line = await reader.readline()
                if not line:
                    break
                try:
                    message = protocol.decode(line)
                except protocol.ProtocolError:
                    self._logger.warning("backend sent a malformed frame; ignored")
                    continue
                if "event" in message:
                    await self._relay(message)
                    continue
                self._resolve(message)
        except (ConnectionError, ValueError, asyncio.IncompleteReadError):
            pass
        finally:
            if self._reader is reader:
                self._writer = None
            self._fail_pending("backend connection closed")

    async def _relay(self, message: dict[str, Any]) -> None:
        name = message.get("event")
        args = message.get("args") or []
        try:
            await self._on_event(name, *args)
        except Exception:
            self._logger.exception("relaying backend event '%s' failed", name)

    def _resolve(self, message: dict[str, Any]) -> None:
        request_id = message.get("id")
        future = self._pending.get(request_id) if isinstance(request_id, int) else None
        if future is None or future.done():
            if "error" in message:
                self._logger.warning("backend error frame without a pending request: %s", message["error"])
            return
        if "error" in message:
            error = message.get("error") or {}
            future.set_exception(BackendCallError(str(error.get("message", "backend error"))))
        else:
            future.set_result(message.get("result"))

    def _fail_pending(self, reason: str) -> None:
        for future in self._pending.values():
            if not future.done():
                future.set_exception(BackendConnectionError(reason))
        self._pending.clear()
