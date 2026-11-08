"""Contract tests for the backend protocol itself, over the real harness socket.

The rest of the contract tier proves each callable's response shape survives
the JSON round-trip; this module pins the protocol behaviors the shim depends
on — method listing, unknown-method and exception mapping, event delivery, and
shutdown — against the real backend the harness serves.
"""

from __future__ import annotations

import pytest
from rpc.client import BackendCallError, BackendConnectionError

from backend import Backend


async def test_method_listing_is_the_backend_callable_surface(harness):
    assert await harness.plugin._client.list_methods() == Backend.callable_names()


async def test_shim_has_a_forwarder_for_every_listed_method(harness):
    for name in Backend.callable_names():
        assert callable(getattr(harness.plugin, name))


async def test_unknown_method_is_an_error_frame(harness):
    with pytest.raises(BackendCallError, match="unknown method: no_such_callable"):
        await harness.plugin._client.call("no_such_callable", [])


async def test_backend_exception_surfaces_as_call_error(harness):
    # ``refresh_cover_artwork`` coerces its arg with int(); a non-numeric string
    # raises inside the backend and must come back as an error frame, not hang.
    with pytest.raises(BackendCallError, match="invalid literal"):
        await harness.plugin.refresh_cover_artwork("not-a-number")


async def test_wrong_arity_surfaces_as_call_error(harness):
    with pytest.raises(BackendCallError):
        await harness.plugin.get_installed_rom()


async def test_event_emitted_by_a_service_reaches_the_relay(harness):
    # ``harness.server.emit`` is the EventEmitter every service was wired with.
    await harness.server.emit("sync_progress", {"running": False})
    await harness.flush_events()
    harness.emit.assert_any_await("sync_progress", {"running": False})


async def test_shutdown_runs_backend_unload_and_closes(harness):
    await harness.plugin._client.shutdown_backend()
    await harness.server.wait_closed()
    with pytest.raises(BackendConnectionError):
        await harness.plugin.get_settings()
