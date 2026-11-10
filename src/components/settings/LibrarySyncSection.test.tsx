import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render } from "@testing-library/react";

vi.mock("../../api/backend", () => ({ clearSyncCache: vi.fn() }));

import { clearSyncCache } from "../../api/backend";
import { getSkipPreview, setSkipPreview } from "../../utils/syncPreferences";
import { LibrarySyncSection } from "./LibrarySyncSection";

describe("LibrarySyncSection", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setSkipPreview(false);
  });

  it("the toggle writes the shared skip-preview preference", () => {
    const { getByTestId } = render(<LibrarySyncSection />);
    fireEvent.click(getByTestId("toggle-input"));
    expect(getSkipPreview()).toBe(true);
  });

  it("follows a change made elsewhere (the QAM)", () => {
    const { getByTestId } = render(<LibrarySyncSection />);
    expect((getByTestId("toggle-input") as HTMLInputElement).checked).toBe(false);
    act(() => setSkipPreview(true));
    expect((getByTestId("toggle-input") as HTMLInputElement).checked).toBe(true);
  });

  it("Force full sync clears the cache and shows the backend's message", async () => {
    vi.mocked(clearSyncCache).mockResolvedValue({ success: true, message: "Next sync will do a full fetch" });
    const { getByText, findByText } = render(<LibrarySyncSection />);
    fireEvent.click(getByText("Force full sync"));
    expect(await findByText("Next sync will do a full fetch")).toBeTruthy();
    expect(clearSyncCache).toHaveBeenCalledTimes(1);
  });

  it("says so when clearing the cache fails", async () => {
    vi.mocked(clearSyncCache).mockRejectedValue(new Error("x"));
    const { getByText, findByText } = render(<LibrarySyncSection />);
    fireEvent.click(getByText("Force full sync"));
    expect(await findByText("Failed to clear sync cache")).toBeTruthy();
  });
});
