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

/** Open the full-screen library: close the Quick Access Menu first, then navigate. */
export function openLibraryRoute(tab?: LibraryTab): void {
  if (tab) currentTab = tab;
  Navigation.CloseSideMenus();
  Navigation.Navigate(LIBRARY_ROUTE);
}
