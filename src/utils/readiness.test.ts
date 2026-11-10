import { describe, expect, it } from "vitest";
import { biosChip, romChip, savesChip } from "./readiness";
import type { FirmwareStatus, SwitchKeyFile } from "../types";

const key = (name: string, state: "missing" | "matches", over: Partial<SwitchKeyFile> = {}): SwitchKeyFile => ({
  file_name: name,
  description: "",
  required: true,
  on_romm: true,
  firmware_id: 1,
  size: 1,
  emulators: [{ id: "eden", label: "Eden", state }],
  ...over,
});
const dl = (status: string, done = 0, total = 100, reason?: string) =>
  ({ status, bytes_downloaded: done, total_bytes: total, reason }) as never;

describe("romChip", () => {
  it("says installed with the file name, or not downloaded", () => {
    expect(romChip({ installed: true, fileName: "Zelda.nsp" })).toMatchObject({ detail: "Installed · Zelda.nsp", tone: "ok" });
    expect(romChip({ installed: true })).toMatchObject({ detail: "Installed" });
    expect(romChip({ installed: false })).toMatchObject({ detail: "Not downloaded", tone: "neutral" });
  });

  it.each([
    ["queued", 0, "Queued"],
    ["downloading", 42, "Downloading 42%"],
    ["extracting", 100, "Extracting"],
    ["paused", 30, "Paused at 30%"],
  ])("a %s download wins over the install state", (status, done, detail) => {
    expect(romChip({ installed: true, download: dl(status, done) }).detail).toBe(detail);
  });

  it("an interrupted download is a warning with its reason", () => {
    expect(romChip({ installed: false, download: dl("interrupted", 10, 100, "disk_full") })).toMatchObject({
      detail: "Interrupted — disk full",
      tone: "warn",
    });
  });

  it("a finished or failed queue entry does not hide the install state", () => {
    expect(romChip({ installed: true, fileName: "a", download: dl("completed", 100) }).detail).toBe("Installed · a");
    expect(romChip({ installed: false, download: dl("failed") }).detail).toBe("Not downloaded");
  });

  it("an unknown size shows 0% instead of NaN", () => {
    expect(romChip({ installed: false, download: dl("downloading", 5, 0) }).detail).toBe("Downloading 0%");
  });
});

describe("biosChip", () => {
  const ok = (keys: SwitchKeyFile[]): FirmwareStatus => ({ success: true, keys });

  it("needs no BIOS outside the Switch, and shows the core", () => {
    expect(biosChip({ platformSlug: "snes", firmware: null, activeCoreLabel: "Snes9x" })).toMatchObject({
      detail: "No BIOS needed · Snes9x",
      tone: "neutral",
    });
    expect(biosChip({ platformSlug: "snes", firmware: null, activeCoreLabel: null }).detail).toBe("No BIOS needed");
  });

  it("Switch: warns about missing required keys and offers Get", () => {
    expect(biosChip({ platformSlug: "switch", firmware: ok([key("prod.keys", "missing"), key("title.keys", "matches")]), activeCoreLabel: null })).toEqual({
      id: "bios", title: "BIOS", detail: "Missing prod.keys", tone: "warn", fix: "get_bios", fixLabel: "Get",
    });
  });

  it("Switch: ready when every required key is there, naming the emulator", () => {
    expect(biosChip({ platformSlug: "switch", firmware: ok([key("prod.keys", "matches")]), activeCoreLabel: "Eden" })).toMatchObject({
      detail: "Keys ready · Eden",
      tone: "ok",
    });
  });

  it("Switch: unknown (not a warning) while the status is unavailable", () => {
    expect(biosChip({ platformSlug: "switch", firmware: null, activeCoreLabel: null })).toMatchObject({ detail: "Keys: status unknown", tone: "neutral" });
    expect(biosChip({ platformSlug: "switch", firmware: { success: false }, activeCoreLabel: null }).tone).toBe("neutral");
  });
});

describe("savesChip", () => {
  it("off when save sync is off", () => {
    expect(savesChip({ saveSyncEnabled: false, status: "conflict", label: "x" })).toMatchObject({ detail: "Save sync off", tone: "neutral" });
  });
  it("conflict is a warning with Resolve", () => {
    expect(savesChip({ saveSyncEnabled: true, status: "conflict", label: "Conflict" })).toMatchObject({ tone: "warn", fix: "resolve_conflict", fixLabel: "Resolve" });
  });
  it("synced shows when", () => {
    expect(savesChip({ saveSyncEnabled: true, status: "synced", label: "2h ago" })).toMatchObject({ detail: "Synced · 2h ago", tone: "ok" });
    expect(savesChip({ saveSyncEnabled: true, status: "synced", label: "" }).detail).toBe("Synced");
  });
  it("on without anything uploaded yet", () => {
    expect(savesChip({ saveSyncEnabled: true, status: "none", label: "" }).detail).toBe("On, nothing uploaded yet");
    expect(savesChip({ saveSyncEnabled: true, status: null, label: "No saves" }).detail).toBe("On · No saves");
  });
});
