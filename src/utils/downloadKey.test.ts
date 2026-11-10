import { describe, it, expect } from "vitest";
import { callArgs, downloadKey, isFirmware } from "./downloadKey";

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
});
