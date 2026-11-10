import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, fireEvent, act } from "@testing-library/react";
import { Navigation } from "@decky/ui";
import { UpdateBanner, UpdateNotice, updateTitle } from "./UpdateBanner";
import { getLibraryTab } from "../utils/libraryRoute";
import type { UpdateCheckSuccess } from "../types";

function info(overrides: Partial<UpdateCheckSuccess> = {}): UpdateCheckSuccess {
  return {
    success: true,
    current_version: "0.32.0",
    latest_version: "0.33.0",
    update_available: true,
    notes: "Short notes",
    asset_url: "https://github.com/ricktap/romm-emudecky-ng/releases/download/v0.33.0/romm-emudecky-ng.zip",
    sha256: "cd".repeat(32),
    html_url: "https://github.com/ricktap/romm-emudecky-ng/releases/tag/v0.33.0",
    plugin_name: "Backend Supplied Name",
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

function buttonByText(container: HTMLElement, text: string): HTMLElement | undefined {
  return Array.from(container.querySelectorAll<HTMLElement>("button, [data-testid='focusable']")).find((b) => b.textContent === text);
}

describe("UpdateBanner (the attention row)", () => {
  it("names the update and opens the Settings tab, where the Plugin card has the Update button", () => {
    const { getByTestId } = render(<UpdateBanner info={info()} />);
    expect(getByTestId("attention-row").textContent).toContain("Update available: v0.33.0");
    expect(getByTestId("attention-row").textContent).toContain("Installed v0.32.0");
    fireEvent.click(getByTestId("attention-row"));
    expect(getLibraryTab()).toBe("settings");
    expect(Navigation.Navigate).toHaveBeenCalledWith("/romm-library");
  });
});

describe("UpdateNotice", () => {
  beforeEach(() => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
  });

  it("shows the version title, installed version and short notes", () => {
    const { getByText, getByTestId } = render(<UpdateNotice info={info()} />);
    expect(getByText("Update available: v0.33.0")).toBeTruthy();
    expect(getByText("Installed: v0.32.0")).toBeTruthy();
    expect(getByTestId("update-notes").textContent).toBe("Short notes");
    expect(updateTitle(info())).toBe("Update available: v0.33.0");
  });

  it("collapses long notes behind a toggle", async () => {
    const long = "x".repeat(400);
    const { container, queryByTestId } = render(<UpdateNotice info={info({ notes: long })} />);
    expect(queryByTestId("update-notes")).toBeNull();
    fireEvent.click(buttonByText(container, "Show release notes")!);
    expect(queryByTestId("update-notes")!.textContent).toBe(long);
    await new Promise((r) => setTimeout(r, 300)); // a second press of the same button within 250 ms counts once
    fireEvent.click(buttonByText(container, "Hide release notes")!);
    expect(queryByTestId("update-notes")).toBeNull();
  });

  it("Update hands Decky the backend-supplied name, version, url and hash", async () => {
    const call = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("DeckyBackend", { call });
    const i = info();
    const { container, queryByText } = render(<UpdateNotice info={i} />);
    fireEvent.click(buttonByText(container, "Update")!);
    await flushAsync();
    expect(call).toHaveBeenCalledWith("utilities/install_plugin", i.asset_url, "Backend Supplied Name", "0.33.0", i.sha256, 2);
    expect(queryByText("Couldn't start the update — download v0.33.0")).toBeNull();
  });

  it("falls back to a download link when DeckyBackend is missing", async () => {
    vi.stubGlobal("DeckyBackend", undefined);
    const { container, getByText } = render(<UpdateNotice info={info()} />);
    fireEvent.click(buttonByText(container, "Update")!);
    await flushAsync();
    expect(getByText("Couldn't start the update — download v0.33.0")).toBeTruthy();
    fireEvent.click(buttonByText(container, "Download v0.33.0")!);
    expect(Navigation.NavigateToExternalWeb).toHaveBeenCalledWith(
      "https://github.com/ricktap/romm-emudecky-ng/releases/tag/v0.33.0",
    );
  });

  it("falls back to a download link when the Decky call throws", async () => {
    vi.stubGlobal("DeckyBackend", { call: vi.fn().mockRejectedValue(new Error("nope")) });
    const { container, getByText } = render(<UpdateNotice info={info()} />);
    fireEvent.click(buttonByText(container, "Update")!);
    await flushAsync();
    expect(getByText("Couldn't start the update — download v0.33.0")).toBeTruthy();
    expect(buttonByText(container, "Download v0.33.0")).toBeTruthy();
  });

  it("falls back when the backend could not read the plugin name", async () => {
    const call = vi.fn();
    vi.stubGlobal("DeckyBackend", { call });
    const { container, getByText } = render(<UpdateNotice info={info({ plugin_name: "" })} />);
    fireEvent.click(buttonByText(container, "Update")!);
    await flushAsync();
    expect(call).not.toHaveBeenCalled();
    expect(getByText("Couldn't start the update — download v0.33.0")).toBeTruthy();
  });
});
