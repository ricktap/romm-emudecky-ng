# Save sync architecture (Switch, standalone emulators)

The decisions and their reasons are in [ADR-0023](../adr/0023-switch-save-sync-standalone-emulators.md), ADR-0016 and
ADR-0017. This page says where the pieces are.

## The rule

`compute_sync_action` (`backend/src/domain/sync_action.rs`) is the only authority on what happens to a save. RomM's
`negotiate` is only a session envelope around a run; its planned operations are ignored. Every upload is `overwrite=false`
(RomM answers `409` when the slot moved past this device), `overwrite=true` only after the user chose *keep local*, and a
restore keeps the replaced save as `.romm-backup`. The kernel was ported from the Python reference and is tested against it
on random scenarios (`backend/tests/fixtures/sync_action_reference.json`).

## Pieces

| Layer | File | Job |
| ----- | ---- | --- |
| domain | `save_archive.rs` | a save as one zip with role-rooted entry names, strict path validation, **RomM's zip content hash** (sorted `name:md5` lines, MD5) |
| domain | `switch_save.rs` | title IDs (tag, base title), `nand_directory` from the emulator's config, Ryujinx `ExtraData0`, Eden/yuzu profiles and `profiles.dat`, the "which folder changed" proposal |
| domain | `sync_action.rs` | the decision kernel and the 409 re-decision |
| domain | `rom_save_state.rs`, `save_status.rs` | per-ROM state (slots, baselines) and the status/label the UI shows |
| adapter | `switch_save_files.rs`, `switch_saves.rs` | the emulators' save folders: find from config, snapshot, collect, restore with backup, Ryujinx indices |
| adapter | `romm/api.rs` (`RommSavesApi`) | `/api/saves`, `/api/sync`, `/api/devices` |
| adapter | `repositories.rs` | `rom_save_states`, `rom_save_files`, `rom_title_ids` |
| service | `services/saves.rs` | `SaveService`: sync, status, conflicts, slots, versions, devices, title and profile choice |

## One sync of one game

1. **Subject.** ROM and install → the resolved emulator (including the per-game override) → the title ID (stored, else a
   single file-name tag) → the profile. Anything missing is *unsupported* and says why; it never fails a launch and never
   asks the network.
2. **Local.** Read the title's files into entries `zero/<TITLE>/…`, `user/<TITLE>/…` (Eden/yuzu) or `index/…` (Ryujinx).
   Hash = RomM's zip hash of the entries.
3. **Server.** `GET /api/saves?rom_id&slot` filtered to `<TITLE>.zip`.
4. **Decide, then do.** Upload (new version), download (verified against RomM's hash and the title before anything is
   written), or a conflict entry for the dialog. A `409` on upload is re-decided from hashes only.
5. **Persist** the baseline (`rom_save_files`) and the slot state.

## Safety checks

- No path from a server archive is trusted: role prefix, no `..`, no absolute paths, no symlinks, size and count limits.
- A write is staged in a sibling directory and swapped in; the old save is kept; a failed swap puts it back.
- A local save of 0 bytes, or below half its last synced size, is a conflict, never uploaded over the good copy.
- The bulk sync skips games whose slot was never confirmed.
- Ryujinx is only restored into an existing index; its backups live outside its `save` directory.

## Callables

`get_save_sync_settings`, `update_save_sync_settings`, `ensure_device_registered`, `list_devices`, `get_save_status`,
`refresh_save_status` (pushes `save_status_updated`), `pre_launch_sync`, `sync_rom_saves`, `sync_all_saves`,
`resolve_sync_conflict`, `is_save_tracking_configured`, `get_save_setup_info`, `confirm_slot_choice`, `get_save_slots`,
`get_slot_saves`, `switch_slot`, `get_slot_delete_info`, `delete_slot`, `delete_local_saves`, `delete_platform_saves`,
`saves_list_file_versions`, `saves_rollback_to_version`, and the new `get_save_title_info`, `set_save_title_id`,
`set_save_profile`.
