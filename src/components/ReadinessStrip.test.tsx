import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render } from "@testing-library/react";
import { toaster } from "@decky/api";

vi.mock("../api/backend", () => ({
  downloadRequiredFirmware: vi.fn(),
  getFirmwareStatus: vi.fn(),
  getInstalledRom: vi.fn(),
  getSaveStatus: vi.fn(),
}));
vi.mock("./SyncConflictModal", () => ({ handleConflicts: vi.fn() }));
vi.mock("./SaveSetupModal", () => ({ showSaveSetupModal: vi.fn() }));

import { downloadRequiredFirmware, getFirmwareStatus, getInstalledRom, getSaveStatus } from "../api/backend";
import { setDownloads } from "../utils/downloadStore";
import { emitDeckyEvent } from "../test-utils/decky-api-mock";
import { handleConflicts } from "./SyncConflictModal";
import { showSaveSetupModal } from "./SaveSetupModal";
import { ReadinessStrip } from "./ReadinessStrip";

const props = {
  romId: 5,
  platformSlug: "switch",
  saveSyncEnabled: true,
  saveSyncStatus: "synced" as const,
  saveSyncLabel: "2h ago",
  activeCoreLabel: "Eden",
};
const keys = (state: "missing" | "matches") => ({
  success: true,
  keys: [{ file_name: "prod.keys", description: "", required: true, on_romm: true, firmware_id: 1, size: 1, emulators: [{ id: "eden", label: "Eden", state }] }],
});
const flush = () => act(async () => { await Promise.resolve(); await Promise.resolve(); });

describe("ReadinessStrip", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    setDownloads([]);
    vi.mocked(getInstalledRom).mockResolvedValue(null);
    vi.mocked(getFirmwareStatus).mockResolvedValue(keys("matches") as never);
  });
  afterEach(() => vi.useRealTimers());

  it("shows the three chips with their states", async () => {
    vi.mocked(getInstalledRom).mockResolvedValue({ rom_id: 5, file_name: "Zelda.nsp" } as never);
    const { getByTestId } = render(<ReadinessStrip {...props} />);
    await flush();
    expect(getByTestId("chip-rom-title").textContent).toBe("ROM installed");
    expect(getByTestId("chip-rom-detail").textContent).toBe("Zelda.nsp");
    expect(getByTestId("chip-bios-title").textContent).toBe("BIOS ready");
    expect(getByTestId("chip-bios-detail").textContent).toBe("Eden");
    expect(getByTestId("chip-saves-title").textContent).toBe("Saves synced");
    expect(getByTestId("chip-saves-detail").textContent).toBe("2h ago");
  });

  it("follows the download queue for this ROM, but not another ROM or a firmware job", async () => {
    const { getByTestId } = render(<ReadinessStrip {...props} />);
    await flush();
    setDownloads([
      { rom_id: 9, rom_name: "X", platform_name: "S", file_name: "x", status: "downloading", progress: 0, bytes_downloaded: 90, total_bytes: 100, resumable: true } as never,
      { kind: "firmware", rom_id: 5, firmware_id: 5, rom_name: "F", platform_name: "S", file_name: "f", status: "downloading", progress: 0, bytes_downloaded: 90, total_bytes: 100, resumable: true } as never,
      { rom_id: 5, rom_name: "Z", platform_name: "S", file_name: "z", status: "downloading", progress: 0, bytes_downloaded: 25, total_bytes: 100, resumable: true } as never,
    ]);
    await act(async () => { await vi.advanceTimersByTimeAsync(1100); });
    expect(getByTestId("chip-rom-title").textContent).toBe("Downloading");
    expect(getByTestId("chip-rom-detail").textContent).toBe("25%");
  });

  it("re-reads the install state when this ROM's download completes", async () => {
    const { getByTestId } = render(<ReadinessStrip {...props} />);
    await flush();
    expect(getByTestId("chip-rom-title").textContent).toBe("Not downloaded");
    vi.mocked(getInstalledRom).mockResolvedValue({ rom_id: 5, file_name: "Z.nsp" } as never);
    act(() => { emitDeckyEvent("download_complete", { rom_id: 4 }); });
    await flush();
    expect(getByTestId("chip-rom-title").textContent).toBe("Not downloaded");
    act(() => { emitDeckyEvent("download_complete", { rom_id: 5 }); });
    await flush();
    expect(getByTestId("chip-rom-detail").textContent).toBe("Z.nsp");
  });

  it("clears the installed state on an uninstall of this ROM", async () => {
    vi.mocked(getInstalledRom).mockResolvedValue({ rom_id: 5, file_name: "Z.nsp" } as never);
    const { getByTestId } = render(<ReadinessStrip {...props} />);
    await flush();
    act(() => { globalThis.dispatchEvent(new CustomEvent("romm_rom_uninstalled", { detail: { rom_id: 5 } })); });
    expect(getByTestId("chip-rom-title").textContent).toBe("Not downloaded");
  });

  it("missing keys: warns, Get queues the required firmware for the platform, and the chip updates when it lands", async () => {
    vi.mocked(getFirmwareStatus).mockResolvedValue(keys("missing") as never);
    vi.mocked(downloadRequiredFirmware).mockResolvedValue({ success: true, message: "Queued 2 downloads", queued: 2 });
    const { getByTestId, getByText } = render(<ReadinessStrip {...props} />);
    await flush();
    expect(getByTestId("chip-bios-title").textContent).toBe("BIOS missing");
    expect(getByTestId("chip-bios-detail").textContent).toBe("prod.keys on RomM");
    await act(async () => { fireEvent.click(getByText("Get")); });
    expect(downloadRequiredFirmware).toHaveBeenCalledWith("switch");
    expect(toaster.toast).toHaveBeenCalledWith(expect.objectContaining({ body: "Queued 2 downloads" }));
    vi.mocked(getFirmwareStatus).mockResolvedValue(keys("matches") as never);
    act(() => { globalThis.dispatchEvent(new CustomEvent("romm_data_changed", { detail: { type: "firmware" } })); });
    await flush();
    expect(getByTestId("chip-bios-title").textContent).toBe("BIOS ready");
  });

  it("a failing Get says so", async () => {
    vi.mocked(getFirmwareStatus).mockResolvedValue(keys("missing") as never);
    vi.mocked(downloadRequiredFirmware).mockRejectedValue(new Error("x"));
    const { getByText } = render(<ReadinessStrip {...props} />);
    await flush();
    await act(async () => { fireEvent.click(getByText("Get")); });
    expect(toaster.toast).toHaveBeenCalledWith(expect.objectContaining({ body: "Could not start the download" }));
  });

  it("other platforms need no BIOS and never ask for the firmware status", async () => {
    const { getByTestId } = render(<ReadinessStrip {...props} platformSlug="snes" />);
    await flush();
    expect(getByTestId("chip-bios-title").textContent).toBe("No BIOS needed");
    expect(getByTestId("chip-bios-detail").textContent).toBe("Eden");
    expect(getFirmwareStatus).not.toHaveBeenCalled();
  });

  it("conflict: Resolve opens the conflict modals with the loaded conflicts, then announces the change", async () => {
    const conflicts = [{ filename: "a.sav" }];
    vi.mocked(getSaveStatus).mockResolvedValue({ conflicts } as never);
    vi.mocked(handleConflicts).mockResolvedValue("resolved");
    const seen: unknown[] = [];
    globalThis.addEventListener("romm_data_changed", (e) => seen.push((e as CustomEvent).detail));
    const { getByText } = render(<ReadinessStrip {...props} saveSyncStatus="conflict" saveSyncLabel="Conflict" />);
    await flush();
    await act(async () => { fireEvent.click(getByText("Resolve")); });
    expect(getSaveStatus).toHaveBeenCalledWith(5);
    expect(handleConflicts).toHaveBeenCalledWith(conflicts);
    expect(seen).toContainEqual({ type: "save_sync", rom_id: 5 });
  });

  it("a cancelled conflict dialog announces nothing; a failed load says so", async () => {
    vi.mocked(getSaveStatus).mockResolvedValue({ conflicts: [] } as never);
    vi.mocked(handleConflicts).mockResolvedValue("cancel");
    const seen: unknown[] = [];
    const rec = (e: Event) => seen.push((e as CustomEvent).detail);
    globalThis.addEventListener("romm_data_changed", rec);
    const { getByText } = render(<ReadinessStrip {...props} saveSyncStatus="conflict" saveSyncLabel="Conflict" />);
    await flush();
    await act(async () => { fireEvent.click(getByText("Resolve")); });
    expect(seen).toEqual([]);
    vi.mocked(getSaveStatus).mockRejectedValue(new Error("x"));
    await new Promise((r) => setTimeout(r, 300)); // a second press within 250 ms counts once
    await act(async () => { fireEvent.click(getByText("Resolve")); });
    expect(toaster.toast).toHaveBeenCalledWith(expect.objectContaining({ body: "Could not load the save conflict" }));
    globalThis.removeEventListener("romm_data_changed", rec);
  });

  it("a game whose save cannot be located offers Choose, which opens the chooser and refreshes the status when something was set", async () => {
    vi.mocked(showSaveSetupModal).mockResolvedValue(true);
    const seen: unknown[] = [];
    const rec = (e: Event) => seen.push((e as CustomEvent).detail);
    globalThis.addEventListener("romm_data_changed", rec);
    const { getByText, getByTestId } = render(<ReadinessStrip {...props} saveSyncStatus="none" saveSyncLabel="Choose save folder" />);
    await flush();
    expect(getByTestId("chip-saves-title").textContent).toBe("Save not found");
    expect(getByTestId("chip-saves-detail").textContent).toBe("Choose save folder");
    await act(async () => { fireEvent.click(getByText("Choose")); });
    expect(showSaveSetupModal).toHaveBeenCalledWith(5);
    expect(seen).toContainEqual({ type: "save_sync", rom_id: 5 });
    vi.mocked(showSaveSetupModal).mockResolvedValue(false);
    seen.length = 0;
    await new Promise((r) => setTimeout(r, 300));
    await act(async () => { fireEvent.click(getByText("Choose")); });
    expect(seen).toEqual([]);
    globalThis.removeEventListener("romm_data_changed", rec);
  });

  it("stops polling and listening on unmount", async () => {
    const spy = vi.spyOn(globalThis, "clearInterval");
    const { unmount } = render(<ReadinessStrip {...props} />);
    await flush();
    unmount();
    expect(spy).toHaveBeenCalled();
  });
});
