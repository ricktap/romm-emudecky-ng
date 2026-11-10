/** One ROM of the library grid (`get_library_overview`). */
export interface LibraryOverviewRom {
  rom_id: number;
  app_id: number;
  name: string;
  platform_slug: string;
  cover_path: string | null;
  installed: boolean;
  /** Queue entry of a ROM that is queued, downloading, paused, interrupted or failed; `null` otherwise. */
  download: { status: string; progress: number } | null;
  /** Problems to surface on the cover (save conflict, required BIOS missing). Empty until save sync lands. */
  issues: string[];
}

export interface LibraryOverviewPlatform {
  platform_slug: string;
  count: number;
  installed: number;
}

export type LibraryOverview =
  | { success: true; roms: LibraryOverviewRom[]; platforms: LibraryOverviewPlatform[] }
  | { success: false; reason?: string; message?: string };
