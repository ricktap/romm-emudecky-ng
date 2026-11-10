import { describe, it, expect } from "vitest";
import { recordBytes, formatSpeed, formatEta, forgetSpeed } from "./downloadSpeed";

describe("downloadSpeed", () => {
  it("measures speed once samples are far enough apart and smooths it", () => {
    expect(recordBytes("a", 0, 0)).toBe(0);
    expect(recordBytes("a", 100, 100)).toBe(0); // window too short
    expect(recordBytes("a", 2_000_000, 1000)).toBeCloseTo(2_000_000, -4);
    const next = recordBytes("a", 2_000_000, 2000); // stalled: speed decays, not zero
    expect(next).toBeGreaterThan(0);
    expect(next).toBeLessThan(2_000_000);
    forgetSpeed("a");
  });

  it("restarts the measurement when the byte count goes backwards", () => {
    recordBytes("b", 5000, 0);
    expect(recordBytes("b", 10, 2000)).toBe(0);
    forgetSpeed("b");
  });

  it("formats speed and ETA", () => {
    expect(formatSpeed(0)).toBe("");
    expect(formatSpeed(512 * 1024)).toBe("512 KB/s");
    expect(formatSpeed(3.5 * 1024 * 1024)).toBe("3.5 MB/s");
    expect(formatEta(100, 0)).toBe("");
    expect(formatEta(500, 100)).toBe("5 s left");
    expect(formatEta(6000, 100)).toBe("1 min left");
    expect(formatEta(100 * 3700, 100)).toBe("1 h 2 min left");
  });
});
