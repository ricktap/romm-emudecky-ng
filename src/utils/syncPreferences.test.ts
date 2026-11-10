import { afterEach, describe, expect, it, vi } from "vitest";
import { getSkipPreview, onSyncPreferencesChange, setSkipPreview } from "./syncPreferences";

describe("syncPreferences", () => {
  afterEach(() => {
    setSkipPreview(false);
    vi.restoreAllMocks();
  });

  it("defaults to showing the preview", () => {
    expect(getSkipPreview()).toBe(false);
  });

  it("keeps the choice and tells subscribers", () => {
    const seen: boolean[] = [];
    const unsub = onSyncPreferencesChange(() => seen.push(getSkipPreview()));
    setSkipPreview(true);
    setSkipPreview(false);
    unsub();
    setSkipPreview(true);
    expect(seen).toEqual([true, false]);
    expect(getSkipPreview()).toBe(true);
  });

  it("falls back to the default when storage throws, and still notifies", () => {
    const spy = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    const fn = vi.fn();
    const unsub = onSyncPreferencesChange(fn);
    expect(getSkipPreview()).toBe(false);
    expect(() => setSkipPreview(true)).not.toThrow();
    expect(fn).toHaveBeenCalledTimes(1);
    unsub();
    spy.mockRestore();
  });
});
