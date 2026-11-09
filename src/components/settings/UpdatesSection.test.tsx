import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, fireEvent, act } from "@testing-library/react";
import { checkForUpdate } from "../../api/backend";
import type { UpdateCheckSuccess } from "../../types";
import { resetUpdateState, getUpdateState } from "../../utils/updateStore";
import { UpdatesSection, updateStatusText } from "./UpdatesSection";

function info(overrides: Partial<UpdateCheckSuccess> = {}): UpdateCheckSuccess {
  return {
    success: true,
    current_version: "0.32.0",
    latest_version: "0.32.0",
    update_available: false,
    notes: "",
    asset_url: "https://github.com/ricktap/romm-emudecky-ng/releases/download/v0.32.0/romm-emudecky-ng.zip",
    sha256: "ab".repeat(32),
    html_url: "https://github.com/ricktap/romm-emudecky-ng/releases/tag/v0.32.0",
    plugin_name: "EmuDecky RomM NG",
    checked_at: "2026-01-01T00:00:00+00:00",
    ...overrides,
  };
}

const flushAsync = () =>
  act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });

function descriptions(container: HTMLElement): string[] {
  return Array.from(container.querySelectorAll('[data-testid="field-desc"]')).map((n) => n.textContent ?? "");
}

describe("UpdatesSection", () => {
  beforeEach(() => {
    vi.mocked(checkForUpdate).mockReset();
    resetUpdateState();
  });

  it("runs a non-forced check on mount and shows the installed version", async () => {
    vi.mocked(checkForUpdate).mockResolvedValue(info());
    const { container } = render(<UpdatesSection />);
    await flushAsync();
    expect(checkForUpdate).toHaveBeenCalledWith(false);
    expect(descriptions(container)[0]).toBe("v0.32.0");
    expect(descriptions(container)[1]).toContain("Up to date (latest release v0.32.0)");
  });

  it("Check for updates forces a check", async () => {
    vi.mocked(checkForUpdate).mockResolvedValue(info());
    const { container } = render(<UpdatesSection />);
    await flushAsync();
    vi.mocked(checkForUpdate).mockClear();
    const btn = Array.from(container.querySelectorAll("button")).find((b) => b.textContent === "Check for updates")!;
    fireEvent.click(btn);
    await flushAsync();
    expect(checkForUpdate).toHaveBeenCalledWith(true);
  });

  it("shows the failure message from a failed check", async () => {
    vi.mocked(checkForUpdate).mockResolvedValue({
      success: false,
      reason: "server_unreachable",
      message: "Could not reach GitHub — check your internet connection",
      current_version: "0.32.0",
      checked_at: "2026-01-01T00:00:00+00:00",
    });
    const { container } = render(<UpdatesSection />);
    await flushAsync();
    expect(descriptions(container)[0]).toBe("v0.32.0");
    expect(descriptions(container)[1]).toContain("Could not reach GitHub — check your internet connection");
  });

  it("shows the update notice when an update is available", async () => {
    vi.mocked(checkForUpdate).mockResolvedValue(info({ latest_version: "0.33.0", update_available: true }));
    const { getAllByText } = render(<UpdatesSection />);
    await flushAsync();
    expect(getAllByText("Update available: v0.33.0").length).toBeGreaterThan(0);
  });

  it("status text covers every state", () => {
    const base = getUpdateState();
    expect(updateStatusText(base)).toBe("Not checked yet");
    expect(updateStatusText({ ...base, checking: true })).toBe("Checking…");
    expect(updateStatusText({ ...base, info: info({ latest_version: null }) })).toBe("No installable release found");
  });
});
