import { describe, it, expect } from "vitest";
import { interruptedLabel, isStoppedStatus, isUnfinishedStatus, stoppedCaptionSuffix } from "./downloadStatus";

describe("downloadStatus", () => {
  it("treats paused and interrupted as stopped", () => {
    expect(isStoppedStatus("paused")).toBe(true);
    expect(isStoppedStatus("interrupted")).toBe(true);
    expect(isStoppedStatus("downloading")).toBe(false);
    expect(isStoppedStatus("failed")).toBe(false);
  });

  it("classifies unfinished statuses", () => {
    for (const s of ["queued", "downloading", "extracting", "paused", "interrupted"]) {
      expect(isUnfinishedStatus(s)).toBe(true);
    }
    for (const s of ["completed", "failed", "cancelled"]) {
      expect(isUnfinishedStatus(s)).toBe(false);
    }
  });

  it("labels interrupt reasons, falling back for unknown ones", () => {
    expect(interruptedLabel("server_unreachable")).toBe("Interrupted — server unreachable");
    expect(interruptedLabel("disk_full")).toBe("Interrupted — disk full");
    expect(interruptedLabel("something_new")).toBe("Interrupted");
    expect(interruptedLabel(null)).toBe("Interrupted");
  });

  it("builds caption suffixes only for stopped downloads", () => {
    expect(stoppedCaptionSuffix("paused")).toBe(" — Paused");
    expect(stoppedCaptionSuffix("interrupted", "restart")).toBe(" — Interrupted — plugin restarted");
    expect(stoppedCaptionSuffix("downloading", "restart")).toBe("");
  });
});
