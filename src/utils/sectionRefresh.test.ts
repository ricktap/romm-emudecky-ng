import { describe, it, expect, vi, beforeEach } from "vitest";
import type { Dispatch, SetStateAction } from "react";
import {
  refreshActiveSlotInBackground,
  refreshCoreInfoInBackground,
} from "./sectionRefresh";
import * as backend from "../api/backend";
import { libretroEmu } from "../test-utils/coreFixtures";
import type { EmulatorOption } from "../types";

interface ActiveSlotState {
  activeSlot: string | null;
  unrelated: number;
}

interface CoreState {
  activeCoreLabel: string | null;
  activeCoreIsDefault: boolean;
  emulators: EmulatorOption[];
  emulatorDataAvailable: boolean;
  platformCoreLabel: string | null;
  hasGameOverride: boolean;
  unrelated: string;
}

const flushMicrotasks = () => new Promise((resolve) => setTimeout(resolve, 0));

/**
 * Build a promise that resolves only when the returned `resolve` is called.
 * Lets a test simulate the "long-running fetch" window (e.g. the 5s
 * `timeoutMs` race inside `getPlatformCoreInfo`) and flip a `cancelled` flag
 * mid-await — proving the helper re-reads the closure after each await
 * instead of capturing a stale boolean snapshot.
 */
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void; reject: (e: unknown) => void } {
  let resolve!: (value: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("refreshActiveSlotInBackground", () => {
  beforeEach(() => vi.restoreAllMocks());

  it("applies active_slot to the setter when not cancelled", async () => {
    vi.mocked(backend.getSaveStatus).mockResolvedValueOnce({
      active_slot: "slot-2",
    } as unknown as Awaited<ReturnType<typeof backend.getSaveStatus>>);

    const setter = vi.fn<(updater: (prev: ActiveSlotState) => ActiveSlotState) => void>();
    refreshActiveSlotInBackground(1, () => false, setter as unknown as Dispatch<SetStateAction<ActiveSlotState>>);
    await flushMicrotasks();

    expect(setter).toHaveBeenCalledOnce();
    const updater = setter.mock.calls[0]![0];
    expect(updater({ activeSlot: null, unrelated: 7 })).toEqual({
      activeSlot: "slot-2",
      unrelated: 7,
    });
  });

  it("skips the setter when cancelled", async () => {
    vi.mocked(backend.getSaveStatus).mockResolvedValueOnce({
      active_slot: "slot-2",
    } as unknown as Awaited<ReturnType<typeof backend.getSaveStatus>>);

    const setter = vi.fn();
    refreshActiveSlotInBackground(1, () => true, setter);
    await flushMicrotasks();

    expect(setter).not.toHaveBeenCalled();
  });

  it("re-reads the cancelled closure after the await (cancelled mid-await)", async () => {
    const d = deferred<{ active_slot: string }>();
    vi.mocked(backend.getSaveStatus).mockReturnValueOnce(
      d.promise as unknown as ReturnType<typeof backend.getSaveStatus>,
    );

    let cancelled = false;
    const setter = vi.fn();
    refreshActiveSlotInBackground(1, () => cancelled, setter);

    // Cancel before the backend call resolves — proves we re-read the closure.
    cancelled = true;
    d.resolve({ active_slot: "slot-2" });
    await flushMicrotasks();

    expect(setter).not.toHaveBeenCalled();
  });

  it("swallows backend errors without invoking the setter", async () => {
    vi.mocked(backend.getSaveStatus).mockRejectedValueOnce(new Error("network"));
    const setter = vi.fn();
    refreshActiveSlotInBackground(1, () => false, setter);
    await flushMicrotasks();
    // active-slot's `.catch` has no observable side effect beyond skipping the
    // setter; asserting the setter never fires is the post-catch state.
    expect(setter).not.toHaveBeenCalled();
  });

  it("falls back to null when active_slot is missing", async () => {
    vi.mocked(backend.getSaveStatus).mockResolvedValueOnce({
      active_slot: null,
    } as unknown as Awaited<ReturnType<typeof backend.getSaveStatus>>);

    const setter = vi.fn<(updater: (prev: ActiveSlotState) => ActiveSlotState) => void>();
    refreshActiveSlotInBackground(1, () => false, setter as unknown as Dispatch<SetStateAction<ActiveSlotState>>);
    await flushMicrotasks();

    const updater = setter.mock.calls[0]![0];
    expect(updater({ activeSlot: "x", unrelated: 1 })).toEqual({
      activeSlot: null,
      unrelated: 1,
    });
  });
});

describe("refreshCoreInfoInBackground", () => {
  beforeEach(() => vi.restoreAllMocks());

  it("keys on rom_id and flows a non-default per-game core back (gold icon, #945)", async () => {
    vi.mocked(backend.getPlatformCoreInfo).mockResolvedValueOnce({
      active_core: "parallel_n64_libretro.so",
      active_core_label: "ParaLLEl N64",
      platform_core_label: null,
      has_game_override: false,
      emulator_data_available: true,
      emulators: [
        libretroEmu("mupen64plus_next_libretro.so", "Mupen64Plus-Next", true),
        libretroEmu("parallel_n64_libretro.so", "ParaLLEl N64"),
      ],
    });

    const setter = vi.fn<(updater: (prev: CoreState) => CoreState) => void>();
    refreshCoreInfoInBackground(404, () => false, setter as unknown as Dispatch<SetStateAction<CoreState>>);
    await flushMicrotasks();

    // Keyed on rom_id so the active core reflects a per-game DB override
    // (epic #945).
    expect(backend.getPlatformCoreInfo).toHaveBeenCalledWith(404);
    expect(setter).toHaveBeenCalledOnce();
    const next = setter.mock.calls[0]![0]({
      activeCoreLabel: null,
      activeCoreIsDefault: true,
      emulators: [],
      emulatorDataAvailable: true,
      platformCoreLabel: null,
      hasGameOverride: false,
      unrelated: "keep",
    });
    expect(next.activeCoreLabel).toBe("ParaLLEl N64");
    // Active core differs from the default → not default.
    expect(next.activeCoreIsDefault).toBe(false);
    expect(next.emulators).toHaveLength(2);
    expect(next.unrelated).toBe("keep");
  });

  it("skips the setter when cancelled at call time", async () => {
    vi.mocked(backend.getPlatformCoreInfo).mockResolvedValueOnce({
      active_core: null,
      active_core_label: null,
      platform_core_label: null,
      has_game_override: false,
      emulator_data_available: true,
      emulators: [],
    });
    const setter = vi.fn();
    refreshCoreInfoInBackground(404, () => true, setter);
    await flushMicrotasks();
    expect(setter).not.toHaveBeenCalled();
  });

  it("re-reads the cancelled closure after the await", async () => {
    const d = deferred<Awaited<ReturnType<typeof backend.getPlatformCoreInfo>>>();
    vi.mocked(backend.getPlatformCoreInfo).mockReturnValueOnce(
      d.promise as unknown as ReturnType<typeof backend.getPlatformCoreInfo>,
    );

    let cancelled = false;
    const setter = vi.fn();
    refreshCoreInfoInBackground(404, () => cancelled, setter);

    cancelled = true;
    d.resolve({
      active_core: null,
      active_core_label: null,
      platform_core_label: null,
      has_game_override: false,
      emulator_data_available: true,
      emulators: [],
    });
    await flushMicrotasks();

    expect(setter).not.toHaveBeenCalled();
  });

  it("logs the error and skips the setter when the fetch rejects", async () => {
    vi.mocked(backend.getPlatformCoreInfo).mockRejectedValueOnce(new Error("network"));
    vi.mocked(backend.debugLog).mockResolvedValue(undefined);
    const setter = vi.fn();
    refreshCoreInfoInBackground(404, () => false, setter);
    await flushMicrotasks();

    expect(setter).not.toHaveBeenCalled();
    // Non-vacuous catch assertion: the `.catch` calls debugLog with the error.
    expect(vi.mocked(backend.debugLog)).toHaveBeenCalledWith(
      expect.stringContaining("Background core info fetch error"),
    );
  });
});
