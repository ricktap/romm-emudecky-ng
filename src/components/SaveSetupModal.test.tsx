import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render } from "@testing-library/react";
import { toaster } from "@decky/api";

vi.mock("../api/backend", () => ({ getSaveTitleInfo: vi.fn(), setSaveTitleId: vi.fn(), setSaveProfile: vi.fn() }));

import { getSaveTitleInfo, setSaveProfile, setSaveTitleId } from "../api/backend";
import { SaveSetupModal } from "./SaveSetupModal";

const info = (over = {}) => ({
  success: true, title_id: null, source: null, proposal: null, profile: null, profiles: [],
  candidates: [{ title_id: "01008CF01BAAC000", files: 3, bytes: 2048, newest: 1_700_000_000 }], ...over,
});
const flush = () => act(async () => { await Promise.resolve(); await Promise.resolve(); });

async function open(over = {}) {
  vi.mocked(getSaveTitleInfo).mockResolvedValue(info(over) as never);
  const onDone = vi.fn();
  const close = vi.fn();
  const utils = render(<SaveSetupModal romId={7} onDone={onDone} close={close} />);
  await flush();
  return { ...utils, onDone, close };
}

describe("SaveSetupModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(setSaveTitleId).mockResolvedValue({ success: true });
    vi.mocked(setSaveProfile).mockResolvedValue({ success: true });
  });

  it("lists the save folders that exist and binds the one the user picks", async () => {
    const { getByText } = await open();
    await act(async () => { fireEvent.click(getByText(/01008CF01BAAC000 · 3 files · 2\.0 KB/)); });
    expect(setSaveTitleId).toHaveBeenCalledWith(7, "01008CF01BAAC000", "manual");
    expect(toaster.toast).toHaveBeenCalledWith(expect.objectContaining({ body: "Save folder set: 01008CF01BAAC000" }));
    expect(getSaveTitleInfo).toHaveBeenCalledTimes(2);
  });

  it("offers the folder that changed while playing as a learned choice", async () => {
    const { getByText } = await open({ proposal: "01008CF01BAAC000" });
    await act(async () => { fireEvent.click(getByText("Use 01008CF01BAAC000 (saved while you played)")); });
    expect(setSaveTitleId).toHaveBeenCalledWith(7, "01008CF01BAAC000", "learned");
  });

  it("with several profiles and none chosen, asks whose saves these are", async () => {
    const { getByText } = await open({ profiles: [{ id: "A".repeat(32), name: "Eden" }, { id: "B".repeat(32), name: "" }] });
    await act(async () => { fireEvent.click(getByText("Eden")); });
    expect(setSaveProfile).toHaveBeenCalledWith(7, "A".repeat(32));
    expect(getByText("B".repeat(32))).toBeTruthy();
  });

  it("does not ask for a profile when one is already in use", async () => {
    const { queryByText } = await open({ profiles: [{ id: "A".repeat(32), name: "Eden" }, { id: "B".repeat(32), name: "x" }], profile: "A".repeat(32) });
    expect(queryByText("Whose saves are these?")).toBeNull();
  });

  it("says what to do when there is nothing to choose from yet", async () => {
    const { getByText } = await open({ candidates: [] });
    expect(getByText("No save folders yet. Play the game once, then come back.")).toBeTruthy();
  });

  it("shows the bound folder, and a refused choice says why", async () => {
    vi.mocked(setSaveTitleId).mockResolvedValue({ success: false, message: "Not a Switch title ID" });
    const { getByText } = await open({ title_id: "0100152000022000" });
    expect(getByText("Save folder: 0100152000022000")).toBeTruthy();
    await act(async () => { fireEvent.click(getByText(/01008CF01BAAC000/)); });
    expect(toaster.toast).toHaveBeenCalledWith(expect.objectContaining({ body: "Not a Switch title ID" }));
  });

  it("a failed call says so instead of closing", async () => {
    vi.mocked(setSaveTitleId).mockRejectedValue(new Error("x"));
    const { getByText } = await open();
    await act(async () => { fireEvent.click(getByText(/01008CF01BAAC000/)); });
    expect(toaster.toast).toHaveBeenCalledWith(expect.objectContaining({ body: "Could not set the save folder" }));
  });

  it("Done closes and tells whether anything changed", async () => {
    const first = await open();
    fireEvent.click(first.getByText("Done"));
    expect(first.onDone).toHaveBeenCalledWith(false);
    expect(first.close).toHaveBeenCalled();
    first.unmount();
    const second = await open({ proposal: "01008CF01BAAC000" });
    await act(async () => { fireEvent.click(second.getByText(/saved while you played/)); });
    fireEvent.click(second.getByText("Done"));
    expect(second.onDone).toHaveBeenCalledWith(true);
  });

  it("a failed load offers a retry", async () => {
    vi.mocked(getSaveTitleInfo).mockRejectedValue(new Error("x"));
    const { getByText } = render(<SaveSetupModal romId={7} onDone={vi.fn()} />);
    await flush();
    vi.mocked(getSaveTitleInfo).mockResolvedValue(info() as never);
    await act(async () => { fireEvent.click(getByText("Could not load. Try again")); });
    expect(getByText(/01008CF01BAAC000/)).toBeTruthy();
  });
});
