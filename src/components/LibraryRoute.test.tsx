import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { Navigation } from "@decky/ui";

vi.mock("./LibraryGrid", () => ({ LibraryGrid: () => <div>library-grid</div> }));
vi.mock("./SettingsPage", () => ({ SettingsPage: () => <div>settings-page</div> }));
vi.mock("./DangerZone", () => ({ DangerZone: () => <div>danger-zone</div> }));
vi.mock("./SystemPage", () => ({ SystemPage: () => <div>system-page</div> }));
vi.mock("./settings/LibrarySyncSection", () => ({ LibrarySyncSection: () => <div>library-sync</div> }));
vi.mock("./DownloadQueue", () => ({ DownloadQueue: ({ onBack }: { onBack: () => void }) => <button onClick={onBack}>downloads-page</button> }));

import { LibraryRoute } from "./LibraryRoute";
import { LIBRARY_ROUTE, openLibraryRoute, getLibraryPlatformFilter, setLibraryPlatformFilter } from "../utils/libraryRoute";

describe("LibraryRoute", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    openLibraryRoute("library");
    vi.clearAllMocks();
  });

  it("opens on the library tab with the grid", () => {
    render(<LibraryRoute />);
    expect(screen.getByText("library-grid")).toBeTruthy();
  });

  it("reaches the moved pages through the tabs: downloads, systems, settings with the sync section and danger zone", () => {
    render(<LibraryRoute />);
    fireEvent.click(screen.getByTestId("tab-downloads"));
    expect(screen.getByText("downloads-page")).toBeTruthy();
    fireEvent.click(screen.getByTestId("tab-systems"));
    expect(screen.getByText("system-page")).toBeTruthy();
    fireEvent.click(screen.getByTestId("tab-settings"));
    expect(screen.getByText("settings-page")).toBeTruthy();
    expect(screen.getByText("library-sync")).toBeTruthy();
    expect(screen.getByText("danger-zone")).toBeTruthy();
  });

  it("remembers the last tab when the route is opened again", () => {
    const first = render(<LibraryRoute />);
    fireEvent.click(screen.getByTestId("tab-systems"));
    first.unmount();
    render(<LibraryRoute />);
    expect(screen.getByText("system-page")).toBeTruthy();
  });

  it("a page's Back leaves the route (history back)", () => {
    render(<LibraryRoute />);
    fireEvent.click(screen.getByTestId("tab-downloads"));
    fireEvent.click(screen.getByText("downloads-page"));
    expect(Navigation.NavigateBack).toHaveBeenCalledTimes(1);
  });

  it("openLibraryRoute closes the side menus, then navigates; a tab argument selects it", () => {
    const order: string[] = [];
    vi.mocked(Navigation.CloseSideMenus).mockImplementation(() => void order.push("close"));
    vi.mocked(Navigation.Navigate).mockImplementation(() => void order.push("navigate"));
    openLibraryRoute("settings");
    expect(order).toEqual(["close", "navigate"]);
    expect(Navigation.Navigate).toHaveBeenCalledWith(LIBRARY_ROUTE);
    render(<LibraryRoute />);
    expect(screen.getByText("settings-page")).toBeTruthy();
  });

  it("opening the library without a platform clears an earlier filter; with one it keeps it", () => {
    setLibraryPlatformFilter("snes");
    openLibraryRoute("library");
    expect(getLibraryPlatformFilter()).toBeNull();
    openLibraryRoute("library", { platform: "switch" });
    expect(getLibraryPlatformFilter()).toBe("switch");
  });
});
