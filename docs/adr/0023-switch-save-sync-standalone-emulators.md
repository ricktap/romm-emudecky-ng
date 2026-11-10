# Switch save sync for standalone emulators (Eden, yuzu, Ryujinx)

## Status

**Proposed** (2026-11-10). Needs Patrick's approval and the device facts in §7 before any code. Issue #33; blocks #34
(provider port + Eden/yuzu), #35 (Ryujinx), #36 (hardware acceptance). Builds on
[ADR-0016](0016-save-sync-hands-detection-to-romm-negotiate.md) and
[ADR-0017](0017-client-baseline-detection-authoritative-negotiate-is-transport.md); it changes neither: client-side
`compute_sync_action` stays the only decision authority, `negotiate` stays transport, automatic uploads use
`overwrite=false`, `overwrite=true` only from an explicit `keep_local`, and nothing that exists nowhere else is ever
deleted (`.romm-backup` quarantine).

## Context

The sync engine of the Python reference (commit `580680864e89af50c376dc946d12cf35cee7f808`, `services/saves/`,
`domain/sync_*.py`, `domain/save_*.py`) models **RetroArch**: one or a few files next to the ROM or in a fixed save
directory, found by file name. Switch emulators differ in four ways, which are the four questions of #33:

1. **Saves are keyed by title ID**, not by ROM file name.
2. **Ryujinx** keys its save directories by an index, not by title ID.
3. **A save is a directory tree**, not a file.
4. **Saves live under a user profile**, and the profile ID differs between installations.

The Rust backend today answers "unsupported" for every Switch game (`adapters/switch_save_safety.rs`): no discovery, no
write, no guessed path. The save callables answer `not_implemented` and the UI says so (0.55.0).

RomM stores a save as **one file** per `(rom, slot)` version (datetime-tagged, with an MD5 content hash). A zip is fine
for RomM: it stores the blob and hashes it (ADR-0016: zip saves carry an undocumented per-entry scheme server-side, our
baseline is the hash of the bytes we upload).

## Decision

### 1. Title ID (the source of truth for locating a save)

A game's title ID comes from, in this order, and is **never guessed**:

1. **A `[0100XXXXXXXXXXXX]` tag in the file name** (the convention of NSP/XCI dumps; our own test data uses it, e.g.
   `Game [010018E011D92000][v0].nsp`). Exactly one 16-hex tag with the `01` prefix is accepted.
2. **RomM metadata**, if the server ever exposes it (today it does not reliably; not relied on).
3. **A user-confirmed mapping**: the game page offers *Choose save folder* with the title folders that exist in the
   emulator's save directory; the choice is stored per ROM (new table `rom_title_ids`, migration 015) and survives
   re-syncs.

Without a title ID the game is **unsupported** and the SAVES chip says so ("Choose save folder"). Update and DLC IDs are
normalised to the **base title ID** (an update is `base | 0x800`; DLC is `(base & ~0xFFF) + 0x1000 + n` and owns no
saves). Parsing the NSP/XCI header is **out**: it needs the user's `prod.keys` to read the CNMT and would couple save
sync to key handling.

### 2. Layouts, one provider per resolved emulator

`SaveLocationProvider::locations_for(emulator, title_id) -> Vec<SaveLocation>` is selected by the **resolved emulator**
(including the per-game override), never by system. A location is a directory plus a way to enumerate and restore it.

| Emulator | Save directory | Notes |
| -------- | -------------- | ----- |
| Eden | `<data>/nand/user/save/0000000000000000/<profile>/<TITLEID>/` | `<data>` from EmuDeck's config; to be verified (§7) |
| yuzu | the same layout under yuzu's data dir | same provider, different root |
| Ryujinx | `<config>/bis/user/save/<index>/`, found through the index in `bis/user/saveMeta` | its own reader (#35): the title-ID→index mapping is read from Ryujinx's own metadata; unreadable index = **unsupported**, never a guess |

Roots are derived from `$HOME` and EmuDeck's `settings.json` (CLAUDE.md), including EmuDeck's symlinked
`Emulation/saves/<emulator>` layout; the provider resolves symlinks and works on the real directory.

### 3. Directory saves: one deterministic archive per `(rom, slot)`

The whole title directory is synced as **one zip**, not per file. Per-file sync would put dozens of RomM rows per game
and no consistent version across them; RomM's slot/version model assumes one object.

- The archive is **deterministic**: entries sorted by path, fixed timestamps (1980-01-01), no extra fields, fixed
  compression level. The same content always yields the same bytes and therefore the same hash, so
  `compute_sync_action` sees "unchanged" instead of false conflicts.
- Entry paths are **relative to the profile folder** (`<TITLEID>/...`), so an archive made on one machine restores
  into another machine's profile.
- File name on RomM: `<TITLEID>.zip` within the user's slot (the slot, wizard and switch gates are unchanged).
- The baseline hash is the MD5 of the archive bytes as uploaded (exact parity with what RomM stores).
- Restore (download) extracts into a temp directory next to the target, verifies, quarantines the current directory as
  `<TITLEID>.romm-backup`, then renames into place. A failed extraction never touches the live save.
- **Safety gates:** no sync while the emulator is running (existing running-app check); an empty or shrunk-to-nothing local
  save never overwrites a non-empty server save (the #1062 guard, applied to the archive's file count and size).

### 4. Profiles

The profile folder is **not part of the identity** of a save. Local rules:

- Exactly one profile folder holds the title: use it.
- Several do: the user picks once (setting `switch_save_profile`, per emulator), shown on the game page; until then the
  game is unsupported ("Choose profile"). No guess, no "first one".
- None: the game has no local save yet (first run). Uploading waits; a download restores into the chosen profile.

### 5. Architecture

- New port `SaveLocationProvider` (domain trait) and `SwitchSaveProvider` implementations (Eden/yuzu, Ryujinx); the
  current "unsupported" `SaveSafety` becomes provider-driven (status, drift, post-exit sync, version-switch safety).
- A pure `save_archive` domain module (deterministic zip build, verify, extract plan) with property tests: same tree →
  same bytes; round trip; traversal and symlink entries rejected (`safe_join` on every entry).
- Callables are ported in the order of the UI dependency: `get_save_status`, `get_save_sync_settings` /
  `update_save_sync_settings`, `pre_launch_sync`, `post_exit_sync`, `sync_rom_saves`, slot callables, conflict resolution,
  rollback. `unportedCallable` loses an entry as each lands.
- Emulator saves for **RetroArch** are out of scope (Switch only until 0.80).

## Alternatives considered

- **Sync per file.** Rejected (see §3). Many rows, no atomic version.
- **Archive the whole `save/<profile>` tree.** Rejected: mixes other games and profiles; a conflict in one game blocks
  another.
- **Parse the title ID from NSP/XCI headers.** Rejected for now (needs keys, fragile); the file-name tag plus a
  user-confirmed mapping covers the cases we know.
- **Pick "the" profile automatically.** Rejected: a wrong guess uploads or overwrites the wrong user's save.
- **Cloud-sync through the emulator's own mechanism.** Does not exist for these emulators.

## Consequences

- Games whose file name carries no title ID need a one-time *Choose save folder* click.
- A save restore replaces a directory, so the quarantine backup can be large; it is kept until the next successful sync
  (retention follows the existing `.romm-backup` rule).
- Hash parity with RomM's own zip-entry hash is **not** required; both sides compare the hash of the uploaded bytes.
- Ryujinx depends on reverse-engineered metadata; if its format changes, those games drop to "unsupported", never to wrong
  data.

## 7. Facts still to verify on a device (as for the firmware, #69)

Needed from the Steam Deck and one Bazzite PC; none of this may be assumed:

1. Where Eden, yuzu and Ryujinx keep their data and whether EmuDeck symlinks them under `Emulation/saves/<emulator>`.
2. The exact layout under `nand/user/save/0000000000000000/`: profile folders (names), title folders, what is inside a
   title folder (file count, sizes, extra metadata files).
3. For Ryujinx: the layout of `bis/user/save/` and `bis/user/saveMeta/` for a game that has a save, and how a title ID
   maps to the index (sample files, sizes).
4. Whether the file-name tag is present in your ROM names (`ls` of the Switch ROMs folder) and whether RomM exposes a
   title ID anywhere in its game JSON.
5. Whether `Emulation` and the emulator data directories are on the same filesystem (affects the atomic rename).

## Acceptance of the decision

Patrick approves the five choices above (title-ID order, deterministic archive per `(rom, slot)`, base-title
normalisation, no automatic profile choice, `rom_title_ids` table) and the device facts are recorded in
`docs/plans/0.55.0-switch-save-sync.md`; then #34 starts.
