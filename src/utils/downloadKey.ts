/**
 * Identity of a download in the shared queue: ROM ids, firmware ids and RomM file ids are different number spaces,
 * so a download is identified by its kind and id, never by `rom_id` alone.
 */

import type { DownloadKind } from "../types";

interface HasIdentity {
  kind?: DownloadKind;
  rom_id?: number;
  firmware_id?: number;
  file_id?: number;
}

export function isFirmware(d: HasIdentity): boolean {
  return d.kind === "firmware";
}

/** A single file of an installed multi-file game (an update or DLC), not a game install. */
export function isGameFile(d: HasIdentity): boolean {
  return d.kind === "rom_file";
}

/** Whether the download installs a ROM (frames from before kinds existed are ROMs). */
export function isRom(d: HasIdentity): boolean {
  return (d.kind ?? "rom") === "rom";
}

function nonRomId(d: HasIdentity): number {
  return (isFirmware(d) ? d.firmware_id : d.file_id) as number;
}

/** `rom:7` / `firmware:3` / `rom_file:302`; frames from before kinds existed are ROMs. */
export function downloadKey(d: HasIdentity): string {
  return isRom(d) ? `rom:${d.rom_id}` : `${d.kind}:${nonRomId(d)}`;
}

/** The id the download callables take for this download, and the kind argument (none for a ROM). */
export function callArgs(d: HasIdentity): [id: number, kind?: DownloadKind] {
  return isRom(d) ? [d.rom_id as number] : [nonRomId(d), d.kind as DownloadKind];
}
