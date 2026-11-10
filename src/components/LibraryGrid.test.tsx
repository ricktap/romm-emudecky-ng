import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render } from "@testing-library/react";
import { Navigation } from "@decky/ui";
import { toaster } from "@decky/api";

vi.mock("../api/backend", () => ({
  getLibraryOverview: vi.fn(),
  getPlatforms: vi.fn(),
  startDownload: vi.fn(),
  pauseDownload: vi.fn(),
  resumeDownload: vi.fn(),
}));

import { getLibraryOverview, getPlatforms, pauseDownload, resumeDownload, startDownload } from "../api/backend";
import { setDownloads } from "../utils/downloadStore";
import { emitDeckyEvent } from "../test-utils/decky-api-mock";
import { getLibraryPlatformFilter, setLibraryPlatformFilter } from "../utils/libraryRoute";
import { loadRommCollections } from "../utils/romCollections";
import { LibraryGrid } from "./LibraryGrid";

vi.mock("../utils/romCollections", () => ({ loadRommCollections: vi.fn() }));

const rom = (id: number, over = {}) => ({
  rom_id: id, app_id: 1000 + id, name: `Game ${id}`, platform_slug: "switch", cover_path: null, installed: false, download: null, issues: [], ...over,
});
const overview = (roms: unknown[], platforms: unknown[]) => ({ success: true, roms, platforms });
const flush = () => act(async () => { await Promise.resolve(); await Promise.resolve(); });
const PLATFORMS = { success: true, platforms: [{ id: 1, name: "Nintendo Switch", slug: "switch", rom_count: 3, sync_enabled: true }, { id: 2, name: "SNES", slug: "snes", rom_count: 1, sync_enabled: false }] };
const sample = () =>
  overview(
    [rom(1, { installed: true, name: "Zelda" }), rom(2, { name: "Mario", platform_slug: "snes" }), rom(3, { name: "Metroid", installed: true, issues: ["x"] }), rom(4, { name: "Kirby" })],
    [{ platform_slug: "switch", count: 3, installed: 2 }, { platform_slug: "snes", count: 1, installed: 0 }],
  );

async function renderGrid() {
  const utils = render(<LibraryGrid />);
  await flush();
  return utils;
}

describe("LibraryGrid", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    setDownloads([]);
    setLibraryPlatformFilter(null);
    vi.mocked(loadRommCollections).mockResolvedValue([]);
    vi.mocked(getLibraryOverview).mockResolvedValue(sample() as never);
    vi.mocked(getPlatforms).mockResolvedValue(PLATFORMS as never);
    vi.mocked(startDownload).mockResolvedValue({ success: true, message: "" });
    vi.mocked(pauseDownload).mockResolvedValue({ success: true, message: "" });
    vi.mocked(resumeDownload).mockResolvedValue({ success: true, message: "" });
  });

  it("shows every game as a cover with the platform rail and the filter counts", async () => {
    const { getByTestId, getByText } = await renderGrid();
    for (const id of [1, 2, 3, 4]) expect(getByTestId(`cover-${id}`)).toBeTruthy();
    expect(getByText("All games (4)")).toBeTruthy();
    expect(getByTestId("rail-switch").textContent).toBe("Nintendo Switch (3)");
    expect(getByTestId("filter-all").textContent).toBe("All 4");
    expect(getByTestId("filter-installed").textContent).toBe("Installed 2");
    expect(getByTestId("filter-romm_only").textContent).toBe("On RomM only 2");
    expect(getByTestId("filter-attention").textContent).toBe("Needs attention 1");
  });

  it("badges: installed, cloud, issue (an issue wins), and a disabled platform is dimmed in the rail", async () => {
    const { getByTestId } = await renderGrid();
    expect(getByTestId("cover-1").querySelector('[data-testid="badge-installed"]')).toBeTruthy();
    expect(getByTestId("cover-2").querySelector('[data-testid="badge-cloud"]')).toBeTruthy();
    expect(getByTestId("cover-3").querySelector('[data-testid="badge-issue"]')).toBeTruthy();
    expect(getByTestId("cover-3").querySelector('[data-testid="badge-installed"]')).toBeNull();
    expect(getByTestId("rail-snes").getAttribute("title")).toBe("Sync is off for this platform");
    expect(getByTestId("rail-switch").getAttribute("title")).toBeNull();
  });

  it("filter chips and the platform rail narrow the covers together; the choice is remembered", async () => {
    const { getByTestId, queryByTestId } = await renderGrid();
    fireEvent.click(getByTestId("filter-installed"));
    expect([1, 2, 3, 4].filter((id) => queryByTestId(`cover-${id}`))).toEqual([1, 3]);
    fireEvent.click(getByTestId("rail-snes"));
    expect(getLibraryPlatformFilter()).toBe("snes");
    expect([1, 2, 3, 4].filter((id) => queryByTestId(`cover-${id}`))).toEqual([]);
    expect(queryByTestId("cover-1")).toBeNull();
    fireEvent.click(getByTestId("filter-all"));
    expect([1, 2, 3, 4].filter((id) => queryByTestId(`cover-${id}`))).toEqual([2]);
  });

  it("starts narrowed to the platform chosen on a game page", async () => {
    setLibraryPlatformFilter("snes");
    const { queryByTestId } = await renderGrid();
    expect([1, 2, 3, 4].filter((id) => queryByTestId(`cover-${id}`))).toEqual([2]);
  });

  it("search filters by name, and says so when nothing matches", async () => {
    const { getAllByTestId, getByText, queryByTestId } = await renderGrid();
    fireEvent.change(getAllByTestId("text-field")[0]!, { target: { value: "zel" } });
    expect([1, 2, 3, 4].filter((id) => queryByTestId(`cover-${id}`))).toEqual([1]);
    fireEvent.change(getAllByTestId("text-field")[0]!, { target: { value: "nothing" } });
    expect(getByText("No games match.")).toBeTruthy();
  });

  it("A on a cover opens the game's Steam page", async () => {
    const { getByTestId } = await renderGrid();
    fireEvent(getByTestId("cover-1"), new CustomEvent("decky-activate"));
    expect(Navigation.Navigate).toHaveBeenCalledWith("/library/app/1001");
  });

  it("the live queue overlays progress, and a finished download refreshes the overview", async () => {
    const { getByTestId } = await renderGrid();
    setDownloads([{ rom_id: 4, rom_name: "Kirby", platform_name: "S", file_name: "k", status: "downloading", progress: 0, bytes_downloaded: 30, total_bytes: 100, resumable: true } as never]);
    await act(async () => { await vi.advanceTimersByTimeAsync(1100); });
    expect(getByTestId("cover-4").querySelector('[data-testid="badge-downloading"]')).toBeTruthy();
    expect((getByTestId("cover-4").querySelector('[data-testid="cover-progress"]') as HTMLElement).style.width).toBe("30%");
    vi.mocked(getLibraryOverview).mockClear();
    act(() => { emitDeckyEvent("download_complete", { rom_id: 4 }); });
    await flush();
    expect(getLibraryOverview).toHaveBeenCalledTimes(1);
  });

  it("refreshes after a sync and after an uninstall", async () => {
    await renderGrid();
    vi.mocked(getLibraryOverview).mockClear();
    act(() => { emitDeckyEvent("sync_complete", {}); });
    await flush();
    act(() => { globalThis.dispatchEvent(new CustomEvent("romm_rom_uninstalled", { detail: { rom_id: 1 } })); });
    await flush();
    expect(getLibraryOverview).toHaveBeenCalledTimes(2);
  });

  it("a cover whose image cannot load falls back to the name", async () => {
    const { getByTestId } = await renderGrid();
    fireEvent.error(getByTestId("cover-1").querySelector("img")!);
    expect(getByTestId("cover-1").querySelector('[data-testid="cover-fallback"]')?.textContent).toBe("Zelda");
  });

  it("empty library, failed load and a rejected load say so", async () => {
    vi.mocked(getLibraryOverview).mockResolvedValue(overview([], []) as never);
    expect((await renderGrid()).container.textContent).toContain("No games synced yet");
    vi.mocked(getLibraryOverview).mockResolvedValue({ success: false, message: "Not connected" } as never);
    expect((await renderGrid()).container.textContent).toContain("Not connected");
    vi.mocked(getLibraryOverview).mockRejectedValue(new Error("x"));
    expect((await renderGrid()).container.textContent).toContain("Could not load the library");
  });

  it("stops polling and listening on unmount", async () => {
    const spy = vi.spyOn(globalThis, "clearInterval");
    const { unmount } = await renderGrid();
    unmount();
    expect(spy).toHaveBeenCalled();
  });

  describe("X quick action", () => {
    const pressX = (el: HTMLElement) => fireEvent(el, new CustomEvent("decky-secondary"));

    it("downloads a ROM that is only on RomM, and does nothing for an installed one", async () => {
      const { getByTestId } = await renderGrid();
      await act(async () => pressX(getByTestId("cover-4")));
      expect(startDownload).toHaveBeenCalledWith(4);
      await act(async () => pressX(getByTestId("cover-1")));
      expect(startDownload).toHaveBeenCalledTimes(1);
      expect(pauseDownload).not.toHaveBeenCalled();
    });

    it("pauses a running transfer and resumes a stopped one", async () => {
      const { getByTestId } = await renderGrid();
      setDownloads([
        { rom_id: 4, rom_name: "K", platform_name: "S", file_name: "k", status: "downloading", progress: 0, bytes_downloaded: 1, total_bytes: 100, resumable: true } as never,
        { rom_id: 2, rom_name: "M", platform_name: "S", file_name: "m", status: "paused", progress: 0, bytes_downloaded: 1, total_bytes: 100, resumable: true } as never,
      ]);
      await act(async () => { await vi.advanceTimersByTimeAsync(1100); });
      await act(async () => pressX(getByTestId("cover-4")));
      expect(pauseDownload).toHaveBeenCalledWith(4);
      await act(async () => pressX(getByTestId("cover-2")));
      expect(resumeDownload).toHaveBeenCalledWith(2);
    });

    it("tells the user when the action is refused or fails", async () => {
      vi.mocked(startDownload).mockResolvedValue({ success: false, message: "Server unreachable" });
      const { getByTestId } = await renderGrid();
      await act(async () => pressX(getByTestId("cover-4")));
      expect(toaster.toast).toHaveBeenCalledWith(expect.objectContaining({ body: "Server unreachable" }));
      vi.mocked(startDownload).mockRejectedValue(new Error("x"));
      await act(async () => pressX(getByTestId("cover-4")));
      expect(toaster.toast).toHaveBeenCalledWith(expect.objectContaining({ body: "Could not download the download" }));
    });
  });

  describe("collections rail", () => {
    const withCollections = () =>
      vi.mocked(loadRommCollections).mockResolvedValue([{ name: "Favourites", appIds: new Set([1001, 1002]) }]);

    it("shows no Collections heading when the device has none", async () => {
      const { queryByText } = await renderGrid();
      expect(queryByText("Collections")).toBeNull();
    });

    it("lists this device's collections with counts and narrows the grid to one", async () => {
      withCollections();
      const { getByTestId, queryByTestId } = await renderGrid();
      expect(getByTestId("rail-collection-Favourites").textContent).toBe("Favourites (2)");
      fireEvent.click(getByTestId("rail-collection-Favourites"));
      expect(getByTestId("cover-1")).toBeTruthy();
      expect(getByTestId("cover-2")).toBeTruthy();
      expect(queryByTestId("cover-3")).toBeNull();
      expect(getByTestId("filter-all").textContent).toBe("All 2");
    });

    it("choosing a platform clears the collection and the other way round", async () => {
      withCollections();
      const { getByTestId, queryByTestId } = await renderGrid();
      fireEvent.click(getByTestId("rail-collection-Favourites"));
      fireEvent.click(getByTestId("rail-snes"));
      expect(getByTestId("cover-2")).toBeTruthy();
      expect(queryByTestId("cover-1")).toBeNull();
      fireEvent.click(getByTestId("rail-collection-Favourites"));
      expect(getByTestId("cover-1")).toBeTruthy();
      expect(getLibraryPlatformFilter()).toBeNull();
    });
  });
});
