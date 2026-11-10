/**
 * Pure logic of the library grid: what state a cover shows, which covers pass the filters, and the counts on the filter
 * chips. The component feeds it the overview rows and the live download queue.
 */

import type { DownloadItem, LibraryOverviewRom } from "../types";
import { isStoppedStatus, isUnfinishedStatus } from "./downloadStatus";

export type GridFilter = "all" | "installed" | "romm_only" | "attention";

export const GRID_FILTERS: { id: GridFilter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "installed", label: "Installed" },
  { id: "romm_only", label: "On RomM only" },
  { id: "attention", label: "Needs attention" },
];

export type CoverState = "installed" | "romm_only" | "queued" | "downloading" | "paused" | "interrupted";

export interface CoverStatus {
  state: CoverState;
  /** 0..100 while a transfer exists, otherwise null. */
  progress: number | null;
  /** Something to look at: an interrupted download or an issue flag. */
  issue: boolean;
}

export type LiveDownloads = ReadonlyMap<number, Pick<DownloadItem, "status" | "bytes_downloaded" | "total_bytes">>;

/** The live queue entries of ROM downloads by rom id (firmware jobs are not covers). */
export function liveDownloads(items: readonly DownloadItem[]): LiveDownloads {
  const map = new Map<number, DownloadItem>();
  for (const d of items) {
    if ((d.kind ?? "rom") === "rom" && isUnfinishedStatus(d.status)) map.set(d.rom_id, d);
  }
  return map;
}

const pct = (done: number, total: number) => (total > 0 ? Math.min(100, Math.floor((done / total) * 100)) : 0);

/** The state a cover shows. The live queue wins over the overview's snapshot, which can be seconds old. */
export function coverStatus(rom: LibraryOverviewRom, live: LiveDownloads): CoverStatus {
  const entry = live.get(rom.rom_id);
  const snapshot = rom.download;
  const status = entry?.status ?? snapshot?.status;
  const progress = entry
    ? pct(entry.bytes_downloaded, entry.total_bytes)
    : snapshot
      ? Math.min(100, Math.max(0, Math.floor(snapshot.progress)))
      : null;
  const issues = rom.issues.length > 0;
  switch (status) {
    case "queued":
      return { state: "queued", progress: 0, issue: issues };
    case "downloading":
    case "extracting":
      return { state: "downloading", progress, issue: issues };
    case "paused":
      return { state: "paused", progress, issue: issues };
    case "interrupted":
      return { state: "interrupted", progress, issue: true };
  }
  if (status && isStoppedStatus(status)) return { state: "paused", progress, issue: issues };
  return { state: rom.installed ? "installed" : "romm_only", progress: null, issue: issues };
}

export interface GridQuery {
  platform: string | null;
  /** Steam app ids of the chosen RomM collection; null = no collection chosen. */
  collection?: ReadonlySet<number> | null;
  filter: GridFilter;
  search: string;
}

const matchesFilter = (status: CoverStatus, filter: GridFilter): boolean => {
  switch (filter) {
    case "all":
      return true;
    case "installed":
      return status.state === "installed";
    case "romm_only":
      return status.state === "romm_only";
    case "attention":
      return status.issue;
  }
};

const matchesSearch = (rom: LibraryOverviewRom, search: string): boolean => {
  const needle = search.trim().toLowerCase();
  return needle === "" || rom.name.toLowerCase().includes(needle);
};

const inCollection = (rom: LibraryOverviewRom, collection?: ReadonlySet<number> | null): boolean =>
  !collection || collection.has(rom.app_id);

/** The covers to show: platform, filter chip and search together (a filter chip never ignores the platform). */
export function visibleRoms(roms: readonly LibraryOverviewRom[], query: GridQuery, live: LiveDownloads): LibraryOverviewRom[] {
  return roms.filter(
    (rom) =>
      (query.platform === null || rom.platform_slug === query.platform) &&
      inCollection(rom, query.collection) &&
      matchesSearch(rom, query.search) &&
      matchesFilter(coverStatus(rom, live), query.filter),
  );
}

/** Counts for the filter chips, under the current platform and search (so the numbers match what a chip would show). */
export function filterCounts(
  roms: readonly LibraryOverviewRom[],
  query: Pick<GridQuery, "platform" | "search" | "collection">,
  live: LiveDownloads,
): Record<GridFilter, number> {
  const counts: Record<GridFilter, number> = { all: 0, installed: 0, romm_only: 0, attention: 0 };
  for (const rom of roms) {
    if (query.platform !== null && rom.platform_slug !== query.platform) continue;
    if (!inCollection(rom, query.collection)) continue;
    if (!matchesSearch(rom, query.search)) continue;
    const status = coverStatus(rom, live);
    counts.all++;
    if (status.state === "installed") counts.installed++;
    if (status.state === "romm_only") counts.romm_only++;
    if (status.issue) counts.attention++;
  }
  return counts;
}

/** What X does on a cover: download a ROM that is not here, pause a running transfer, resume a stopped one. */
export type QuickAction = "download" | "pause" | "resume" | null;

export function quickAction(status: CoverStatus): QuickAction {
  switch (status.state) {
    case "romm_only":
      return "download";
    case "downloading":
    case "queued":
      return "pause";
    case "paused":
    case "interrupted":
      return "resume";
    case "installed":
      return null;
  }
}

/** The image URL Steam serves for a shortcut's custom portrait art. */
export function coverUrl(appId: number): string {
  return `https://steamloopback.host/customimages/${appId}p.png`;
}
