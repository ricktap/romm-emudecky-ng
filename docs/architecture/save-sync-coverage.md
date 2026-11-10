# Save sync coverage

| System | Emulator | Status |
| ------ | -------- | ------ |
| Nintendo Switch | Eden | supported (title ID from the file name, RomM, a played session or a manual pick; profiles) |
| Nintendo Switch | yuzu | supported (same layout as Eden; the NAND path comes from yuzu's own config) |
| Nintendo Switch | Ryujinx | supported where the machine already has a save for the game; the index is found through Ryujinx's own metadata |
| Everything else | any | not available before 0.80 (retro) and 0.90 (modern consoles) |

Per-emulator detail and the reasons are in [ADR-0023](../adr/0023-switch-save-sync-standalone-emulators.md); how it works in
[Save sync architecture](save-file-sync-architecture.md).
