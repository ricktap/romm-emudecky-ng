import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, waitFor } from "@testing-library/react";
import { Navigation } from "@decky/ui";

vi.mock("../api/backend", () => ({
  getSaveSyncSettings: vi.fn(),
  updateSaveSyncSettings: vi.fn(),
  getSettings: vi.fn(),
}));
vi.mock("../utils/scrollHelpers", () => ({ scrollToTop: vi.fn() }));

import { getSaveSyncSettings, getSettings, updateSaveSyncSettings } from "../api/backend";
import { getSkipPreview, setSkipPreview } from "../utils/syncPreferences";
import { QamSettings } from "./QamSettings";

const saveSync = { save_sync_enabled: true, sync_before_launch: true, sync_after_exit: false, default_slot: null, autocleanup_limit: 3 };

describe("QamSettings", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setSkipPreview(false);
    vi.mocked(getSettings).mockResolvedValue({ romm_url: "https://romm.local:8080/x" } as never);
    vi.mocked(getSaveSyncSettings).mockResolvedValue(saveSync);
    vi.mocked(updateSaveSyncSettings).mockResolvedValue({ success: true });
  });

  it("shows the server host and opens the full Settings tab", async () => {
    const { findByText, getByText } = render(<QamSettings onBack={vi.fn()} />);
    expect(await findByText("romm.local:8080")).toBeTruthy();
    fireEvent.click(getByText("All settings"));
    expect(Navigation.Navigate).toHaveBeenCalledWith("/romm-library");
  });

  it("Back calls onBack", async () => {
    const onBack = vi.fn();
    const { getByText, findByText } = render(<QamSettings onBack={onBack} />);
    await findByText("romm.local:8080");
    fireEvent.click(getByText("Back"));
    expect(onBack).toHaveBeenCalled();
  });

  it("says the settings are unavailable when they cannot be read", async () => {
    vi.mocked(getSaveSyncSettings).mockRejectedValue(new Error("backend down"));
    const { findByText, queryByText } = render(<QamSettings onBack={vi.fn()} />);
    expect(await findByText("Save sync settings unavailable")).toBeTruthy();
    expect(queryByText("Enable save sync")).toBeNull();
  });

  it("changing a save-sync toggle saves the whole settings object", async () => {
    const { findByText, getAllByTestId } = render(<QamSettings onBack={vi.fn()} />);
    await findByText("Sync after exit");
    // toggles in order: enable, before launch, after exit, skip preview
    await act(async () => {
      fireEvent.click(getAllByTestId("toggle-input")[2]!);
    });
    expect(updateSaveSyncSettings).toHaveBeenCalledWith({ ...saveSync, sync_after_exit: true });
  });

  it("hides the triggers while save sync is off", async () => {
    vi.mocked(getSaveSyncSettings).mockResolvedValue({ ...saveSync, save_sync_enabled: false });
    const { findByText, queryByText } = render(<QamSettings onBack={vi.fn()} />);
    await findByText("Enable save sync");
    expect(queryByText("Sync before launch")).toBeNull();
  });

  it("puts a toggle back when saving fails", async () => {
    vi.mocked(updateSaveSyncSettings).mockRejectedValue(new Error("fail"));
    const { findByText, getAllByTestId } = render(<QamSettings onBack={vi.fn()} />);
    await findByText("Sync after exit");
    await act(async () => {
      fireEvent.click(getAllByTestId("toggle-input")[2]!);
    });
    await waitFor(() => expect((getAllByTestId("toggle-input")[2] as HTMLInputElement).checked).toBe(false));
  });

  it("the skip-preview toggle writes the shared preference", async () => {
    const { findByText, getAllByTestId } = render(<QamSettings onBack={vi.fn()} />);
    await findByText("Sync after exit");
    fireEvent.click(getAllByTestId("toggle-input")[3]!);
    expect(getSkipPreview()).toBe(true);
  });
});
