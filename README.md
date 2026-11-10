# EmuDecky RomM Manager - Decky Loader Plugin

> **Fork** of [danielcopper/decky-romm-sync](https://github.com/danielcopper/decky-romm-sync) (via
> [melikootje/Romm-emudecky](https://github.com/melikootje/Romm-emudecky)) — ported from RetroDECK to EmuDeck. Roadmap:
> [`EMUDECK_PORT_PLAN.md`](EMUDECK_PORT_PLAN.md).
>
> Unofficial community plugin — not affiliated with or endorsed by EmuDeck, RomM, or Decky Loader.

A [Decky Loader](https://decky.xyz/) plugin that syncs your self-hosted [RomM](https://github.com/rommapp/romm) library
into Steam as non-steam shortcuts. Games appear directly in your Steam library, launch through EmuDeck, and keep their
saves in sync across devices through your RomM server.

## Features

- **Library sync** — Pulls platforms and ROMs from your RomM server and creates Steam shortcuts, complete with cover
  art, hero banners, and logos (with optional [SteamGridDB](https://www.steamgriddb.com/) artwork)
- **Save sync** — Keeps a Switch game's save in sync across devices through your RomM server (Eden, yuzu, Ryujinx): never overwrites silently, keeps a backup of what it replaces, and asks when both sides changed
- **ROM downloads** — Download ROMs on demand with progress tracking and a managed download queue
- **Switch firmware and keys** — See which keys and firmware RomM has, compare them with your emulators (Eden, yuzu, Ryujinx), download what is missing, pick the firmware version, and replace a key with a backup
- **RomM Library** — A full-screen library of your games with platform rail, filters, search and cover grid, the full download queue, Switch keys and firmware, and all settings
- **Game detail page** — Play or download, a readiness strip (ROM, BIOS, saves) with one-tap fixes, and the actions menu right on each game's Steam page
- **Per-platform control** — Choose exactly which platforms get synced
- **Controller friendly** — Full gamepad navigation throughout the plugin UI
- **Steam Input config** — Per-shortcut Steam Input mode (Default / Force On / Force Off)
- **EmuDeck path migration** — Move your EmuDeck installation between storage locations without re-syncing
- **EmuDeck health diagnostics** — Detects misconfigured input drivers and EmuDeck configuration issues that break menu
  navigation

## What works today

| Area | Status |
| ---- | ------ |
| Systems | **Nintendo Switch** only. Retro systems are planned for 0.80, modern consoles for 0.90 |
| Switch emulators | **Eden, yuzu, Ryujinx**, launched through EmuDeck (tested on the Steam Deck; Bazzite not yet verified) |
| Library sync, downloads, games on the Steam page | Working |
| Switch keys and firmware | Working: Eden, yuzu and Ryujinx (list, version choice, download, replace with backup) |
| Save sync | Working for Eden, yuzu and Ryujinx; **not yet verified on all devices** (hardware acceptance is open) |
| Other systems' BIOS | Not available until 0.80 / 0.90 |

The plugin is an unofficial community plugin and not affiliated with EmuDeck or RomM. The roadmap is in [`EMUDECK_PORT_PLAN.md`](EMUDECK_PORT_PLAN.md).

## Screenshots

> The screenshots below show an older layout; new ones follow with the next release.


|                                   QAM panel                                    |                                        Game detail page                                         |
| :----------------------------------------------------------------------------: | :---------------------------------------------------------------------------------------------: |
|         ![EmuDecky RomM Manager QAM panel](assets/screenshot-qam.jpg)          |              ![Game detail page with metadata](assets/screenshot-game-detail.jpg)               |
|                              **BIOS management**                               |                                      **Per-game actions**                                       |
| ![BIOS status showing a required file to download](assets/screenshot-bios.jpg) | ![RomM actions menu: sync saves, download BIOS, refresh artwork](assets/screenshot-actions.jpg) |

## Requirements

- [Decky Loader](https://decky.xyz/) on your Steam Deck or Linux HTPC — the plugin lives in Steam's gamepad UI, so it
  works in the Deck's Game Mode **or** in Big Picture Mode on any Linux PC
- A running [RomM](https://github.com/rommapp/romm) server, **version 4.9.0 or newer** (the plugin stays inert against
  older servers)
- [EmuDeck](https://www.emudeck.com/) for launching games

## Installation

### From the Decky Store

> ⚠️ **Not available yet.** The plugin will be submitted to the [Decky Store](https://plugins.deckbrew.xyz/) with the
> **v1.0** release. Until then, use the manual install below.

Once published, install it straight from Decky's built-in store — open the Quick Access Menu → **Decky** → store icon,
search for **EmuDecky RomM Manager**, and install. No Developer Mode required.

### From ZIP or URL

This is the current method while v1.0 is in progress. It requires **Developer Mode** in Decky Loader (Decky settings →
gear icon → toggle **Developer Mode**).

1. Download the latest `romm-emudecky-ng.zip` from the
   [releases page](https://github.com/ricktap/romm-emudecky-ng/releases)
2. In Decky settings → **Developer** tab → **Install Plugin from ZIP** (or **from URL** with the
   [latest release link](https://github.com/ricktap/romm-emudecky-ng/releases/latest/download/romm-emudecky-ng.zip))

> Full step-by-step instructions, including first-time setup, are in
> [Getting Started](docs/user-guide/getting-started.md).

## Quick start

1. Open the Quick Access Menu and select **EmuDecky RomM Manager**
2. Open **Settings → All settings** (the RomM Library's Settings tab), enter your RomM server URL and credentials, then hit **Test Connection**
3. Back in the Quick Access Menu, open **Platforms & collections** and enable the platforms you want to sync
4. Hit **Sync Library** — your ROMs appear as non-steam shortcuts

See [The RomM Library and the Quick Access Menu](docs/user-guide/romm-library.md) for where everything is, the [User Guide](docs/user-guide/syncing-your-library.md) for syncing details,
[save sync](docs/user-guide/save-sync.md), and
[Switch firmware and keys](docs/user-guide/bios-management.md).

## Contributing

Build from source, run the tests, and read the architecture reference on the documentation site:

- [Development setup](docs/contributing/development.md)
- [Frontend dev loop](docs/contributing/frontend-dev-loop.md) — live-reload the UI
  into a windowed Big Picture on the Deck, no Game Mode switching
- [Backend architecture](docs/architecture/backend-architecture.md)

## Acknowledgments

This plugin stands on the shoulders of some great projects:

- [danielcopper/decky-romm-sync](https://github.com/danielcopper/decky-romm-sync) — the upstream project this is forked
  from. Its RetroDECK integration was the foundation; this fork replaces the host layer with EmuDeck.
- [melikootje/Romm-emudecky](https://github.com/melikootje/Romm-emudecky) — started the EmuDeck port (EmuDeck path and
  launcher adapters) that this fork continues.
- [RomM](https://github.com/rommapp/romm) — the self-hosted ROM manager at the heart of this plugin. RomM provides the
  library, metadata, cover art, and save file storage that makes the entire sync experience possible
- [EmuDeck](https://www.emudeck.com/) — the all-in-one emulation solution for Steam Deck that bundles ES-DE, RetroArch,
  and standalone emulators with launcher scripts. Our entire launch chain runs through EmuDeck
- [Decky Loader](https://decky.xyz/) — the plugin framework that makes all of this possible
- [Valve](https://www.valvesoftware.com/) — for the Steam Deck, SteamOS, and an open enough platform to build on
- [Unifideck](https://github.com/ma3ke/unifideck) — inspiration for game detail page injection techniques and gamepad
  navigation patterns
- [MetaDeck](https://github.com/EmuDeck/MetaDeck) — inspiration for store patching patterns used in metadata display on
  non-Steam shortcuts
- [Argosy](https://github.com/rommapp/argosy-launcher) — RomM's Android device-sync client. Its baseline-anchored
  save-conflict handling — client-side detection layered over RomM's negotiate transport, with a keep-local/keep-remote
  prompt on genuine divergence — validated the posture this plugin's save sync takes
- [Grout](https://github.com/rommapp/grout) — RomM's Linux handheld client. Its 409-driven upload reconciliation (POST
  with overwrite=false, downgrading to a download when the local save is unchanged and surfacing a conflict when it
  diverged) informed this plugin's negotiate upload and conflict path

## License

GPL-3.0
