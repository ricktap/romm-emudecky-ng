/**
 * Pure helpers for the RomM play-section row: label resolution, core-info
 * shaping, and the timeout-promise primitive used by connection probing.
 *
 * Anything that takes inputs and returns outputs without touching component
 * state belongs here. Anything that talks to the backend belongs in
 * sectionRefresh.ts. Anything stateful belongs in the component itself.
 */

import type { CoreInfo, EmulatorOption, SaveStatus, SaveSyncDisplay } from "../types";
import { hasAnySaveConflict } from "./saveStatus";
import { formatTimeAgo } from "./formatters";

/** Core-selection fields for the play-section row, derived from the dedicated
 *  `get_platform_core_info` path (#923). */
export interface CoreInfoFields {
  activeCoreLabel: string | null;
  activeCoreIsDefault: boolean;
  emulators: EmulatorOption[];
  emulatorDataAvailable: boolean;
  platformCoreLabel: string | null;
  hasGameOverride: boolean;
  gameOverrideLabel?: string | null;
}

export interface SaveSyncResolution {
  status: "synced" | "conflict" | "none";
  label: string;
}

/** Resolve the human-readable save-sync label from the backend's typed display
 *  payload. Backend ships a static `label` for every case except
 *  `synced + has-recent-check`, where it leaves `label` null and passes
 *  `last_sync_check_at` through for time-ago formatting at render time. */
export function resolveSaveSyncLabel(display: SaveSyncDisplay): string {
  if (display.label !== null) return display.label;
  if (display.last_sync_check_at) {
    return formatTimeAgo(display.last_sync_check_at) ?? "Not synced";
  }
  return "Not synced";
}

/** Map a SaveSyncDisplay (typed display payload) to a status+label pair.
 *  Defensive fallback handles a SaveStatus missing the pre-computed display —
 *  should not occur in current callers, kept conservative. */
export function applySaveSyncDisplay(
  display: SaveSyncDisplay | undefined,
  saveStatus: SaveStatus | null,
): SaveSyncResolution {
  if (display) {
    return { status: display.status, label: resolveSaveSyncLabel(display) };
  }
  if (hasAnySaveConflict(saveStatus)) return { status: "conflict", label: "Conflict" };
  return { status: "none", label: "No saves" };
}

/** Project a CoreInfo response (from the dedicated `get_platform_core_info`
 *  path, #923) into the core-selection fields the play-section row needs. The
 *  active core is "default" when it equals the default emulator or no override
 *  is set. */
export function extractCoreInfo(coreInfo: CoreInfo): CoreInfoFields {
  const activeCoreLabel = coreInfo.active_core_label ?? null;
  const emulators = coreInfo.emulators;
  const defaultEmulator = emulators.find((e) => e.is_default);
  const activeCoreIsDefault = !activeCoreLabel || activeCoreLabel === defaultEmulator?.label;
  return {
    activeCoreLabel,
    activeCoreIsDefault,
    emulators,
    emulatorDataAvailable: coreInfo.emulator_data_available,
    platformCoreLabel: coreInfo.platform_core_label ?? null,
    hasGameOverride: coreInfo.has_game_override,
    gameOverrideLabel: coreInfo.game_override_label ?? null,
  };
}

/** Promise that rejects after `ms` milliseconds. Pair with `Promise.race` to
 *  enforce a timeout on an otherwise unbounded async call. */
export function timeoutMs(ms: number): Promise<never> {
  return new Promise<never>((_, reject) => setTimeout(() => reject(new Error("timeout")), ms));
}
