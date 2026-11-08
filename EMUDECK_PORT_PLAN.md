# EmuDeck Port Plan — `ricktap/romm-emudecky-ng`

> **This is the single roadmap for this fork.** It replaces both the upstream roadmap (danielcopper's GitHub Projects
> board) and the earlier draft of this file. Source brief: Patrick's "RomM Sync Plugin EmuDeck Fix Plan" (Obsidian,
> 2026-11-08). Where the brief and the code disagreed, the code was checked and the disagreement is resolved in §3.
>
> Fork chain: `danielcopper/decky-romm-sync` (RetroDECK) → `melikootje/Romm-emudecky` (started the EmuDeck port) →
> `ricktap/romm-emudecky-ng` (this repo, default branch `emudeck-port`).

---

## 1. Goal and target environment

A Decky Loader plugin that brings a self-hosted RomM library onto **EmuDeck** machines: browse the library from the QAM,
install/uninstall individual games, get a Steam shortcut that **launches on the first try**, sync saves (including
standalone emulators), and download firmware/BIOS/keys that RomM already hosts.

**Target machines — all three must behave identically:**

| Machine            | OS                              | Notes                                        |
| ------------------ | ------------------------------- | -------------------------------------------- |
| Steam Deck         | SteamOS                         | internal storage and SD card (`storagePath`) |
| Bazzite box #1, #2 | Fedora Atomic / uBlue (Bazzite) | Decky Loader + EmuDeck + Steam Big Picture   |

Nothing may assume "this is a Deck". Everything is derived from `$HOME`, `~/.config/EmuDeck/settings.json`, and EmuDeck
conventions. Never hardcode `/home/deck`.

**EmuDeck facts the plugin relies on** (verify on-device before encoding new ones):

- `~/.config/EmuDeck/settings.json` → `storagePath` is the **parent** of `Emulation/` (e.g. `/home/deck`,
  `/run/media/mmcblk0p1`). Missing or unparseable file = visible hard error, **never** a silent fallback to
  `~/Emulation`.
- ROMs: `<storagePath>/Emulation/roms/<system>/` (ES-DE system names, e.g. `switch`).
- BIOS: `<storagePath>/Emulation/bios/` (+ per-emulator subfolders).
- EmuDeck launcher scripts: `<storagePath>/Emulation/tools/launchers/<name>.sh`.
- Standalone emulators are host-native AppImages in `~/Applications/` (`Eden.AppImage`, `yuzu.AppImage`; Ryujinx is
  `~/Applications/publish/Ryujinx` + `Ryujinx.sh`). No flatpak sandbox.
- ES-DE ships `es_find_rules.xml` / `es_systems.xml` with emulator definitions and command templates. This includes Eden
  (`%EMULATOR_EDEN% -f -g %ROM%`, staticpath `~/Applications/Eden*.AppImage`).
- A launch shape that works on the Deck today (hand-made shortcut): `Exe=~/Applications/Eden.AppImage`,
  `LaunchOptions=vblank_mode=0 %command% -f -g "'<rom>'"`.

---

## 2. Where the code actually is (verified 2026-11-08)

The brief was written from a **deployed build**. The repo has moved since then: melikootje's last two commits added
`EmuDeckPathsAdapter`, `EmuDeckLauncherAdapter`, and `domain/emudeck_launch.py`. Status of each brief claim against the
current `emudeck-port` HEAD:

| Brief claim                                                        | Reality in code                                                                                                                                                                                                                                                                                                                                                                                      |
| ------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Download queue is memory-only; restarts lose partial downloads     | **True, and worse.** `DownloadService.cleanup_leftover_tmp_files()` **deletes** every `.tmp` on startup (`services/downloads.py:221`). HTTP Range resume and in-session pause/resume already work.                                                                                                                                                                                                   |
| ES-DE install probe only knows RetroDECK paths                     | **True.** `adapters/es_de_config.py` maps `/app/…` and `/var/{data,config}/…` to RetroDECK locations. It never expands host `~/…` globs against the real home.                                                                                                                                                                                                                                       |
| Shortcuts fall back to an empty `LaunchOptions`                    | **True, but the cause is now different.** `resolve_emudeck_invocation` returns `needs_setup` for **every** standalone emulator, because the per-system launcher map in `adapters/emudeck_launchers.py` is still commented out (deliberately: "don't commit guessed launcher names"). RetroArch resolves to `<launchers>/retroarch.sh -L <cores>/<core>.so`, which has not been verified on a device. |
| `rom-launcher` relies on a `binfmt_misc` handler                   | **Not true of the current wrapper.** `bin/rom-launcher` is a pure `exec "$@"` (ADR-0009). With empty launch options it exits with a usage error, which is the "does nothing" symptom. The real bug is the unresolved invocation (row above). See D2.                                                                                                                                                 |
| Shortcuts must be edited in `shortcuts.vdf` with the `vdf` package | **The plugin does not write `shortcuts.vdf`.** Shortcuts are created and updated live through `SteamClient.Apps.AddShortcut` / `SetAppLaunchOptions` from the frontend (`src/utils/steamShortcuts.ts`). No Steam restart is needed. See D1.                                                                                                                                                          |
| Per-game emulator override must be built                           | **Mostly exists.** `roms.emulator_override` column, `RomRepository.set_emulator_override`, `CoreChangeModal`, ADR-0011. Still missing: options gated on EmuDeck detection, and standalone choices that actually render.                                                                                                                                                                              |
| Download queue UI must be built                                    | **Partly exists.** `src/components/DownloadQueue.tsx` already has pause/resume/cancel. Still missing: persistence, reorder, speed/ETA.                                                                                                                                                                                                                                                               |
| Firmware API needed                                                | **Exists.** `RommApi.list_firmware()` / `download_firmware()` and `services/firmware.py` (BIOS status + download). Downloads are a plain byte copy that bypasses the queue, and nothing is filtered per platform in the API call.                                                                                                                                                                    |
| Save sync only works for RetroArch                                 | **True.** `domain/save_layout.py` models RetroArch layouts only. There is no `SaveLocationProvider`. Nothing knows Eden/yuzu/Ryujinx save locations.                                                                                                                                                                                                                                                 |
| Single `romm_url` setting                                          | **True.** Also, the token is bound to one origin (`romm_api_token_origin`, CLAUDE.md "Token-host binding"), so a second URL needs design work. See D6.                                                                                                                                                                                                                                               |

Local dev note: this Mac has no `mise` and no `.venv`, so the test baseline still has to be recorded (Phase 0).

---

## 3. Decisions

### Settled

- **D1 — Shortcuts stay on the SteamClient API. Do not write `shortcuts.vdf`.** The live API is instant and needs no
  Steam restart. It cannot corrupt other people's shortcuts the way a whole-file rewrite can, and the whole frontend is
  built around it. The brief's vdf procedure (round-trip check, backup, entry-count check) **does** stay as the
  mandatory recipe for any future direct vdf write. `adapters/steam_config.py` already edits `localconfig.vdf` and
  `screenshots.vdf` with the vendored `vdf` package, and those writes must follow it too.
- **D5 — Download concurrency: one active download, reorder = "move to front / runs next".** A parallel downloader with
  bandwidth splitting is a possible later step.
- **Architecture rule (kept from the original plan):** no `if emudeck` branches in services. Host-specific logic lives
  in adapters and `domain/emudeck_launch.py`. `bootstrap.py` is the only place where the choice is made.
- **One launch resolver:** every path that writes `launch_options` (sync, download-complete, version switch, core or
  emulator change, disc change, relaunch re-bake) goes through `resolve_emulator_invocation` →
  `resolve_emudeck_invocation`. A per-game override is just another input to that resolver.
- **No guessed paths, no guessed saves.** Unreadable EmuDeck config is a visible health failure. Save sync is offered
  only where a verified `SaveLocationProvider` exists.

- **D2 — `rom-launcher` stays the shortcut Exe (decided 2026-11-08).** Steam derives the appId from `exe + name`, so
  switching emulator only changes `launch_options`. The appId stays the same and artwork, collections, playtime and the
  rom↔appId binding all survive. The wrapper did not cause the failure the brief saw; the empty launch options did (§2).
  The brief's working shape becomes `Exe=rom-launcher`,
  `LaunchOptions=vblank_mode=0 %command% <home>/Applications/Eden.AppImage -f -g "<rom>"`. Steam's `VAR=val %command%`
  syntax sets the env var, and the wrapper execs the rest. Without `%command%`, Steam would pass `vblank_mode=0` to the
  wrapper as a plain argument, which breaks `exec`. Verify on-device in Phase 2.
- **D3 — RetroDECK support is dropped entirely (decided 2026-11-08).** Remove the RetroDECK probes (`es_de_config`
  sandbox mapping), the flatpak invocation leftovers, `adapters/retrodeck_paths.py` and `lib/retrodeck_health.py` once
  their EmuDeck replacements cover every caller. Do not add a RetroDECK fallback anywhere, and do not keep compatibility
  shims for it.
- **D4 — Standalone emulators are invoked directly, through `rom-launcher` (decided 2026-11-08).** The chain is always
  Steam → `rom-launcher` → emulator binary (e.g. `~/Applications/Eden.AppImage -f -g "<rom>"`). EmuDeck's
  `tools/launchers/*.sh` scripts are **not** in the launch chain for standalone emulators. They are only an extra "is it
  installed" signal. Keep one verified table per emulator (binary location/glob + args template, seeded from ES-DE's
  find rules and command templates) and fill it only from on-device checks. RetroArch's route (EmuDeck `retroarch.sh` vs
  the RetroArch binary directly) is checked the same way in Phase 2.
- **D6 — The token is bound to both configured origins (decided 2026-11-08).** Both URLs point at the same RomM server,
  so one token is valid on both. Store `romm_api_token_origins: [internal, external]`, stamped at sign-in from the two
  URLs the user typed. The bearer is sent only to one of those origins, so the "never leak the token to a foreign host"
  guarantee still holds. Device-id forgetting triggers only when **neither** origin matches the previous set.
- **D7 — Firmware lands in the plugin's own store, then is hard-linked into EmuDeck (decided 2026-11-08).**
  - Downloads go to a plugin-owned firmware store under the plugin data dir (`DECKY_PLUGIN_RUNTIME_DIR/firmware/…`).
    That store is the canonical copy and the source for re-linking.
  - The file is then hard-linked into `<Emulation>/bios/<emulator-subdir>/` (exact subfolders for Eden/yuzu/Ryujinx keys
    and firmware are checked on-device).
  - Hard links only work within one filesystem. When EmuDeck lives on the SD card or another drive, `link()` fails with
    `EXDEV`. **To verify in Phase 7:** is a copy fallback acceptable (doubles disk use), or should the store live on the
    same filesystem as `<Emulation>` (e.g. `<storagePath>/…/.romm-sync/firmware`)? Never fall back to a symlink without
    checking that each emulator follows it.
  - Never auto-install into the emulator. Tell the user it still has to be installed from the emulator's own UI.
- **D8 — The plugin is renamed "EmuDecky RomM Manager" (decided 2026-11-08).** Display name `EmuDecky RomM Manager`
  (`plugin.json`, QAM title, toasts). Technical slug `romm-emudecky`, used for the package name, release zip, install
  folder `~/homebrew/plugins/romm-emudecky`, User-Agent `romm-emudecky/<version>`, the device client name registered on
  RomM, and the `localStorage` key. Upstream issue links, CHANGELOG history, ADRs, SQL migrations and the old
  `romm-sync:playtime` server note keep their old names. The slug is a play on Decky Loader + EmuDeck. The GitHub repo
  is `ricktap/romm-emudecky-ng` (renamed from `Romm-emudecky`; docs site `ricktap.github.io/romm-emudecky-ng`), and its
  README heading is "EmuDecky RomM Manager - Decky Loader Plugin". Everywhere else, including the QAM, toasts, docs and
  store, the name is just "EmuDecky RomM Manager". README and store description say the plugin is unofficial and not
  affiliated with EmuDeck or RomM.
  - **On each machine, uninstall the old "RomM Sync" plugin before installing EmuDecky RomM Manager.** The new folder
    means fresh settings and database (sign in again). Shortcut ownership is detected only by the `…/bin/rom-launcher`
    suffix, so two installed copies would each claim, and could remove, the other's shortcuts.

---

## 4. Phases

Each phase is shippable and testable on real hardware before the next one starts. Every phase updates the docs it
touches in the same PR (CLAUDE.md docs rule). Build order follows the brief.

### Phase 0 — Baseline (½ day)

- [x] Local baseline recorded and made green (`chore/green-baseline`, 2026-11-08).
- [ ] Deploy the current HEAD to the Deck. Confirm the plugin loads and that the EmuDeck path health reads correctly on
      internal storage **and** on SD.

**Green baseline (2026-11-08, macOS / Apple Silicon).** Every CI gate passes locally:

- pytest: 4922 passed, coverage 95% (branch).
- vitest: 1669 passed, coverage 92.8% statements / 88.5% branches.
- ruff check + format, basedpyright (0 errors), lint-imports (8 contracts), and every `scripts/check_*` gate.
- pnpm lint (1 `exhaustive-deps` warning), prettier, typecheck, build, size (639.9 of 650 kB — tight; watch it in Phases
  4/7).
- `deno fmt --check`.

Toolchain: Python 3.11.14 (`uv venv --python 3.11`, deps from `requirements-dev.lock`), pnpm 10.29.3, deno 2.9.8. The
frontend suite passes on both Node 24 (the pin) and Node 25. Node ≥ 25 ships a built-in global `localStorage` that
shadowed happy-dom's and broke 23 `sessionManager` tests. `vitest.config.ts` now passes `--no-experimental-webstorage`
to the test workers, so no local flag is needed.

What it took to get green (inherited issues, not regressions):

- ruff import-sort / unused-import / line-length and `ruff format` on melikootje's EmuDeck commits.
- `deno fmt` on 7 docs from the rebrand commit.
- `check_settings_owner.py`: added a documented `FOREIGN_SETTINGS_READERS` allowlist. `adapters/emudeck_paths.py` reads
  EmuDeck's own `settings.json` (same filename, different file).
- `check_cosmic_call_bans.sh` / `check_no_bare_ignores.sh`: grep only `*.py`. Before, a local `__pycache__` `.pyc` file
  made them fail.
- Two tests that only passed on Linux or with a specific checkout name. `test_persistence` now detects a directory fd
  with `os.fstat` instead of `/proc`, and the rom-launcher exe test compares against `DECKY_PLUGIN_DIR` instead of
  requiring the folder to be named `decky-romm-sync`.

### Phase 1 — Durable download queue (brief §5.1) — _highest priority_

- New SQLite aggregate `DownloadJob`, added via a `sqlite_migrations` step, a repository, and `UnitOfWork` per
  `docs/architecture/database-design.md`. Columns: `rom_id` / `firmware_id` (kind discriminator), `target_path`,
  `tmp_path`, `total_size`, `bytes_downloaded`, `status` (`queued` / `downloading` / `paused` / `failed` / `completed`),
  `range_supported`, `sibling_group_key`, `position` (queue order), `created_at`, `updated_at`. Verb-named mutations
  (`enqueue`, `record_progress`, `pause`, `resume`, `fail`, `complete`, `move_to_front`).
- Write progress at most every ~N MB or ~5 s. Do not write per chunk, to spare the SSD.
- **Startup reconciliation replaces `cleanup_leftover_tmp_files` blanket deletion:**
  - a `.tmp` that has a live job row → stat the real size, correct `bytes_downloaded`, and mark it resumable;
  - an orphan `.tmp` with no job row → delete it (today's behaviour).
- After reconciling, auto-resume in the background with a visible "Resuming…" state.
- Existing HTTP Range logic (`adapters/romm/http.py`, `_range_supported`) is reused unchanged.
- **Accept:** kill `plugin_loader` (and separately power off) mid-transfer on a multi-GB ROM. After restart the download
  continues near where it stopped, and the user only had to reopen the QAM.

### Phase 2 — EmuDeck install detection + launch resolution (brief §5.2)

- `es_de_config` probe: add host-native resolution. Expand `~` against the real `$HOME` and match `staticpath` globs.
  Use `<Emulation>/tools/launchers/<name>.sh` as a second "installed" signal. Remove the RetroDECK branches (D3).
- Fill in the verified emulator invocation table (D4: direct binary through `rom-launcher`). Start with **Eden, yuzu,
  Ryujinx**, then one RetroArch system (SNES/GBA), PS2 (PCSX2), and GC/Wii (Dolphin) from the original plan's validation
  set. Only values confirmed on a real EmuDeck install go in the table.
- Support an env prefix (`vblank_mode=0`) in the rendered invocation (D2 detail).
- Unresolvable cases surface as `needs_setup` / `launcher_missing` in the game-detail UI. They are never silently empty.
- **Profile the sluggishness here** (brief §5.5.4): time the QAM library render and game-detail open; check for
  per-render probes and slow SQLite paths. Fix what is found before Phase 4.
- **Accept:** a never-before-synced game gets a shortcut that launches on the first try under Eden, yuzu and Ryujinx,
  plus the RetroArch/PS2/GC set. Paths with spaces and special characters work. Version switch and reinstall re-bake
  correctly. Repeat on one Bazzite box.

### Phase 3 — Internal/external RomM URL fallback (brief §2)

- Settings `romm_url_internal`, `romm_url_external` (either may be empty). Settings migration from `romm_url` →
  `romm_url_internal`.
- Connection policy lives in the HTTP adapter layer. Try internal with a short **connect** timeout (a few seconds, not a
  short read timeout, so slow responses are not misread as failures). Fall back to external only on a genuine connect
  failure. Cache the winner for a short TTL and re-probe when the cached choice fails.
- Token binding per D6. Update CLAUDE.md "Token-host binding" and `docs/architecture/backend-architecture.md` in the
  same PR.
- **Accept:** on LAN the internal URL is used; with internal unreachable the external URL takes over automatically; the
  token is never sent to an unlisted origin.

### Phase 4 — Install/uninstall UX + Downloads view (brief §5.5, §5.7)

- **Install** (per game): enqueue via Phase 1 → land in the EmuDeck roms folder → bake the shortcut (Phase 2) → state
  "Installed — launchable from your Steam Library".
- **Uninstall** (per game): remove the files and clear or retire the shortcut in one action. No orphan in either
  direction. Audit `rom_removal` + `shortcut_removal` for this.
- **Sync** stays a separate action that refreshes listing, metadata and artwork only. Audit that it never implicitly
  downloads or deletes.
- **Downloads view:** extend `DownloadQueue.tsx` with speed, ETA, move-to-front/reorder, and state read from the
  persisted queue, so it is correct right after a QAM or plugin reopen.
- **Accept:** install → play → uninstall round trip leaves no file and no stray shortcut. The queue reorders and
  survives restart.

### Phase 5 — Per-game emulator override on EmuDeck (brief §5.6)

- Reuse ADR-0011's `emulator_override` + `CoreChangeModal`. Choices come **only** from emulators that Phase 2 detection
  reports as installed.
- Changing the override triggers the existing re-bake via `relaunch_options_resolver` (observe its UoW-ordering rule,
  #1154). With D2 = keep wrapper, the appId stays the same.
- Per-platform default choice (original plan Milestone 6) comes along for free if the existing per-platform core
  preference is wired to the same detection.
- **Accept:** Zelda: Echoes of Wisdom overridden to Ryujinx launches in Ryujinx; other Switch games stay on Eden; an
  uninstalled emulator can't be picked.

### Phase 6 — Save sync for standalone emulators (brief §5.4) — _needs an ADR first_

This is the most design-heavy phase. Write an ADR before coding.

- **`SaveLocationProvider` Protocol** (from the original plan):
  `locations_for(system, emulator_id, rom) ->
  list[SaveLocation]`. RetroArch becomes the first implementation
  (wrapping today's `save_layout`). Eden, yuzu and Ryujinx follow. The provider is selected by the **resolved emulator**
  (including the per-game override), never by system alone (stop condition).
- **Open design questions the ADR must answer:**
  - _Title ID source._ Switch saves are keyed by title ID (yuzu/Eden:
    `nand/user/save/0000000000000000/<profile>/<TITLEID>/`). Options: RomM metadata, parse it from the XCI/NSP header,
    or a user-confirmed mapping.
  - _Ryujinx layout._ `~/.config/Ryujinx/bis/user/save/<index>/` is keyed by an index in `saveMeta` / `imkvdb`, not by
    title ID. That needs a reader for the index.
  - _Directory saves._ Switch saves are directories, while the sync engine (`services/saves/`) is file-oriented. Do we
    archive the folder into one blob or sync each file? This interacts with RomM's slot/version model and with
    ADR-0017's `compute_sync_action` authority.
  - _Profiles._ Which emulator user profile is synced.
- Also bring RetroArch-on-EmuDeck save sync to parity first (original plan Milestone 7, step 1): `retroarch_config` /
  `retroarch_core_info` must read EmuDeck's RetroArch config.
- Until a provider exists for the resolved emulator, show save sync as unsupported for that game. Never guess.
- **Accept (per emulator):** in-game save → exit → the correct save (checked by timestamp and content) is on RomM;
  restore on a second machine (Bazzite) works; conflict path works.

### Phase 7 — Firmware/BIOS browser (brief §5.3)

- Per-platform list from `/api/firmware` (filter by `platform_id`) showing file name and human-readable size. Several
  firmware versions per platform are fine.
- Downloads go through the **Phase 1 queue** (job kind = firmware), not the current direct byte copy.
- Landing per D7: plugin firmware store → hard link into the EmuDeck bios subfolder, with the cross-filesystem case
  settled on-device. Show explicit copy for Switch-style emulators: "Saved to … — install it from Eden's or Ryujinx's
  own Install Firmware menu."
- Original plan Milestone 5 still applies to classic BIOS: verify PS1, PS2, Dreamcast and one special-subfolder case.

### Phase 8 — Health, migration UX, wording, polish

- EmuDeck health banner (absent / unreadable / invalid / root missing). The backend health states already exist in
  `lib/emudeck_health.py`.
- Replace the RetroDECK-home migration UX with "EmuDeck storage path changed": detect it, warn, verify installed paths,
  and offer rescan/rebind. Do not move files automatically yet.
- Remove all remaining RetroDECK wording from the UI. Rename `RetroDeckPaths` → `EmulationPaths` and friends once
  behaviour is covered by tests (original plan §12).
- Put an honest support matrix in the README covering systems, emulators and save sync per emulator.

---

## 5. Kept from the original plan

- **Platform mapping:** RomM slug → local system folder, normalized once (ADR-0010). Acceptance rule: every platform
  enabled in the UI resolves to the exact EmuDeck roms directory. The compatibility table to test (`dc→dreamcast`,
  `psx`, `ps2`, `ps3`, `ngp` / `neo-geo-pocket→ngp`, `gc`, `wii`, `n3ds`, `nds`, `psp`, `saturn`, `snes`, `gba`,
  **`switch`**) still needs a test file. Unknown slugs keep the verbatim fallback only inside the ROM root, with the
  traversal guard (`safe_join`).
- **BIOS vocabulary is separate from ROM-system normalization.**
- **Contract fakes** for `EmulationPaths`, the launch resolver, and `SaveLocationProvider`. Services run against fakes.
- **Stop conditions:** pause and redesign if any of these happen. Launcher names differ across EmuDeck versions;
  regenerated launcher scripts invalidate stored paths; a service needs to know the host; more than one module builds
  emulator commands; save paths are inferred from system alone; external-storage failure leads to writes on internal
  storage; re-bake logic diverges between install, reinstall, core change and version switch.

---

## 6. Test matrix (on hardware)

| Case                     | Deck internal | Deck SD | Bazzite #1 | Bazzite #2 |
| ------------------------ | :-----------: | :-----: | :--------: | :--------: |
| EmuDeck detected         |       ✓       |    ✓    |     ✓      |     ✓      |
| Download + resume        |       ✓       |    ✓    |     ✓      |            |
| Eden / yuzu / Ryujinx    |       ✓       |    ✓    |     ✓      |            |
| RetroArch system         |       ✓       |         |     ✓      |            |
| Uninstall leaves nothing |       ✓       |    ✓    |            |            |
| URL fallback             |       ✓       |         |     ✓      |            |
| Switch save round-trip   |       ✓       |         |     ✓      |            |
| SD ejected → no writes   |      n/a      |    ✓    |    n/a     |    n/a     |

---

## 7. Non-goals

- Auto-installing firmware or keys into Ryujinx/Eden/yuzu. There is no safe CLI/API for it; the plugin stops at "the
  file is on disk".
- Any content source other than the user's own RomM instance.
- Reimplementing EmuDeck, or keeping a second full copy of its emulator database.
- Windows/macOS.
- Writing `shortcuts.vdf` directly (D1). If that ever becomes necessary, the brief's vdf safety recipe is mandatory.

---

## 8. Docs cleanup tracker

The docs are cleaned up phase by phase, with each doc rewritten in the PR that changes the behaviour it describes.
Current RetroDECK mention counts (`grep -ic`):

| Doc                                                       | Count | Rewrite in                  |
| --------------------------------------------------------- | ----: | --------------------------- |
| `docs/architecture/config-source-parsers.md`              |    38 | Phase 2                     |
| `docs/architecture/save-file-sync-architecture.md`        |    26 | Phase 6                     |
| `docs/architecture/core-emulator-selection.md`            |     5 | Phase 2 / 5                 |
| `docs/architecture/save-sync-coverage.md`                 |     2 | Phase 6                     |
| `docs/architecture/database-design.md`                    |     2 | Phase 1                     |
| `docs/user-guide/retrodeck-path-migration.md`             |     — | Phase 8 (replace)           |
| `docs/user-guide/{save-sync,bios-management}.md`, `index` |   1–2 | Phases 6 / 7                |
| `docs/adr/*`                                              |  many | **Do not edit** (see below) |

ADRs are historical records of upstream decisions. Leave them as they are. When this fork changes a decision, add a new
ADR that supersedes the old one and link back to it (e.g. "ADR-0023: EmuDeck host-native install probe, supersedes the
RetroDECK parts of ADR-0020").
