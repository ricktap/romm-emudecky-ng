# Handoff — EmuDecky RomM Manager

Written 2026-11-09 so another engineer (human or AI) can take over. Read this first, then `CLAUDE.md` (hard constraints),
then `EMUDECK_PORT_PLAN.md` (roadmap + decisions). Where this file and the code disagree, the code wins.

## What this is

A Decky Loader plugin (repo `ricktap/romm-emudecky-ng`, owner Patrick) that syncs a self-hosted RomM library onto
**EmuDeck** machines (Steam Deck + two Bazzite PCs) as Steam shortcuts: install/uninstall, launch via emulators, save
sync, firmware. **Switch only until v0.75** (Eden / Ryujinx / yuzu); other systems return by v0.90; Decky store at v1.0.0.
Fork chain: `danielcopper/decky-romm-sync` (RetroDECK) → `melikootje/Romm-emudecky` → this repo. `#NNN` refs inside old
code/docs/ADRs are **upstream** issues.

- Display name "EmuDecky RomM Manager". Decky-list name **"EmuDecky RomM NG"** — permanent (Decky's updater finds the
  plugin by it; must stay Docker-safe: letters, digits, spaces, `.-_`; a test enforces this). Slug/folder `romm-emudecky-ng`.
- Self-update works (proven on the Deck): the plugin checks GitHub releases and hands the zip to Decky's installer.

## State of the code (2026-11-09)

| Branch | What it is |
| ------ | ---------- |
| `main` | **Python backend**, shipping. Latest release **v0.33.2**. Only bugfixes + Switch features until the swap. Python 5294 tests, frontend 1721. |
| `rust` | Long-running branch for the **0.40.0 Rust rewrite**. One PR per step is merged into it. `main` is merged into it when needed. |

Rewrite steps (GitHub milestone **0.40.0**, issues #8–#14; the Python code + its tests are the specification):

| Step | Status |
| ---- | ------ |
| 1a #8 shim + socket protocol (`main.py`, `docs/architecture/backend-protocol.md`) | done, released in v0.33.0 (on `main`) |
| 1b #9 Rust foundation (protocol server, DB actor, settings + connection) | merged into `rust` |
| 2 #10 download queue, adoption, Switch launch | merged into `rust` |
| 3a #11 part 1: removals (`REMOVED_TILL_0_90_0.md`) | merged into `rust` (PR #21); CI and the release dry-run are green |
| 3a part 2: port library sync, shortcuts, collections | merged into `rust` (PR #22); CI green |
| 3b part 1: emulator/launch-file choices and relaunch reconciliation | merged into `rust` (PR #23); CI and release dry run green |
| 3b part 2: game page, metadata, cover refresh and SteamGridDB | merged into `rust` (PR #24); CI and release dry run green |
| 3b remainder: version choice, launch checks, removal, playtime, updates and Steam Input | implemented on `rust-3b-complete`; integration tracked by its PR into `rust` |
| 4 #12 firmware, 5 #13 save sync, 6 #14 delete Python | not started |

After PR #22: Rust 365 tests / 96.90% lines, Python 5275, frontend 1530 (fewer because removed features' tests went).

Step-3a part-2 implementation in [PR #22](https://github.com/ricktap/romm-emudecky-ng/pull/22):

- Ported library preferences/reads and the complete sequential sync callable/event flow: work planning,
  paginated/incremental fetch, sibling component keys/ranking/collapse, preview/apply with TTL, heartbeat,
  scoped cancellation/acknowledgement, atomic unit commits, post-commit adoption, stale handling and collections.
- Ported cover cache/download/publish/base64/progress/startup cleanup, Steam Input with backup + VDF round-trip
  verification, shortcut removal/reconcile, and atomic icon files applied via SteamClient (no shortcuts.vdf writes).
  `save_shortcut_icon` now returns an optional `icon_path` for Rust; existing Python `{success}` responses remain
  compatible. Content-addressed icon paths prevent stale saves from overwriting newer selected images.
- Review fixes include full re-fetch for groups whose Steam binding was removed out-of-band, protection of
  bound siblings covered only by a partial collection view, empty launch options for unresolved emulators,
  durable run termination before terminal events, and serialized admission/task tracking.
- Validation: Rust **365 tests / 96.90% line coverage** (`--fail-under-lines 80`), Python **5275 passed**
  (two pre-existing unawaited-download-coroutine warnings), frontend **1530 passed**, pinned pnpm build passed.
  `backend/tests/domain_parity.rs` compares the sibling kernels against 128 deterministic Python-generated
  cases and reversed fetch order. `sync_lifecycle.rs` uses controlled time with real HTTP/SQLite for TTL,
  busy apply, heartbeat renewal, timeout and late acknowledgement. `library.rs` drives the real binary/socket.
- `get_registry_platforms` is required by DangerZone's per-platform shortcut removal. Keep it.
- `get_artwork_base64` belongs to this step because syncManager calls it BEFORE acknowledging a unit.
- Reporter commits identity/metadata, publishes art/applies Steam Input and runs post-commit adoption before
  signalling the orchestrator. Its late-ack path executes the same post-commit adoption exactly once.
- PR #22 merged into `rust` on 2026-11-09 with green CI. Issue #11 covers more than this part and must not be closed prematurely.
  The shipping Python backend on `main` is unchanged; no release tag is part of step 3a.

Step-3b part-1 implementation in [PR #23](https://github.com/ricktap/romm-emudecky-ng/pull/23):

- Ported `get_platform_core_info`, `set_game_core`, `clear_game_core`, `get_disc_selection`, `select_disc`,
  `get_rom_relaunch_options`, and `get_installed_relaunch_options`. Switch only; non-Switch core/disc extras stay removed.
- Picker payloads retain the Python shapes and disabled/not-installed reasons. Per-game overrides win over platform
  preferences, then the first installed emulator. Stale pins fall back without being erased; clearing follows the default.
- All re-bakes share `LaunchService`, preserve the chosen launch file, and keep the shortcut app ID. Reads snapshot
  ROM/install pairs in SQLite, then probe outside the transaction. Conditional pin-only writes reject concurrent
  install/binding/preference changes with `state_changed` rather than acknowledging a stale update.
- Two intentional corrections to legacy Python behavior: unresolved emulators produce empty launch options (also for
  download/adoption), as required by the launcher contract; launch-file pins retain the exact enumerated basename,
  including significant whitespace.
- Tests port the Python core/disc/relaunch and Switch-launch contracts, including real binary/socket calls, restart
  persistence, shell quoting, database-write failures, and stale-snapshot rejection. PR #23 merged into `rust` with green
  CI and [release dry run](https://github.com/ricktap/romm-emudecky-ng/actions/runs/42683954989).
  No shipping version bump or release tag belongs to this part.
- Local validation: Rust **377 tests / 96.97% line coverage** (`--fail-under-lines 80`), Python **5275 passed**
  (the two pre-existing unawaited-download-coroutine warnings), frontend **1530 passed**, pinned pnpm build passed.

Step-3b part-2 implementation in [PR #24](https://github.com/ricktap/romm-emudecky-ng/pull/24):

- Ported `get_cached_game_detail`, `get_rom_metadata`, `get_all_metadata_cache`, `fetch_cover_base64`,
  `refresh_cover_artwork`, and all six SteamGridDB callables. `get_artwork_base64` now also honors pending-sync
  and persisted legacy cover paths before the existing cache/staging/grid fallback.
- Game-page reads snapshot SQLite state without waiting for HTTP; uninstalled games schedule the existing background
  adoption check. Preserve metadata TTL, save-status and version payloads. Removed BIOS/achievement badges remain null
  and never request background refreshes.
- Passive SGDB reads use cache, then stored/pending IDs only. Explicit resolution prefers RomM, then local state,
  IGDB cross-reference, and a six-result name picker with optional thumbnails. Manual picks replace four image caches
  without persisting the selected ID. Key saves preserve masked values and roll back on persistence failure.
- SGDB API authentication is isolated from RomM and from CDN downloads, including redirects. Artwork downloads use
  atomic sidecars with cleanup on failure; cover sync/fetch/refresh share a writer lock. Cover refresh rechecks the
  shortcut binding after HTTP before publishing. No `shortcuts.vdf` writes.
- Tests use real SQLite/files, fake HTTP, and real binary/socket calls. They cover offline reads, restart persistence,
  background adoption, UTC release years, key/auth failures, CDN credential separation, partial transfers,
  simultaneous cover writers, binding changes and safe startup pruning.
- Local validation: Rust **399 tests / 97.05% line coverage** (`--fail-under-lines 80`), Python **5275 passed**
  (the two pre-existing unawaited-download-coroutine warnings), frontend **1530 passed**, pinned pnpm build passed.
  PR #24 merged into `rust` with green CI and [release dry run](https://github.com/ricktap/romm-emudecky-ng/actions/runs/42689842733).
- No shipping version bump or tag; issue #11 still includes the remaining step-3b scope.

Step-3b remainder (`rust-3b-complete`, final part of #11):

- Ported all 14 remaining callables: version list/switch, launch gate, local drift/core-change checks, individual/bulk
  uninstall, Steam Input application, self-update checks, and all five playtime/session callables.
- Version switching conditionally moves only the shortcut binding, preserves per-version installs, pins, saves and
  history, rejects stale snapshots and canonical metadata conflicts, and shares the existing launch resolver.
  Download admission, switching and uninstall share a mutation lock; running downloads block destructive uninstall.
  Bulk uninstall reports partial failures and returns app IDs so the frontend clears launch options; failed installs
  stay retryable. Shared system folders and shortcut identities survive.
- Playtime uses the existing SQLite schema. Session folding and outbox enqueue are atomic, offline recording survives
  restart, and boot-relative CLOCK_MONOTONIC matches Python's persisted suspend-aware markers. Native session POSTs
  are not retried by HTTP: the outbox groups by recorded device, limits each flush to 100 rows, correlates results by
  index, isolates malformed 422 batches, bounds acknowledged errors to five attempts, and protects newer rows from
  stale acknowledgements. Reconciliation never lowers totals/counts and persists/clears the missing-scope notice.
- GitHub update checks retain trusted asset/hash validation, six-hour success/fifteen-minute failure caching, forced
  checks and background startup/shutdown. The feed uses a separate unauthenticated strict-TLS client.
- Save-safety is an injected boundary for step 5. Production Switch save discovery is explicitly unsupported until the
  ADR/provider exists: no guessed paths or save writes. Launch checks warn after a failed status read when tracked
  saves exist; standalone emulators have no libretro core change. Local drift remains false until the provider can
  enumerate files. Post-exit unsupported sync is a benign skip, and removed migration/achievement work is not revived.
- Validation includes real SQLite/files, fake HTTP and real binary/socket calls, restart persistence, rejected/out-of-order
  acknowledgements, save warnings, updater trust/caching, Steam Input backups, uninstall partial failure and paused
  download cleanup. All 81 step-1–3 callables are ported; the remaining 25 belong to firmware and save sync (steps 4–5).
- Local validation: Rust **431 tests / 97.12% line coverage** (`--fail-under-lines 80`), Python **5275 passed**
  (the two known unawaited-download-coroutine warnings), frontend **1530 passed**, pinned pnpm production build passed.
- No shipping version bump or release tag. The `rust` → `main` swap and milestone release still need Patrick's approval.

## How the rewrite works

- **Architecture:** `main.py` is a thin Decky shim. It spawns `bin/romm-emudecky-ng-backend` (Rust), talks NDJSON over a
  Unix socket, forwards every callable and relays events to `decky.emit`. Forwarders are installed at import time
  (`--list-methods`) because Decky serves frontend calls in parallel with `_main`; calls wait for readiness. The frontend
  never changes: callable names/arity/response shapes and event payloads are the contract.
- **Callable surface:** on `rust`, `backend/callables.json` is the source of truth (106 names). Unported callables answer
  `not implemented in the Rust backend yet: <name>`. `scripts/check_callable_manifest.py` / `check_event_parity.py` check
  the frontend against it.
- **Crate layout** (`backend/`): `domain/` pure · `services/` (trait ports in `ports.rs`) · `adapters/` (all I/O) · `rpc/` ·
  `app.rs` (composition root + callable table). tokio, reqwest+rustls (no OpenSSL), rusqlite behind ONE DB-actor thread,
  migrations embedded from `backend/migrations/` (shared with Python). It continues the existing DB + `settings.json`.
- **Per step:** read the Python implementation + tests → port faithfully → port the tests (unit + binary integration
  tests against a fake RomM) → keep **≥80% line coverage** (`cd backend && cargo llvm-cov --locked --summary-only
  --fail-under-lines 80`, enforced in CI) → PR into `rust` → CI + a release dry run → merge → close the issue.
- **Swap at 0.40.0** (ask Patrick first — it's a major/milestone release): PR `rust` → `main`, step 6 deletes `py_modules/`
  and the Python tests, tag `v0.40.0`. No hybrid releases (Python and Rust services would share state badly).
- **Docker is not installed locally.** The Decky CLI builds the backend in Docker (`backend/Dockerfile`, holo-toolchain-rust);
  verify it with a dry run: `gh workflow run release.yml --repo ricktap/romm-emudecky-ng --ref <branch>` (build + smoke test,
  never publishes without a `v*` tag).

### Remaining scope

- **4** firmware/keys for Switch (Phase 7 in the plan), **5** Switch save sync (Phase 6 — needs an ADR first: title IDs,
  Ryujinx index layout, folder-shaped saves), then **6**.
- **Feature phases not yet built** (build them in Rust, after step 3): Phase 3 internal/external RomM URL with the token bound
  to both origins (D6), Phase 4 Downloads view with reorder (backend `move_to_front` exists), Phase 8 health/EmuDeck
  storage-path check.

## Decisions made in conversation (not all recorded elsewhere)

- Roadmap in plan §0; Switch-only means other-system code may be removed during the rewrite, never extended.
- **Kept** after the feature review: SteamGridDB, Steam Input per shortcut, playtime sync, RomM collections→Steam, firmware.
  **Removed**: RetroDECK migration, RetroAchievements, BIOS per platform, RetroArch extras, non-Switch download extras
  (`REMOVED_TILL_0_90_0.md` has reasons + where the old code lives: tag `v0.33.2`).
- Rewrite tests are ported to Rust (no Python contract tests after the swap). Rust target ≈80% line coverage.
- Shortcut exe stays `bin/rom-launcher-ng`; standalone emulators are launched directly (not via EmuDeck's `tools/launchers`).
- Switch adoption matches by **normalized name** (size is useless: XCI files are padded; four different games were all
  7,985,954,816 bytes). Folder games launch the largest `.xci`/`.nsp`, overridable per game.

## Working agreements with Patrick

- **No linters, formatters, type-check gates, SonarCloud, release-please.** CI is two workflows: `test.yml` (tests on every
  push; Rust job enforces ≥80% coverage) and `release.yml` (on a `v*` tag; also `workflow_dispatch` dry run).
- **Semver; releases are tags on `main`:** bump `version` in `package.json` + `plugin.json` in the PR, merge, then
  `git tag vX.Y.Z && git push origin vX.Y.Z`; verify the release asset and its sha256 digest (the self-updater needs it).
  Patrick authorized me to release **minor and patch** versions of the `main` line without asking; **ask first** for
  major/milestone releases (0.40.0) and never move/delete existing tags or releases without being told to.
- Patrick tests on a Steam Deck; he runs read-only diagnostics over SSH and pastes/saves the output. Bazzite boxes are untested.
  Don't read `~/homebrew/settings/` or any credentials.
- Process that worked: research → design agreed via questions → implementation agent with a precise brief → **independent
  review** (read the risky parts, run all suites yourself) → PR → CI → merge. Reviews found real bugs agents missed
  (startup race in the shim, `$`/backtick injection in launch options, a failed job-table read deleting partials).
  Agents also tend to add lint/format gates — remove them.

## Gotchas

- **Git over SSH can hang:** use `GIT_SSH_COMMAND="ssh -o BatchMode=yes -o ConnectTimeout=15"`. A push once hung because
  `backend/target` (1.9 GB) was committed — `.gitignore` covers `backend/target` and `backend/out`; check `git status` before `git add -A`.
- Claude Code's auto-mode classifier blocked merges/tags/force-operations Patrick hadn't explicitly asked for. With an
  explicit "merge it"/"release it" they went through. Don't try to work around a block — ask.
- GitHub only auto-closes issues via "Closes #N" on merges into the **default** branch; close `rust`-branch issues by hand.
- Python tests: `.venv/bin/python -m pytest tests/ -q -p no:cacheprovider` (venv: `uv venv --python 3.11 .venv &&
  uv pip install -r requirements-dev.lock`). Frontend: `node_modules/.bin/vitest run`, `npx -y pnpm@10.29.3 build`
  (Node 24 pinned; Node 25 works too). Rust toolchain pinned in `backend/rust-toolchain.toml` (1.99.1).
- The release zip's top folder must be `romm-emudecky-ng` (the build checks it).
- `docs/` is upstream reference material and still mentions RetroDECK in many places; `CHANGELOG.md` is upstream history.
  `scripts/check_event_parity.py` has an `EXEMPT` set for legacy Python-only events; revisit at step 6.

## Open items / needs a device

- Rust backend has never run on a Deck: socket permissions (0600) vs the plugin user, `PR_SET_PDEATHSIG` under Decky's process
  model, binary on SteamOS and Bazzite, self-signed-TLS path (untested).
- Phase 1 acceptance on a real server: kill `plugin_loader` / power off mid multi-GB download, real disk-full, extraction of a
  >4 GB Switch folder (ZIP64 is unit-tested only).
- RomM content URL percent-encodes the file name twice — faithful to the Python client, which works on Patrick's server; worth a look.
- PS3 folder-boot rewrite not applied to a ROM adopted in the same sync (irrelevant until PS3 returns).

## Where memory lives

Patrick's Claude Code auto-memory (`~/.claude/projects/-Users-patrick-WebstormProjects-Romm-emudecky/memory/`) holds two notes
(release autonomy, version roadmap). Everything in them is repeated above, so nothing is lost if it isn't available.
