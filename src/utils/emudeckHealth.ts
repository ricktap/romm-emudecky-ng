/**
 * EmuDeck health for the banners: read when a surface mounts and again every 15 s, so putting the SD card back (or
 * pulling it out) shows up without reopening anything. `null` until the first answer; a failed read keeps the last one.
 */

import { useEffect, useState } from "react";
import { getEmudeckHealth } from "../api/backend";
import type { EmuDeckHealth, EmuDeckHealthInfo } from "../types";

export const EMUDECK_HEALTH_POLL_MS = 15000;

const TITLES: Record<Exclude<EmuDeckHealth, "ok">, string> = {
  absent: "EmuDeck not set up",
  unreadable: "EmuDeck settings unreadable",
  invalid: "EmuDeck has no storage location",
  root_missing: "EmuDeck storage not available",
};

/** The banner title of an unhealthy state. */
export function emudeckTitle(health: EmuDeckHealth): string {
  return health === "ok" ? "EmuDeck is set up" : TITLES[health];
}

/** The banner line of a moved storage location, or `null` when it did not move. */
export function movedSummary(info: EmuDeckHealthInfo | null): string | null {
  if (!info?.moved) return null;
  const missing = info.missing ?? 0;
  const installed = info.installed ?? 0;
  if (installed === 0) return "The storage location changed. Nothing was installed yet.";
  if (missing === 0) return `The storage location changed. All ${installed} installed games were found.`;
  return `The storage location changed. ${missing} of ${installed} installed games were not found.`;
}

export function useEmudeckHealth(): EmuDeckHealthInfo | null {
  const [info, setInfo] = useState<EmuDeckHealthInfo | null>(null);
  useEffect(() => {
    let alive = true;
    const read = () => {
      Promise.resolve()
        .then(() => getEmudeckHealth())
        .then((r) => alive && r?.health && setInfo(r))
        .catch(() => undefined);
    };
    read();
    const poll = setInterval(read, EMUDECK_HEALTH_POLL_MS);
    return () => {
      alive = false;
      clearInterval(poll);
    };
  }, []);
  return info;
}
