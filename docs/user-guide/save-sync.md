# Save sync

Save sync keeps a game's save in step between your devices through your RomM server. Play on the Steam Deck, continue on
a PC; the save follows.

It works for **Nintendo Switch games on Eden, yuzu and Ryujinx**. Other systems come later (see the support table in the
README).

## What it does

- **Before a game starts**, the plugin asks RomM for the newest save of this game's slot and downloads it if it is newer
  than yours.
- **After the game exits**, it uploads your changed save as a new version.
- **Never silently overwrites.** If both this device and another one changed the save, a dialog asks which one to keep.
  Whatever it replaces on this device is kept next to the save as `<TITLE>.romm-backup`.

Save sync runs in Game Mode, where the plugin is loaded, and for launches that go through the plugin (the Play button, or
a Steam deep link). Changes made in Desktop Mode are picked up at the next sync.

## Turning it on

1. Open the Quick Access Menu → **Settings** (or **All settings**) and switch **Enable save sync** on. The same page has
   *Sync before launch* and *Sync after exit*.
2. Open a game. The **Saves** chip under the Play button tells you where it stands.

The first time, a **slot** is chosen for each game (the default is `default`). A slot is one timeline of saves, so you
can keep, say, a normal and a challenge run apart.

## Finding a game's save

A Switch save is stored under the game's **title ID** (for example `01008CF01BAAC000`), and the plugin never guesses one.
It uses, in this order:

1. A `[01XXXXXXXXXXXXXX]` tag in the game's file name, if there is one.
2. The title ID RomM has for the game, if RomM has one.
3. **The save that appeared while you played.** After a session in which exactly one save folder changed, the **Saves**
   chip shows **Choose save folder**. Open it, and confirm the folder it suggests.
4. A folder you pick yourself from the list of folders the emulator has.

Until a game's title is known, its saves are not synced and the game launches normally.

### Profiles

Eden and yuzu keep saves per user profile. If there is exactly one profile, it is used. If there are several, the
**Saves** chip shows **Choose profile** once; the choice is remembered for that emulator. (Saves that belong to no user
are always part of the save.)

### Ryujinx

Ryujinx stores a save under an index that differs per machine. The plugin finds it through the game's own metadata and
only ever replaces an index that already exists. On a machine that has never saved the game, start the game once in
Ryujinx, then sync again.

## When something is not synced

The **Saves** chip says why:

| Chip | What to do |
| ---- | ---------- |
| Save sync off | Switch it on in Settings |
| Choose save folder | Open the chip and pick the game's save folder |
| Choose profile | Open the chip and pick the profile |
| Emulator save folder not found | Start the emulator once so it creates its folders |
| Server unreachable | Nothing is lost; it syncs the next time RomM is reachable |
| Conflict | Press **Resolve** and choose which save to keep |

## Slots and history

On a game's **Saves** tab you can switch slot, see the saves in each slot, delete a slot you no longer need, and roll back
to an older version. A switch is refused while this device has changes that were never uploaded.

## Where the backups are

A restore never deletes the save it replaces. On Eden and yuzu it is kept inside the profile folder as
`<TITLE>.romm-backup` (and `-1`, `-2`… for later ones). On Ryujinx it is kept beside Ryujinx's `save` folder in
`save.romm-backups`. Delete them by hand when you are sure you do not need them.

## Technical notes

How the decisions are made, what is hashed and why the save is a zip are in
[ADR-0023](../adr/0023-switch-save-sync-standalone-emulators.md) and
[Save sync architecture](../architecture/save-file-sync-architecture.md).
