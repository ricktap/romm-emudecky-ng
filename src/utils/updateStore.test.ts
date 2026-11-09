import { describe, it, expect, beforeEach, vi } from "vitest";
import { checkForUpdate } from "../api/backend";
import type { UpdateCheckSuccess } from "../types";
import { getUpdateState, onUpdateStateChange, refreshUpdateState, resetUpdateState } from "./updateStore";

function updateInfo(overrides: Partial<UpdateCheckSuccess> = {}): UpdateCheckSuccess {
  return {
    success: true,
    current_version: "0.32.0",
    latest_version: "0.33.0",
    update_available: true,
    notes: "Fixes",
    asset_url: "https://github.com/ricktap/romm-emudecky-ng/releases/download/v0.33.0/romm-emudecky-ng.zip",
    sha256: "ab".repeat(32),
    html_url: "https://github.com/ricktap/romm-emudecky-ng/releases/tag/v0.33.0",
    plugin_name: "EmuDecky RomM NG",
    checked_at: "2026-01-01T00:00:00+00:00",
    ...overrides,
  };
}

describe("updateStore", () => {
  beforeEach(() => {
    vi.mocked(checkForUpdate).mockReset();
    resetUpdateState();
  });

  it("stores a successful check and notifies subscribers", async () => {
    const fn = vi.fn();
    const unsub = onUpdateStateChange(fn);
    vi.mocked(checkForUpdate).mockResolvedValue(updateInfo());
    await refreshUpdateState(false);
    unsub();
    expect(checkForUpdate).toHaveBeenCalledWith(false);
    expect(getUpdateState()).toEqual({
      info: updateInfo(),
      error: null,
      currentVersion: "0.32.0",
      checkedAt: "2026-01-01T00:00:00+00:00",
      checking: false,
    });
    expect(fn).toHaveBeenCalled();
  });

  it("passes force through", async () => {
    vi.mocked(checkForUpdate).mockResolvedValue(updateInfo());
    await refreshUpdateState(true);
    expect(checkForUpdate).toHaveBeenCalledWith(true);
  });

  it("a failed check keeps the last known update and records the message", async () => {
    vi.mocked(checkForUpdate).mockResolvedValueOnce(updateInfo());
    await refreshUpdateState(false);
    vi.mocked(checkForUpdate).mockResolvedValueOnce({
      success: false,
      reason: "rate_limited",
      message: "GitHub rate limit reached — try again later",
      current_version: "0.32.0",
      checked_at: "2026-01-01T01:00:00+00:00",
    });
    const state = await refreshUpdateState(true);
    expect(state.info).toEqual(updateInfo());
    expect(state.error).toBe("GitHub rate limit reached — try again later");
    expect(state.checkedAt).toBe("2026-01-01T01:00:00+00:00");
    expect(state.checking).toBe(false);
  });

  it("a rejected call is folded into error, never thrown", async () => {
    vi.mocked(checkForUpdate).mockRejectedValue(new Error("bridge down"));
    const state = await refreshUpdateState(false);
    expect(state.error).toBe("Could not check for updates: Error: bridge down");
    expect(state.checking).toBe(false);
  });

  it("an empty response leaves the state untouched apart from checking", async () => {
    vi.mocked(checkForUpdate).mockResolvedValue(undefined as never);
    const state = await refreshUpdateState(false);
    expect(state).toEqual({ info: null, error: null, currentVersion: null, checkedAt: null, checking: false });
  });
});
