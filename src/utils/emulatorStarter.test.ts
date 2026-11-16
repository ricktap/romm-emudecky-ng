import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../api/backend", () => ({
  getEmulatorStarter: vi.fn(),
  rememberEmulatorStarter: vi.fn(),
  logInfo: vi.fn(),
  logError: vi.fn(),
}));

import { getEmulatorStarter, logError, rememberEmulatorStarter } from "../api/backend";
import { ensureEmulatorStarter } from "./emulatorStarter";
import { stubAppStore, stubCollectionStore } from "../test-utils/steamStubs";

const EXE = "/home/u/Applications/Eden.AppImage";
const info = (over: Partial<{ exe: string | null; start_dir: string | null; app_id: number | null }> = {}) => ({
  success: true,
  name: "Eden",
  exe: EXE,
  start_dir: "/home/u/Applications",
  app_id: null,
  ...over,
});

let apps: { AddShortcut: ReturnType<typeof vi.fn>; SetShortcutName: ReturnType<typeof vi.fn>; SetShortcutExe: ReturnType<typeof vi.fn>; SetShortcutStartDir: ReturnType<typeof vi.fn> };

describe("ensureEmulatorStarter", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    apps = {
      AddShortcut: vi.fn().mockResolvedValue(777),
      SetShortcutName: vi.fn(),
      SetShortcutExe: vi.fn(),
      SetShortcutStartDir: vi.fn(),
    };
    vi.stubGlobal("SteamClient", { Apps: apps });
    stubCollectionStore([10]);
    stubAppStore({ 10: { display_name: "Mario Kart" } });
    vi.mocked(rememberEmulatorStarter).mockResolvedValue({ success: true });
  });

  it("adds an Eden shortcut that runs Eden itself and remembers it", async () => {
    vi.mocked(getEmulatorStarter).mockResolvedValue(info());
    expect(await ensureEmulatorStarter(0)).toBe("created");
    expect(apps.AddShortcut).toHaveBeenCalledWith("Eden", EXE, "", "");
    expect(apps.SetShortcutName).toHaveBeenCalledWith(777, "Eden");
    expect(apps.SetShortcutExe).toHaveBeenCalledWith(777, EXE);
    expect(apps.SetShortcutStartDir).toHaveBeenCalledWith(777, "/home/u/Applications");
    expect(rememberEmulatorStarter).toHaveBeenCalledWith(777);
  });

  it("adopts an existing Eden entry instead of adding a second one", async () => {
    stubCollectionStore([10, 11]);
    stubAppStore({ 10: { display_name: "Mario Kart" }, 11: { display_name: "eden" } });
    vi.mocked(getEmulatorStarter).mockResolvedValue(info());
    expect(await ensureEmulatorStarter(0)).toBe("adopted");
    expect(apps.AddShortcut).not.toHaveBeenCalled();
    expect(rememberEmulatorStarter).toHaveBeenCalledWith(11);
  });

  it("keeps the shortcut it made, and never makes one again after the user removed it", async () => {
    stubAppStore({ 777: { display_name: "Eden" } });
    vi.mocked(getEmulatorStarter).mockResolvedValue(info({ app_id: 777 }));
    expect(await ensureEmulatorStarter(0)).toBe("kept");
    stubAppStore({});
    expect(await ensureEmulatorStarter(0)).toBe("removed_by_user");
    expect(apps.AddShortcut).not.toHaveBeenCalled();
    expect(rememberEmulatorStarter).not.toHaveBeenCalled();
  });

  it("does nothing while Eden is not installed or Steam is not there", async () => {
    vi.mocked(getEmulatorStarter).mockResolvedValue(info({ exe: null, start_dir: null }));
    expect(await ensureEmulatorStarter(0)).toBe("not_installed");
    vi.stubGlobal("SteamClient", undefined);
    expect(await ensureEmulatorStarter(0)).toBe("unavailable");
    expect(apps.AddShortcut).not.toHaveBeenCalled();
  });

  it("a failed call is logged, never thrown; Steam refusing the shortcut remembers nothing", async () => {
    vi.mocked(getEmulatorStarter).mockRejectedValue(new Error("socket"));
    expect(await ensureEmulatorStarter(0)).toBe("unavailable");
    expect(logError).toHaveBeenCalledWith(expect.stringContaining("Could not add the Eden shortcut"));
    vi.mocked(getEmulatorStarter).mockResolvedValue(info());
    apps.AddShortcut.mockResolvedValue(0);
    expect(await ensureEmulatorStarter(0)).toBe("unavailable");
    expect(rememberEmulatorStarter).not.toHaveBeenCalled();
  });

  it("runs started together (plugin start and a sync) make one shortcut", async () => {
    vi.mocked(getEmulatorStarter).mockResolvedValue(info());
    const [a, b] = await Promise.all([ensureEmulatorStarter(0), ensureEmulatorStarter(0)]);
    expect([a, b]).toEqual(["created", "created"]);
    expect(apps.AddShortcut).toHaveBeenCalledTimes(1);
  });
});
