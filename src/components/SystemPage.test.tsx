// CATCH-REJECTION ASSERTION RULE: every catch block with a setX(...) side effect has its side effect asserted
// (the surfaced error / note string), not just that the rejecting call happened.
//
// SystemPage catch sites (all asserted below):
//   - refresh try/catch → setError(`Failed to fetch firmware status: ${e}`)
//   - refresh failure branch → setError(result.message || fallback)
//   - runDownload catch → setNote(`Download failed: ${e}`)

import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, fireEvent, act } from "@testing-library/react";
import { SystemPage, keyStateSummary } from "./SystemPage";
import * as backend from "../api/backend";
import type { FirmwareStatus, SwitchKeyFile } from "../types";

vi.mock("../utils/scrollHelpers", () => ({ scrollToTop: vi.fn() }));

vi.mock("@decky/ui", async () => {
  const { createElement: ce } = await import("react");
  type AnyProps = Record<string, unknown> & { children?: unknown };
  const passthrough = (tag: string) => (p: AnyProps) => ce(tag, {}, p.children as never);
  return {
    PanelSection: (p: AnyProps & { title?: unknown }) =>
      ce(
        "section",
        { "data-testid": "panel-section" },
        typeof p.title === "string" ? ce("h2", { "data-testid": "panel-title" }, p.title) : null,
        p.children as never,
      ),
    PanelSectionRow: passthrough("div"),
    ButtonItem: ({ children, onClick, disabled }: AnyProps & { onClick?: () => void; disabled?: boolean }) =>
      ce("button", { onClick, disabled }, children as never),
    Field: (p: AnyProps & { label?: unknown; description?: unknown }) =>
      ce(
        "div",
        { "data-testid": "field" },
        ce("span", { "data-testid": "field-label" }, p.label as never),
        ce("span", { "data-testid": "field-desc" }, p.description as never),
      ),
  };
});

const flushAsync = () =>
  act(async () => {
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
    firmware: [
      { firmware_id: 5, file_name: "Firmware.23.0.1.zip", version: "23.0.1", size: 340422248, selected: true, downloaded: false, local_size: null },
      { firmware_id: 6, file_name: "Firmware.9.2.0.zip", version: "9.2.0", size: 500, selected: false, downloaded: true, local_size: 500 },
    ],
    unrecognized: 0,
    ...overrides,
  };
}

async function renderPage(result: FirmwareStatus | Error) {
  if (result instanceof Error) vi.mocked(backend.getFirmwareStatus).mockRejectedValue(result);
  else vi.mocked(backend.getFirmwareStatus).mockResolvedValue(result);
  const utils = render(<SystemPage onBack={() => {}} />);
  await flushAsync();
  return utils;
}

const texts = (c: HTMLElement, id: string) => Array.from(c.querySelectorAll(`[data-testid="${id}"]`)).map((e) => e.textContent);
const buttonByText = (c: HTMLElement, text: string) =>
  Array.from(c.querySelectorAll("button")).find((b) => b.textContent?.includes(text)) as HTMLButtonElement | undefined;

describe("SystemPage — Switch firmware and keys", () => {
  beforeEach(() => {
    vi.mocked(backend.downloadRequiredFirmware).mockReset();
    vi.mocked(backend.downloadAllFirmware).mockReset();
  });

  it("lists the keys with their state per emulator, marking optional and not-on-RomM files", async () => {
    const { container } = await renderPage(
      status({ keys: [key(), key({ file_name: "console.keys", required: false, on_romm: false, emulators: [] })] }),
    );
    const labels = texts(container, "field-label");
    expect(labels).toContain("prod.keys");
    expect(labels).toContain("console.keys (optional) — not on RomM");
    const descriptions = texts(container, "field-desc");
    expect(descriptions).toContain("Eden: matches RomM · Ryujinx: differs from RomM — kept as is");
    expect(descriptions).toContain("No Switch emulator found");
  });

  it("flags missing required keys in the section title", async () => {
    const { container } = await renderPage(status());
    expect(texts(container, "panel-title")).toContain("Keys — required keys missing");
    const ok = await renderPage(status({ keys: [key()] }));
    expect(texts(ok.container, "panel-title")).toContain("Keys");
  });

  it("lists firmware versions with the newest marked and the download state, and says where the archive goes", async () => {
    const { container } = await renderPage(status());
    const labels = texts(container, "field-label");
    expect(labels).toContain("23.0.1 (newest)");
    expect(labels).toContain("9.2.0");
    const descriptions = texts(container, "field-desc");
    expect(descriptions).toContain("324.7 MB — not downloaded");
    expect(descriptions).toContain("500 B — downloaded");
    expect(container.textContent).toContain("saved to /sd/Emulation/bios/switch");
    expect(container.textContent).toContain("Install Firmware menu");
  });

  it("shows an offline notice and no download buttons when RomM is unreachable", async () => {
    const { container } = await renderPage(status({ server_offline: true, firmware: [] }));
    expect(texts(container, "field-label")).toContain("Server offline");
    expect(buttonByText(container, "Download Required")).toBeUndefined();
    expect(buttonByText(container, "Download All")).toBeUndefined();
  });

  it("says when no Switch emulator is installed", async () => {
    const { container } = await renderPage(status({ emulators: [], keys: [key({ emulators: [] })] }));
    expect(texts(container, "field-label")).toContain("No Switch emulator found");
  });

  it("says when RomM has no Switch files", async () => {
    const { container } = await renderPage(status({ keys: [key({ on_romm: false, emulators: [] })], firmware: [] }));
    expect(texts(container, "field-label")).toContain("No Switch firmware on RomM");
  });

  it("surfaces a failed status as an error row (resolved failure and thrown)", async () => {
    const failed = await renderPage({ success: false, message: "RomM said no" });
    expect(texts(failed.container, "field-desc")).toContain("RomM said no");
    const thrown = await renderPage(new Error("socket closed"));
    expect(texts(thrown.container, "field-desc")).toContain("Failed to fetch firmware status: Error: socket closed");
  });

  it("downloads show the result note; the not-implemented answer is shown instead of a silent no-op", async () => {
    vi.mocked(backend.downloadRequiredFirmware).mockResolvedValue({
      success: false,
      message: "Firmware management is not implemented yet (planned for 0.45.0).",
    });
    const { container } = await renderPage(status());
    await act(async () => {
      fireEvent.click(buttonByText(container, "Download Required")!);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(backend.downloadRequiredFirmware).toHaveBeenCalledWith("switch");
    expect(texts(container, "field-label")).toContain("Firmware management is not implemented yet (planned for 0.45.0).");
  });

  it("Download All uses the all call, refreshes after a success, and shows a thrown download as a failure note", async () => {
    vi.mocked(backend.downloadAllFirmware).mockResolvedValueOnce({ success: true, message: "Downloaded 2 files" });
    const { container } = await renderPage(status());
    const before = vi.mocked(backend.getFirmwareStatus).mock.calls.length;
    await act(async () => {
      fireEvent.click(buttonByText(container, "Download All")!);
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(backend.downloadAllFirmware).toHaveBeenCalledWith("switch");
    expect(vi.mocked(backend.getFirmwareStatus).mock.calls.length).toBe(before + 1);
    expect(texts(container, "field-label")).toContain("Downloaded 2 files");

    vi.mocked(backend.downloadRequiredFirmware).mockRejectedValueOnce(new Error("io"));
    await act(async () => {
      fireEvent.click(buttonByText(container, "Download Required")!);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(texts(container, "field-label")).toContain("Download failed: Error: io");
  });
});

describe("keyStateSummary", () => {
  it("joins the emulators and labels every state", () => {
    expect(
      keyStateSummary(
        key({
          emulators: [
            { id: "eden", label: "Eden", state: "present" },
            { id: "yuzu", label: "yuzu", state: "missing" },
          ],
        }),
      ),
    ).toBe("Eden: present · yuzu: missing");
    expect(keyStateSummary(key({ emulators: [] }))).toBe("No Switch emulator found");
  });
});
