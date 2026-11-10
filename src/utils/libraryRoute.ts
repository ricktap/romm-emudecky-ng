import { Navigation } from "@decky/ui";

export const LIBRARY_ROUTE = "/romm-library";

export type LibraryTab = "library" | "downloads" | "systems" | "settings";

// Module-level, like QAMPanel's page: the last tab survives leaving and re-entering the route.
let currentTab: LibraryTab = "library";

export function getLibraryTab(): LibraryTab {
  return currentTab;
}

export function setLibraryTab(tab: LibraryTab): void {
  currentTab = tab;
}

// The platform the Library tab is narrowed to ("Show in RomM Library" on a game page), or null for everything.
let platformFilter: string | null = null;

export function getLibraryPlatformFilter(): string | null {
  return platformFilter;
}

export function setLibraryPlatformFilter(slug: string | null): void {
  platformFilter = slug;
}

export interface OpenLibraryOptions {
  /** Narrow the Library tab to this platform; leaving it out clears an earlier filter. */
  platform?: string;
}

/** Open the full-screen library: close the Quick Access Menu first, then navigate. */
export function openLibraryRoute(tab?: LibraryTab, options: OpenLibraryOptions = {}): void {
  if (tab) currentTab = tab;
  platformFilter = options.platform ?? null;
  Navigation.CloseSideMenus();
  Navigation.Navigate(LIBRARY_ROUTE);
}
