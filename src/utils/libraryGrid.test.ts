import { describe, expect, it } from "vitest";
import { coverStatus, coverUrl, filterCounts, liveDownloads, quickAction, visibleRoms, type CoverState } from "./libraryGrid";
import type { DownloadItem, LibraryOverviewRom } from "../types";

const rom = (id: number, over: Partial<LibraryOverviewRom> = {}): LibraryOverviewRom => ({
  rom_id: id, app_id: 1000 + id, name: `Game ${id}`, platform_slug: "switch", cover_path: null, installed: false, download: null, issues: [], ...over,
});
const dl = (id: number, status: string, done = 0, total = 100, kind?: "firmware"): DownloadItem =>
  ({ rom_id: id, rom_name: "x", platform_name: "p", file_name: "f", status, progress: 0, bytes_downloaded: done, total_bytes: total, resumable: true, ...(kind ? { kind } : {}) }) as never;

describe("liveDownloads", () => {
  it("keeps unfinished ROM jobs only", () => {
    const m = liveDownloads([dl(1, "downloading"), dl(2, "completed"), dl(3, "failed"), dl(4, "downloading", 0, 1, "firmware"), dl(5, "paused")]);
    expect([...m.keys()].sort()).toEqual([1, 5]);
  });
});

describe("coverStatus", () => {
  const none = liveDownloads([]);
  it("installed vs on RomM only", () => {
    expect(coverStatus(rom(1, { installed: true }), none)).toEqual({ state: "installed", progress: null, issue: false });
    expect(coverStatus(rom(1), none)).toEqual({ state: "romm_only", progress: null, issue: false });
  });
  it("the live queue wins over the overview snapshot, with progress", () => {
    const live = liveDownloads([dl(1, "downloading", 25, 100)]);
    expect(coverStatus(rom(1, { download: { status: "queued", progress: 0 } }), live)).toEqual({ state: "downloading", progress: 25, issue: false });
  });
  it("falls back to the snapshot when the live queue has nothing", () => {
    expect(coverStatus(rom(1, { download: { status: "paused", progress: 40.7 } }), none)).toEqual({ state: "paused", progress: 40, issue: false });
    expect(coverStatus(rom(1, { download: { status: "queued", progress: 0 } }), none).state).toBe("queued");
  });
  it("interrupted is an issue; an issue flag shows on any state", () => {
    expect(coverStatus(rom(1, { download: { status: "interrupted", progress: 10 } }), none)).toMatchObject({ state: "interrupted", issue: true });
    expect(coverStatus(rom(1, { installed: true, issues: ["save_conflict"] }), none)).toMatchObject({ state: "installed", issue: true });
  });
  it("unknown size is 0%, never NaN; a finished entry does not hide the install state", () => {
    expect(coverStatus(rom(1), liveDownloads([dl(1, "downloading", 5, 0)])).progress).toBe(0);
    expect(coverStatus(rom(1, { installed: true }), liveDownloads([dl(1, "completed")])).state).toBe("installed");
  });
  it("extracting counts as downloading", () => {
    expect(coverStatus(rom(1), liveDownloads([dl(1, "extracting", 100, 100)])).state).toBe("downloading");
  });
});

describe("visibleRoms and filterCounts", () => {
  const roms = [
    rom(1, { name: "Zelda", installed: true }),
    rom(2, { name: "Mario", platform_slug: "snes" }),
    rom(3, { name: "Metroid", installed: true, issues: ["x"] }),
    rom(4, { name: "Kirby" }),
  ];
  const live = liveDownloads([dl(4, "downloading", 50)]);
  const q = (over = {}) => ({ platform: null, filter: "all" as const, search: "", ...over });

  it("a collection narrows by app id, together with platform, chip and search", () => {
    const collection = new Set([1001, 1002, 1003]);
    expect(visibleRoms(roms, q({ collection }), live).map((r) => r.rom_id)).toEqual([1, 2, 3]);
    expect(visibleRoms(roms, q({ collection, platform: "snes" }), live).map((r) => r.rom_id)).toEqual([2]);
    expect(visibleRoms(roms, q({ collection, filter: "installed", search: "met" }), live).map((r) => r.rom_id)).toEqual([3]);
    expect(filterCounts(roms, { platform: null, search: "", collection }, live)).toMatchObject({ all: 3, installed: 2 });
    expect(visibleRoms(roms, q({ collection: null }), live)).toHaveLength(4);
  });

  it("filters by chip", () => {
    expect(visibleRoms(roms, q({ filter: "installed" }), live).map((r) => r.rom_id)).toEqual([1, 3]);
    expect(visibleRoms(roms, q({ filter: "romm_only" }), live).map((r) => r.rom_id)).toEqual([2]);
    expect(visibleRoms(roms, q({ filter: "attention" }), live).map((r) => r.rom_id)).toEqual([3]);
  });
  it("platform and search narrow together, case-insensitively", () => {
    expect(visibleRoms(roms, q({ platform: "switch", search: "  ME" }), live).map((r) => r.rom_id)).toEqual([3]);
    expect(visibleRoms(roms, q({ platform: "snes", filter: "installed" }), live)).toEqual([]);
  });
  it("counts follow platform and search, not the chip", () => {
    expect(filterCounts(roms, { platform: null, search: "" }, live)).toEqual({ all: 4, installed: 2, romm_only: 1, attention: 1 });
    expect(filterCounts(roms, { platform: "switch", search: "" }, live)).toEqual({ all: 3, installed: 2, romm_only: 0, attention: 1 });
    expect(filterCounts(roms, { platform: null, search: "zel" }, live)).toEqual({ all: 1, installed: 1, romm_only: 0, attention: 0 });
  });
});

describe("quickAction and coverUrl", () => {
  it("maps state to X", () => {
    const s = (state: CoverState) => ({ state, progress: null, issue: false });
    expect(quickAction(s("romm_only"))).toBe("download");
    expect(quickAction(s("downloading"))).toBe("pause");
    expect(quickAction(s("queued"))).toBe("pause");
    expect(quickAction(s("paused"))).toBe("resume");
    expect(quickAction(s("interrupted"))).toBe("resume");
    expect(quickAction(s("installed"))).toBeNull();
  });
  it("points at Steam's custom portrait art", () => {
    expect(coverUrl(123)).toBe("https://steamloopback.host/customimages/123p.png");
  });
});

import { platformAbbrev, platformTag, tintFor } from "./libraryGrid";

describe("cover helpers", () => {
  it("tintFor is stable per name and always one of the palette colours", () => {
    expect(tintFor("Zelda")).toBe(tintFor("Zelda"));
    expect(tintFor("Zelda")).toMatch(/^#[0-9a-f]{6}$/);
  });
  it("platformTag / platformAbbrev make short tags", () => {
    expect(platformTag("switch")).toBe("NSW");
    expect(platformTag("super_nintendo")).toBe("SUPE");
    expect(platformAbbrev("Nintendo 64")).toBe("N64");
    expect(platformAbbrev("Game Boy Advance")).toBe("GBA");
    expect(platformAbbrev("Nintendo Switch")).toBe("NSW");
    expect(platformAbbrev("Mega Drive / Genesis")).toBe("MDG");
    expect(platformAbbrev("")).toBe("?");
  });
});
