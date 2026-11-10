import { beforeEach, describe, expect, it, vi } from "vitest";
import { toaster } from "@decky/api";

vi.mock("../api/backend", () => ({ setGameCore: vi.fn(), clearGameCore: vi.fn(), invalidateCachedGameDetail: vi.fn() }));
vi.mock("./steamShortcuts", () => ({ setLaunchOptionsConfirmed: vi.fn() }));

import { clearGameCore, invalidateCachedGameDetail, setGameCore } from "../api/backend";
import { setLaunchOptionsConfirmed } from "./steamShortcuts";
import { changeGameCore } from "./gameCore";

const toasts = () => vi.mocked(toaster.toast).mock.calls.map((c) => c[0].body);

describe("changeGameCore", () => {
  let events: unknown[];
  beforeEach(() => {
    vi.clearAllMocks();
    events = [];
    globalThis.addEventListener("romm_data_changed", (e) => events.push((e as CustomEvent).detail), { once: true });
    vi.mocked(setLaunchOptionsConfirmed).mockResolvedValue(true);
  });

  it("pin: confirms the baked launch options before announcing success", async () => {
    vi.mocked(setGameCore).mockResolvedValue({ success: true, app_id: 7, launch_options: "run" } as never);
    expect(await changeGameCore(7, 42, "switch", { kind: "pin", label: "Eden" })).toBe(true);
    expect(setGameCore).toHaveBeenCalledWith(42, "Eden");
    expect(setLaunchOptionsConfirmed).toHaveBeenCalledWith(7, "run");
    expect(toasts()).toEqual(["Core set to Eden"]);
    expect(invalidateCachedGameDetail).toHaveBeenCalledWith(7);
    expect(events).toEqual([{ type: "core_changed", platform_slug: "switch" }]);
  });

  it("clear: says it follows the system core", async () => {
    vi.mocked(clearGameCore).mockResolvedValue({ success: true } as never);
    expect(await changeGameCore(7, 42, "switch", { kind: "clear" })).toBe(true);
    expect(clearGameCore).toHaveBeenCalledWith(42);
    expect(setLaunchOptionsConfirmed).not.toHaveBeenCalled();
    expect(toasts()).toEqual(["Now following the system core"]);
  });

  it("an unconfirmed bake never claims success: a restart-Steam toast, no announcement", async () => {
    vi.mocked(setGameCore).mockResolvedValue({ success: true, app_id: 7, launch_options: "run" } as never);
    vi.mocked(setLaunchOptionsConfirmed).mockResolvedValue(false);
    expect(await changeGameCore(7, 42, "switch", { kind: "pin", label: "Eden" })).toBe(false);
    expect(toasts()).toEqual(["Core saved — restart Steam to apply"]);
    expect(events).toEqual([]);
  });

  it("a refused change shows the backend's message, or a default", async () => {
    vi.mocked(setGameCore).mockResolvedValue({ success: false, message: "Not installed" } as never);
    expect(await changeGameCore(7, 42, "switch", { kind: "pin", label: "X" })).toBe(false);
    vi.mocked(clearGameCore).mockResolvedValue({ success: false, message: "" } as never);
    expect(await changeGameCore(7, 42, "switch", { kind: "clear" })).toBe(false);
    expect(toasts()).toEqual(["Not installed", "Failed to reset core"]);
    expect(events).toEqual([]);
  });

  it("a thrown call is a failure toast", async () => {
    vi.mocked(setGameCore).mockRejectedValue(new Error("x"));
    expect(await changeGameCore(7, 42, "switch", { kind: "pin", label: "X" })).toBe(false);
    expect(toasts()).toEqual(["Failed to set core"]);
  });
});
