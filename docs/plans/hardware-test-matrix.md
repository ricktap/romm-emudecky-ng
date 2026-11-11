# Hardware test matrix (#47)

Run with the newest release on each machine: Deck internal storage, Deck with SD card, Bazzite #1 and Bazzite #2. The
Switch save round trip has its own checklist ([save-sync-acceptance.md](save-sync-acceptance.md), #36). The automated tests
already cover the logic of every row; this is about the real devices. Note ✔ / ✘ per machine. A ✘ needs the step, the
banner or chip text and the plugin log lines (QAM → Settings → Plugin).

| Case                        | Deck internal | Deck SD | Bazzite #1 | Bazzite #2 |
| --------------------------- | :-----------: | :-----: | :--------: | :--------: |
| 1 EmuDeck detected          |               |         |            |            |
| 2 Download + resume         |               |         |            |     n/a    |
| 3 Eden / yuzu / Ryujinx     |               |         |            |     n/a    |
| 4 Per-game / per-platform emulator (#42) |  |         |            |     n/a    |
| 5 Uninstall leaves nothing (#41) |          |         |    n/a     |     n/a    |
| 6 URL fallback (#39)        |               |   n/a   |            |     n/a    |
| 7 SD ejected → no writes (#43) |    n/a     |         |    n/a     |     n/a    |
| 8 Storage moved notice (#44) |    n/a       |         |    n/a     |     n/a    |

## 1. EmuDeck detected

- [ ] Open the QAM: no *Needs attention* row about EmuDeck. RomM Library: no banner above the tabs.
- [ ] Systems & BIOS: the Switch row names the installed emulators in the **Emulator** column.

## 2. Download + resume

- [ ] Download a game (a few GB). The file lands under `<storage>/Emulation/roms/switch/`.
- [ ] Downloads tab: speed and time left are shown. Press **Pause**, then **Resume**: it continues, no restart from 0.
- [ ] Download two games. **Start now** on the waiting one: it runs, the other pauses.
- [ ] Interrupt: switch the network off for a minute while downloading. Switch it on: within about 30 s the download
      carries on by itself.
- [ ] Restart the plugin (or Decky) mid-download: the partial is still there and resumes.

## 3. Eden / yuzu / Ryujinx

- [ ] For each installed emulator: set it for the platform (Systems & BIOS → Emulator) and start a game. It opens in that
      emulator.
- [ ] Switch keys: Systems & BIOS shows the key state per emulator; **Download BIOS** fills only what is missing and keeps
      a differing key of yours (shown as *differs*; Replace asks first and keeps a `.romm-backup`).

## 4. Emulator choice (#42)

- [ ] Platform default = Ryujinx: every game without its own choice starts in Ryujinx (check two games).
- [ ] On one game, game page → ··· → emulator → pick Eden: that game starts in Eden, the others still in Ryujinx.
- [ ] An emulator that is not installed is not offered.
- [ ] The Steam shortcut is the same entry before and after (artwork, collections and playtime stay).

## 5. Uninstall leaves nothing (#41)

- [ ] Install a single-file game and a game with several files (folder). Play each briefly.
- [ ] Game page → ··· → Uninstall ROM…: the confirm says what stays (shortcut, playtime, saves on RomM).
- [ ] Afterwards `Emulation/roms/switch/` holds no file of the game and no folder of it. Only the empty `switch/` folder
      stays (EmuDeck's own).
- [ ] The shortcut is still there, shows *Download* again, and starting it does not try to launch a missing file.
- [ ] Run a library sync: it does not download anything and does not delete any file.

## 6. URL fallback (#39)

- [ ] Settings → Connection: enter the **External URL**, sign in once.
- [ ] At home (internal URL reachable): the library loads; in RomM's access log the requests come from the internal address.
- [ ] Away from home, or with the internal address blocked: within a few seconds the same actions work through the
      external URL, no new sign-in.
- [ ] Remove the External URL again: only the internal one is used.

## 7. SD ejected → no writes (#43, Deck SD)

- [ ] Take the SD card out. Within 15 s the QAM shows **EmuDeck storage not available**; the RomM Library has the same
      banner.
- [ ] Start a download: refused with the same sentence. Nothing is created on internal storage
      (`ls ~/Emulation` must not exist or must not have grown).
- [ ] Put the card back: the banner goes away on its own; the same download starts.

## 8. Storage moved (#44)

- [ ] Move EmuDeck's storage (or change `storagePath` in `~/.config/EmuDeck/settings.json`): *EmuDeck storage moved* shows
      with the number of games not found. No file was touched.
- [ ] **Use this location** ends the notice.

## Afterwards

Failures become issues under the right milestone. All ✔ closes #47, #41 and #42.
