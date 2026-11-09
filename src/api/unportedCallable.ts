import { callable, toaster } from "@decky/api";

/**
 * Callables whose Rust port is still pending (firmware: 0.45.0, save sync: 0.55.0).
 * The UI for them stays visible; the backend answers "not implemented in the Rust
 * backend yet" and this wrapper turns that into a toast + a normal failure result.
 *
 * The Python implementation (removed in rewrite step 6) lives at the commit noted
 * in HANDOFF.md, "Python reference implementation".
 */
export const NOT_IMPLEMENTED_MARKER = "not implemented in the Rust backend yet";
export const NOT_IMPLEMENTED_REASON = "not_implemented";

type Feature = "firmware" | "saves";

const FEATURE_MESSAGES: Record<Feature, string> = {
  firmware: "Firmware management is not implemented yet (planned for 0.45.0).",
  saves: "Save sync is not implemented yet (planned for 0.55.0).",
};

/** Minimum gap between two toasts for the same feature, so a burst of calls shows one. */
export const TOAST_THROTTLE_MS = 5000;

/**
 * `silent` callables are passive reads the UI fires on its own (page render, game
 * launch). They get the failure result without a toast; only calls the user
 * triggers by acting on the feature toast.
 */
const UNPORTED: Record<string, { feature: Feature; silent: boolean }> = {
  // firmware
  get_firmware_status: { feature: "firmware", silent: true },
  download_all_firmware: { feature: "firmware", silent: false },
  download_required_firmware: { feature: "firmware", silent: false },
  // save sync — passive reads
  ensure_device_registered: { feature: "saves", silent: true },
  list_devices: { feature: "saves", silent: true },
  get_save_status: { feature: "saves", silent: true },
  refresh_save_status: { feature: "saves", silent: true },
  pre_launch_sync: { feature: "saves", silent: true },
  get_save_sync_settings: { feature: "saves", silent: true },
  is_save_tracking_configured: { feature: "saves", silent: true },
  get_save_setup_info: { feature: "saves", silent: true },
  // save sync — user actions
  update_save_sync_settings: { feature: "saves", silent: false },
  sync_rom_saves: { feature: "saves", silent: false },
  sync_all_saves: { feature: "saves", silent: false },
  resolve_sync_conflict: { feature: "saves", silent: false },
  get_save_slots: { feature: "saves", silent: false },
  get_slot_saves: { feature: "saves", silent: false },
  switch_slot: { feature: "saves", silent: false },
  get_slot_delete_info: { feature: "saves", silent: false },
  delete_slot: { feature: "saves", silent: false },
  confirm_slot_choice: { feature: "saves", silent: false },
  delete_local_saves: { feature: "saves", silent: false },
  delete_platform_saves: { feature: "saves", silent: false },
  saves_list_file_versions: { feature: "saves", silent: false },
  saves_rollback_to_version: { feature: "saves", silent: false },
};

/** Names routed through {@link unportedCallable}; the test pins this against the backend manifest. */
export const UNPORTED_CALLABLE_NAMES: readonly string[] = Object.keys(UNPORTED);

const lastToastAt = new Map<Feature, number>();

/** Forget the toast throttle (tests). */
export function resetNotImplementedToasts(): void {
  lastToastAt.clear();
}

function isNotImplemented(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  return message.includes(NOT_IMPLEMENTED_MARKER);
}

function toastOnce(feature: Feature): void {
  const now = Date.now();
  const last = lastToastAt.get(feature);
  if (last !== undefined && now - last < TOAST_THROTTLE_MS) return;
  lastToastAt.set(feature, now);
  toaster.toast({ title: "EmuDecky RomM Manager", body: FEATURE_MESSAGES[feature] });
}

/**
 * Like `callable()`, for a backend method that is not ported yet. A "not implemented"
 * rejection becomes `{ success: false, reason: "not_implemented", message }` (cast to
 * the callable's declared result, since callers already handle failure results) and
 * a throttled toast; any other rejection propagates unchanged.
 */
export function unportedCallable<Args extends unknown[], Result>(name: string): (...args: Args) => Promise<Result> {
  const entry = UNPORTED[name];
  if (!entry) throw new Error(`${name} is not registered in UNPORTED (src/api/unportedCallable.ts)`);
  const call = callable<Args, Result>(name);
  return async (...args: Args): Promise<Result> => {
    try {
      return await call(...args);
    } catch (error) {
      if (!isNotImplemented(error)) throw error;
      if (!entry.silent) toastOnce(entry.feature);
      return {
        success: false,
        reason: NOT_IMPLEMENTED_REASON,
        message: FEATURE_MESSAGES[entry.feature],
      } as unknown as Result;
    }
  };
}
