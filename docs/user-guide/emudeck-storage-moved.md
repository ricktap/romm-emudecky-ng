# When EmuDeck's storage moves

If EmuDeck now stores its files somewhere else than the last time the plugin looked (a new SD card, a reinstall that
picked another drive), the plugin tells you instead of silently looking at the wrong place.

## What you see

- **Quick Access Menu → Needs attention:** *EmuDeck storage moved*, with how many of your installed games were not found
  at the new location.
- **RomM Library → Systems & BIOS:** a card with the old and the new location.

## What the plugin does and does not do

It never moves, copies or deletes a file. Games that stayed on the old location show as *not installed*.

To carry on:

1. Copy your `Emulation/roms` folders to the new location, or download the games again.
2. Press **Use this location** on the card. The plugin remembers the new place and the notice goes away.

The other states (storage not available, EmuDeck not set up, unreadable or invalid settings) are described in
[Troubleshooting](troubleshooting.md).
