"""Frontend-shim ↔ backend wire: newline-delimited JSON over a Unix socket.

Everything that defines, speaks, or hosts the language-neutral backend protocol
lives here — the framing and message shapes (``protocol``), the shim-side client
(``client``), and the backend-side server (``server``). Callable *behavior*
does not belong here; it lives behind the server (today the Python
:class:`backend.Backend`). The wire contract itself is documented in
``docs/architecture/backend-protocol.md``.
"""
