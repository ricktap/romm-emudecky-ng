#!/bin/sh
# Read-only facts about an EmuDeck machine, for filling the plugin's emulator table and platform-mapping tests.
# Run it on the Deck (Desktop Mode terminal or SSH) and on a Bazzite machine; paste the output.
# It only lists names and paths. It reads no credentials, writes nothing, and does not touch ~/homebrew/settings.

echo "== machine"
uname -srm
grep -E '^(NAME|VERSION_ID|VARIANT_ID)=' /etc/os-release 2>/dev/null

echo; echo "== EmuDeck settings (storagePath only)"
CFG="$HOME/.config/EmuDeck/settings.json"
if [ -f "$CFG" ]; then grep -o '"storagePath"[^,}]*' "$CFG"; else echo "no $CFG"; fi

STORAGE=$(grep -o '"storagePath"[^,}]*' "$CFG" 2>/dev/null | sed 's/.*: *"\(.*\)"/\1/')
[ -n "$STORAGE" ] || STORAGE="$HOME"

echo; echo "== roms folders ($STORAGE/Emulation/roms)"
ls -1 "$STORAGE/Emulation/roms" 2>&1

echo; echo "== bios folders ($STORAGE/Emulation/bios)"
ls -1 "$STORAGE/Emulation/bios" 2>&1 | head -60

echo; echo "== ~/Applications"
ls -1 "$HOME/Applications" 2>&1
ls -1 "$HOME/Applications/publish" 2>/dev/null | head -20

echo; echo "== flatpaks"
flatpak list --app --columns=application,version 2>&1 | head -40

echo; echo "== RetroArch cores"
CORES="$HOME/.var/app/org.libretro.RetroArch/config/retroarch/cores"
if [ -d "$CORES" ]; then ls -1 "$CORES" | sed 's/_libretro.so$//'; else echo "no $CORES"; fi

echo; echo "== RetroArch launchers EmuDeck wrote"
ls -1 "$HOME/.config/EmuDeck/backend/tools/launchers" 2>/dev/null | head -60
ls -1 "$HOME/Emulation/tools/launchers" 2>/dev/null | head -60
[ -n "$STORAGE" ] && ls -1 "$STORAGE/Emulation/tools/launchers" 2>/dev/null | head -60

echo; echo "== RetroArch config (cores dir, saves dir)"
RA="$HOME/.var/app/org.libretro.RetroArch/config/retroarch/retroarch.cfg"
[ -f "$RA" ] && grep -E '^(libretro_directory|savefile_directory|savestate_directory|system_directory|sort_savefiles_enable|sort_savefiles_by_content_enable|input_driver) ' "$RA" || echo "no $RA"
