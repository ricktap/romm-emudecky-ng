#!/bin/sh
# Read-only. Lists where the Switch emulators keep their saves (names, sizes, filesystems), for ADR-0023 §7.
# Prints to the terminal only: it creates, changes and deletes nothing, and it does not read any save file's contents.
# Usage:  sh save-sync-device-facts.sh > save-facts.txt 2>&1      (run it on the Deck and on a Bazzite PC)

echo "== host"; uname -a; echo "HOME=$HOME"; echo
EMUDECK="$HOME/.config/EmuDeck/settings.json"
STORAGE=$(sed -n 's/.*"storagePath"[^"]*"\([^"]*\)".*/\1/p' "$EMUDECK" 2>/dev/null | head -1)
echo "== EmuDeck storagePath: ${STORAGE:-<not found>}"; echo
list() { echo "-- $1"; if [ -e "$1" ]; then ls -la "$1" 2>&1 | head -40; else echo "(missing)"; fi; echo; }
tree2() { echo "-- $1 (3 levels, names and sizes only)"; if [ -e "$1" ]; then find "$1" -maxdepth 3 -printf '%y %s %p\n' 2>/dev/null | head -80; else echo "(missing)"; fi; echo; }

for emu in eden yuzu; do
  echo "== $emu"
  list "$HOME/.local/share/$emu"
  tree2 "$HOME/.local/share/$emu/nand/user/save"
  list "$HOME/.config/$emu"
done
echo "== Ryujinx"
list "$HOME/.config/Ryujinx"
tree2 "$HOME/.config/Ryujinx/bis/user/save"
tree2 "$HOME/.config/Ryujinx/bis/user/saveMeta"
list "$HOME/.config/Ryujinx/bis/system/save"
echo "== flatpak data dirs (Bazzite)"
ls -d "$HOME"/.var/app/*[Ee]den* "$HOME"/.var/app/*[Yy]uzu* "$HOME"/.var/app/*[Rr]yujinx* 2>/dev/null
echo
if [ -n "$STORAGE" ]; then
  echo "== EmuDeck saves folder"
  list "$STORAGE/Emulation/saves"
  for e in yuzu eden Ryujinx; do list "$STORAGE/Emulation/saves/$e"; done
  echo "== filesystems"
  for p in "$HOME/.local/share/yuzu" "$HOME/.local/share/eden" "$HOME/.config/Ryujinx" "$STORAGE/Emulation"; do
    [ -e "$p" ] && { printf '%s -> ' "$p"; df --output=source,fstype "$p" | tail -1; }
  done
  echo; echo "== Switch ROM file names (first 15)"
  ls "$STORAGE/Emulation/roms/switch" 2>/dev/null | head -15
fi
