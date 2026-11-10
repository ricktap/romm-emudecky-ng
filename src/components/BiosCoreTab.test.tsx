import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render } from "@testing-library/react";
import { toaster } from "@decky/api";

vi.mock("../api/backend", () => ({
  downloadRequiredFirmware: vi.fn(),
  getFirmwareStatus: vi.fn(),
  getPlatformCoreInfo: vi.fn(),
}));
vi.mock("../utils/gameCore", () => ({ changeGameCore: vi.fn() }));

import { downloadRequiredFirmware, getFirmwareStatus, getPlatformCoreInfo } from "../api/backend";
import { changeGameCore } from "../utils/gameCore";
import { BiosCoreTab } from "./BiosCoreTab";

const keysStatus = (state: "missing" | "matches") => ({
  success: true,
  keys: [
    { file_name: "prod.keys", description: "", required: true, on_romm: true, firmware_id: 1, size: 1, emulators: [{ id: "eden", label: "Eden", state }] },
    { file_name: "console.keys", description: "", required: false, on_romm: true, firmware_id: 2, size: 1, emulators: [] },
  ],
});
const core = (over = {}) => ({
  emulators: [
    { label: "Eden", kind: "standalone", core_so: null, is_default: true, bakeable: true, reason: null },
    { label: "Ryujinx", kind: "standalone", core_so: null, is_default: false, bakeable: true, reason: null },
    { label: "Broken", kind: "standalone", core_so: null, is_default: false, bakeable: false, reason: "inject" },
  ],
  emulator_data_available: true,
  active_core: "eden",
  active_core_label: "Eden",
  platform_core_label: "Eden",
  has_game_override: false,
  ...over,
});
const flush = () => act(async () => { await Promise.resolve(); await Promise.resolve(); });
const renderTab = async (platformSlug = "switch") => {
  const utils = render(<BiosCoreTab appId={7} romId={42} platformSlug={platformSlug} />);
  await flush();
  return utils;
};

describe("BiosCoreTab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getFirmwareStatus).mockResolvedValue(keysStatus("matches") as never);
    vi.mocked(getPlatformCoreInfo).mockResolvedValue(core() as never);
    vi.mocked(changeGameCore).mockResolvedValue(true);
  });

  it("lists each key with its state per emulator, optional ones marked", async () => {
    const { container } = await renderTab();
    expect(container.textContent).toContain("prod.keys");
    expect(container.textContent).toContain("Eden: ready");
    expect(container.textContent).toContain("console.keys (optional)");
    expect(container.textContent).toContain("No emulator reads this file");
    expect(container.textContent).not.toContain("Get required files");
  });

  it("offers Get required files only while a required key is missing, and queues them for the platform", async () => {
    vi.mocked(getFirmwareStatus).mockResolvedValue(keysStatus("missing") as never);
    vi.mocked(downloadRequiredFirmware).mockResolvedValue({ success: true, message: "Queued 2" , queued: 2});
    const { getByText } = await renderTab();
    await act(async () => { fireEvent.click(getByText("Get required files")); });
    expect(downloadRequiredFirmware).toHaveBeenCalledWith("switch");
    expect(toaster.toast).toHaveBeenCalledWith(expect.objectContaining({ body: "Queued 2" }));
  });

  it("a failing download says so", async () => {
    vi.mocked(getFirmwareStatus).mockResolvedValue(keysStatus("missing") as never);
    vi.mocked(downloadRequiredFirmware).mockRejectedValue(new Error("x"));
    const { getByText } = await renderTab();
    await act(async () => { fireEvent.click(getByText("Get required files")); });
    expect(toaster.toast).toHaveBeenCalledWith(expect.objectContaining({ body: "Could not start the download" }));
  });

  it("refreshes when key files arrive", async () => {
    vi.mocked(getFirmwareStatus).mockResolvedValue(keysStatus("missing") as never);
    const { container } = await renderTab();
    expect(container.textContent).toContain("Eden: missing");
    vi.mocked(getFirmwareStatus).mockResolvedValue(keysStatus("matches") as never);
    act(() => { globalThis.dispatchEvent(new CustomEvent("romm_data_changed", { detail: { type: "firmware" } })); });
    await flush();
    expect(container.textContent).toContain("Eden: ready");
  });

  it("other platforms need no BIOS and never ask for the firmware status", async () => {
    const { container } = await renderTab("snes");
    expect(container.textContent).toContain("No BIOS needed for this platform");
    expect(getFirmwareStatus).not.toHaveBeenCalled();
  });

  it("says when the key status cannot be read", async () => {
    vi.mocked(getFirmwareStatus).mockResolvedValue({ success: false, message: "offline" } as never);
    const { container } = await renderTab();
    expect(container.textContent).toContain("Key status unavailable");
    expect(container.textContent).toContain("offline");
  });

  it("shows the active emulator; a click pins another one, the active and non-bakeable ones cannot be picked", async () => {
    const { getByText } = await renderTab();
    expect((getByText("Eden ✓") as HTMLButtonElement).disabled).toBe(true);
    expect((getByText("Broken") as HTMLButtonElement).disabled).toBe(true);
    await act(async () => { fireEvent.click(getByText("Ryujinx")); });
    expect(changeGameCore).toHaveBeenCalledWith(7, 42, "switch", { kind: "pin", label: "Ryujinx" });
  });

  it("with an override: says so and offers to follow the system emulator again", async () => {
    vi.mocked(getPlatformCoreInfo).mockResolvedValue(core({ has_game_override: true, active_core_label: "Ryujinx" }) as never);
    const { container, getByText } = await renderTab();
    expect(container.textContent).toContain("Set for this game");
    await act(async () => { fireEvent.click(getByText("Use system emulator")); });
    expect(changeGameCore).toHaveBeenCalledWith(7, 42, "switch", { kind: "clear" });
  });

  it("reloads the emulator after core_changed (from the play section's menu too)", async () => {
    const { container } = await renderTab();
    expect(container.textContent).toContain("Following the system emulator");
    vi.mocked(getPlatformCoreInfo).mockResolvedValue(core({ has_game_override: true }) as never);
    act(() => { globalThis.dispatchEvent(new CustomEvent("romm_data_changed", { detail: { type: "core_changed" } })); });
    await flush();
    expect(container.textContent).toContain("Set for this game");
  });

  it("says when emulator data or info is unavailable", async () => {
    vi.mocked(getPlatformCoreInfo).mockResolvedValue(core({ emulator_data_available: false }) as never);
    expect((await renderTab()).container.textContent).toContain("Emulator list unavailable");
    vi.mocked(getPlatformCoreInfo).mockRejectedValue(new Error("x"));
    expect((await renderTab()).container.textContent).toContain("Emulator info unavailable");
  });
});
