import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { Navigation } from "@decky/ui";

vi.mock("../api/backend", () => ({ getLibraryOverview: vi.fn() }));
vi.mock("./SettingsPage", () => ({ SettingsPage: () => <div>settings-page</div> }));
vi.mock("./DangerZone", () => ({ DangerZone: () => <div>danger-zone</div> }));
vi.mock("./SystemPage", () => ({ SystemPage: () => <div>system-page</div> }));
vi.mock("./DownloadQueue", () => ({ DownloadQueue: ({ onBack }: { onBack: () => void }) => <button onClick={onBack}>downloads-page</button> }));

import { getLibraryOverview } from "../api/backend";
import { LIBRARY_ROUTE, LibraryRoute, openLibraryRoute } from "./LibraryRoute";

const overview = {
  success: true,
  roms: [
    { rom_id: 1, app_id: 10, name: "A", platform_slug: "switch", cover_path: null, installed: true, download: null, issues: [] },
    { rom_id: 2, app_id: 11, name: "B", platform_slug: "switch", cover_path: null, installed: false, download: null, issues: [] },
  ],
  platforms: [{ platform_slug: "switch", count: 2, installed: 1 }],
};

describe("LibraryRoute", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getLibraryOverview).mockResolvedValue(overview as never);
  });

  it("opens on the library tab with the overview counts", async () => {
    render(<LibraryRoute />);
    expect(await screen.findByText("2 games, 1 installed")).toBeTruthy();
    expect(screen.getByText("switch: 2 games, 1 installed")).toBeTruthy();
  });

  it("shows the failure message when the overview cannot be loaded", async () => {
    vi.mocked(getLibraryOverview).mockResolvedValue({ success: false, message: "Not connected" } as never);
    render(<LibraryRoute />);
    expect(await screen.findByText("Not connected")).toBeTruthy();
  });

  it("shows the failure text when the overview call rejects", async () => {
    vi.mocked(getLibraryOverview).mockRejectedValue(new Error("socket closed"));
    render(<LibraryRoute />);
    expect(await screen.findByText("Could not load the library")).toBeTruthy();
  });

  it("reaches the moved pages through the tabs: downloads, systems, settings with the danger zone", async () => {
    render(<LibraryRoute />);
    fireEvent.click(screen.getByTestId("tab-downloads"));
    expect(screen.getByText("downloads-page")).toBeTruthy();
    fireEvent.click(screen.getByTestId("tab-systems"));
    expect(screen.getByText("system-page")).toBeTruthy();
    fireEvent.click(screen.getByTestId("tab-settings"));
    expect(screen.getByText("settings-page")).toBeTruthy();
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
});
