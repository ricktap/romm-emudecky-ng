"""Sanity checks for ``backend/callables.json``.

``backend/callables.json`` is the list of wire callables the native Rust backend
serves (and prints for ``--list-methods``); it is the source of truth for the
backend callable surface on the Rust branch. The legacy Python ``Backend`` is
no longer authoritative. Frontend parity (both directions) is enforced by
``scripts/check_callable_manifest.py`` and ``tests/contract/test_callable_manifest.py``;
this module pins the file's own shape and the frontend-coverage direction.
"""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parent.parent
_CALLABLES = _REPO_ROOT / "backend" / "callables.json"
_SCRIPT_PATH = _REPO_ROOT / "scripts" / "check_callable_manifest.py"


def _load_names() -> list[str]:
    names = json.loads(_CALLABLES.read_text(encoding="utf-8"))
    assert isinstance(names, list)
    assert all(isinstance(n, str) and n for n in names)
    return names


def _frontend_names() -> set[str]:
    spec = importlib.util.spec_from_file_location("check_callable_manifest", _SCRIPT_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return set(module.parse_frontend_callables(_REPO_ROOT / "src"))


def test_callables_json_is_sorted():
    names = _load_names()
    assert names == sorted(names), "keep callables.json sorted"


def test_callables_json_is_unique():
    names = _load_names()
    assert len(names) == len(set(names)), "callables.json lists a name twice"


def test_every_frontend_callable_is_listed():
    missing = sorted(_frontend_names() - set(_load_names()))
    assert missing == [], f"frontend declares callables the backend does not list: {missing}"
