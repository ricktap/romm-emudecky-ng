import { describe, expect, it } from "vitest";
import { crc32, forcedName, solveSuffix, stableGameId, steamShortcutId, steamShortcutIdQuoted } from "./shortcutId";

describe("crc32", () => {
  it("matches the standard check values", () => {
    expect(crc32("123456789")).toBe(0xcbf43926);
    expect(crc32("")).toBe(0);
    expect(crc32("a")).toBe(0xe8b7be43);
  });
  it("hashes UTF-8 bytes", () => {
    expect(crc32("é")).toBe(crc32(new Uint8Array([0xc3, 0xa9])));
  });
});

describe("steam ids", () => {
  it("sets the top bit and stays unsigned", () => {
    for (const [e, n] of [["/a/rom-launcher-ng", "Zelda"], ["/home/deck/x", ""], ["", "n"]] as const) {
      const id = steamShortcutId(e, n);
      expect(id).toBeGreaterThanOrEqual(0x80000000);
      expect(id).toBeLessThanOrEqual(0xffffffff);
      expect(id).toBe((crc32(e + n) | 0x80000000) >>> 0);
    }
  });
  it("the quoted form differs from the plain one", () => {
    expect(steamShortcutIdQuoted("/a", "n")).toBe((crc32('"/a"n') | 0x80000000) >>> 0);
    expect(steamShortcutIdQuoted("/a", "n")).not.toBe(steamShortcutId("/a", "n"));
  });
  it("a game's stable id is deterministic and depends only on the game", () => {
    expect(stableGameId(42)).toBe(stableGameId("42"));
    expect(stableGameId(42)).not.toBe(stableGameId(43));
    expect(stableGameId(42)).toBeGreaterThanOrEqual(0x80000000);
  });
});

describe("solveSuffix / forcedName", () => {
  const prefixes = ["", "/home/deck/homebrew/plugins/romm-emudecky-ng/bin/rom-launcher-ng", "/home/patrick/.local/x/bin/rom-launcher-ng", "ünïcode/πath"];
  it("hits any target CRC with printable ASCII, for any prefix", () => {
    let seed = 12345;
    const next = () => (seed = (Math.imul(seed, 1103515245) + 12345) >>> 0);
    for (const prefix of prefixes) {
      for (let i = 0; i < 200; i++) {
        const target = next();
        const suffix = solveSuffix(prefix, target)!;
        expect(suffix).not.toBeNull();
        expect(suffix).toHaveLength(7);
        expect(/^[@A-Z[\\\]^_]{7}$/.test(suffix)).toBe(true);
        expect(crc32(prefix + suffix)).toBe(target);
      }
    }
  });
  it("edge targets: 0 and 0xFFFFFFFF", () => {
    for (const t of [0, 0xffffffff, 0x80000000]) expect(crc32("p" + solveSuffix("p", t)!)).toBe(t);
  });
  it("forcedName gives the exact Steam id on every device, whatever the exe path", () => {
    const target = stableGameId(1234);
    for (const exe of ["/home/deck/h/bin/rom-launcher-ng", "/home/patrick/h/bin/rom-launcher-ng", "/var/home/u/bin/rom-launcher-ng"]) {
      const name = forcedName(exe, "romm-", target)!;
      expect(name.startsWith("romm-")).toBe(true);
      expect(steamShortcutId(exe, name)).toBe(target);
    }
  });
});
