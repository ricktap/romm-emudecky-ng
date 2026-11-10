/**
 * Pin or clear a game's emulator override and confirm it reached the Steam shortcut. Shared by the BIOS & Core tab;
 * mirrors the play section's core menu: the backend persists the pin and re-bakes the launch options, the frontend
 * confirm-sets them on the shortcut BEFORE claiming success (an unconfirmed bake gets a distinct "restart Steam"
 * toast, and the pin is kept: the next re-sync re-bakes from it).
 */

import { toaster } from "@decky/api";
import { clearGameCore, setGameCore, invalidateCachedGameDetail } from "../api/backend";
import { setLaunchOptionsConfirmed } from "./steamShortcuts";

export type CoreChange = { kind: "pin"; label: string } | { kind: "clear" };

const toast = (body: string) => toaster.toast({ title: "EmuDecky RomM Manager", body });

/** Returns true when the change took effect (and sibling components were told through `core_changed`). */
export async function changeGameCore(
  appId: number,
  romId: number,
  platformSlug: string,
  change: CoreChange,
): Promise<boolean> {
  const failure = change.kind === "pin" ? "Failed to set core" : "Failed to reset core";
  try {
    const result = change.kind === "pin" ? await setGameCore(romId, change.label) : await clearGameCore(romId);
    if (!result.success) {
      toast(result.message || failure);
      return false;
    }
    if (result.launch_options !== undefined && result.app_id != null) {
      const confirmed = await setLaunchOptionsConfirmed(result.app_id, result.launch_options);
      if (!confirmed) {
        toast("Core saved — restart Steam to apply");
        return false;
      }
    }
    toast(change.kind === "pin" ? `Core set to ${change.label}` : "Now following the system core");
    invalidateCachedGameDetail(appId);
    globalThis.dispatchEvent(new CustomEvent("romm_data_changed", { detail: { type: "core_changed", platform_slug: platformSlug } }));
    return true;
  } catch {
    toast(failure);
    return false;
  }
}
