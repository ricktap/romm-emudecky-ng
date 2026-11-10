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
    const { getByLabelText, findByText } = render(<QamSettings onBack={onBack} />);
    await findByText("romm.local:8080");
    fireEvent.click(getByLabelText("Back"));
    expect(onBack).toHaveBeenCalled();
  });

  it("says the settings are unavailable when they cannot be read", async () => {
    vi.mocked(getSaveSyncSettings).mockRejectedValue(new Error("backend down"));
    const { findByText, queryByLabelText } = render(<QamSettings onBack={vi.fn()} />);
    expect(await findByText("Save sync settings unavailable")).toBeTruthy();
    expect(queryByLabelText("Save sync")).toBeNull();
  });

  it("changing a save-sync toggle saves the whole settings object", async () => {
    const { findByText, getByLabelText } = render(<QamSettings onBack={vi.fn()} />);
    await findByText("Sync after exit");
    await act(async () => {
      fireEvent.click(getByLabelText("Sync after exit"));
    });
    expect(updateSaveSyncSettings).toHaveBeenCalledWith({ ...saveSync, sync_after_exit: true });
  });

  it("hides the triggers while save sync is off", async () => {
    vi.mocked(getSaveSyncSettings).mockResolvedValue({ ...saveSync, save_sync_enabled: false });
    const { findByLabelText, queryByLabelText } = render(<QamSettings onBack={vi.fn()} />);
    await findByLabelText("Save sync");
    expect(queryByLabelText("Sync before launch")).toBeNull();
  });

  it("puts a toggle back when saving fails", async () => {
    vi.mocked(updateSaveSyncSettings).mockRejectedValue(new Error("fail"));
    const { findByText, getByLabelText } = render(<QamSettings onBack={vi.fn()} />);
    await findByText("Sync after exit");
    await act(async () => {
      fireEvent.click(getByLabelText("Sync after exit"));
    });
    await waitFor(() => expect(getByLabelText("Sync after exit").className).not.toContain("rm-on"));
  });

  it("the skip-preview toggle writes the shared preference", async () => {
    const { findByText, getByLabelText } = render(<QamSettings onBack={vi.fn()} />);
    await findByText("Sync after exit");
    fireEvent.click(getByLabelText("Skip sync preview"));
    expect(getSkipPreview()).toBe(true);
  });
});
