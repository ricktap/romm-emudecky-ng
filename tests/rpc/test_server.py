"""Behavior of :class:`rpc.server.BackendServer`, driven over a real socket.

Uses a tiny in-test registry (the real callable surface is exercised by the
contract tier) and both the real :class:`rpc.client.BackendClient` and a raw
stream connection where the raw bytes on the wire are the point.
"""

from __future__ import annotations

import asyncio
import json
import logging
from unittest.mock import AsyncMock

import pytest

from rpc import protocol
from rpc.client import BackendCallError, BackendClient, BackendConnectionError
from rpc.server import BackendServer

_LOG = logging.getLogger("rpc-test")


def _registry(server_ref: list[BackendServer]):
    async def echo(*args):
        return list(args)

    async def add(a, b):
        return a + b

    async def boom():
        raise ValueError("kaboom")

    async def emits_then_returns(name):
        await server_ref[0].emit("progress", {"name": name})
        return "done"

    async def returns_set():
        return {1, 2}

    async def slow():
        await asyncio.sleep(0.2)
        return "slow"

    return {
        "echo": echo,
        "add": add,
        "boom": boom,
        "emits_then_returns": emits_then_returns,
        "returns_set": returns_set,
        "slow": slow,
    }


@pytest.fixture
async def served():
    ref: list[BackendServer] = []
    shutdown_hook = AsyncMock()
    server = BackendServer(logger=_LOG, on_shutdown=shutdown_hook)
    ref.append(server)
    server.set_registry(_registry(ref))
    await server.start(protocol.socket_path_for(None))
    events = AsyncMock()
    assert server.socket_path is not None
    client = await BackendClient.connect(server.socket_path, events, _LOG)
    yield server, client, events, shutdown_hook
    await client.close()
    await server.close()


class TestDispatch:
    async def test_result_round_trip(self, served):
        _server, client, _events, _hook = served
        assert await client.call("add", [2, 3]) == 5
        assert await client.call("echo", ["a b", None, {"k": [1]}]) == ["a b", None, {"k": [1]}]

    async def test_list_methods(self, served):
        _server, client, _events, _hook = served
        assert await client.list_methods() == sorted(
            ["echo", "add", "boom", "emits_then_returns", "returns_set", "slow"]
        )

    async def test_unknown_method_is_error(self, served):
        _server, client, _events, _hook = served
        with pytest.raises(BackendCallError, match="unknown method: nope"):
            await client.call("nope", [])

    async def test_exception_maps_to_error_message(self, served):
        _server, client, _events, _hook = served
        with pytest.raises(BackendCallError, match="kaboom"):
            await client.call("boom", [])

    async def test_wrong_arity_maps_to_error(self, served):
        _server, client, _events, _hook = served
        with pytest.raises(BackendCallError):
            await client.call("add", [1])

    async def test_non_json_result_maps_to_error(self, served):
        _server, client, _events, _hook = served
        with pytest.raises(BackendCallError, match="not JSON-serializable"):
            await client.call("returns_set", [])

    async def test_concurrent_requests_do_not_block_each_other(self, served):
        _server, client, _events, _hook = served
        slow = asyncio.ensure_future(client.call("slow", []))
        await asyncio.sleep(0.01)
        assert await client.call("add", [1, 1]) == 2
        assert not slow.done()
        assert await slow == "slow"


class TestEvents:
    async def test_event_relayed_before_response_resolves(self, served):
        _server, client, events, _hook = served
        assert await client.call("emits_then_returns", ["x"]) == "done"
        events.assert_awaited_once_with("progress", {"name": "x"})

    async def test_events_before_any_client_are_buffered(self):
        server = BackendServer(logger=_LOG)
        await server.start(protocol.socket_path_for(None))
        await server.emit("early", 1)
        events = AsyncMock()
        assert server.socket_path is not None
        client = await BackendClient.connect(server.socket_path, events, _LOG)
        await client.list_methods()  # round-trip: the buffered frame precedes it
        events.assert_awaited_once_with("early", 1)
        await client.close()
        await server.close()

    async def test_non_json_event_is_dropped(self, served):
        server, client, events, _hook = served
        await server.emit("bad", {1, 2})
        await client.list_methods()
        events.assert_not_awaited()


class TestRawWire:
    async def test_malformed_line_gets_error_frame(self, served):
        server, _client, _events, _hook = served
        reader, writer = await asyncio.open_unix_connection(server.socket_path)
        writer.write(b"not json\n")
        await writer.drain()
        reply = json.loads(await reader.readline())
        assert reply["id"] is None
        assert "malformed" in reply["error"]["message"]
        writer.close()

    async def test_bad_request_shape_gets_error(self, served):
        server, _client, _events, _hook = served
        reader, writer = await asyncio.open_unix_connection(server.socket_path)
        writer.write(b'{"id": 4, "method": 12, "args": []}\n')
        await writer.drain()
        reply = json.loads(await reader.readline())
        assert reply["id"] == 4
        assert "error" in reply
        writer.close()


class TestShutdown:
    async def test_shutdown_runs_hook_answers_then_closes(self, served):
        server, client, _events, hook = served
        await client.shutdown_backend()
        hook.assert_awaited_once()
        await asyncio.wait_for(server.wait_closed(), 1)
        with pytest.raises(BackendConnectionError):
            await client.call("add", [1, 2])

    async def test_shutdown_hook_failure_is_reported(self):
        server = BackendServer(logger=_LOG, on_shutdown=AsyncMock(side_effect=RuntimeError("stuck")))
        await server.start(protocol.socket_path_for(None))
        assert server.socket_path is not None
        client = await BackendClient.connect(server.socket_path, AsyncMock(), _LOG)
        with pytest.raises(BackendCallError, match="stuck"):
            await client.call(protocol.SHUTDOWN, [])
        await asyncio.wait_for(server.wait_closed(), 1)
        await client.close()
