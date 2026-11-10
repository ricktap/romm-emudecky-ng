import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, fireEvent, act } from "@testing-library/react";

// The pages are not under test here; each shows its name and a way to navigate.
vi.mock("./MainPage", () => ({
  MainPage: ({ onNavigate }: { onNavigate: (p: "downloads" | "system" | "settings") => void }) => (
    <div>
      <span>MAIN PAGE</span>
      <button onClick={() => onNavigate("downloads")}>open downloads</button>
      <button onClick={() => onNavigate("system")}>open system</button>
      <button onClick={() => onNavigate("settings")}>open settings</button>
    </div>
  ),
}));
vi.mock("./DownloadQueue", () => ({ DownloadQueue: () => <span>DOWNLOADS PAGE</span> }));
vi.mock("./SystemPage", () => ({ SystemPage: () => <span>SYSTEM PAGE</span> }));
vi.mock("./SettingsPage", () => ({ SettingsPage: () => <span>SETTINGS PAGE</span> }));
vi.mock("./LibraryPage", () => ({ LibraryPage: () => <span>LIBRARY PAGE</span> }));
vi.mock("./DangerZone", () => ({ DangerZone: () => <span>DANGER PAGE</span> }));
vi.mock("../utils/scrollHelpers", () => ({ findOutermostScrollParent: () => null, scrollToTop: vi.fn() }));

import { QAMPanel } from "./QAMPanel";

const cancel = (el: HTMLElement) => fireEvent(el, new CustomEvent("decky-cancel", { bubbles: true, cancelable: true }));

describe("QAMPanel — B steps back to the main page", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  async function openAndGet(label: string, expected: string) {
    const utils = render(<QAMPanel />);
    await act(async () => {
      fireEvent.click(utils.getByText(label));
      await vi.advanceTimersByTimeAsync(100);
    });
    expect(utils.getByText(expected)).toBeInTheDocument();
    return utils;
  }

  it.each([
    ["open downloads", "DOWNLOADS PAGE"],
    ["open system", "SYSTEM PAGE"],
    ["open settings", "SETTINGS PAGE"],
  ])("from a sub-menu (%s), B goes back to the main page", async (label, expected) => {
    const { getByTestId, getByText, queryByText } = await openAndGet(label, expected);
    await act(async () => {
      cancel(getByTestId("focusable"));
      await vi.advanceTimersByTimeAsync(100);
    });
    expect(getByText("MAIN PAGE")).toBeInTheDocument();
    expect(queryByText(expected)).toBeNull();
  });

  it("the main page itself is not wrapped, so B there is left to close the panel as before", () => {
    // (the page is module state; earlier tests always end back on the main page)
    const { queryByTestId, getByText } = render(<QAMPanel />);
    expect(getByText("MAIN PAGE")).toBeInTheDocument();
    expect(queryByTestId("focusable")).toBeNull();
  });

  it("goes back one level at a time: sub-menu, then main page, and stays on main", async () => {
    const { getByTestId, getByText, queryByTestId } = await openAndGet("open system", "SYSTEM PAGE");
    await act(async () => {
      cancel(getByTestId("focusable"));
      await vi.advanceTimersByTimeAsync(100);
    });
    expect(getByText("MAIN PAGE")).toBeInTheDocument();
    expect(queryByTestId("focusable")).toBeNull();
  });
});
