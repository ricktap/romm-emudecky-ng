import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, render } from "@testing-library/react";

vi.mock("../api/backend", () => ({ getEmudeckHealth: vi.fn() }));
import { getEmudeckHealth } from "../api/backend";
import { EMUDECK_HEALTH_POLL_MS, emudeckTitle, movedSummary, useEmudeckHealth } from "./emudeckHealth";

const Probe = () => {
  const info = useEmudeckHealth();
  return <div data-testid="probe">{info ? info.health : "none"}</div>;
};
const info = (health: string, ok = false) => ({ health, ok, message: "m", storage_path: null }) as never;

describe("emudeckTitle", () => {
  it("has a distinct title per unhealthy state", () => {
    const titles = (["absent", "unreadable", "invalid", "root_missing"] as const).map(emudeckTitle);
    expect(new Set(titles).size).toBe(4);
    expect(emudeckTitle("root_missing")).toBe("EmuDeck storage not available");
  });
});

describe("movedSummary", () => {
  const moved = (installed: number, missing: number) => ({ health: "ok", ok: true, message: "", storage_path: "/new", moved: true, installed, missing }) as never;
  it("is null when the location did not move", () => {
    expect(movedSummary(null)).toBeNull();
    expect(movedSummary({ health: "ok", ok: true, message: "", storage_path: "/sd" })).toBeNull();
  });
  it("says how many installed games were not found", () => {
    expect(movedSummary(moved(5, 2))).toBe("The storage location changed. 2 of 5 installed games were not found.");
    expect(movedSummary(moved(5, 0))).toBe("The storage location changed. All 5 installed games were found.");
    expect(movedSummary(moved(0, 0))).toBe("The storage location changed. Nothing was installed yet.");
  });
});

describe("useEmudeckHealth", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.mocked(getEmudeckHealth).mockReset();
  });
  afterEach(() => vi.useRealTimers());

  it("is null until the first answer, then follows the card being taken out and put back", async () => {
    vi.mocked(getEmudeckHealth).mockResolvedValue(info("ok", true));
    const { getByTestId, unmount } = render(<Probe />);
    expect(getByTestId("probe").textContent).toBe("none");
    await act(async () => {
      await Promise.resolve();
    });
    expect(getByTestId("probe").textContent).toBe("ok");
    vi.mocked(getEmudeckHealth).mockResolvedValue(info("root_missing"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(EMUDECK_HEALTH_POLL_MS + 10);
    });
    expect(getByTestId("probe").textContent).toBe("root_missing");
    unmount();
    const calls = vi.mocked(getEmudeckHealth).mock.calls.length;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(EMUDECK_HEALTH_POLL_MS * 2);
    });
    expect(vi.mocked(getEmudeckHealth).mock.calls.length).toBe(calls);
  });

  it("keeps the last answer when a read fails", async () => {
    vi.mocked(getEmudeckHealth).mockResolvedValueOnce(info("invalid"));
    const { getByTestId } = render(<Probe />);
    await act(async () => {
      await Promise.resolve();
    });
    vi.mocked(getEmudeckHealth).mockRejectedValue(new Error("x"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(EMUDECK_HEALTH_POLL_MS + 10);
    });
    expect(getByTestId("probe").textContent).toBe("invalid");
  });
});
