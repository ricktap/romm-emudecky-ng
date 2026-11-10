import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { Navigation, GamepadButton } from "@decky/ui";
import { getSettings, getSyncStats } from "../api/backend";
import { setDownloads } from "../utils/downloadStore";
import { setRommConnectionState } from "../utils/connectionState";

vi.mock("./LibraryGrid", () => ({ LibraryGrid: ({ search }: { search: string }) => <div>library-grid[{search}]</div> }));
vi.mock("./SettingsPage", () => ({ SettingsPage: () => <div>settings-page</div> }));
vi.mock("./SystemPage", () => ({ SystemPage: () => <div>system-page</div> }));
vi.mock("./DownloadQueue", () => ({ DownloadQueue: ({ onBack }: { onBack: () => void }) => <button onClick={onBack}>downloads-page</button> }));

import { LibraryRoute, headerStatus, hostOf } from "./LibraryRoute";
import { LIBRARY_ROUTE, openLibraryRoute, getLibraryPlatformFilter, setLibraryPlatformFilter } from "../utils/libraryRoute";

describe("LibraryRoute", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    openLibraryRoute("library");
    vi.clearAllMocks();
  });

  it("opens on the library tab with the grid", () => {
    render(<LibraryRoute />);
    expect(screen.getByText("library-grid[]")).toBeTruthy();
  });

  it("reaches the pages through the tabs: downloads, systems, settings", () => {
    render(<LibraryRoute />);
    fireEvent.click(screen.getByTestId("tab-downloads"));
    expect(screen.getByText("downloads-page")).toBeTruthy();
    fireEvent.click(screen.getByTestId("tab-systems"));
    expect(screen.getByText("system-page")).toBeTruthy();
    fireEvent.click(screen.getByTestId("tab-settings"));
    expect(screen.getByText("settings-page")).toBeTruthy();
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

  describe("header", () => {
    beforeEach(() => {
      vi.mocked(getSettings).mockResolvedValue({ romm_url: "https://romm.home.lan:8443/x" } as never);
      vi.mocked(getSyncStats).mockResolvedValue({ last_sync: new Date(Date.now() - 2 * 3600e3).toISOString() } as never);
      setRommConnectionState("connected");
      setDownloads([]);
    });

    it("shows the server host and when the library was last synced", async () => {
      render(<LibraryRoute />);
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(screen.getByText("romm.home.lan:8443 · synced 2h ago")).toBeTruthy();
    });

    it("typing in the search field hands the text to the grid", () => {
      render(<LibraryRoute />);
      fireEvent.change(screen.getByLabelText("Search games"), { target: { value: "zelda" } });
      expect(screen.getByText("library-grid[zelda]")).toBeTruthy();
    });

    it("the Sync button opens the Quick Access Menu", () => {
      const open = vi.fn();
      (Navigation as unknown as Record<string, unknown>).OpenQuickAccessMenu = open;
      render(<LibraryRoute />);
      fireEvent.click(screen.getByTestId("header-sync"));
      expect(open).toHaveBeenCalledTimes(1);
    });

    it("the Downloads tab carries the number of unfinished downloads", async () => {
      vi.useFakeTimers();
      setDownloads([{ rom_id: 1, status: "downloading" }, { rom_id: 2, status: "queued" }, { rom_id: 3, status: "completed" }] as never);
      render(<LibraryRoute />);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1100);
      });
      expect(screen.getByTestId("tab-downloads").textContent).toBe("Downloads2");
      vi.useRealTimers();
    });

    it("L1 and R1 move between the tabs and stop at the ends", () => {
      render(<LibraryRoute />);
      const root = screen.getByTestId("library-root");
      const press = (button: number) => fireEvent(root, new CustomEvent("decky-button-down", { detail: { button } }));
      press(GamepadButton.BUMPER_LEFT);
      expect(screen.getByText("library-grid[]")).toBeTruthy();
      press(GamepadButton.BUMPER_RIGHT);
      expect(screen.getByText("downloads-page")).toBeTruthy();
      press(GamepadButton.BUMPER_RIGHT);
      press(GamepadButton.BUMPER_RIGHT);
      press(GamepadButton.BUMPER_RIGHT);
      expect(screen.getByText("settings-page")).toBeTruthy();
    });
  });
});

describe("headerStatus / hostOf", () => {
  it("dots and wording follow the connection", () => {
    expect(headerStatus("h", null, "connected")).toEqual({ dot: "", text: "h" });
    expect(headerStatus("h", "2020-01-01T00:00:00Z", "checking").dot).toBe(" rm-amber");
    expect(headerStatus("h", null, "offline")).toEqual({ dot: " rm-red", text: "h · not connected" });
    expect(headerStatus("", null, "offline").text).toBe("RomM · not connected");
  });
  it("hostOf keeps an unparseable URL as is", () => {
    expect(hostOf("https://a.b:1/x")).toBe("a.b:1");
    expect(hostOf("nonsense")).toBe("nonsense");
  });
});
