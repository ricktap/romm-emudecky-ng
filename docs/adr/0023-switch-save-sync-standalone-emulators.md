# Switch save sync for standalone emulators (Eden, yuzu, Ryujinx)

## Status

**Proposed** (2026-11-10), revised the same day with the device facts of §7 (Steam Deck and a Bazzite PC). Patrick agrees with the direction; open points are listed at the end. Issue #33; blocks #34
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

1. **A `[0100XXXXXXXXXXXX]` tag in the file name.** Exactly one 16-hex tag with the `01` prefix. *Device finding:* on the Deck
   only one of about ten Switch ROMs carries it (the others are folders or untagged `.xci` names), so this is the
   exception, not the rule.
2. **RomM's `title_id`**: the game JSON has a top-level `title_id` (and `save_target` / `save_target_layout`); on the checked game (Metroid Dread) all three are `null`, so it is used when RomM fills it, never required.
3. **Learned from play.** Around a play session the plugin notes which title folders in the emulator's save directory
   changed (new or modified files). The one folder that changed is **proposed** on the game page ("This game saved to
   `<TITLEID>`: use it?") and bound only after the user confirms. Several changed folders (a game that touches two titles) or none:
   no proposal.
4. **A manual pick**: *Choose save folder* lists the title folders that exist locally.

The confirmed ID is stored per ROM (new table `rom_title_ids`, migration 015) and survives re-syncs. Without an ID the game
is **unsupported** and the SAVES chip says so. Update and DLC IDs are normalised to the **base title ID** (an update is
`base | 0x800`; DLC owns no saves). Parsing NSP/XCI headers stays **out** (it needs `prod.keys`).

### 2. Layouts, one provider per resolved emulator

`SaveLocationProvider::locations_for(emulator, title_id) -> Vec<SaveLocation>` is selected by the **resolved emulator**
(including the per-game override), never by system. The roots come from the emulator's **own configuration**, because the
default and the configured location differ in practice (§7):

| Emulator | Where the saves are | How the provider finds them |
| -------- | ------------------- | --------------------------- |
| Eden | `<nand_directory>/user/save/0000000000000000/<profile>/<TITLEID>/` | `nand_directory` from `~/.config/eden/qt-config.ini`; the Deck's is `~/.local/share/eden/nand` (internal), the Bazzite PC's is `~/Emulation/storage/eden/nand` |
| yuzu | the same layout | `nand_directory` from `~/.config/yuzu/qt-config.ini`; on the Deck it ends up on the SD card (`Emulation/storage/yuzu/nand`, reachable as `Emulation/saves/yuzu/saves`) while `~/.local/share/yuzu/nand/user/save` is absent |
| Ryujinx | `<config>/bis/user/save/<index>/`, `<index>` e.g. `0000000000000001` | no title ID in the path: see Ryujinx below |

EmuDeck's `Emulation/saves/<emulator>` entries are symlinks or mirrors of those directories (and absent for some
emulators); the provider never relies on them, it resolves the configured directory and canonicalises it. A missing
config, a missing directory or an unreadable file means **unsupported**, never a fallback path. EmuDeck's `storagePath` can
be the literal text `$HOME` on a Bazzite PC (the existing `expand` handles it).

**Ryujinx.** An index directory holds `ExtraData0` and `ExtraData1` (512 bytes each), the two commit directories `0` and
`1` (each with `save.meta` and the data), and a `.lock`. The title is identified from the index's `ExtraData0` header (the
save attribute, verified with a hex dump, §7: bytes 0-7 little-endian = program/title ID, bytes 8-23 = user ID, byte 0x20 = save type, `1` = account save). Only indices whose type is account (`1`) and whose title ID matches belong to the game; device, cache and temporary saves are not synced. The index
number differs per machine. Consequences:

- The **whole index directory** is the unit (both commit directories and the extra data), except `.lock`; a partial copy
  would corrupt Ryujinx's double buffering.
- A restore **only replaces the contents of an index that already exists locally** for that title. Creating a new save index
  would mean writing Ryujinx's own index (`imkvdb` / `saveMeta`), which we do not do. A machine that never saved the game
  shows "start the game once on this machine, then sync" instead of restoring.

### 3. Directory saves: one deterministic archive per `(rom, slot)`

The whole title directory is synced as **one zip**, not per file. Per-file sync would put dozens of RomM rows per game
and no consistent version across them; RomM's slot/version model assumes one object.

- The archive is **deterministic**: entries sorted by path, fixed timestamps (1980-01-01), no extra fields, fixed
  compression level. The same content always yields the same bytes and therefore the same hash, so
  `compute_sync_action` sees "unchanged" instead of false conflicts.
- Entry paths carry the **role**, not the IDs (`zero/…`, `user/…` for Eden/yuzu; `index/…` for Ryujinx), so an archive made on one
  machine restores into another machine's profile or index.
- File name on RomM: `<TITLEID>.zip` within the user's slot (the slot, wizard and switch gates are unchanged).
- The hash is **RomM's zip content hash**, not the hash of the archive bytes: entries sorted by name, `name:md5(entry)` lines joined by `\n`, MD5 of that text, directory entries skipped (ADR-0016; the Python reference `combine_zip_entry_hashes`). Exact parity is required, or sync would round-trip forever. Entry names are part of the hash, which is why they are role-based and identical on every device.
- Restore (download) extracts into a temp directory next to the target, verifies, quarantines the current directory as
  `<TITLEID>.romm-backup`, then renames into place. A failed extraction never touches the live save.
- **Safety gates:** no sync while the emulator is running (existing running-app check); an empty or shrunk-to-nothing local
  save never overwrites a non-empty server save (the #1062 guard, applied to the archive's file count and size).

### 4. Profiles (Eden, yuzu)

*Device finding (Deck, Eden):* two folders under `save/0000000000000000/`: a user profile (`7EBDCC30…`) and the
**all-zero profile** `00000000000000000000000000000000`, and one title (`0100152000022000`) has a save in **both**. The
all-zero folder holds saves not bound to a user; a user profile folder holds the per-user ones. Profile IDs differ per
installation. So a title's save is the pair, and the archive keeps the roles, not the IDs:

- `zero/<TITLEID>/…` from the all-zero folder, restored to the all-zero folder;
- `user/<TITLEID>/…` from the chosen user profile folder, restored into the chosen profile of the target machine.

Which user profile: exactly one non-zero profile holds the title (or exists at all) = that one; several = the user picks once
(setting `switch_save_profile` per emulator); until then the game is unsupported ("Choose profile"). No guess, no "first".
The profile list shown to the user comes from the emulator's `profiles.dat` (names), not from folder names.

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
- The archive bytes do not decide the hash (only the entries do), so a re-zip with other timestamps never looks like a change; the deterministic build is for tidiness, the hash scheme is for correctness.
- Ryujinx depends on reverse-engineered metadata; if its format changes, those games drop to "unsupported", never to wrong
  data.

## 7. Facts recorded from devices (2026-11-10)

**Steam Deck** (`/home/deck`, internal `ext4` on `nvme0n1p8`, `Emulation` on the SD card `ext4`, EmuDeck `storagePath`
`/run/media/mmcblk0p1`):

- Eden: `nand_directory=/home/deck/.local/share/eden/nand` (default). Saves under `nand/user/save/0000000000000000/` with two
  profile folders (a user profile and the all-zero profile), three title folders in total, one of them in both. `Emulation/saves/eden` does not exist.
- yuzu: data in `~/.local/share/yuzu` but `nand/user/save` is absent there; `Emulation/saves/yuzu/saves` is a symlink to
  `Emulation/storage/yuzu/nand/user/save/` on the SD card with one user profile folder (`A2302A18…`) and no title folder yet.
  yuzu's config path for `nand_directory` was not captured (see open points).
- Ryujinx: `~/.config/Ryujinx/bis/user/save/0000000000000001/` with `ExtraData0/1` (512 B), `0/` and `1/` (each `save.meta` 2,656 B
  plus a data directory) and `.lock`; `bis/user/saveMeta/0000000000000001/00000001.meta` (262,240 B). `Emulation/saves/ryujinx/saves`
  shows the same tree (EmuDeck's mirror/symlink).
- ROM names: mostly folders or untagged `.xci` names; one `.nsp` with a `[TITLEID][v0]` tag.

**Bazzite PC** (`/home/patrick`, `btrfs` on `/home`, `storagePath` the literal `$HOME`):

- Eden: configured `nand_directory=/home/patrick/Emulation/storage/eden/nand`, **not** the default `~/.local/share/eden/nand`
  (that one only holds `system`). The configured `…/nand/user/save` exists and is empty; `Emulation/saves/eden/{saves,profiles}` exist.
- yuzu is not installed; Ryujinx exists with empty save directories; no Flatpak data directories.
- `Emulation` and the emulator data are on the same filesystem (so a temp directory next to the target is on the same device).

**What this changed in the ADR:** title ID from file-name tag is rare (so "learned from play" and a manual pick matter), roots must
come from the emulator config, the all-zero profile is part of a save, and Ryujinx can only restore into an existing local index.

## Open points answered (2026-11-10, Deck, read-only)

1. **RomM game JSON** (`GET /api/roms/22`, Metroid Dread): top-level `title_id: null`, `save_target: null`, `save_target_layout: null`; a
   multi-file game folder (`.xci` plus update `.hdf`). So the field exists but is not filled for this game.
2. **Ryujinx `ExtraData0`** of index `0000000000000001`: bytes `00c0 aa1b f08c 0001` = little-endian `0x01008CF01BAAC000`, the title ID of
   *The Legend of Zelda: Echoes of Wisdom*, confirmed by Ryujinx's own log line (`Application Loaded: … [01008cf01baac000]`). Bytes 8-23
   hold the user ID (`01 00 …`), byte 0x20 is `01` (account save). The numeric index is machine-specific and never generalised.
3. **yuzu `nand_directory`** on the Deck: `/run/media/mmcblk0p1/Emulation/storage/yuzu/nand` (SD card), so saves are under
   `…/nand/user/save`, found through the config as designed.
4. **Profile names** (`profiles.dat`, verified with a hex dump of the Deck's file): 16-byte header, 8 records of 200 bytes; a record holds the UUID twice, a timestamp and a 32-byte name; the save folder name is the UUID with **all 16 bytes reversed**, upper-case hex (`3cdbbca0…bd7e` → `7EBDCC305B4640571756A9D7A0BCDB3C`, name `Eden`). The plugin shows these names instead of the hex folders.
   Eden profile `7EBDCC30…`: display name `Eden`, the only non-empty record in `profiles.dat` (slot 0), read from
   `<nand>/system/save/8000000000000010/su/avators/profiles.dat`. The single user profile is therefore chosen automatically; the all-zero
   folder keeps its role.
5. Deck internal drive vs SD: the temp-directory-next-to-target rule keeps the rename atomic; no action needed.

## Acceptance of the decision

Patrick approves the choices (title-ID order incl. learning from play, deterministic archive per `(rom, slot)` with role-based entry
paths, base-title normalisation, the zero profile plus one chosen user profile, Ryujinx restore only into an existing index,
`rom_title_ids` table). Patrick agreed with the direction on 2026-11-10; the open points above are answered, then #34 starts.
