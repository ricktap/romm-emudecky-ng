# EmuDecky RomM Manager — Decky Loader plugin

Syncs a self-hosted RomM library onto **EmuDeck** machines (Steam Deck + two Bazzite PCs) as Steam non-Steam shortcuts:
install/uninstall games, launch via EmuDeck emulators, save sync, BIOS/firmware. Python backend (`py_modules/`) behind
a thin Decky shim (`main.py`), React/TypeScript frontend (`src/`). The backend is being rewritten in Rust for 0.40.0
(GitHub milestone "0.40.0"); the shim's socket protocol is the contract both must satisfy.

- **Released as v0.40.0 (2026-11-10):** the Python backend is gone; `backend/` (Rust) is the backend and `py_modules/`
  keeps only the shim's `rpc/` client. The Python-specific sections below (layers, constraints, save sync, tests) describe
  the **reference implementation at commit `580680864e89af50c376dc946d12cf35cee7f808`** — they are the spec for the Rust ports, and for firmware (0.45.0) and
  save sync (0.55.0), which are not ported yet (see HANDOFF.md, "Python reference implementation").
- **Start with [`HANDOFF.md`](HANDOFF.md)** — current state, the Rust-rewrite workflow and remaining steps, working
  agreements with Patrick, and gotchas.
- **Roadmap and decisions:** [`EMUDECK_PORT_PLAN.md`](EMUDECK_PORT_PLAN.md) — read it before starting work.
- `REMOVED_TILL_0_90_0.md` lists features deliberately dropped during the rewrite (Switch-only until v0.80).
- **Fork of** `danielcopper/decky-romm-sync` (RetroDECK). `#NNN` refs in code/docs/ADRs are **upstream** issues.
  RetroDECK support is dropped; leftover RetroDECK names (e.g. `RetroDeckPaths`) are naming debt, not targets.
- `docs/` is upstream reference material — useful, but not authoritative over the plan or the code.

## Commands

- Setup: `mise run setup` (or `uv venv --python 3.11 .venv && uv pip install -r requirements-dev.lock && pnpm install`)
- Rust tests: `cd backend && cargo llvm-cov --locked --summary-only --fail-under-lines 80` · Shim/script tests: `python -m pytest tests/ -q` ·
  Frontend tests: `pnpm test` · Build: `pnpm build`
- Deploy to a device: `mise run dev` (build + copy to `~/homebrew/plugins/romm-emudecky-ng` + restart `plugin_loader`)
- Python deps: edit `requirements-dev.txt`, then `mise run lock-update`.

**CI** (`.github/workflows/`): `test.yml` runs both test suites on every push to any branch. `release.yml` runs on a
`v*` tag: stamps the version from the tag, builds and smoke-tests `romm-emudecky-ng.zip`, publishes a GitHub Release with
the commit subjects since the previous tag as notes. To release: `git tag v0.27.0 && git push origin v0.27.0`. There is
no linting/formatting in CI — don't add any.

## Environment facts

- Never assume a Steam Deck: derive paths from `$HOME` and `~/.config/EmuDeck/settings.json` (`storagePath` is the
  **parent** of `Emulation/`). Never hardcode `/home/deck`. Missing/unreadable EmuDeck config = visible health error,
  never a silent fallback to `~/Emulation`.
- Decky's embedded Python is **3.11**; third-party runtime deps are vendored in `py_modules/_vendor/` (Decky has no
  package manager). Only adapters import `_vendor`.
- Plugin id/folder/User-Agent slug: `romm-emudecky-ng` (must equal the repo name — the release zip's top folder is the
  checkout dir and becomes the install folder). Display name: "EmuDecky RomM Manager", except the Decky plugin list / QAM entry (`plugin.json` `name` + `definePlugin` in
  `src/index.tsx`), which use the short "EmuDecky RomM NG". **Never change it** (and keep it Docker-safe: letters, digits, spaces, `.` `-` `_` — the Decky CLI builds the Rust backend under an image tag derived from it) — Decky finds the installed
  plugin by this name when self-updating. Side-by-side installs with
  upstream `decky-romm-sync` are supported (see plan D8).

## Key technical constraints

- **Shortcuts:** create/update via `SteamClient.Apps.*` from the frontend, never by writing `shortcuts.vdf` (instant, no
  Steam restart). `AddShortcut` ignores startDir/launchOptions — set them with `SetShortcut*` / `SetAppLaunchOptions`
  after ~500 ms (50 ms between ops); don't pass quoted exe paths. `Set*` returns void → confirm with
  `setLaunchOptionsConfirmed` (fire, then poll `AppDetails`).
- **appId = CRC(exe + name).** launchOptions/startDir changes keep the appId (artwork, collections, playtime survive);
  exe/name changes create a new shortcut.
- **Launcher model:** every shortcut's exe is `bin/rom-launcher-ng`, a pure `exec "$@"` wrapper. `launch_options` carries
  the full command (emulator invocation + `"<rom-path>"`), or `""` when uninstalled/unresolvable (`needs_setup` /
  `launcher_missing`, which the UI must surface). One shared resolver builds every `launch_options`:
  `resolve_emulator_invocation` → `domain/emudeck_launch.resolve_emudeck_invocation`. Ownership = exe ends with
  `/bin/rom-launcher-ng`; rom_id↔appId lives in `roms.shortcut_app_id`.
- **No scrolling in the Steam UI:** on the Deck the cursor only jumps between focusable elements (button, input, toggle, select). Plain text between two of them is unreachable once it is below the screen edge, so keep text short and make information rows `focusable` (a self-closing `<Field … focusable />`); never put long text, logs or result lists in non-focusable rows.
- **Frontend API:** `@decky/ui` + `@decky/api`, `callable()` (not `decky-frontend-lib` / `callPluginMethod`). Every
  callable on `Backend` (`py_modules/backend.py`) must be `async def`. Don't push bulk data through `decky.emit()` — use per-item callables.
- **Callable failures** return `{success: False, reason, message}` (`reason` from `lib.list_result.ErrorCode` or a
  short string). Never `error` / `error_code` keys.
- **RomM:** requires ≥ 4.9.0 (`_MIN_REQUIRED_VERSION` in `py_modules/backend.py`). Filter param is `platform_ids` (plural); cover URLs
  contain unencoded spaces; lists are paginated `{"items": [...], "total": N}`. Send a real User-Agent
  (`romm-emudecky-ng/<version>`, read from `package.json`) — Cloudflare/SteamGridDB 403 the Python default.
- **Token-host binding:** the API token is sent only to the origin it was minted for (`romm_api_token_origin`);
  mismatch raises `TokenHostMismatchError`. Sign-in persists URL+token+origin atomically only on success. Changing
  origin forgets the registered save-sync device id. (Plan D6 extends this to an internal + external URL pair.)
- **Save sync:** `compute_sync_action` is the only decision authority; RomM `negotiate` is transport only. Automatic
  uploads use `overwrite=false` (409 → `resolve_upload_conflict`); `overwrite=true` only from an explicit `keep_local`.
  Every read-modify-write of a `RomSaveState` holds `SyncEngine.rom_lock(rom_id)`. Never delete data that exists
  nowhere else — save-file removals go through `MatrixExecutor.quarantine_local_file` (`.romm-backup`).
- **Paths from the server** go through `safe_join` (`lib/path_safety.py`).
- **settings.json** is written only by `adapters/persistence.py` (tmp → fsync → replace → fsync dir). A corrupt file is
  backed up, never silently reset, and the user sees a persistent notice until they dismiss it.

## Architecture (Cosmic Python, kept light)

- Layers: `domain/` pure functions (no I/O, stdlib only) · `services/` orchestration · `adapters/` all I/O ·
  `lib/` cross-cutting helpers · `models/` data shapes. Services never import adapters; adapters never import services.
- Services depend on Protocols from `services/protocols/` and take one frozen `XxxServiceConfig` keyword argument.
  No direct clocks/sleep/uuid/random in services — inject `Clock` / `Sleeper` / `UuidGen`.
- `bootstrap.py` is the only place adapters are instantiated and wired. `py_modules/backend.py` (`Backend`) holds the
  callable surface and lifecycle work. `main.py` is only the Decky shim: it starts the backend, forwards every callable
  over the Unix-socket protocol (`docs/architecture/backend-protocol.md`) and relays events to `decky.emit`. Its
  forwarders exist from import time, because Decky serves calls in parallel with `_main`; calls wait for the backend
  to be ready. Never put logic in the shim.
- SQLite aggregates are mutated only through verb-named methods (`mark_installed`, not `set_installed`). Don't open a
  UoW while another is open on the same path (e.g. `relaunch_options_resolver` resolves outside the write UoW).
- No `if emudeck` branches in services — host specifics live in adapters and `domain/emudeck_launch.py`.

## Tests

- Backend changes need unit tests (happy path, failure path, edge cases); tests mirror the source tree under `tests/`.
- `tests/contract/` drives the real `Plugin` over the real `bootstrap()` with only the outer edges faked — call
  callables exactly as the frontend does and assert the response shape. Callable and event parity between frontend and
  backend is checked there (via `scripts/check_callable_manifest.py` / `check_event_parity.py`).
- Property tests (hypothesis) cover the save-sync kernels. A property that encodes an open bug is
  `xfail(strict=True)`, never weakened.
- Frontend: vitest + happy-dom; drive backend events with `emitDeckyEvent` from `src/test-utils/decky-api-mock.ts`.
  Catch-path tests must assert the post-catch effect, not just that the call happened.

## Security

- Never read credentials from `~/homebrew/settings/` without explicit permission, never pass them to agents, never log
  secrets.

## Working style

- Research unknowns before writing code; discuss non-trivial architecture choices with Patrick first.
- Keep it small: no new tooling, gates, or process unless asked. Verify behavior with tests and on the device.
