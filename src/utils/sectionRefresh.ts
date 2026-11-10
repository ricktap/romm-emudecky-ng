/**
 * Fire-and-forget background refresh helpers for the play-section row.
 *
 * Each helper hits a single backend callable, merges the response into the
 * caller's state via a typed setter, and swallows errors (logging where it's
 * useful for debugging). Generic over the consumer's state shape so the
 * helpers stay decoupled from any particular component's full state.
 */

import type { Dispatch, SetStateAction } from "react";
import { getSaveStatus, getPlatformCoreInfo, debugLog } from "../api/backend";
import { extractCoreInfo, type CoreInfoFields } from "./playSection";

interface ActiveSlotFields {
  activeSlot: string | null;
}

export function refreshActiveSlotInBackground<S extends ActiveSlotFields>(
  romId: number,
  cancelled: () => boolean,
  setter: Dispatch<SetStateAction<S>>,
): void {
  getSaveStatus(romId)
    .then((saveStatus) => {
      if (!cancelled() && "active_slot" in saveStatus) {
        setter((prev) => ({ ...prev, activeSlot: saveStatus.active_slot ?? null }));
      }
    })
    .catch(() => {});
}

/** Refresh core-selection state from the dedicated `get_platform_core_info`
 *  path (#923). Keyed on the rom_id so the
 *  active core reflects a per-game DB override (epic #945) when one is pinned. */
export function refreshCoreInfoInBackground<S extends CoreInfoFields>(
  romId: number,
  cancelled: () => boolean,
  setter: Dispatch<SetStateAction<S>>,
): void {
  getPlatformCoreInfo(romId)
    .then((coreInfo) => {
      if (!cancelled()) {
        setter((prev) => ({
          ...prev,
          ...extractCoreInfo(coreInfo),
        }));
      }
    })
    .catch((e) => debugLog(`Background core info fetch error: ${e}`));
}
