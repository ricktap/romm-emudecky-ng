import { describe, it, expect } from "vitest";
import { callArgs, downloadKey, isFirmware, isGameFile, isRom } from "./downloadKey";

describe("downloadKey", () => {
  it("keeps a ROM id and a firmware id with the same number apart", () => {
    expect(downloadKey({ rom_id: 7 })).toBe("rom:7");
    expect(downloadKey({ kind: "rom", rom_id: 7 })).toBe("rom:7");
    expect(downloadKey({ kind: "firmware", firmware_id: 7 })).toBe("firmware:7");
    expect(downloadKey({ rom_id: 7 })).not.toBe(downloadKey({ kind: "firmware", firmware_id: 7 }));
  });

  it("tells the kinds apart and builds the callable arguments", () => {
    expect(isFirmware({ kind: "firmware", firmware_id: 3 })).toBe(true);
    expect(isFirmware({ rom_id: 3 })).toBe(false);
    expect(callArgs({ rom_id: 7 })).toEqual([7]);
    expect(callArgs({ kind: "firmware", firmware_id: 3 })).toEqual([3, "firmware"]);
  });

  it("keeps a game file apart from a ROM and firmware with the same number", () => {
    const file = { kind: "rom_file" as const, rom_id: 0, file_id: 7 };
    expect(downloadKey(file)).toBe("rom_file:7");
    expect(new Set([downloadKey(file), downloadKey({ rom_id: 7 }), downloadKey({ kind: "firmware", firmware_id: 7 })]).size).toBe(3);
    expect(callArgs(file)).toEqual([7, "rom_file"]);
    expect([isGameFile(file), isRom(file), isFirmware(file)]).toEqual([true, false, false]);
    expect([isRom({ rom_id: 1 }), isRom({ kind: "rom", rom_id: 1 }), isRom({ kind: "firmware", firmware_id: 1 })]).toEqual([true, true, false]);
  });
});
