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
    expect(romChip({ installed: true, fileName: "Zelda.nsp" })).toMatchObject({ title: "ROM installed", detail: "Zelda.nsp", tone: "ok" });
    expect(romChip({ installed: true })).toMatchObject({ title: "ROM installed", detail: "Ready to play" });
    expect(romChip({ installed: false })).toMatchObject({ title: "Not downloaded", detail: "On RomM", tone: "neutral" });
  });

  it.each([
    ["queued", 0, "Queued", "Waiting for the transfer slot"],
    ["downloading", 42, "Downloading", "42%"],
    ["extracting", 100, "Extracting", "Unpacking the download"],
    ["paused", 30, "Download paused", "At 30%"],
  ])("a %s download wins over the install state", (status, done, title, detail) => {
    expect(romChip({ installed: true, download: dl(status, done) })).toMatchObject({ title, detail });
  });

  it("an interrupted download is a warning with its reason", () => {
    expect(romChip({ installed: false, download: dl("interrupted", 10, 100, "disk_full") })).toMatchObject({
      title: "Download interrupted",
      detail: "disk full",
      tone: "warn",
    });
  });

  it("a finished or failed queue entry does not hide the install state", () => {
    expect(romChip({ installed: true, fileName: "a", download: dl("completed", 100) })).toMatchObject({ title: "ROM installed", detail: "a" });
    expect(romChip({ installed: false, download: dl("failed") }).title).toBe("Not downloaded");
  });

  it("an unknown size shows 0% instead of NaN", () => {
    expect(romChip({ installed: false, download: dl("downloading", 5, 0) })).toMatchObject({ title: "Downloading", detail: "0%" });
  });
});

describe("biosChip", () => {
  const ok = (keys: SwitchKeyFile[]): FirmwareStatus => ({ success: true, keys });

  it("needs no BIOS outside the Switch, and shows the core", () => {
    expect(biosChip({ platformSlug: "snes", firmware: null, activeCoreLabel: "Snes9x" })).toMatchObject({
      title: "No BIOS needed",
      detail: "Snes9x",
      tone: "ok",
    });
    expect(biosChip({ platformSlug: "snes", firmware: null, activeCoreLabel: null }).detail).toBe("");
  });

  it("Switch: warns about missing required keys and offers Get", () => {
    expect(biosChip({ platformSlug: "switch", firmware: ok([key("prod.keys", "missing"), key("title.keys", "matches")]), activeCoreLabel: null })).toEqual({
      id: "bios", title: "BIOS missing", detail: "prod.keys on RomM", tone: "warn", fix: "get_bios", fixLabel: "Get",
    });
  });

  it("Switch: ready when every required key is there, naming the emulator", () => {
    expect(biosChip({ platformSlug: "switch", firmware: ok([key("prod.keys", "matches")]), activeCoreLabel: "Eden" })).toMatchObject({
      title: "BIOS ready",
      detail: "Eden",
      tone: "ok",
    });
  });

  it("Switch: unknown (not a warning) while the status is unavailable", () => {
    expect(biosChip({ platformSlug: "switch", firmware: null, activeCoreLabel: null })).toMatchObject({ title: "BIOS status unknown", tone: "neutral" });
    expect(biosChip({ platformSlug: "switch", firmware: { success: false }, activeCoreLabel: null }).tone).toBe("neutral");
  });
});

describe("savesChip: choosing the save", () => {
  it("a game whose save cannot be located yet offers one tap to choose it", () => {
    for (const label of ["Choose save folder", "Choose profile"]) {
      expect(savesChip({ saveSyncEnabled: true, status: "none", label })).toEqual({
        id: "saves", title: "Save not found", detail: label, tone: "warn", fix: "choose_save", fixLabel: "Choose",
      });
    }
  });
  it("other labels keep the plain state", () => {
    expect(savesChip({ saveSyncEnabled: true, status: "none", label: "Emulator save folder not found" })).toMatchObject({ tone: "ok", title: "Save sync on", detail: "Emulator save folder not found" });
    expect(savesChip({ saveSyncEnabled: false, status: "none", label: "Choose save folder" }).fix).toBeUndefined();
  });
});

describe("savesChip", () => {
  it("off when save sync is off", () => {
    expect(savesChip({ saveSyncEnabled: false, status: "conflict", label: "x" })).toMatchObject({ title: "Save sync off", detail: "Saves stay on this device", tone: "neutral" });
  });
  it("conflict is a warning with Resolve", () => {
    expect(savesChip({ saveSyncEnabled: true, status: "conflict", label: "Conflict" })).toMatchObject({ tone: "warn", fix: "resolve_conflict", fixLabel: "Resolve" });
  });
  it("synced shows when", () => {
    expect(savesChip({ saveSyncEnabled: true, status: "synced", label: "2h ago" })).toMatchObject({ title: "Saves synced", detail: "2h ago", tone: "ok" });
  });
  it("on without anything uploaded yet", () => {
    expect(savesChip({ saveSyncEnabled: true, status: "none", label: "" })).toMatchObject({ title: "Save sync on", detail: "Uploads after the first session" });
    expect(savesChip({ saveSyncEnabled: true, status: null, label: "No saves" }).detail).toBe("No saves");
  });
});
