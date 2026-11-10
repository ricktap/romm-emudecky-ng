/**
 * What the game page's readiness strip says: three chips (ROM, BIOS, Saves), each with a title, a one-line detail,
 * a tone and an optional one-tap fix. Pure: the component feeds it data, this decides the words.
 */

import type { DownloadItem, FirmwareStatus } from "../types";
import { interruptedLabel } from "./downloadStatus";
import { missingRequiredKeys } from "./missingKeys";
import { CHOOSE_PROFILE_LABEL, CHOOSE_SAVE_LABEL } from "../types/saves";

export type ChipTone = "ok" | "warn" | "neutral";
export type ChipFix = "get_bios" | "resolve_conflict" | "choose_save";

export interface ReadinessChip {
  id: "rom" | "bios" | "saves";
  title: string;
  detail: string;
  tone: ChipTone;
  /** The action the chip offers; its label is `fixLabel`. The primary button already handles the ROM. */
  fix?: ChipFix;
  fixLabel?: string;
}

export interface RomChipInput {
  installed: boolean;
  /** File name of the installed ROM, shown as the detail. */
  fileName?: string | null | undefined;
  /** The ROM's entry in the download queue, if any. */
  download?: Pick<DownloadItem, "status" | "bytes_downloaded" | "total_bytes" | "reason"> | null | undefined;
}

export function romChip({ installed, fileName, download }: RomChipInput): ReadinessChip {
  if (download) {
    const pct = download.total_bytes > 0 ? Math.floor((download.bytes_downloaded / download.total_bytes) * 100) : 0;
    switch (download.status) {
      case "queued":
        return { id: "rom", title: "Queued", detail: "Waiting for the transfer slot", tone: "neutral" };
      case "downloading":
        return { id: "rom", title: "Downloading", detail: `${pct}%`, tone: "neutral" };
      case "extracting":
        return { id: "rom", title: "Extracting", detail: "Unpacking the download", tone: "neutral" };
      case "paused":
        return { id: "rom", title: "Download paused", detail: `At ${pct}%`, tone: "neutral" };
      case "interrupted":
        return { id: "rom", title: "Download interrupted", detail: interruptedLabel(download.reason).replace("Interrupted — ", ""), tone: "warn" };
    }
  }
  if (installed) return { id: "rom", title: "ROM installed", detail: fileName ?? "Ready to play", tone: "ok" };
  return { id: "rom", title: "Not downloaded", detail: "On RomM", tone: "neutral" };
}

export interface BiosChipInput {
  platformSlug: string;
  /** `get_firmware_status` result; `null` until it arrived or when it could not be read. */
  firmware: FirmwareStatus | null;
  activeCoreLabel: string | null;
}

/** Only the Switch has BIOS files today (firmware + keys); every other platform needs none until 0.80. */
export function biosChip({ platformSlug, firmware, activeCoreLabel }: BiosChipInput): ReadinessChip {
  if (platformSlug !== "switch") {
    return { id: "bios", title: "No BIOS needed", detail: activeCoreLabel ?? "", tone: "ok" };
  }
  if (!firmware?.success) return { id: "bios", title: "BIOS status unknown", detail: "Could not read the keys", tone: "neutral" };
  const missing = missingRequiredKeys(firmware);
  if (missing.length > 0) {
    return { id: "bios", title: "BIOS missing", detail: `${missing.join(", ")} on RomM`, tone: "warn", fix: "get_bios", fixLabel: "Get" };
  }
  return { id: "bios", title: "BIOS ready", detail: activeCoreLabel ?? "Keys installed", tone: "ok" };
}

export interface SavesChipInput {
  saveSyncEnabled: boolean;
  status: "synced" | "conflict" | "none" | null;
  label: string;
}

export function savesChip({ saveSyncEnabled, status, label }: SavesChipInput): ReadinessChip {
  if (!saveSyncEnabled) return { id: "saves", title: "Save sync off", detail: "Saves stay on this device", tone: "neutral" };
  if (status === "conflict") {
    return { id: "saves", title: "Save conflict", detail: "This device and RomM both changed", tone: "warn", fix: "resolve_conflict", fixLabel: "Resolve" };
  }
  // The backend cannot locate this game's save yet: one tap opens the chooser.
  if (label === CHOOSE_SAVE_LABEL || label === CHOOSE_PROFILE_LABEL) {
    return { id: "saves", title: "Save not found", detail: label, tone: "warn", fix: "choose_save", fixLabel: "Choose" };
  }
  if (status === "synced") return { id: "saves", title: "Saves synced", detail: label, tone: "ok" };
  return { id: "saves", title: "Save sync on", detail: label || "Uploads after the first session", tone: "ok" };
}
