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
| 3a part 2: port library sync, shortcuts, collections | **next up — not started** (scope below). Work on the `rust` branch: create `rust-3a-sync-port` from it, one PR back into `rust` |
| 3b: remaining callables (game page, SGDB, playtime, …) | not started |
| 4 #12 firmware, 5 #13 save sync, 6 #14 delete Python | not started |

`rust` branch numbers: Rust 286 tests / 97% lines, Python 5275, frontend 1528 (fewer because removed features' tests went).

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

- **3a part 2** — port library sync (`py_modules/services/library/*`, `domain/` sync/preview/sibling/shortcut-data pieces,
  `shortcut_removal`, startup healing's sync-run parts). Callables: `get_platforms`, `save_platform_sync`,
  `set_all_platforms_sync`, `get_collections`, `save_collection_sync`, `set_all_collections_sync`, `start_sync`,
  `sync_preview`, `sync_apply_delta`, `sync_cancel_preview`, `cancel_sync`, `get_sync_status`, `get_sync_stats`,
  `sync_heartbeat`, `report_unit_results`, `report_removal_results`, `reconcile_shortcuts`, `get_app_id_rom_id_map`,
  `clear_sync_cache`, `remove_all_shortcuts`, `remove_platform_shortcuts`, `save_shortcut_icon`, + sync events
  (`sync_apply_unit`, progress/stage; payloads unchanged — the frontend `syncManager` drives them). Hook adoption in:
  `plan_adoptions` before shortcuts are built, `commit_adoptions` after the unit commits (mirror the Python orchestrator), so
  adopted games get Eden launch options in the same sync. Check whether `get_registry_platforms` is BIOS-only (then remove).
- **3b** — game page (`get_cached_game_detail`, metadata, covers), SteamGridDB (6 callables), emulator/launch-file/version
  choice (`get_platform_core_info`, `set_game_core`, `clear_game_core`, `check_core_change`, `get_disc_selection`,
  `select_disc`, `get_version_list`, `switch_version`), launch checks (`evaluate_launch`, `get_*_relaunch_options`,
  `check_local_drift`), `remove_rom`, `uninstall_all_roms`, `check_for_update`, playtime (`record_session_start`,
  `finalize_game_session`, `get_all_playtime`, `get_playtime_scope_notice`, `reconcile_playtime`), `apply_steam_input_setting`.
  Run the same feature review as 3a before porting; anything dropped goes into `REMOVED_TILL_0_90_0.md`.
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
