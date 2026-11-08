"""Connection behavior of :class:`rpc.client.BackendClient`: connect, reconnect, close."""

from __future__ import annotations

import asyncio
import logging
from unittest.mock import AsyncMock

import pytest

from rpc import protocol
from rpc.client import BackendCallError, BackendClient, BackendConnectionError
from rpc.server import BackendServer

_LOG = logging.getLogger("rpc-test")


async def _ping():
    return "pong"


async def _serve(path: str) -> BackendServer:
    server = BackendServer(logger=_LOG, registry={"ping": _ping})
    await server.start(path)
    return server


async def test_connect_times_out_without_server():
    path = protocol.socket_path_for(None)
    with pytest.raises(BackendConnectionError):
        await BackendClient.connect(path, AsyncMock(), _LOG, timeout=0.1)


async def test_connect_waits_for_late_server():
    path = protocol.socket_path_for(None)

    async def late():
        await asyncio.sleep(0.1)
        return await _serve(path)

    server_task = asyncio.ensure_future(late())
    client = await BackendClient.connect(path, AsyncMock(), _LOG, timeout=2)
    assert await client.call("ping") == "pong"
    await client.close()
    await (await server_task).close()


async def test_reconnects_on_next_call_after_server_restart():
    path = protocol.socket_path_for(None)
    server = await _serve(path)
    client = await BackendClient.connect(path, AsyncMock(), _LOG)
    assert await client.call("ping") == "pong"

    await server.close()
    await asyncio.sleep(0.05)  # reader loop observes EOF
    assert not client.connected

    server = await _serve(path)
    assert await client.call("ping") == "pong"
    assert client.connected
    await client.close()
    await server.close()


async def test_pending_call_fails_when_connection_drops():
    path = protocol.socket_path_for(None)
    started = asyncio.Event()

    async def hang():
        started.set()
        await asyncio.Event().wait()

    server = BackendServer(logger=_LOG, registry={"hang": hang})
    await server.start(path)
    client = await BackendClient.connect(path, AsyncMock(), _LOG)
    call = asyncio.ensure_future(client.call("hang"))
    await started.wait()
    await server.close()
    with pytest.raises(BackendConnectionError):
        await asyncio.wait_for(call, 1)
    await client.close()


async def test_closed_client_refuses_calls():
    path = protocol.socket_path_for(None)
    server = await _serve(path)
    client = await BackendClient.connect(path, AsyncMock(), _LOG)
    await client.close()
    with pytest.raises(BackendConnectionError):
        await client.call("ping")
    await server.close()


async def test_non_json_args_raise_call_error():
    path = protocol.socket_path_for(None)
    server = await _serve(path)
    client = await BackendClient.connect(path, AsyncMock(), _LOG)
    with pytest.raises(BackendCallError, match="not JSON-serializable"):
        await client.call("ping", [object()])
    await client.close()
    await server.close()


async def test_failing_event_relay_does_not_break_the_connection():
    path = protocol.socket_path_for(None)
    server = await _serve(path)
    relay = AsyncMock(side_effect=RuntimeError("frontend gone"))
    client = await BackendClient.connect(path, relay, _LOG)
    await server.emit("evt", 1)
    assert await client.call("ping") == "pong"
    relay.assert_awaited_once_with("evt", 1)
    await client.close()
    await server.close()
