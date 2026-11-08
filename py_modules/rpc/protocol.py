"""Wire format of the backend protocol — framing, message shapes, socket path.

The contract any backend implementation (the in-process Python server today, a
Rust binary later) must honour byte-for-byte. One JSON object per line (UTF-8,
``\\n``-terminated); JSON only — no pickling, no binary frames. See
``docs/architecture/backend-protocol.md`` for the prose specification.
"""

from __future__ import annotations

import json
import os
import tempfile
from typing import Any

# Reserved method names. Real callables never start with ``__`` (the callable
# surface is public ``snake_case`` names), so these cannot collide.
LIST_METHODS = "__list_methods__"
SHUTDOWN = "__shutdown__"

SOCKET_FILENAME = "backend.sock"

# Name prefix of the long-lived connection tasks (server connection handlers,
# client reader loops) so callers that drain "background work" can skip them.
TASK_NAME_PREFIX = "rpc:"

# Maximum bytes in one framed line. Some callables return base64 artwork, so the
# asyncio default (64 KiB) is far too small; both ends use the same ceiling.
MAX_LINE_BYTES = 64 * 1024 * 1024

# ``sun_path`` holds 108 bytes on Linux and 104 on macOS (including the NUL).
# Stay under the smaller so the same rule holds on every dev host.
_MAX_SOCKET_PATH = 100


class ProtocolError(Exception):
    """A frame that is not valid protocol JSON (malformed line, wrong shape)."""


def encode(message: dict[str, Any]) -> bytes:
    """Serialize one protocol message to a single ``\\n``-terminated UTF-8 line.

    ``allow_nan=False`` keeps the output strict JSON (``NaN``/``Infinity`` are not
    JSON and a non-Python backend's parser would reject them). Raises
    ``TypeError`` / ``ValueError`` when *message* holds a non-JSON value.
    """
    return (json.dumps(message, ensure_ascii=False, separators=(",", ":"), allow_nan=False) + "\n").encode("utf-8")


def decode(line: bytes) -> dict[str, Any]:
    """Parse one framed line into a protocol message dict.

    Raises :class:`ProtocolError` when the line is not a JSON object.
    """
    try:
        message = json.loads(line.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ProtocolError(f"malformed frame: {exc}") from exc
    if not isinstance(message, dict):
        raise ProtocolError("frame is not a JSON object")
    return message


def request(request_id: int, method: str, args: list[Any]) -> dict[str, Any]:
    """Build a request message ``{"id", "method", "args"}``."""
    return {"id": request_id, "method": method, "args": args}


def result_response(request_id: Any, result: Any) -> dict[str, Any]:
    """Build a success response ``{"id", "result"}``."""
    return {"id": request_id, "result": result}


def error_response(request_id: Any, message: str) -> dict[str, Any]:
    """Build a failure response ``{"id", "error": {"message"}}``."""
    return {"id": request_id, "error": {"message": message}}


def event(name: str, args: list[Any]) -> dict[str, Any]:
    """Build a server→shim event message ``{"event", "args"}``."""
    return {"event": name, "args": args}


def socket_path_for(runtime_dir: str | None) -> str:
    """Return the backend socket path for *runtime_dir*.

    ``<runtime_dir>/backend.sock`` when that fits the platform's ``sun_path``
    limit; otherwise (no runtime dir, or a deep temp dir in tests) a fresh short
    directory under the system temp root, so binding never fails on length.
    """
    if runtime_dir:
        candidate = os.path.join(runtime_dir, SOCKET_FILENAME)
        if len(candidate.encode("utf-8")) <= _MAX_SOCKET_PATH:
            return candidate
    short_root = "/tmp" if os.path.isdir("/tmp") else tempfile.gettempdir()
    return os.path.join(tempfile.mkdtemp(prefix="rmsock-", dir=short_root), SOCKET_FILENAME)
