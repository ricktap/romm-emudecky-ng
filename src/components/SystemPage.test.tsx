// CATCH-REJECTION ASSERTION RULE: every catch block with a setX(...) side effect has its side effect asserted
// (the surfaced error / note string), not just that the rejecting call happened.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, fireEvent, act } from "@testing-library/react";
import { SystemPage, keyStateSummary } from "./SystemPage";
import * as backend from "../api/backend";
import { showModal } from "@decky/ui";
import type { FirmwareStatus, SwitchKeyFile } from "../types";

const flushAsync = () =>
  act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });

function key(overrides: Partial<SwitchKeyFile> = {}): SwitchKeyFile {
  return {
    file_name: "prod.keys",
    description: "Console keys",
    required: true,
    on_romm: true,
    firmware_id: 3,
    size: 16025,
    emulators: [
      { id: "eden", label: "Eden", state: "matches" },
      { id: "ryujinx", label: "Ryujinx", state: "differs" },
    ],
    ...overrides,
  };
}

function status(overrides: Partial<FirmwareStatus> = {}): FirmwareStatus {
  return {
    success: true,
    server_offline: false,
    platform_slug: "switch",
    archive_dir: "/sd/Emulation/bios/switch",
    emulators: [
      { id: "eden", label: "Eden", keys_dir: "/home/deck/.local/share/eden/keys" },
      { id: "ryujinx", label: "Ryujinx", keys_dir: "/home/deck/.config/Ryujinx/system" },
    ],
    keys: [key(), key({ file_name: "title.keys", firmware_id: 4, emulators: [{ id: "eden", label: "Eden", state: "missing" }] })],
    selected_version: "23.0.1",
    version_pinned: false,
    firmware: [
      { firmware_id: 5, file_name: "Firmware.23.0.1.zip", version: "23.0.1", size: 340422248, selected: true, downloaded: false, local_size: null },
      { firmware_id: 6, file_name: "Firmware.9.2.0.zip", version: "9.2.0", size: 500, selected: false, downloaded: true, local_size: 500 },
    ],
    unrecognized: 0,
    ...overrides,
  };
}


const PLATFORMS = {
  success: true,
  platforms: [
    { id: 1, name: "Nintendo Switch", slug: "switch", rom_count: 3, sync_enabled: true },
    { id: 2, name: "Super Nintendo", slug: "snes", rom_count: 5, sync_enabled: false },
  ],
};

async function renderPage(result: FirmwareStatus | Error) {
  if (result instanceof Error) vi.mocked(backend.getFirmwareStatus).mockRejectedValue(result);
  else vi.mocked(backend.getFirmwareStatus).mockResolvedValue(result);
  vi.mocked(backend.getPlatforms).mockResolvedValue(PLATFORMS as never);
  const utils = render(<SystemPage />);
  await flushAsync();
  return utils;
}

/** Press a control and let the async handler finish (a second press within 250 ms counts once, so wait first). */
async function press(el: Element | null | undefined) {
  expect(el).toBeTruthy();
  await act(async () => {
    await new Promise((r) => setTimeout(r, 300));
    fireEvent.click(el!);
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

function lastOf<T>(items: T[]): T {
  return items[items.length - 1]!;
}

describe("SystemPage — the table", () => {
  beforeEach(() => {
    vi.mocked(backend.savePlatformSync).mockReset().mockResolvedValue({ success: true, message: "" });
    vi.mocked(backend.downloadRequiredFirmware).mockReset();
    vi.mocked(backend.downloadAllFirmware).mockReset();
    vi.mocked(backend.saveFirmwareVersion).mockReset();
  });

  it("has a row per platform plus the Switch row, with game counts", async () => {
    const { getByTestId } = await renderPage(status());
    expect(getByTestId("system-snes").textContent).toContain("Super Nintendo");
    expect(getByTestId("system-snes").textContent).toContain("5 games");
    expect(getByTestId("system-snes").textContent).toContain("No BIOS check yet");
    expect(getByTestId("system-switch").textContent).toContain("3 games · firmware and keys");
  });

  it("the Switch row shows the emulators and 'Missing required keys' with its first required key file", async () => {
    const { getByTestId } = await renderPage(status());
    const row = getByTestId("system-switch").textContent!;
    expect(row).toContain("Eden, Ryujinx");
    expect(row).toContain("Missing required keys");
    expect(row).toContain("prod.keys");
  });

  it("Ready when every required key is there; offers no Download BIOS then", async () => {
    const { getByTestId, queryByTestId } = await renderPage(status({ keys: [key({ emulators: [{ id: "eden", label: "Eden", state: "matches" }] })] }));
    expect(getByTestId("system-switch").textContent).toContain("Ready");
    expect(queryByTestId("download-required")).toBeNull();
  });

  it("the sync switch saves the platform and says changes apply on the next sync; a failure undoes it", async () => {
    const { getByLabelText, getByTestId } = await renderPage(status());
    expect(getByLabelText("Sync Super Nintendo").className).not.toContain("rm-on");
    await press(getByLabelText("Sync Super Nintendo"));
    expect(backend.savePlatformSync).toHaveBeenCalledWith(2, true);
    expect(getByTestId("systems-note").textContent).toBe("Changes apply on the next sync");
    expect(getByLabelText("Sync Super Nintendo").className).toContain("rm-on");
    vi.mocked(backend.savePlatformSync).mockRejectedValue(new Error("x"));
    await press(getByLabelText("Sync Super Nintendo"));
    expect(getByTestId("systems-note").textContent).toBe("Could not save the platform setting");
    expect(getByLabelText("Sync Super Nintendo").className).toContain("rm-on");
  });

  it("lists the Switch row even when RomM has no Switch platform", async () => {
    vi.mocked(backend.getFirmwareStatus).mockResolvedValue(status());
    vi.mocked(backend.getPlatforms).mockResolvedValue({ success: true, platforms: [] } as never);
    const { getByTestId } = render(<SystemPage />);
    await flushAsync();
    expect(getByTestId("system-switch").textContent).toContain("Firmware and keys");
  });
});

describe("SystemPage — moved storage location", () => {
  const moved = { health: "ok", ok: true, message: "", storage_path: "/new/card", moved: true, accepted_path: "/old/card", installed: 4, missing: 3 };
  beforeEach(() => {
    vi.mocked(backend.getEmudeckHealth).mockReset();
    vi.mocked(backend.acceptEmudeckStoragePath).mockReset();
  });

  it("shows nothing when the location did not move", async () => {
    vi.mocked(backend.getEmudeckHealth).mockResolvedValue({ health: "ok", ok: true, message: "", storage_path: "/sd" });
    expect((await renderPage(status())).queryByTestId("storage-moved")).toBeNull();
  });

  it("names the old and new location, says nothing was moved, and what was not found", async () => {
    vi.mocked(backend.getEmudeckHealth).mockResolvedValue(moved as never);
    const { getByTestId } = await renderPage(status());
    const card = getByTestId("storage-moved").textContent!;
    expect(card).toContain("/old/card");
    expect(card).toContain("/new/card");
    expect(card).toContain("3 of 4 installed games were not found");
    expect(card).toContain("Nothing was moved or deleted");
  });

  it("Use this location accepts it and the card goes away", async () => {
    vi.mocked(backend.getEmudeckHealth).mockResolvedValueOnce(moved as never);
    vi.mocked(backend.acceptEmudeckStoragePath).mockResolvedValue({ success: true, message: "Using the new EmuDeck location" });
    const { getByTestId, queryByTestId } = await renderPage(status());
    vi.mocked(backend.getEmudeckHealth).mockResolvedValue({ health: "ok", ok: true, message: "", storage_path: "/new/card", moved: false });
    await press(getByTestId("accept-storage"));
    expect(backend.acceptEmudeckStoragePath).toHaveBeenCalledTimes(1);
    expect(getByTestId("systems-note").textContent).toBe("Using the new EmuDeck location");
    expect(queryByTestId("storage-moved")).toBeNull();
  });

  it("a failed accept says so and keeps the card", async () => {
    vi.mocked(backend.getEmudeckHealth).mockResolvedValue(moved as never);
    vi.mocked(backend.acceptEmudeckStoragePath).mockRejectedValue(new Error("x"));
    const { getByTestId } = await renderPage(status());
    await press(getByTestId("accept-storage"));
    expect(getByTestId("systems-note").textContent).toBe("Could not save the location");
    expect(getByTestId("storage-moved")).toBeTruthy();
  });
});

describe("SystemPage — Switch firmware and keys", () => {
  beforeEach(() => {
    vi.mocked(backend.downloadRequiredFirmware).mockReset();
    vi.mocked(backend.downloadAllFirmware).mockReset();
    vi.mocked(backend.saveFirmwareVersion).mockReset();
  });

  it("lists the keys with their state per emulator, marking optional and not-on-RomM files", async () => {
    const { container } = await renderPage(
      status({ keys: [key(), key({ file_name: "console.keys", required: false, on_romm: false, emulators: [] })] }),
    );
    expect(container.textContent).toContain("prod.keys");
    expect(container.textContent).toContain("console.keys (optional) — not on RomM");
    expect(container.textContent).toContain("Eden: matches RomM · Ryujinx: differs from RomM — kept as is");
    expect(container.textContent).toContain("No Switch emulator found");
  });

  it("flags missing required keys in the card title", async () => {
    expect((await renderPage(status())).container.textContent).toContain("Switch keys — required keys missing");
  });

  it("lists firmware versions with the newest marked and the download state, and says where the archive goes", async () => {
    const { container } = await renderPage(status());
    expect(container.textContent).toContain("23.0.1 (selected)");
    expect(container.textContent).toContain("not downloaded");
    expect(container.textContent).toContain("9.2.0");
    expect(container.textContent).toContain("/sd/Emulation/bios/switch");
  });

  it("shows an offline notice and no download buttons when RomM is unreachable", async () => {
    const { container, queryByTestId } = await renderPage(status({ server_offline: true }));
    expect(container.textContent).toContain("Server offline");
    expect(queryByTestId("download-required")).toBeNull();
    expect(queryByTestId("download-all")).toBeNull();
  });

  it("says when no Switch emulator is installed", async () => {
    const { container } = await renderPage(status({ emulators: [], keys: [] }));
    expect(container.textContent).toContain("No Switch emulator found");
    expect(container.textContent).toContain("Install Eden, yuzu or Ryujinx with EmuDeck first");
  });

  it("says when RomM has no Switch files", async () => {
    const { container } = await renderPage(status({ keys: [], firmware: [] }));
    expect(container.textContent).toContain("No Switch firmware on RomM");
  });

  it("surfaces a failed status as an error row (resolved failure and thrown)", async () => {
    expect((await renderPage({ success: false, message: "RomM said no" } as FirmwareStatus)).container.textContent).toContain("RomM said no");
    expect((await renderPage(new Error("boom"))).container.textContent).toContain("Failed to fetch firmware status: Error: boom");
  });

  it("Download BIOS queues the required downloads and points to the Downloads tab", async () => {
    vi.mocked(backend.downloadRequiredFirmware).mockResolvedValue({ success: true, message: "Queued 2 downloads", queued: 2 });
    const { getByTestId } = await renderPage(status());
    await press(getByTestId("download-required"));
    expect(backend.downloadRequiredFirmware).toHaveBeenCalledWith("switch");
    expect(getByTestId("systems-note").textContent).toBe("Queued 2 downloads — progress is in Downloads");
  });

  it("Download all, plain notes when nothing was queued, and refused or thrown downloads as notes", async () => {
    vi.mocked(backend.downloadAllFirmware).mockResolvedValue({ success: true, message: "Everything is already here" });
    const { getByTestId } = await renderPage(status());
    await press(getByTestId("download-all"));
    expect(getByTestId("systems-note").textContent).toBe("Everything is already here");
    vi.mocked(backend.downloadAllFirmware).mockResolvedValue({ success: false, message: "Server refused" });
    await press(getByTestId("download-all"));
    expect(getByTestId("systems-note").textContent).toBe("Server refused");
    vi.mocked(backend.downloadAllFirmware).mockRejectedValue(new Error("net"));
    await press(getByTestId("download-all"));
    expect(getByTestId("systems-note").textContent).toBe("Download failed: Error: net");
  });

  it("the version button offers the newest-follows option and every version, and saves the choice", async () => {
    vi.mocked(backend.saveFirmwareVersion).mockResolvedValue({ success: true, message: "" });
    const { getByTestId } = await renderPage(status());
    expect(getByTestId("choice-Firmware version").textContent).toBe("Newest available (23.0.1)");
    await press(getByTestId("choice-Firmware version"));
    expect(backend.saveFirmwareVersion).toHaveBeenCalledWith("23.0.1");
  });

  it("shows the pinned version as selected and reports a failed save", async () => {
    vi.mocked(backend.saveFirmwareVersion).mockResolvedValue({ success: false, message: "Could not pin" });
    const { getByTestId } = await renderPage(status({ version_pinned: true, selected_version: "9.2.0" }));
    expect(getByTestId("choice-Firmware version").textContent).toBe("9.2.0");
    await press(getByTestId("choice-Firmware version"));
    expect(getByTestId("systems-note").textContent).toBe("Could not pin");
    vi.mocked(backend.saveFirmwareVersion).mockRejectedValue(new Error("x"));
    await press(getByTestId("choice-Firmware version"));
    expect(getByTestId("systems-note").textContent).toBe("Could not save the firmware version: Error: x");
  });

  it("refreshes when a firmware download finishes (the global listener announces it)", async () => {
    await renderPage(status());
    const calls = vi.mocked(backend.getFirmwareStatus).mock.calls.length;
    await act(async () => {
      globalThis.dispatchEvent(new CustomEvent("romm_data_changed", { detail: { type: "firmware" } }));
      await Promise.resolve();
    });
    expect(vi.mocked(backend.getFirmwareStatus).mock.calls.length).toBe(calls + 1);
    await act(async () => {
      globalThis.dispatchEvent(new CustomEvent("romm_data_changed", { detail: { type: "other" } }));
      await Promise.resolve();
    });
    expect(vi.mocked(backend.getFirmwareStatus).mock.calls.length).toBe(calls + 1);
  });
});

describe("SystemPage — replacing a key that differs from RomM's", () => {
  beforeEach(() => {
    vi.mocked(showModal).mockClear();
    vi.mocked(backend.replaceFirmwareKey).mockReset();
  });
  const confirm = () => {
    const el = lastOf(vi.mocked(showModal).mock.calls)[0] as { props: Record<string, unknown> };
    return el.props;
  };

  it("offers an explicit Replace button only for a key that differs, per emulator", async () => {
    const { getByTestId, queryByTestId } = await renderPage(status());
    expect(getByTestId("replace-prod.keys-ryujinx")).toBeTruthy();
    expect(queryByTestId("replace-prod.keys-eden")).toBeNull();
  });

  it("offers nothing for a key RomM does not have, and nothing while RomM is offline", async () => {
    expect((await renderPage(status({ keys: [key({ on_romm: false })] }))).queryByTestId("replace-prod.keys-ryujinx")).toBeNull();
    expect((await renderPage(status({ server_offline: true }))).queryByTestId("replace-prod.keys-ryujinx")).toBeNull();
  });

  it("asks first, naming the backup, and does nothing until the user confirms", async () => {
    const { getByTestId } = await renderPage(status());
    await press(getByTestId("replace-prod.keys-ryujinx"));
    const props = confirm();
    expect(props.strTitle).toBe("Replace Ryujinx's prod.keys?");
    expect(String(props.strDescription)).toContain("prod.keys.romm-backup");
    expect(backend.replaceFirmwareKey).not.toHaveBeenCalled();
  });

  it("confirming replaces the key for that emulator, shows the result and refreshes", async () => {
    vi.mocked(backend.replaceFirmwareKey).mockResolvedValue({ success: true, message: "Replaced prod.keys" });
    const { getByTestId } = await renderPage(status());
    const calls = vi.mocked(backend.getFirmwareStatus).mock.calls.length;
    await press(getByTestId("replace-prod.keys-ryujinx"));
    await act(async () => {
      (confirm().onOK as () => void)();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(backend.replaceFirmwareKey).toHaveBeenCalledWith(3, "ryujinx");
    expect(getByTestId("systems-note").textContent).toBe("Replaced prod.keys");
    expect(vi.mocked(backend.getFirmwareStatus).mock.calls.length).toBe(calls + 1);
  });

  it("shows a refused replacement and a thrown one as notes without refreshing", async () => {
    const { getByTestId } = await renderPage(status());
    const calls = vi.mocked(backend.getFirmwareStatus).mock.calls.length;
    await press(getByTestId("replace-prod.keys-ryujinx"));
    vi.mocked(backend.replaceFirmwareKey).mockResolvedValue({ success: false, message: "Checksum mismatch" });
    await act(async () => {
      (confirm().onOK as () => void)();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(getByTestId("systems-note").textContent).toBe("Checksum mismatch");
    vi.mocked(backend.replaceFirmwareKey).mockRejectedValue(new Error("disk"));
    await act(async () => {
      (confirm().onOK as () => void)();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(getByTestId("systems-note").textContent).toBe("Could not replace the key: Error: disk");
    expect(vi.mocked(backend.getFirmwareStatus).mock.calls.length).toBe(calls);
  });
});

describe("keyStateSummary", () => {
  it("joins the emulators and labels every state", () => {
    expect(
      keyStateSummary(
        key({
          emulators: [
            { id: "a", label: "A", state: "matches" },
            { id: "b", label: "B", state: "present" },
            { id: "c", label: "C", state: "missing" },
          ],
        }),
      ),
    ).toBe("A: matches RomM · B: present · C: missing");
    expect(keyStateSummary(key({ emulators: [] }))).toBe("No Switch emulator found");
  });
});
