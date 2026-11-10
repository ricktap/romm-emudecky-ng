# The RomM Library and the Quick Access Menu

The plugin has three places, each with its own job.

| Where | For |
| ----- | --- |
| **Quick Access Menu** (the **...** button) | A glance and quick actions: connection, sync, what needs attention, downloads |
| **RomM Library** (full screen) | Browsing your games, the full download queue, Switch keys and firmware, all settings |
| **A game's Steam page** | Play or download, and what the game needs (ROM, BIOS, saves) |

You cannot scroll in the Steam UI. The cursor jumps from one button, field or toggle to the next, so the plugin keeps
text short and every information row can be focused.

## Quick Access Menu

Open the **...** menu, then **EmuDecky RomM Manager**.

- **Status line:** whether the plugin is connected, and to which host. Without a connection, syncing and downloading are
  off and installed games still launch.
- **Library:** how many games and platforms you have, when you last synced, and the **Sync Library** button. A sync shows
  a preview of what it would change first (**Apply Sync** / **Cancel**); turn the preview off with **Skip sync preview**
  in Settings. After a run the button says **Sync Again**.
- **Needs attention** (only when something is open): an update, a settings-reset or playtime notice, or missing Switch
  keys. Missing keys open the Systems & BIOS tab.
- **Manage:** **Downloads** (with the number of unfinished ones), **Platforms & collections**, **Settings** and
  **Open RomM Library**.

Press **B** on a page to go back to the main page; **B** on the main page closes the menu.

## RomM Library (full screen)

Open it with **Open RomM Library** in the Quick Access Menu. **L1** / **R1** switch tabs, **B** leaves.

### Library

- **Left:** *All games*, one entry per platform with its game count (a platform whose sync is off is dimmed), and below
  them your RomM *Collections* on this device. A platform and a collection are alternatives: choosing one clears the other.
- **Top:** a search field, and filter chips with counts: *All*, *Installed*, *On RomM only*, *Needs attention*.
- **Grid:** one cover per game. Badges: green check = installed, cloud = only on RomM, blue = downloading or queued (with
  a progress bar along the bottom edge), amber = paused, interrupted or something needs a look.
- **A** opens the game's Steam page. **X** downloads a game that is only on RomM, pauses a running download, or resumes
  a stopped one.

If a cover shows only a name, Steam has no image for that shortcut yet (see *Refresh Artwork* below).

### Downloads

The whole queue, in the order it will run: pause or resume each download (or **Pause all** / **Resume all**), cancel
one, and the finished list. The running download shows its speed and the time left. **Start now** on a waiting or
paused game puts it first and pauses the running one (resume that one later). A download that stopped because the
server was unreachable continues by itself, within about 30 seconds of the server being back.
**Open** on a finished game goes to its Steam page.

### Systems & BIOS

The Switch keys and firmware: what RomM has, what each emulator has, and the buttons to download or replace them. See
[Switch firmware and keys](bios-management.md).

### Settings

Connection, library sync (including **Skip sync preview** and **Force full sync**), artwork, save sync, registered
devices, updates and the **Danger Zone**. Every destructive action asks first.

## A game's Steam page

- **Play button:** *Download*, then progress with pause/resume (*In Queue* while it waits), then *Play*.
- **Chips next to it:** the disc or the version in use (*Disc 2*, *All discs*, *Europe*). Press one to change it.
- **Readiness strip** below: three chips.
  - **ROM:** installed, queued, downloading x %, paused or interrupted.
  - **BIOS:** *No BIOS needed*, *Keys ready*, or *Missing prod.keys* with a **Get** button that downloads the required
    files.
  - **Saves:** *Save sync off*, *Synced*, or a conflict with a **Resolve** button.
- **RomM actions menu** (the controller icon): *Refresh Artwork*, *Refresh Metadata*, *Sync Save Files*, *Save History*,
  and, for an installed game, *Delete Local Saves…* and *Uninstall ROM…*. Both of those ask first and say what is kept:
  the Steam shortcut with its artwork and collections, your playtime and your saves on RomM.
- **Show in RomM Library** (the grid icon) opens the Library tab narrowed to the game's platform.
- **Tabs below:** *Overview*, *Saves* (when save sync is on) and *BIOS & Core* (the files the platform needs, the
  emulator the game launches with and a per-game override).
