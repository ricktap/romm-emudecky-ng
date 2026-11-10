![EmuDecky RomM Manager](assets/banner.png)

# EmuDecky RomM Manager

A [Decky Loader](https://decky.xyz/) plugin that syncs your self-hosted [RomM](https://github.com/rommapp/romm) ROM
library into Steam as Non-Steam shortcuts. Games launch through EmuDeck.

- Browse your entire RomM library directly from Steam's Gaming Mode
- Download ROMs on-demand with cover art, hero banners, logos, and metadata
- Download and check the Switch firmware and keys your emulators need
- Sync save files between devices through your RomM server

![A RomM game's detail page in Steam showing the injected EmuDecky RomM Manager panel](assets/screenshot-game-detail.jpg)

## User Guide

1. **[Getting Started](user-guide/getting-started.md)** — Prerequisites, installation, and first-time setup
2. **[Configuration](user-guide/configuration.md)** — Connection settings, SteamGridDB API key, Steam Input, debug
   options
3. **[The RomM Library and the Quick Access Menu](user-guide/romm-library.md)** — Where everything is: QAM, full-screen library, game page
4. **[Syncing Your Library](user-guide/syncing-your-library.md)** — How sync works, per-platform toggles, collections,
   artwork
5. **[Managing Games](user-guide/managing-games.md)** — Game detail panel, downloading ROMs, uninstalling, refreshing
   metadata
6. **[Switch firmware and keys](user-guide/bios-management.md)** — Checking keys per emulator, choosing a firmware version, downloading, replacing a key
7. **[When EmuDeck's storage moves](user-guide/emudeck-storage-moved.md)** — What the plugin tells you when EmuDeck now
   stores on another drive
8. **[Save Sync](user-guide/save-sync.md)** — Auto-sync, conflict resolution modes, manual sync, failed sync retries
9. **[Troubleshooting](user-guide/troubleshooting.md)** — Common issues, fixes, Danger Zone explained

## Technical Reference

Developer-oriented documentation for contributors and those interested in the internals.

- **[Steam Non-Steam Shortcuts](architecture/steam-non-steam-shortcuts.md)** — AddShortcut API, VDF format, app ID
  generation
- **[Backend Architecture](architecture/backend-architecture.md)** — Service/adapter architecture, dependency diagram,
  boundary enforcement
- **[Config Source Parsers](architecture/config-source-parsers.md)** — One-parser-per-source principle, source catalog,
  parser layout template for local config/metadata files
- **[Core and Emulator Selection](architecture/core-emulator-selection.md)** — Per-game vs per-system core overrides,
  the single read seam, and how the chosen core is baked into the launch command
- **[EmuDeck Launch Resolution](architecture/emudeck-launch-resolution.md)** — How the plugin resolves EmuDeck launcher
  scripts for per-platform launch commands
- **[Development](contributing/development.md)** — Developer setup, building, testing, dev reload
- **[Save File Sync Architecture](architecture/save-file-sync-architecture.md)** — Three-way conflict detection, session
  tracking, state schema, device registration
- **[Steam Remote Play and Cross-Device Shortcuts](architecture/steam-remote-play.md)** — Remote Play discovery
  protocol, phantom shortcuts, detection APIs
