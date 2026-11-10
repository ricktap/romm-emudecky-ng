import { readFileSync } from "node:fs";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { call, toast } = vi.hoisted(() => ({ call: vi.fn(), toast: vi.fn() }));

vi.mock("@decky/api", () => ({
  callable: () => call,
  toaster: { toast },
}));

// The global test setup stubs this module; this file tests the real one.
const { NOT_IMPLEMENTED_REASON, TOAST_THROTTLE_MS, UNPORTED_CALLABLE_NAMES, resetNotImplementedToasts, unportedCallable } =
  await vi.importActual<typeof import("./unportedCallable")>("./unportedCallable");

const NOT_IMPLEMENTED = { success: false, reason: "not_implemented", message: "not implemented in the Rust backend yet: x" };

describe("unportedCallable", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.useRealTimers();
    resetNotImplementedToasts();
  });

  it("passes a real result through untouched", async () => {
    call.mockResolvedValue({ success: true, synced: 2 });
    const result = await unportedCallable("sync_rom_saves")(7);
    expect(result).toEqual({ success: true, synced: 2 });
    expect(call).toHaveBeenCalledWith(7);
    expect(toast).not.toHaveBeenCalled();
  });

  it("passes other failures through untouched, without a toast", async () => {
    const failure = { success: false, reason: "server_unreachable", message: "down" };
    call.mockResolvedValue(failure);
    expect(await unportedCallable("sync_rom_saves")(7)).toEqual(failure);
    expect(toast).not.toHaveBeenCalled();
  });

  it("toasts and resolves to a friendly failure when the user acts on an unported feature", async () => {
    call.mockResolvedValue(NOT_IMPLEMENTED);
    const result = await unportedCallable("sync_rom_saves")(7);
    expect(result).toEqual({
      success: false,
      reason: NOT_IMPLEMENTED_REASON,
      message: "Save sync is not implemented yet (planned for 0.55.0).",
    });
    expect(toast).toHaveBeenCalledTimes(1);
    expect(toast.mock.calls[0][0].body).toContain("Save sync is not implemented yet");
  });

  it("names firmware separately from save sync", async () => {
    call.mockResolvedValue(NOT_IMPLEMENTED);
    await unportedCallable("download_all_firmware")("switch");
    expect(toast.mock.calls[0][0].body).toContain("Firmware management is not implemented yet");
  });

  it("resolves the pre-launch sync to a not_implemented failure without a toast: launches treat it as a benign skip", async () => {
    call.mockResolvedValue(NOT_IMPLEMENTED);
    const result = await unportedCallable("pre_launch_sync")(7);
    expect(result).toMatchObject({ success: false, reason: NOT_IMPLEMENTED_REASON });
    expect(toast).not.toHaveBeenCalled();
  });

  it("does not toast for passive reads: it rejects, as callers already handle", async () => {
    call.mockResolvedValue(NOT_IMPLEMENTED);
    await expect(unportedCallable("get_save_status")(7)).rejects.toThrow("Save sync is not implemented yet");
    expect(toast).not.toHaveBeenCalled();
  });

  it("toasts, then rejects, for status-shaped user actions", async () => {
    call.mockResolvedValue(NOT_IMPLEMENTED);
    await expect(unportedCallable("saves_rollback_to_version")(1, "a", 2)).rejects.toThrow("not implemented yet");
    expect(toast).toHaveBeenCalledTimes(1);
  });

  it("shows one toast per feature inside the throttle window, then another after it", async () => {
    vi.useFakeTimers();
    call.mockResolvedValue(NOT_IMPLEMENTED);
    const sync = unportedCallable("sync_rom_saves");
    await sync(1);
    await sync(2);
    await unportedCallable("switch_slot")(1, "a");
    expect(toast).toHaveBeenCalledTimes(1);
    await unportedCallable("download_all_firmware")("switch");
    expect(toast).toHaveBeenCalledTimes(2);
    vi.advanceTimersByTime(TOAST_THROTTLE_MS + 1);
    await sync(3);
    expect(toast).toHaveBeenCalledTimes(3);
  });

  it("propagates a real transport failure unchanged", async () => {
    call.mockRejectedValue(new Error("socket closed"));
    await expect(unportedCallable("sync_rom_saves")(1)).rejects.toThrow("socket closed");
    expect(toast).not.toHaveBeenCalled();
  });

  it("rejects a name that is not registered", () => {
    expect(() => unportedCallable("get_platforms")).toThrow(/not registered/);
  });
});

describe("the registered names", () => {
  it("are exactly the callables the Rust backend has not ported", () => {
    const root = `${process.cwd()}/`; // vitest runs from the repo root
    const all: string[] = JSON.parse(readFileSync(`${root}backend/callables.json`, "utf8"));
    const app = readFileSync(`${root}backend/src/app.rs`, "utf8");
    const block = app.slice(app.indexOf("PORTED_CALLABLES"));
    const ported = new Set([...block.slice(0, block.indexOf("];")).matchAll(/"([a-z_0-9]+)"/g)].map((m) => m[1]));
    const unported = all.filter((name) => !ported.has(name)).sort();
    // When a callable is ported, switch it from unportedCallable() back to callable()
    // in src/api/backend.ts and drop it from UNPORTED.
    expect([...UNPORTED_CALLABLE_NAMES].sort()).toEqual(unported);
  });
});
