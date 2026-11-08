"""``backend.serve_in_process`` and the callable registry of :class:`backend.Backend`.

Per-callable delegation is covered in ``test_plugin_callable_delegation.py``;
this file pins the registry and the in-process launch/shutdown wiring.
"""

from __future__ import annotations

import logging
from unittest.mock import AsyncMock, patch

from backend import Backend, BackendEnv, serve_in_process
from rpc import protocol
from rpc.client import BackendClient

_LOG = logging.getLogger("backend-test")


def test_callable_names_are_the_public_async_methods():
    names = Backend.callable_names()
    assert "get_settings" in names
    assert "start_download" in names
    assert not any(name.startswith("_") for name in names)
    assert "callables" not in names  # sync helper, not a callable


def test_migration_blocked_callables_are_registered():
    assert "select_disc" in Backend.callable_names()


async def test_serve_in_process_starts_backend_serves_and_shuts_down(tmp_path):
    seen: dict[str, BackendEnv] = {}

    async def fake_main(self, env):
        seen["env"] = env
        self.settings = {"_settings_reset_notice": None}

    unload = AsyncMock()
    with patch.object(Backend, "_main", fake_main), patch.object(Backend, "_unload", unload):
        server = await serve_in_process(
            protocol.socket_path_for(None),
            settings_dir=str(tmp_path / "s"),
            runtime_dir=str(tmp_path / "r"),
            plugin_dir=str(tmp_path / "p"),
            user_home=str(tmp_path / "h"),
            logger=_LOG,
        )
        env = seen["env"]
        assert env.settings_dir == str(tmp_path / "s")
        assert env.emit == server.emit

        events = AsyncMock()
        client = await BackendClient.connect(server.socket_path, events, _LOG)
        assert sorted(await client.list_methods()) == Backend.callable_names()
        assert await client.call("get_settings_reset_notice") == {"pending": False, "backed_up_to": None}

        await client.shutdown_backend()
        await server.wait_closed()
        unload.assert_awaited_once()
        await client.close()
