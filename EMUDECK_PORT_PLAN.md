# EmuDeck Port Plan — `ricktap/romm-emudecky-ng`

> **This is the single roadmap for this fork.** It replaces both the upstream roadmap (danielcopper's GitHub Projects
> board) and the earlier draft of this file. Source brief: Patrick's "RomM Sync Plugin EmuDeck Fix Plan" (Obsidian,
> 2026-11-08). Where the brief and the code disagreed, the code was checked and the disagreement is resolved in §3.
>
> Fork chain: `danielcopper/decky-romm-sync` (RetroDECK) → `melikootje/Romm-emudecky` (started the EmuDeck port) →
> `ricktap/romm-emudecky-ng` (this repo, default branch `main`).

---

## 0. Version roadmap (set 2026-11-09)

| Versions        | Focus                                                                                                  |
| --------------- | ------------------------------------------------------------------------------------------------------ |
| up to **0.75**  | **Nintendo Switch only**: Eden / Ryujinx / yuzu launch, adoption, downloads, Switch save sync, Switch firmware/keys |
| **0.40.0**      | Rust backend rewrite (GitHub milestone "0.40.0"), still Switch-only. It is built on the long-running `rust` branch and swapped in all at once (no hybrid releases); `main` gets only bugfixes until then. Tests are ported to Rust, with the Python tests as the spec |
| 0.75 → **0.90** | add the other systems back (RetroArch, PS2 / GC / PS1, Wii U / Vita, …)                                  |
| 0.90 → **1.0.0**| prepare for the official Decky plugin store                                                              |

What this means for the work below:

- Phases that target other systems (the non-Switch parts of Phases 2, 6 and 7) are deferred to the 0.75–0.90 range.
- The rewrite's "what's still needed" reviews (steps 3–5) may remove or skip non-Switch features until then.
- Releases follow semver. Minor and patch releases are cut as soon as they are reviewed and tested.

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
then-current `emudeck-port` HEAD (`160d806`, the branch is now `main`):

| Brief claim                                                        | Reality in code                                                                                                                                                                                                                                                                                                                                                                                      |
| ------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Download queue is memory-only; restarts lose partial downloads     | **True, and worse.** `DownloadService.cleanup_leftover_tmp_files()` **deletes** every `.tmp` on startup (`services/downloads.py:221`). HTTP Range resume and in-session pause/resume already work.                                                                                                                                                                                                   |
| ES-DE install probe only knows RetroDECK paths                     | **True.** `adapters/es_de_config.py` maps `/app/…` and `/var/{data,config}/…` to RetroDECK locations. It never expands host `~/…` globs against the real home.                                                                                                                                                                                                                                       |
| Shortcuts fall back to an empty `LaunchOptions`                    | **True, but the cause is now different.** `resolve_emudeck_invocation` returns `needs_setup` for **every** standalone emulator, because the per-system launcher map in `adapters/emudeck_launchers.py` is still commented out (deliberately: "don't commit guessed launcher names"). RetroArch resolves to `<launchers>/retroarch.sh -L <cores>/<core>.so`, which has not been verified on a device. |
| `rom-launcher-ng` relies on a `binfmt_misc` handler                   | **Not true of the current wrapper.** `bin/rom-launcher-ng` is a pure `exec "$@"` (ADR-0009). With empty launch options it exits with a usage error, which is the "does nothing" symptom. The real bug is the unresolved invocation (row above). See D2.                                                                                                                                                 |
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

- **D2 — `rom-launcher-ng` stays the shortcut Exe (decided 2026-11-08).** Steam derives the appId from `exe + name`, so
  switching emulator only changes `launch_options`. The appId stays the same and artwork, collections, playtime and the
  rom↔appId binding all survive. The wrapper did not cause the failure the brief saw; the empty launch options did (§2).
  The brief's working shape becomes `Exe=rom-launcher-ng`,
  `LaunchOptions=vblank_mode=0 %command% <home>/Applications/Eden.AppImage -f -g "<rom>"`. Steam's `VAR=val %command%`
  syntax sets the env var, and the wrapper execs the rest. Without `%command%`, Steam would pass `vblank_mode=0` to the
  wrapper as a plain argument, which breaks `exec`. Verify on-device in Phase 2.
- **D3 — RetroDECK support is dropped entirely (decided 2026-11-08).** Remove the RetroDECK probes (`es_de_config`
  sandbox mapping), the flatpak invocation leftovers, `adapters/retrodeck_paths.py` and `lib/retrodeck_health.py` once
  their EmuDeck replacements cover every caller. Do not add a RetroDECK fallback anywhere, and do not keep compatibility
  shims for it.
- **D4 — Standalone emulators are invoked directly, through `rom-launcher-ng` (decided 2026-11-08).** The chain is always
  Steam → `rom-launcher-ng` → emulator binary (e.g. `~/Applications/Eden.AppImage -f -g "<rom>"`). EmuDeck's
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
- **D8 — The plugin is named "EmuDecky RomM Manager" (decided 2026-11-08).** Display name `EmuDecky RomM Manager`
  (`plugin.json`, QAM title, toasts, docs, store). The technical slug `romm-emudecky-ng` equals the repo name
  (`ricktap/romm-emudecky-ng`) and is used for the package name, release zip, install folder
  `~/homebrew/plugins/romm-emudecky-ng`, User-Agent `romm-emudecky-ng/<version>`, the device client name registered on
  RomM, and the `localStorage` key. The README heading is "EmuDecky RomM Manager - Decky Loader Plugin". README and
  store description say the plugin is unofficial and not affiliated with EmuDeck or RomM. Upstream issue links,
  CHANGELOG history, ADRs, SQL migrations and the old `romm-sync:playtime` server note keep their old names.
  - **Side-by-side installs are supported.** The distinct folder gives this plugin its own settings and database, and
    its shortcuts launch through `bin/rom-launcher-ng` ("next generation"). Ownership is detected by the exe ending in
    `/bin/rom-launcher-ng`, which never matches upstream's `/bin/rom-launcher` and vice versa, so neither plugin sees
    or touches the other's shortcuts. A frontend test pins this.
  - **Short name in Decky (2026-11-08):** the Decky plugin list and QAM entry (`plugin.json` `name`, `definePlugin`
    in `src/index.tsx`) say "EmuDecky RomM NG", because the full name was too long there. This name is **permanent**: self-update (Phase 1.6) finds
    the installed plugin by it. Everything else keeps
    "EmuDecky RomM Manager".
    Renamed from "EmuDecky RomM+" on 2026-11-09: the Decky CLI turns the name into a Docker image tag when it builds a
    native backend, and `+` is invalid there. The name must stay Docker-safe (letters, digits, spaces, `.`, `-`,
    `_`); a test enforces this.
  - The release workflow fails the build if the zip's top folder (the install folder) is not `romm-emudecky-ng`.

---

## 4. Phases

Each phase is shippable and testable on real hardware before the next one starts. Build order follows the brief.

### Phase 0 — Baseline (½ day)

- [x] Green baseline, then the repo slimmed down to tests + release only (2026-11-08).
- [ ] Deploy the current HEAD to the Deck. Confirm the plugin loads and that the EmuDeck path health reads correctly on
      internal storage **and** on SD.

**Baseline (2026-11-08):** 4813 Python tests and 1669 frontend tests pass (frontend on Node 24 and 25), and `pnpm build`
works. Python 3.11 (`uv venv --python 3.11`), pnpm 10.29.3.

**Repo simplification (2026-11-08):** CI is now two workflows. `test.yml` runs both test suites on every push to any
branch. `release.yml` builds and publishes a release when a `v*` tag is pushed, with notes generated from the commits
since the previous tag.

Removed:

- all linters, formatters and type-check gates (ruff, basedpyright, import-linter, eslint, prettier, deno fmt,
  markdownlint, size-limit), the pre-commit hook, and the `scripts/check_*` architecture gates;
- release-please, renovate, the labeler, PR-title / issue-assign / docs-check workflows, issue templates and
  CODEOWNERS;
- SonarCloud, and the MkDocs site build and deploy (`docs/` stays as plain Markdown).

The callable and event parity checks survive as contract tests. `CLAUDE.md` is a short page of real constraints.

### Phase 1 — Durable download queue (brief §5.1) — _highest priority_

Design agreed 2026-11-08:

- **Table `download_jobs`** (migration 012), aggregate `DownloadJob`. It has one row per ROM whose download is not
  finished, with status `queued` / `downloading` / `extracting` / `paused` / `interrupted` and an `interrupt_reason`.
  It stores the paths, `total_bytes` and `position` (queue order). Completed, cancelled and failed jobs delete their
  row; the recent-downloads history stays in memory. Mutations are verb-named (`enqueue`, `start`, `begin_extract`,
  `pause`, `interrupt`, `move_to_front`, …). Firmware joins in Phase 7.
- **No progress writes.** The `.tmp` file size is the progress; rows change only on status transitions (no SSD wear).
- **Errors.**
  - Network/server-unreachable, auth, and **disk-full** errors → `interrupted`. The partial is kept, so the user can
    free space and resume.
  - Only errors that can never succeed as-is (404, size mismatch, unsafe path) → `failed`, and the partial is deleted.
- **Startup reconciliation** replaces the blanket `.tmp` deletion:
  - A `.tmp` with a job row is kept; its real size is the resume point.
  - Orphan `.tmp` files are deleted.
  - A job that died in multi-file extraction re-extracts from its complete `.zip.tmp`.
  - Jobs that were running or queued at the crash become `interrupted` and auto-resume in the background, in order.
  - **User-paused jobs stay paused** across restarts and never auto-resume.
  - If RomM is unreachable, jobs stay `interrupted` and the user resumes them.
- **One download at a time** (D5). A scheduler picks the next `queued` job by `position`, replacing the 2-slot
  semaphore. `move_to_front` exists in the backend; its UI is Phase 4.
- HTTP Range logic (`adapters/romm/http.py`) is reused unchanged. A `.tmp` already at full size skips the transfer.
- **Accept:** kill `plugin_loader` (and separately power off) mid-transfer on a multi-GB ROM. After restart the download
  continues near where it stopped, and the user only had to reopen the QAM.

**Status:** implemented on `feat/durable-download-queue`; the on-device acceptance test is open. Notes from the build:

- A stream cut short counts as a network drop (`interrupted`). Only an overshoot, 0 bytes, or a 4xx other than 408/429
  is `failed`.
- `ENOSPC` is detected anywhere in the exception cause chain.
- Plugin unload (`shutdown`) keeps job rows and partials, so the next start resumes them.
- Cancel works on paused and interrupted jobs too, deleting the row and the partial.
- Auto-resume stops at the first unreachable server or rejected sign-in, and marks the remaining jobs with that reason.
- If the job table can't be read at startup, no ROM `.tmp` is deleted.
- BIOS `.tmp` files are still wiped at startup until firmware joins the queue (Phase 7).

### Phase 1.5 — Adopt games already on disk (found while testing v0.31)

Games already on the device (copied by hand, or downloaded by the old RomM Sync plugin) showed "Download", because a
game counted as installed only when this plugin downloaded it itself.

- **Match rule (decided 2026-11-08):** exact path and size, no hashing.
  - A ROM counts as installed when its file is exactly where this plugin would download it (`roms/<system>/<RomM
    file name>`) and its size matches RomM's `fs_size_bytes`.
  - Multi-file games: every file RomM lists must be present at its expected path with the right size.
  - Renamed copies are not matched (no false positives); looser matching can come later if needed.
- **When:** during library sync for each synced platform, and on opening a game's detail page (one cheap `stat`), so
  the page is correct immediately.
- **Adoption** writes the same `RomInstall` record a finished download writes and goes through the same launch-options
  path, so uninstall, version switch and launch treat it like a downloaded game.
- Launching adopted Switch games still needs Phase 2.
- **Normalized-name fallback for Switch (decided 2026-11-08, after on-device data).**
  - Why: hand-copied games never match exactly. RomM stores Switch games as folders with sanitized names
    (`Catherine_ Full Body`), while copies use scene names (`… NSW-NiiNTENDO.xci`) or other spellings. Size cannot
    help: XCI files are padded to cart size, and four different games on the Deck are all exactly 7,985,954,816 bytes.
  - Rule: when the exact rule finds nothing, a disk entry in `roms/switch/` matches a RomM game if their names are
    **exactly equal after normalizing**: lowercase; `_` `:` and other punctuation as spaces; drop the file extension,
    bracketed/parenthesised tags (`[0100…]`, `[v0]`, `(US-EU)`) and `NSW-<group>` scene suffixes.
  - Safety: pairing is one-to-one only; an ambiguous match adopts nothing. The entry must be a `.xci`/`.nsp` file, or
    a folder containing one (the launch file is the largest, as for downloads). No size check for Switch.
  - The recorded install path uses the EmuDeck-configured roms root, not a resolved mount path.

### Phase 1.6 — Self-update from GitHub releases (proposed 2026-11-08)

**Status (2026-11-08): implemented, needs on-device check.** Backend `check_for_update(force)` callable
(`services/update.py`, `domain/self_update.py`, `adapters/github_releases.py`); QAM `UpdateBanner` + Settings
"Updates" section; Decky handoff in `src/utils/selfUpdate.ts`. Not yet verified on a Deck: that
`window.DeckyBackend.call("utilities/install_plugin", …)` opens Decky's modal and replaces the install in place.

Installing every release by URL again is tedious. Decky's own update badge only works for plugins in the selected
store, and a custom store would **replace** the official one for the user, so neither fits. The established community
pattern (decky-nexus, deckyemu, Decky-Metadata) is an in-plugin check plus Decky's own installer:

- **Check (backend):** `GET api.github.com/repos/ricktap/romm-emudecky-ng/releases/latest` through the HTTP adapter
  (with our User-Agent).
  - When: on plugin start, at most every 6 h, plus a "Check for updates" button.
  - Compares semver against `package.json` `version`.
  - Returns latest version, release notes, the **versioned** asset URL, and the asset's sha256 (from the API's `digest`).
  - Only asset URLs under `https://github.com/ricktap/romm-emudecky-ng/releases/download/` are accepted, and a hash is
    required.
  - Unauthenticated rate limit is 60/h per IP, which is plenty.
- **Install (frontend):**
  - The QAM shows "Update available: vX.Y.Z" with the notes and an **Update** button.
  - The button calls `window.DeckyBackend.call("utilities/install_plugin", url, <plugin.json name>, version, sha256, 2)`
    (2 = UPDATE).
  - Decky downloads, verifies the sha256, and shows **its own confirmation modal**, which a plugin cannot skip.
  - Decky then replaces the folder and reloads the plugin. Settings and database survive, because they live outside
    the plugin folder.
- **Constraint:** Decky matches the installed plugin by `plugin.json` `"name"`, so **"EmuDecky RomM NG" must never change
  again**; a rename breaks updating. A test pins that the name used for installing is read from `plugin.json`.
- **Risk:** `utilities/install_plugin` is internal and undocumented, but it has been stable through Decky v3.x and many
  plugins use it. If the call fails, the UI falls back to "Download vX.Y.Z" with the release link.
- **Later, optional:** submit to the official Decky store (a PR to decky-plugin-database per release, with review).
  This gives native update notifications without the internal API.

### Phase 2 — EmuDeck emulator detection + launch (Switch first), built with rewrite step 1a (#8)

**Deck facts (diagnostic, 2026-11-08, SteamOS 3.9.5):**

- `storagePath = /run/media/mmcblk0p1` (SD card).
- `~/Applications` holds `Eden.AppImage`, `yuzu.AppImage`, `pcsx2-Qt.AppImage`, `Cemu.AppImage`, `DuckStation.AppImage`,
  `Vita3K/`, and `publish/Ryujinx` + `publish/Ryujinx.sh`.
- RetroArch and Dolphin are flatpaks; RetroArch cores are in
  `~/.var/app/org.libretro.RetroArch/config/retroarch/cores`.
- EmuDeck's `tools/launchers/*.sh` forward the ROM argument, but run `git pull` and a cloud-sync upload on every
  launch. So per D4 they stay out of the launch chain.
- ES-DE's `es_systems.xml` / `es_find_rules.xml` are only inside `ES-DE.AppImage`; `~/ES-DE/custom_systems` has a Wii
  U-only file. That is why the ES-DE-XML lookup resolves everything to `needs_setup` on EmuDeck.
- Switch games are a mix of single `.xci`/`.nsp` files and **folders** (base + update/DLC).

**Decisions (2026-11-08):**

- **Our own small emulator table replaces the ES-DE XML lookup.** Each entry is system → emulators → (locate: glob
  under `~/Applications` relative to `$HOME`, or a flatpak id) → args template.
  - "Installed" means the located file exists and is executable.
  - The default emulator per system is the table's first installed entry.
  - The per-game emulator override (`roms.emulator_override`) chooses only among installed entries.
  - The ES-DE-XML-based resolution code goes once nothing uses it (rewrite review).
- **Switch first:**
  - Eden (default): `~/Applications/Eden*.AppImage -f -g "<rom>"`, verified by a working hand-made shortcut.
  - yuzu: `~/Applications/yuzu*.AppImage -f -g "<rom>"`.
  - Ryujinx: `~/Applications/publish/Ryujinx.sh "<rom>"`.
  - Every standalone launch gets the `vblank_mode=0 %command%` env prefix. Shortcut exe stays `rom-launcher-ng` (D2).
  - yuzu and Ryujinx args are confirmed by the on-device test before release.
- **Folder games:** launch the largest `.xci`/`.nsp` in the folder by default. This is overridable per game on the same
  game-config page as the emulator override, reusing the per-game launch-file selection (`roms.selected_disc`,
  ADR-0014).
- Other systems (RetroArch, PS2/GC/PS1, Wii U/Vita) come after Switch is verified.

**Rewrite step 1a (#8) in the same go:** `main.py` becomes the shim; callables travel as newline-delimited JSON over a
Unix socket in `DECKY_PLUGIN_RUNTIME_DIR`; contract tests drive the plugin through the shim. Decky runs as a frozen
executable with no separate Python interpreter, so until Rust exists the backend server behind the socket runs inside
the plugin process. Only what sits behind the socket changes later.

**Accept:** Eden, Ryujinx and yuzu each launch a Switch game (one single-file, one folder game) from Steam on the first
try. Changing a game's emulator or launch file re-bakes the shortcut. The contract tests pass through the shim.

**Status:** accepted on the Deck (v0.33.1, 2026-11-09). Eden, Ryujinx and yuzu all launch Switch games from Steam (single-file and folder), and the launch-file override works. Review fixes:

- The shim installs its forwarders at import time and gates calls on backend readiness, because Decky serves frontend
  calls in parallel with `_main`.
- Launch-option quoting now escapes `$` and backticks; Steam runs launch options through a shell, and the old quoting
  let a file name trigger command substitution.

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
