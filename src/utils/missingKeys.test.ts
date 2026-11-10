import { describe, expect, it } from "vitest";
import { missingRequiredKeys } from "./missingKeys";
import type { FirmwareStatus, SwitchKeyFile } from "../types";

const key = (over: Partial<SwitchKeyFile>): SwitchKeyFile => ({
  file_name: "prod.keys",
  description: "",
  required: true,
  on_romm: true,
  firmware_id: 1,
  size: 1,
  emulators: [{ id: "eden", label: "Eden", state: "missing" }],
  ...over,
});

describe("missingRequiredKeys", () => {
  it("names required keys RomM has that an emulator lacks", () => {
    const status: FirmwareStatus = { success: true, keys: [key({}), key({ file_name: "title.keys" })] };
    expect(missingRequiredKeys(status)).toEqual(["prod.keys", "title.keys"]);
  });

  it("ignores optional keys, keys RomM does not host and keys every emulator has", () => {
    const status: FirmwareStatus = {
      success: true,
      keys: [
        key({ file_name: "console.keys", required: false }),
        key({ file_name: "a.keys", on_romm: false }),
        key({ file_name: "b.keys", emulators: [{ id: "eden", label: "Eden", state: "differs" }] }),
      ],
    };
    expect(missingRequiredKeys(status)).toEqual([]);
  });

  it("raises nothing when the status is unavailable or failed", () => {
    expect(missingRequiredKeys(null)).toEqual([]);
    expect(missingRequiredKeys({ success: false, keys: [key({})] })).toEqual([]);
  });
});
