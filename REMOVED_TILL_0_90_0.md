# Removed until v0.90.0

The plugin is **Switch-only until v0.80** (see `EMUDECK_PORT_PLAN.md` §0). During the Rust rewrite (milestone 0.40.0)
these features were deliberately **not ported**. They come back, where still wanted, in v0.80.0 (retro systems) or v0.90.0 (modern
consoles), when the other systems return.

**Where to find the old code:** the last Python release before the rewrite is tag **`v0.33.2`**; the full Python backend on the rewrite line is at commit **`580680864e89af50c376dc946d12cf35cee7f808`** (branch `main` until
0.40.0). Paths below refer to the tag.

| Feature | Why it was removed | Python source at `v0.33.2` | Removed in |
| ------- | ------------------ | -------------------------- | ---------- |
| **RetroDECK migration**: migration status, banners and blocked pages, moving files between RetroDECK homes (`get_migration_status`, `refresh_migration_state`, `migrate_retrodeck_files`, `dismiss_retrodeck_migration`, `get_retrodeck_status`) | RetroDECK support is dropped (decision D3). An EmuDeck storage-path check replaces it later (Phase 8). | `services/migration.py`, `domain/migration_paths.py`, `adapters/migration_file.py`, `lib/migration_gate.py`, `lib/retrodeck_health.py`, `adapters/retrodeck_paths.py`; UI `MigrationBlockedPage.tsx`, `MigrationBlockedCard.tsx`, `MigrationConflictModal.tsx`, `utils/retrodeckHealth.ts` | step 3 |
| **RetroAchievements**: achievements tab and progress (`get_achievements`, `get_achievement_progress`) | No RetroAchievements for Switch. | `services/achievements.py`, `domain/achievements.py`; UI achievements in `RomMGameInfoPanel.tsx` / `RomMPlaySection.tsx` | step 3 |
| **BIOS per platform**: status, download and delete (`check_platform_bios`, `delete_platform_bios`, `get_bios_status`, BIOS parts of `download_*_firmware`) | Switch has no BIOS. Switch firmware/keys are handled separately (step 4 / Phase 7). | `services/firmware.py` (BIOS registry parts), `domain/bios.py`, `domain/bios_file.py`, `defaults/bios_registry.json`; UI `SystemPage.tsx` BIOS section, `utils/biosColor.ts` | step 3 |
| **RetroArch extras**: libretro-core save-compatibility prompts, per-system core selection (`set_system_core`), input-driver fix (`fix_retroarch_input_driver`), save-sort migration (`get_save_sort_migration_status`, `migrate_save_sort_files`, `dismiss_save_sort_migration`) | RetroArch only; Switch uses standalone emulators. | `services/cores.py`, `adapters/retroarch_config.py`, `adapters/retroarch_core_info.py`, `services/migration.py` (save sort); UI `SystemPage.tsx` cores, `SaveSortMigrationSection.tsx` | step 3 |
| **Download extras for other systems**: PS3 `.SFB` fix, M3U playlist generation, multi-disc handling outside the Switch launch-file pick, ES-DE folder-collapse rename, folder-boot ("direct") launch | Other systems only. | `services/downloads.py` (`_post_download_multi_io`), `domain/disc_formats.py`, `domain/disc_selection.py` | step 2 |
| **ES-DE XML emulator resolution and libretro/standalone launchers for non-Switch systems** | Unreadable on EmuDeck (inside `ES-DE.AppImage`); Switch uses our own emulator table. | `adapters/es_de_config.py`, `domain/emulator_commands.py`, `services/active_core_resolver.py` (ES-DE path), `domain/emudeck_launch.py` (libretro/standalone branches) | steps 2–3 |

Kept on purpose after the step-3 review (2026-11-09): SteamGridDB artwork, Steam Input per-shortcut setting, playtime
sync to RomM, RomM collections → Steam collections.

The retained `check_core_change` callable returns `{"changed": false}` for standalone Switch emulators, which have no
libretro core. Version-switch save-stranding checks and the launch gate depend on an injected save-safety port. The
production provider remains explicitly unsupported until step 5's standalone-save ADR and discovery implementation;
no Switch save paths are guessed, and no existing saves or baselines are removed by this boundary.
