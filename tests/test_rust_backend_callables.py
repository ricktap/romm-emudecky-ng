"""The Rust backend lists exactly the Python backend's callable surface.

``backend/callables.json`` is the list the native binary serves (and prints for
``--list-methods``); the Python :class:`backend.Backend` stays the spec until
every callable is ported, and the frontend parity gate checks against it. A
callable added to or removed from one side without the other fails here.
"""

from __future__ import annotations

import json
from pathlib import Path

from backend import Backend

_CALLABLES = Path(__file__).resolve().parent.parent / "backend" / "callables.json"


def test_rust_callable_list_matches_python_backend():
    rust = json.loads(_CALLABLES.read_text())
    assert rust == sorted(rust), "keep callables.json sorted"
    assert rust == Backend.callable_names()
