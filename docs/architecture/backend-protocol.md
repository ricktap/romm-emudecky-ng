# Backend protocol

The frontend never talks to the backend directly. Decky calls methods on the `Plugin` class in `main.py` (the **shim**),
and the shim forwards every call over a local socket to the **backend**. Today the backend is the Python code in
`py_modules/` (`backend.Backend`), running inside the plugin process. The 0.40.0 rewrite replaces it with a Rust binary
that speaks this same protocol; the contract tests (`tests/contract/`) run through this protocol so they keep passing
unchanged.

## Transport

- A Unix stream socket at `<DECKY_PLUGIN_RUNTIME_DIR>/backend.sock`. If that path is too long for `sun_path` (over 100
  bytes, which happens with deep test temp dirs), a short directory under `/tmp` is used instead
  (`rpc.protocol.socket_path_for`).
- Newline-delimited JSON: one UTF-8 JSON object per line, `\n`-terminated. Newlines inside strings are escaped by JSON,
  so a line is always one message. Strict JSON only (no `NaN`/`Infinity`), no pickling, no binary frames. Bytes never
  cross the socket; large data (artwork) travels as base64 strings, up to 64 MiB per line.

## Messages

| Direction       | Shape                                              | Meaning                                         |
| --------------- | -------------------------------------------------- | ----------------------------------------------- |
| shim → backend  | `{"id": n, "method": "<name>", "args": [...]}`     | Call a callable with positional JSON arguments. |
| backend → shim  | `{"id": n, "result": <json>}`                      | The call succeeded.                             |
| backend → shim  | `{"id": n, "error": {"message": "..."}}`           | The call failed (exception, bad arity, …).      |
| backend → shim  | `{"event": "<name>", "args": [...]}`               | An event for the frontend (`decky.emit`).       |
| shim → backend  | `{"id": n, "method": "__list_methods__"}`          | Result: the list of callable names.             |
| shim → backend  | `{"id": n, "method": "__shutdown__"}`              | Run shutdown work, answer, then close.          |

- `id` is chosen by the shim and echoed back. Requests are handled concurrently, so responses can arrive in any order.
- An unknown method gets an error response (`unknown method: <name>`). A frame that is not valid JSON gets an error
  response with `"id": null`.
- Events and responses share one ordered stream. An event emitted while a call runs is written before that call's
  response, so the shim relays it before the call returns.
- Events emitted while no shim is connected (during startup) are buffered and sent to the first shim that connects.
- Names starting with `__` are reserved for the protocol; callable names are public `snake_case`.

## Lifecycle

1. Decky calls `Plugin._main()`. The shim calls `_launch_backend(socket_path)` — the only place that knows which backend
   runs. Today it calls `backend.serve_in_process`, which runs the startup work (`Backend._main`: wire services,
   migrations, startup healing, background tasks) and then binds the socket. Later it spawns `bin/<binary>` instead.
2. The shim connects (retrying until the socket accepts), sends `__list_methods__`, and adds one `async def` forwarder per
   name to `Plugin`. Decky resolves the frontend's `callable("name")` to these forwarders.
3. Each event frame is passed to `decky.emit(name, *args)`.
4. Decky calls `Plugin._unload()`. The shim sends `__shutdown__`; the backend runs its shutdown work (`Backend._unload`),
   answers, and closes. The shim then closes its connection.

If the connection drops, pending calls fail and the next call reconnects once. If the backend fails to start, the shim
logs it and the plugin stays inert.

## Code

| Piece                          | File                         |
| ------------------------------ | ---------------------------- |
| Framing and message shapes     | `py_modules/rpc/protocol.py` |
| Backend side (server)          | `py_modules/rpc/server.py`   |
| Shim side (client)             | `py_modules/rpc/client.py`   |
| Python backend + its callables | `py_modules/backend.py`      |
| Decky shim                     | `main.py`                    |

The callable surface is the public `async def` methods of `backend.Backend`. `scripts/check_callable_manifest.py` (and
`tests/contract/test_callable_manifest.py`) compares it with the frontend's `callable<...>("name")` declarations; it
reads `py_modules/backend.py`, not `main.py`, because the shim's methods are generated.
