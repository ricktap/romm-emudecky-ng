import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, fireEvent, act } from "@testing-library/react";

vi.mock("../../api/backend", () => ({ clearSyncCache: vi.fn() }));
vi.mock("../../utils/syncPreferences", () => {
  let skip = false;
  const listeners = new Set<() => void>();
  return {
    getSkipPreview: () => skip,
    setSkipPreview: (v: boolean) => {
      skip = v;
      listeners.forEach((l) => l());
    },
    onSyncPreferencesChange: (l: () => void) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
  };
});

import { clearSyncCache } from "../../api/backend";
import { LibrarySection, buildRegionOptions, AUTO_REGION, DEFAULT_REGION_LABEL } from "./LibrarySection";

describe("buildRegionOptions", () => {
  it("starts with the default and the anchors, adds library regions sorted and de-duplicated", () => {
    const opts = buildRegionOptions(["Japan", "Korea", "Brazil", "Korea"], AUTO_REGION);
    expect(opts.map((o) => o.label)).toEqual([DEFAULT_REGION_LABEL, "World", "USA", "Europe", "Japan", "Brazil", "Korea"]);
  });
  it("keeps a selected region that is no longer in the library", () => {
    expect((() => { const o = buildRegionOptions([], "Australia"); return o[o.length - 1]; })()).toEqual({ data: "Australia", label: "Australia" });
  });
});

describe("LibrarySection", () => {
  beforeEach(() => vi.mocked(clearSyncCache).mockReset());

  it("the region button cycles to the next region", () => {
    const onChange = vi.fn();
    const { getByTestId } = render(<LibrarySection preferredRegion="auto" libraryRegions={[]} onPreferredRegionChange={onChange} />);
    expect(getByTestId("choice-Preferred region").textContent).toBe("Default");
    fireEvent.click(getByTestId("choice-Preferred region"));
    expect(onChange).toHaveBeenCalledWith("World");
  });

  it("the skip-preview switch flips the shared preference", () => {
    const { getByLabelText } = render(<LibrarySection preferredRegion="auto" libraryRegions={[]} onPreferredRegionChange={vi.fn()} />);
    expect(getByLabelText("Skip preview").className).not.toContain("rm-on");
    fireEvent.click(getByLabelText("Skip preview"));
    expect(getByLabelText("Skip preview").className).toContain("rm-on");
  });

  it("Full resync clears the cache and shows the result, or the failure", async () => {
    vi.mocked(clearSyncCache).mockResolvedValueOnce({ success: true, message: "Cache cleared" });
    const { getByTestId, container } = render(<LibrarySection preferredRegion="auto" libraryRegions={[]} onPreferredRegionChange={vi.fn()} />);
    await act(async () => {
      fireEvent.click(getByTestId("full-resync"));
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(container.textContent).toContain("Cache cleared");
    vi.mocked(clearSyncCache).mockRejectedValueOnce(new Error("x"));
    await act(async () => {
      await new Promise((r) => setTimeout(r, 300));
      fireEvent.click(getByTestId("full-resync"));
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(container.textContent).toContain("Failed to clear sync cache");
  });
});
