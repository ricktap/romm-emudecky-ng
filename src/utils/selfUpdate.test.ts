import { describe, it, expect, vi, beforeEach } from "vitest";
import { Navigation } from "@decky/ui";
import { startSelfUpdate, openReleasePage, DECKY_INSTALL_ROUTE, DECKY_INSTALL_TYPE_UPDATE } from "./selfUpdate";

const TARGET = {
  asset_url: "https://github.com/ricktap/romm-emudecky-ng/releases/download/v1.2.3/romm-emudecky-ng.zip",
  plugin_name: "Name From plugin.json",
  latest_version: "1.2.3",
  sha256: "ab".repeat(32),
};

describe("startSelfUpdate", () => {
  beforeEach(() => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
  });

  it("calls DeckyBackend.call with the exact install_plugin args (UPDATE = 2)", async () => {
    const call = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("DeckyBackend", { call });
    await expect(startSelfUpdate(TARGET)).resolves.toBe(true);
    expect(call).toHaveBeenCalledWith(
      "utilities/install_plugin",
      TARGET.asset_url,
      "Name From plugin.json",
      "1.2.3",
      TARGET.sha256,
      2,
    );
    expect(DECKY_INSTALL_ROUTE).toBe("utilities/install_plugin");
    expect(DECKY_INSTALL_TYPE_UPDATE).toBe(2);
  });

  it("falls back to DeckyBackend.callable when .call is missing", async () => {
    const inner = vi.fn().mockResolvedValue(undefined);
    const callable = vi.fn(() => inner);
    vi.stubGlobal("DeckyBackend", { callable });
    await expect(startSelfUpdate(TARGET)).resolves.toBe(true);
    expect(callable).toHaveBeenCalledWith("utilities/install_plugin");
    expect(inner).toHaveBeenCalledWith(TARGET.asset_url, TARGET.plugin_name, "1.2.3", TARGET.sha256, 2);
  });

  it("returns false when DeckyBackend is unavailable", async () => {
    vi.stubGlobal("DeckyBackend", undefined);
    await expect(startSelfUpdate(TARGET)).resolves.toBe(false);
  });

  it("returns false when DeckyBackend has neither call nor callable", async () => {
    vi.stubGlobal("DeckyBackend", {});
    await expect(startSelfUpdate(TARGET)).resolves.toBe(false);
  });

  it("returns false and logs when the call throws", async () => {
    vi.stubGlobal("DeckyBackend", { call: vi.fn().mockRejectedValue(new Error("boom")) });
    await expect(startSelfUpdate(TARGET)).resolves.toBe(false);
    expect(console.error).toHaveBeenCalledWith("[RomM] Decky install_plugin call failed:", expect.any(Error));
  });

  it.each(["asset_url", "plugin_name", "latest_version", "sha256"] as const)(
    "never calls Decky when %s is empty",
    async (field) => {
      const call = vi.fn();
      vi.stubGlobal("DeckyBackend", { call });
      await expect(startSelfUpdate({ ...TARGET, [field]: "" })).resolves.toBe(false);
      expect(call).not.toHaveBeenCalled();
    },
  );
});

describe("openReleasePage", () => {
  it("opens the URL in Steam's browser", () => {
    openReleasePage("https://github.com/ricktap/romm-emudecky-ng/releases/tag/v1.2.3");
    expect(Navigation.NavigateToExternalWeb).toHaveBeenCalledWith(
      "https://github.com/ricktap/romm-emudecky-ng/releases/tag/v1.2.3",
    );
  });

  it("swallows a navigation error", () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    vi.mocked(Navigation.NavigateToExternalWeb).mockImplementationOnce(() => {
      throw new Error("no browser");
    });
    expect(() => openReleasePage("https://example.invalid")).not.toThrow();
    expect(console.error).toHaveBeenCalledWith("[RomM] could not open release page:", expect.any(Error));
  });
});
