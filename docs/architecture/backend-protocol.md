# Backend protocol

The frontend never talks to the backend directly. Decky calls methods on the `Plugin` class in `main.py` (the **shim**),
and the shim forwards every call over a local socket to the **backend**. On the `rust` branch (the 0.40.0 rewrite) the
backend is a native binary, `bin/romm-emudecky-ng-backend`, built from `backend/` and spawned by the shim as a child
process. The Python backend in `py_modules/` (`backend.Backend`) speaks the same protocol and stays in the tree as the
specification of every callable not yet ported; the Python contract tests (`tests/contract/`) still drive it through
this protocol, and the Rust integration tests (`backend/tests/`) drive the binary the same way.

## Transport

- A Unix stream socket at `<DECKY_PLUGIN_RUNTIME_DIR>/backend.sock`. If that path is too long for `sun_path` (over 100
  bytes, which happens with deep test temp dirs), a short directory under `/tmp` is used instead
  (`rpc.protocol.socket_path_for`).
- Newline-delimited JSON: one UTF-8 JSON object per line, `\n`-terminated. Newlines inside strings are escaped by JSON,
  so a line is always one message. Strict JSON only (no `NaN`/`Infinity`), no pickling, no binary frames. Bytes never
  cross the socket; large data (artwork) travels as base64 strings, up to 64 MiB per line.

## Messages

| Direction      | Shape                                          | Meaning                                         |
| -------------- | ---------------------------------------------- | ----------------------------------------------- |
| shim → backend | `{"id": n, "method": "<name>", "args": [...]}` | Call a callable with positional JSON arguments. |
| backend → shim | `{"id": n, "result": <json>}`                  | The call succeeded.                             |
| backend → shim | `{"id": n, "error": {"message": "..."}}`       | The call failed (exception, bad arity, …).      |
| backend → shim | `{"event": "<name>", "args": [...]}`           | An event for the frontend (`decky.emit`).       |
| shim → backend | `{"id": n, "method": "__list_methods__"}`      | Result: the list of callable names.             |
| shim → backend | `{"id": n, "method": "__shutdown__"}`          | Run shutdown work, answer, then close.          |

- `id` is chosen by the shim and echoed back. Requests are handled concurrently, so responses can arrive in any order.
- An unknown method gets an error response (`unknown method: <name>`). A frame that is not valid JSON gets an error
  response with `"id": null`.
- Events and responses share one ordered stream. An event emitted while a call runs is written before that call's
  response, so the shim relays it before the call returns.
- Events emitted while no shim is connected (during startup) are buffered and sent to the first shim that connects.
- Names starting with `__` are reserved for the protocol; callable names are public `snake_case`.

## Lifecycle

1. At import, the shim runs `bin/romm-emudecky-ng-backend --list-methods` (prints the callable names as a JSON array)
   and adds one `async def` forwarder per name to `Plugin`. Decky serves frontend calls in parallel with `_main`, so the
   forwarders must exist before the backend runs; a call that arrives during startup waits (up to 60 s) for it. A
   missing or broken binary yields an empty list (logged) and the plugin stays inert.
2. Decky calls `Plugin._main()`. The shim spawns `bin/romm-emudecky-ng-backend --socket <path>` with the Decky
   directories in its environment (`DECKY_PLUGIN_SETTINGS_DIR`, `…_RUNTIME_DIR`, `…_LOG_DIR`, `DECKY_PLUGIN_DIR`,
   `DECKY_USER_HOME`). The backend runs its startup work (SQLite migrations, settings load + upgrade, legacy credential
   migration, download-queue reconciliation, which keeps job partials, deletes orphan `.tmp` files, and auto-resumes
   interrupted downloads in the background) and then binds the socket (mode `0600`). Events emitted before the shim
   connects are buffered and delivered on connect.
3. The shim connects (retrying until the socket accepts; it gives up at once if the process exits first), sends
   `__list_methods__`, and adds forwarders for any name the static list missed.
4. Each event frame is passed to `decky.emit(name, *args)`.
5. Decky calls `Plugin._unload()`. The shim sends `__shutdown__`; the backend runs its shutdown work (running downloads
   stop and keep their `download_jobs` row and partial file, so the next start resumes them), answers, closes, and
   exits. The shim closes its connection and reaps the process (killing it after 10 s).

If the connection drops, pending calls fail and the next call reconnects once. If the backend fails to start, the shim
logs it and the plugin stays inert. The backend also exits cleanly on `SIGTERM`, and on Linux asks the kernel to send it
`SIGTERM` when the plugin process dies (`PR_SET_PDEATHSIG`), so it never outlives Decky. It logs to
`<DECKY_PLUGIN_LOG_DIR>/backend.log` (stderr when that variable is unset).

Callables the Rust backend has not ported yet are still listed (the frontend's callable surface never changes) but
answer with the failure result `{"success": false, "reason": "not_implemented", "message": "not implemented in the Rust backend yet: <name>"}` (a result, not an error frame: Decky turns an error frame into an opaque "Python Exception"). `PORTED_CALLABLES` in `backend/src/app.rs`
lists the ported ones (settings + connection from step 1b; the download queue from step 2: `start_download`,
`cancel_download`, `pause_download`, `resume_download`, `get_download_queue`, `get_installed_rom`; and the first
slice of step 3a: platform/collection reads and preferences, registry platforms, sync statistics, appId mappings,
and clearing completed sync history). Startup also marks orphaned running sync records as interrupted.
Sync execution, preview/apply, status/heartbeat and unit acknowledgement callables are wired to the sequential
Rust pipeline. Cover caching/publication, Steam Input and removal/reconciliation are connected.
Icon files are saved atomically with an optional `icon_path` response, then applied via SteamClient.Apps.SetShortcutIcon.
The frontend cover reader `get_artwork_base64` reads staged cache bytes before unit acknowledgement. The listed surface is `backend/callables.json`;
`tests/test_rust_backend_callables.py` keeps it identical to the Python `Backend`.

## Code

| Piece                                  | File                                                     |
| -------------------------------------- | -------------------------------------------------------- |
| Decky shim (spawn, forwarders, relay)  | `main.py`                                                |
| Shim side (client)                     | `py_modules/rpc/client.py`, `py_modules/rpc/protocol.py` |
| Rust backend: protocol server          | `backend/src/rpc/`                                       |
| Rust backend: composition + callables  | `backend/src/app.rs`, `backend/callables.json`           |
| Rust backend: entry point              | `backend/src/main.rs`                                    |
| Rust backend: downloads + adoption     | `backend/src/services/{downloads,adoption,launch}.rs`    |
| Python backend (spec, in-process only) | `py_modules/backend.py`, `py_modules/rpc/server.py`      |

`scripts/check_callable_manifest.py` (and `tests/contract/test_callable_manifest.py`) compares the frontend's
`callable<...>("name")` declarations with the public `async def` methods of `backend.Backend`; it reads
`py_modules/backend.py`, not `main.py`, because the shim's methods are generated.

## Building the native backend

`backend/` follows the Decky plugin template for native backends. `decky plugin build` sees `backend/Dockerfile`, builds
that image (`ghcr.io/steamdeckhomebrew/holo-toolchain-rust`, i.e. SteamOS with rustup and gcc), mounts `backend/` at
`/backend` and the zip's `bin/` staging directory at `/backend/out`, and runs `backend/entrypoint.sh`, which runs
`make`. The `Makefile` builds `cargo build --release --locked` and copies the binary to `out/`, so it ships as
`bin/romm-emudecky-ng-backend` next to the existing `bin/` files. The compiler version is pinned in
`backend/rust-toolchain.toml`. TLS uses rustls with the bundled Mozilla roots and SQLite is compiled in, so the binary
needs no OpenSSL or system SQLite (SteamOS and Bazzite).

Locally: `cd backend && cargo test` (unit + integration tests; the integration tests spawn the real binary on a temp
socket against a fake RomM server), `cargo llvm-cov --summary-only` for coverage.
