"""Fixtures for the callable contract-test tier.

Exposes the ``harness`` fixture: the real :class:`main.Plugin` shim talking
over a real Unix socket to the real backend, wired through the real
:func:`bootstrap` with only the network edges faked. See
:mod:`tests.contract._harness` for the build recipe and the real-vs-faked
boundary.
"""

from __future__ import annotations

from unittest import mock

import pytest

from tests.contract._harness import build_contract_harness


@pytest.fixture(autouse=True)
def _isolate_system_flatpak_root(tmp_path):
    """Keep the contract harness hermetic — never read the host's real RetroDECK.

    The real ``CoreResolver`` / RetroArch core-info reader probe the system
    flatpak root (``/var/lib/flatpak``) before the per-user one. On a dev machine
    with RetroDECK installed, that real ``es_systems.xml`` would win over anything
    a test seeds under ``tmp_path``. Repointing the system root at a nonexistent
    tmp path makes the per-user seed the only source, so contract tests are
    deterministic on any machine.
    """
    with mock.patch("adapters.flatpak_install.SYSTEM_FLATPAK_ROOT", str(tmp_path / "no_system_flatpak")):
        yield


@pytest.fixture
async def harness(tmp_path):
    """A wired real ``Plugin`` plus the fake edges a contract test drives.

    Async so the harness binds the test's *running* event loop into the
    services' ``RuntimeBundle.loop`` — the callables ``await`` on that loop,
    and a mismatched loop would raise "got Future attached to a different
    loop" on the first ``run_in_executor``.

    Each test gets a fresh ``tmp_path`` (own SQLite db, settings file, file
    stores), so state never leaks between contract tests.
    """
    built = await build_contract_harness(tmp_path)
    try:
        yield built
    finally:
        await built.close()
