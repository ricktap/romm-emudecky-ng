import { callable, toaster } from "@decky/api";
import { NOT_IMPLEMENTED_REASON } from "../types/saves";

/**
 * Callables whose Rust port is still pending (save sync: 0.55.0).
 * The UI for them stays visible; the backend answers `reason: "not_implemented"` and this wrapper
 * turns that into a toast + a normal failure.
 *
 * The Python implementation (removed in rewrite step 6) lives at the commit noted
 * in HANDOFF.md, "Python reference implementation".
 */
export { NOT_IMPLEMENTED_REASON };

type Feature = "saves";

const FEATURE_MESSAGES: Record<Feature, string> = {
  saves: "Save sync is not implemented yet (planned for 0.55.0).",
};

/** Minimum gap between two toasts for the same feature, so a burst of calls shows one. */
export const TOAST_THROTTLE_MS = 5000;

/**
 * How a not-implemented answer reaches the caller:
 * - `silent`: passive reads the UI fires on its own (page render, game launch). No toast.
 * - `throws`: the declared result is status-shaped (no `success` field), so a failure object would be
 *   misread; reject instead, which those callers already handle in their catch paths.
 * Everything else toasts and resolves to `{ success: false, reason: "not_implemented", message }`.
 */
interface Entry {
  feature: Feature;
  silent?: boolean;
  throws?: boolean;
}

const UNPORTED: Record<string, Entry> = {
  // save sync — passive reads (rejecting, like any unreachable backend call; callers catch it)
  ensure_device_registered: { feature: "saves", silent: true, throws: true },
  list_devices: { feature: "saves", silent: true, throws: true },
  get_save_status: { feature: "saves", silent: true, throws: true },
  refresh_save_status: { feature: "saves", silent: true, throws: true },
  // Resolves to a failure result with reason `not_implemented`; the launch paths treat that as a benign
  // skip (isBenignSyncSkip) so a game without save sync launches without a warning.
  pre_launch_sync: { feature: "saves", silent: true },
  get_save_sync_settings: { feature: "saves", silent: true, throws: true },
  is_save_tracking_configured: { feature: "saves", silent: true, throws: true },
  get_save_setup_info: { feature: "saves", silent: true, throws: true },
  // save sync — user actions
  update_save_sync_settings: { feature: "saves" },
  sync_rom_saves: { feature: "saves" },
  sync_all_saves: { feature: "saves" },
  resolve_sync_conflict: { feature: "saves" },
  get_save_slots: { feature: "saves" },
  get_slot_saves: { feature: "saves" },
  switch_slot: { feature: "saves" },
  get_slot_delete_info: { feature: "saves" },
  delete_slot: { feature: "saves" },
  confirm_slot_choice: { feature: "saves" },
  delete_local_saves: { feature: "saves" },
  delete_platform_saves: { feature: "saves" },
  saves_list_file_versions: { feature: "saves", throws: true },
  saves_rollback_to_version: { feature: "saves", throws: true },
};

/** Names routed through {@link unportedCallable}; the test pins this against the backend manifest. */
export const UNPORTED_CALLABLE_NAMES: readonly string[] = Object.keys(UNPORTED);

const lastToastAt = new Map<Feature, number>();

/** Forget the toast throttle (tests). */
export function resetNotImplementedToasts(): void {
  lastToastAt.clear();
}

function isNotImplemented(result: unknown): boolean {
  return typeof result === "object" && result !== null && (result as { reason?: unknown }).reason === NOT_IMPLEMENTED_REASON;
}

function toastOnce(feature: Feature): void {
  const now = Date.now();
  const last = lastToastAt.get(feature);
  if (last !== undefined && now - last < TOAST_THROTTLE_MS) return;
  lastToastAt.set(feature, now);
  toaster.toast({ title: "EmuDecky RomM Manager", body: FEATURE_MESSAGES[feature] });
}

/**
 * Like `callable()`, for a backend method that is not ported yet. The backend answers those with
 * `{ success: false, reason: "not_implemented" }` (a result, because Decky reduces a thrown backend
 * error to an opaque "Python Exception"). This swaps in the user-facing message and, unless the call
 * is a passive read, shows a throttled toast. See {@link Entry} for how the answer reaches the caller.
 */
export function unportedCallable<Args extends unknown[], Result>(name: string): (...args: Args) => Promise<Result> {
  const entry = UNPORTED[name];
  if (!entry) throw new Error(`${name} is not registered in UNPORTED (src/api/unportedCallable.ts)`);
  const call = callable<Args, Result>(name);
  return async (...args: Args): Promise<Result> => {
    const result = await call(...args);
    if (!isNotImplemented(result)) return result;
    if (!entry.silent) toastOnce(entry.feature);
    const message = FEATURE_MESSAGES[entry.feature];
    if (entry.throws) throw new Error(message);
    return { success: false, reason: NOT_IMPLEMENTED_REASON, message } as unknown as Result;
  };
}
