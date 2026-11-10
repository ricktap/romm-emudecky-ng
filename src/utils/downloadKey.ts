/**
 * Identity of a download in the shared queue: a ROM id and a firmware id are different number spaces, so a
 * download is identified by its kind and id, never by `rom_id` alone.
 */

import type { DownloadKind } from "../types";

interface HasIdentity {
  kind?: DownloadKind;
  rom_id?: number;
  firmware_id?: number;
}

export function isFirmware(d: HasIdentity): boolean {
  return d.kind === "firmware";
}

/** `rom:7` / `firmware:3`; frames from before kinds existed are ROMs. */
export function downloadKey(d: HasIdentity): string {
  return isFirmware(d) ? `firmware:${d.firmware_id}` : `rom:${d.rom_id}`;
}

/** The id the download callables take for this download, and the kind argument (none for a ROM). */
export function callArgs(d: HasIdentity): [id: number, kind?: DownloadKind] {
  return isFirmware(d) ? [d.firmware_id as number, "firmware"] : [d.rom_id as number];
}
