import { describe, it, expect } from "vitest";
import {
  WAITING_TEXT,
  attemptText,
  formatElapsed,
  phaseButtonLabel,
  phaseElapsedSeconds,
  phaseLines,
  retryRemainingSeconds,
} from "./downloadPhase";

const START = 1_000_000; // epoch seconds
const at = (secondsAfter: number) => (START + secondsAfter) * 1000;

describe("formatElapsed", () => {
  it("formats seconds, minutes and hours", () => {
    expect(formatElapsed(0)).toBe("0s");
    expect(formatElapsed(45.9)).toBe("45s");
    expect(formatElapsed(134)).toBe("2m 14s");
    expect(formatElapsed(125)).toBe("2m 05s");
    expect(formatElapsed(3600 + 120)).toBe("1h 02m");
    expect(formatElapsed(-5)).toBe("0s");
  });
});

describe("waiting phase", () => {
  const waiting = { phase: "waiting" as const, phase_since: START, attempt: 1, max_attempts: 3 };

  it("counts elapsed time locally from phase_since and never goes negative", () => {
    expect(phaseElapsedSeconds(waiting, at(134))).toBe(134);
    expect(phaseElapsedSeconds(waiting, at(-10))).toBe(0);
    expect(phaseElapsedSeconds({ phase: "waiting" }, at(5))).toBe(0);
  });

  it("says the archive MAY be preparing and shows elapsed time, without an invented percentage", () => {
    const [headline, detail] = phaseLines(waiting, at(134));
    expect(headline).toBe(WAITING_TEXT);
    expect(headline).toContain("may be preparing");
    expect(detail).toBe("Elapsed: 2m 14s");
    expect(`${headline} ${detail}`).not.toMatch(/%/);
  });

  it("names the attempt on a retry only", () => {
    expect(attemptText(waiting)).toBe("");
    const retry = { ...waiting, attempt: 2 };
    expect(attemptText(retry)).toBe("attempt 2 of 3");
    expect(phaseLines(retry, at(5))[1]).toBe("Elapsed: 5s · attempt 2 of 3");
  });

  it("has a short label for the game-page button", () => {
    expect(phaseButtonLabel(waiting, at(134))).toBe("Waiting for RomM… 2m 14s");
  });
});

describe("retrying phase", () => {
  const retrying = { phase: "retrying" as const, phase_since: START, attempt: 2, max_attempts: 3, retry_delay_s: 4 };

  it("counts the back-off down", () => {
    expect(retryRemainingSeconds(retrying, at(0))).toBe(4);
    expect(retryRemainingSeconds(retrying, at(3))).toBe(1);
    expect(retryRemainingSeconds(retrying, at(9))).toBe(0);
  });

  it("shows the attempt and the back-off", () => {
    expect(phaseLines(retrying, at(1))).toEqual(["Connection problem — retrying", "Attempt 2 of 3 in 3s"]);
    expect(phaseButtonLabel(retrying, at(1))).toBe("Retrying (2/3) in 3s");
  });
});
