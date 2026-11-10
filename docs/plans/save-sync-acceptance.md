# Save sync: hardware acceptance (#36)

Run with v0.51.0 or newer. RomM must be reachable. Use one Switch game that has a save in Eden on the Deck and a second
machine (Bazzite) with Eden. Note the result of each step; a failed step needs the Saves chip text and the plugin log lines
(see the end).

## 0. Setup (both machines)

- [ ] Plugin updated, save sync **on** (QAM → Settings → Enable save sync).
- [ ] Same RomM user on both machines.

## 1. Deck: the save goes up

- [ ] Open the game: the Saves chip shows **Choose save folder** (or already a state if the title is in the file name).
- [ ] Play it once (create or change a save), exit.
- [ ] Open the chip → **Choose** → confirm the suggested folder (the one that changed). Result: chip changes.
- [ ] Press **Sync Save Files** in the RomM menu (or relaunch the game).
- [ ] In RomM (web) the game has a save named `<TITLEID>.zip` in slot `default`. ✔ / ✘
- [ ] The Deck's save folder is unchanged. ✔ / ✘

## 2. Bazzite: the save comes down

- [ ] Same game installed. Saves chip → **Choose** the matching save folder if asked (or none exists yet: start the game
      once in Eden and exit, so the folder exists).
- [ ] **Sync Save Files**. The Deck's progress is in the game. ✔ / ✘
- [ ] A `<TITLEID>.romm-backup` exists next to the replaced save (if there was one). ✔ / ✘

## 3. Conflict

- [ ] With RomM unreachable (or sync off), change the save on **both** machines.
- [ ] Sync on the Deck, then on Bazzite: Bazzite shows the conflict dialog.
- [ ] **Use server**: Bazzite takes the Deck's save, its own is kept as `.romm-backup`. ✔ / ✘
- [ ] Repeat once and choose **Keep local**: RomM now has the local save as the newest version. ✔ / ✘

## 4. Slots and history

- [ ] Saves tab → switch to a new slot: the old save is moved aside, nothing is uploaded into the new slot.
- [ ] Saves tab → roll back to an older version: the game has that save, RomM shows it as the newest. ✔ / ✘

## 5. Ryujinx (a machine that already has this game saved in Ryujinx)

- [ ] Set the game's emulator to Ryujinx (per-game core).
- [ ] The Saves chip finds the save without a choice (the title is read from Ryujinx's metadata). ✔ / ✘
- [ ] Sync uploads it; on a machine **without** a Ryujinx save for the game the chip says to start the game once. ✔ / ✘

## What to send back when a step fails

- The Saves chip text and the step number.
- The log lines (the plugin log in `~/homebrew/logs/romm-emudecky-ng/` or Decky's log view) around the time, and for
  `get_save_title_info` / `get_save_status` the answer if you can capture it.
- Do not send save files.
