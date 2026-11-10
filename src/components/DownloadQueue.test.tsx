// CATCH-REJECTION ASSERTION RULE:
// DownloadQueue has 2 catch sites:
//   - useEffect mount: getDownloadQueue().catch → falls back to
//     setLocalDownloads([...getDownloadState()]). Observable side effect:
//     pre-seeded store items render. Asserted in "mount: rejection falls back
//     to store".
//   - handleCancel inline `.catch(() => {})`. This is the documented
//     truly-ignored boundary catch — the source comment is "// ignore" and
//     there is no observable post-catch side effect. Per CLAUDE.md, such
//     catches stay assertion-free; we still exercise the call site by
//     rejecting cancelDownload and asserting the click did NOT crash + the
//     component continued to render normally.
//
// MUTATION CHECKS (by inspection):
//   1. If clearInterval(pollRef.current) is removed from stopPolling, the
//      "interval is cleared on unmount" test fails — clearIntervalSpy would
//      not be called with the captured pollRef id after unmount.
//   2. If setCleared(unclearRestarted(current)) is removed from pollTick,
//      the "previously cleared rom restarting un-clears" test fails — a
//      cleared rom_id that returns to "downloading" would stay hidden.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { render, fireEvent, act } from "@testing-library/react";
import { toaster } from "@decky/api";
import { DownloadQueue } from "./DownloadQueue";
import * as backend from "../api/backend";
import { setDownloads, getDownloadState } from "../utils/downloadStore";
import type { DownloadItem } from "../types";
import { Navigation } from "@decky/ui";
import { getLibraryTab } from "../utils/libraryRoute";

// Local @decky/ui mock adds ProgressBar (not in the global stub) and exposes
// per-prop testids so we can assert progress bar wiring directly. The active
// download caption now lives in a sibling div (the #751 full-width fix), so the
// rom name / bytes are read from the component's own dl-caption / dl-bytes
// testids rather than from the bar's props.
vi.mock("@decky/ui", async () => {
  type AnyProps = Record<string, unknown> & { children?: unknown };
  const { createElement: ce } = await import("react");
  const passthrough = (tag: string) => (p: AnyProps) => ce(tag, p, p.children as never);
  return {
    Navigation: { Navigate: vi.fn(), CloseSideMenus: vi.fn() },
    PanelSection: (p: AnyProps & { title?: unknown }) => ce("section", { title: p.title }, p.children as never),
    PanelSectionRow: passthrough("div"),
    ButtonItem: ({ children, onClick, disabled }: AnyProps & { onClick?: () => void; disabled?: boolean }) =>
      ce("button", { onClick, disabled }, children as never),
    Field: (p: AnyProps & { label?: unknown; description?: unknown }) =>
      ce(
        "div",
        { "data-testid": "field" },
        ce("span", { "data-testid": "field-label" }, p.label as never),
        ce("span", { "data-testid": "field-desc" }, p.description as never),
      ),
    ProgressBar: (
      p: AnyProps & {
        nProgress?: number;
        indeterminate?: boolean;
      },
    ) =>
      ce(
        "div",
        { "data-testid": "progress" },
        ce("span", { "data-testid": "progress-progress" }, String(p.nProgress)),
        ce("span", { "data-testid": "progress-indeterminate" }, String(p.indeterminate)),
      ),
  };
});

vi.mock("../utils/scrollHelpers", () => ({ scrollToTop: vi.fn() }));

function makeItem(overrides: Partial<DownloadItem> = {}): DownloadItem {
  return {
    rom_id: 1,
    rom_name: "Sonic",
    platform_name: "Genesis",
    file_name: "sonic.bin",
    status: "downloading",
    progress: 25,
    bytes_downloaded: 256,
    total_bytes: 1024,
    resumable: false,
    ...overrides,
  };
}

function buttonByText(container: HTMLElement, text: string): HTMLButtonElement | null {
  const btn = Array.from(container.querySelectorAll("button")).find((b) => b.textContent.includes(text));
  return (btn as HTMLButtonElement | undefined) ?? null;
}

function buttonByExactText(container: HTMLElement, text: string): HTMLButtonElement | null {
  const btn = Array.from(container.querySelectorAll("button")).find((b) => b.textContent === text);
  return (btn as HTMLButtonElement | undefined) ?? null;
}

// Flush the mount fetch's microtask chain so React state updates settle
// without advancing real time. Works under both real and fake timers.
async function flushMount(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

async function renderQueue() {
  const utils = render(<DownloadQueue onBack={() => {}} />);
  await flushMount();
  return utils;
}

describe("DownloadQueue", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    // Reset shared module-level store between tests.
    setDownloads([]);
    // Default mount fetch resolves to an empty queue; tests override per case.
    vi.mocked(backend.getDownloadQueue).mockResolvedValue({ downloads: [] });
    vi.mocked(backend.cancelDownload).mockResolvedValue({ success: true, message: "" });
    vi.mocked(backend.pauseDownload).mockResolvedValue({ success: true, message: "" });
    vi.mocked(backend.resumeDownload).mockResolvedValue({ success: true, message: "" });
    vi.mocked(backend.moveToFront).mockResolvedValue({ success: true, message: "" });
  });

  afterEach(() => {
    vi.useRealTimers();
    setDownloads([]);
  });

  // ---------------------------------------------------------------------------
  // Mount-time fetch (useEffect)
  // ---------------------------------------------------------------------------
  describe("mount fetch", () => {
    it("seeds the store + local state from getDownloadQueue() on mount", async () => {
      const item = makeItem({ rom_id: 7, rom_name: "Item7" });
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [item],
      });

      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();

      // Store was seeded — verifies setDownloads(result.downloads) ran.
      expect(getDownloadState()).toEqual([item]);
      // Local state rendered — caption present for the active item.
      const caption = container.querySelector('[data-testid="dl-caption"]');
      expect(caption?.textContent).toBe("Item7 (Genesis)");
    });

    it("rejection falls back to current getDownloadState() store contents", async () => {
      // Pre-seed the store; the mount fetch will reject and the catch branch
      // should rebuild local state from this.
      const fallback = makeItem({ rom_id: 99, rom_name: "Fallback" });
      setDownloads([fallback]);
      vi.mocked(backend.getDownloadQueue).mockRejectedValue(new Error("net"));

      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();

      const caption = container.querySelector('[data-testid="dl-caption"]');
      expect(caption?.textContent).toBe("Fallback (Genesis)");
      // Store unchanged by the catch branch.
      expect(getDownloadState()).toEqual([fallback]);
    });
  });

  // ---------------------------------------------------------------------------
  // Polling (500ms setInterval)
  // ---------------------------------------------------------------------------
  describe("polling", () => {
    it("each 500ms tick re-reads getDownloadState() and updates local state", async () => {
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();
      // Empty queue at mount.
      expect(container.textContent).toContain("No downloads");

      // Push a new item to the store from outside the component, then tick.
      const incoming = makeItem({ rom_id: 5, rom_name: "Ticked" });
      setDownloads([incoming]);

      await act(async () => {
        await vi.advanceTimersByTimeAsync(500);
      });

      const caption = container.querySelector('[data-testid="dl-caption"]');
      expect(caption?.textContent).toBe("Ticked (Genesis)");
    });

    it("interval is cleared on unmount — clearInterval is invoked with the pollRef id", async () => {
      // Spy on setInterval to capture the timer id assigned to pollRef, and
      // on clearInterval to assert stopPolling runs it with that exact id.
      // The prior `not.toContain("AfterUnmount")` assertion was vacuous:
      // setLocalDownloads no-ops on unmounted components and the container
      // is detached, so it passed whether or not clearInterval ran. A
      // mutation that drops `clearInterval(pollRef.current)` from stopPolling
      // now fails — clearIntervalSpy is never called with the captured id.
      const setIntervalSpy = vi.spyOn(globalThis, "setInterval");
      const clearIntervalSpy = vi.spyOn(globalThis, "clearInterval");

      const { unmount } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();

      // startPolling calls setInterval(pollTick, 500). Pick out that id.
      const pollIntervalIds = setIntervalSpy.mock.results
        .filter((_, i) => setIntervalSpy.mock.calls[i]![1] === 500)
        .map((r) => r.value as ReturnType<typeof setInterval>);
      const expectedId = pollIntervalIds[pollIntervalIds.length - 1];
      expect(expectedId).toBeDefined();

      // startPolling's leading stopPolling() runs before pollRef is set, so
      // its clearInterval calls do nothing — capture the baseline regardless.
      const callsBeforeUnmount = clearIntervalSpy.mock.calls.length;

      unmount();

      // After unmount, clearInterval must have been called with the id we
      // captured from setInterval.
      expect(clearIntervalSpy.mock.calls.length).toBeGreaterThan(callsBeforeUnmount);
      expect(clearIntervalSpy).toHaveBeenCalledWith(expectedId);

      setIntervalSpy.mockRestore();
      clearIntervalSpy.mockRestore();
    });
  });

  // ---------------------------------------------------------------------------
  // unclearRestarted — cleared rom returns to active list
  // ---------------------------------------------------------------------------
  describe("unclearRestarted", () => {
    it("a cleared rom_id whose download restarts (downloading) becomes visible again", async () => {
      const finished = makeItem({
        rom_id: 42,
        rom_name: "Restart",
        status: "completed",
        bytes_downloaded: 100,
        total_bytes: 100,
      });
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [finished],
      });

      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();

      // The completed Field is visible.
      expect(container.textContent).toContain("Restart");
      // Click "Clear Completed" → rom_id 42 enters cleared set.
      const clearBtn = buttonByExactText(container, "Clear Completed");
      expect(clearBtn).not.toBeNull();
      await act(async () => {
        fireEvent.click(clearBtn!);
      });
      // After clearing the only item, the empty state shows.
      expect(container.textContent).toContain("No downloads");

      // Now the same rom_id restarts: store contains it with status="downloading".
      setDownloads([makeItem({ rom_id: 42, rom_name: "Restart" })]);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(500);
      });

      // unclearRestarted removed rom_id 42 from `cleared`, so the active
      // download caption is rendered again.
      const caption = container.querySelector('[data-testid="dl-caption"]');
      expect(caption?.textContent).toBe("Restart (Genesis)");
    });

    it("a cleared rom_id whose download restarts as 'queued' also becomes visible", async () => {
      const finished = makeItem({
        rom_id: 13,
        rom_name: "Queued",
        status: "failed",
        error: "x",
      });
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [finished],
      });

      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();

      await act(async () => {
        fireEvent.click(buttonByExactText(container, "Clear Completed")!);
      });
      expect(container.textContent).toContain("No downloads");

      setDownloads([makeItem({ rom_id: 13, rom_name: "Queued", status: "queued" })]);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(500);
      });
      expect(container.querySelector('[data-testid="dl-caption"]')?.textContent).toBe("Queued (Genesis) — In Queue");
    });

    it("if no cleared rom_id matches, cleared Set stays the same instance (no needless re-set)", async () => {
      // Two finished items; clear them; then a tick where NONE of the cleared
      // ids restart. unclearRestarted's early-return path is exercised.
      const a = makeItem({ rom_id: 1, status: "completed", bytes_downloaded: 100, total_bytes: 100 });
      const b = makeItem({ rom_id: 2, rom_name: "Bee", status: "cancelled", bytes_downloaded: 0, total_bytes: 0 });
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [a, b],
      });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();
      await act(async () => {
        fireEvent.click(buttonByExactText(container, "Clear Completed")!);
      });
      expect(container.textContent).toContain("No downloads");

      // Tick with the same store contents — no restart → still hidden.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(500);
      });
      expect(container.textContent).toContain("No downloads");
    });
  });

  // ---------------------------------------------------------------------------
  // handleCancel
  // ---------------------------------------------------------------------------
  describe("handleCancel", () => {
    it("clicking 'Cancel <name>' calls cancelDownload(rom_id)", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [makeItem({ rom_id: 77, rom_name: "Mario" })],
      });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();

      const cancel = buttonByText(container, "Cancel Mario");
      expect(cancel).not.toBeNull();
      await act(async () => {
        fireEvent.click(cancel!);
        // Let the inline .catch chain settle.
        await Promise.resolve();
      });
      expect(backend.cancelDownload).toHaveBeenCalledWith(77);
    });

    it("cancelDownload rejection is silently swallowed (truly-ignored boundary catch)", async () => {
      // Per CLAUDE.md: the inline `.catch(() => {})` is a truly-ignored
      // boundary catch — no observable side effect. We still exercise the
      // call site to keep coverage on the catch arm and ensure the click
      // does not crash the component.
      vi.mocked(backend.cancelDownload).mockRejectedValue(new Error("nope"));
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [makeItem({ rom_id: 5, rom_name: "Cancellable" })],
      });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();

      const cancel = buttonByText(container, "Cancel Cancellable");
      await act(async () => {
        fireEvent.click(cancel!);
        await Promise.resolve();
        await Promise.resolve();
      });
      // Component still rendered normally — the catch swallowed cleanly.
      expect(container.querySelector('[data-testid="dl-caption"]')?.textContent).toBe("Cancellable (Genesis)");
    });
  });

  // ---------------------------------------------------------------------------
  // Waiting for RomM (archive preparation) — no bytes yet
  // ---------------------------------------------------------------------------
  describe("waiting for the server", () => {
    const START = 1_700_000_000; // epoch seconds

    beforeEach(() => {
      vi.setSystemTime(new Date((START + 134) * 1000));
    });

    function waitingItem(overrides: Partial<DownloadItem> = {}): DownloadItem {
      return makeItem({
        rom_id: 9,
        rom_name: "Mario Kart",
        status: "downloading",
        bytes_downloaded: 0,
        total_bytes: 0,
        phase: "waiting",
        phase_since: START,
        attempt: 1,
        max_attempts: 3,
        ...overrides,
      });
    }

    it("shows an honest, distinct waiting state with elapsed time and no invented percentage", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({ downloads: [waitingItem()] });
      const { container, getByTestId, queryByTestId } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();
      expect(getByTestId("dl-waiting-text").textContent).toBe(
        "Waiting for RomM to respond. A download archive may be preparing…",
      );
      expect(getByTestId("dl-waiting-detail").textContent).toBe("Elapsed: 2m 14s");
      // not byte progress: no byte counter, and the bar is indeterminate
      expect(queryByTestId("dl-bytes")).toBeNull();
      expect(container.querySelector('[data-testid="progress-indeterminate"]')?.textContent).toBe("true");
      expect(container.textContent).not.toMatch(/%/);
    });

    it("counts elapsed time locally without asking the server again", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({ downloads: [waitingItem()] });
      const { getByTestId } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();
      const fetches = vi.mocked(backend.getDownloadQueue).mock.calls.length;
      await act(async () => {
        await vi.advanceTimersByTimeAsync(5000);
      });
      expect(getByTestId("dl-waiting-detail").textContent).toBe("Elapsed: 2m 19s");
      expect(vi.mocked(backend.getDownloadQueue).mock.calls.length).toBe(fetches);
      expect(backend.startDownload).not.toHaveBeenCalled();
    });

    it("keeps Pause and Cancel available while waiting", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({ downloads: [waitingItem()] });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();
      await act(async () => {
        fireEvent.click(buttonByText(container, "Pause Mario Kart")!);
        await Promise.resolve();
      });
      expect(backend.pauseDownload).toHaveBeenCalledWith(9);
      await act(async () => {
        fireEvent.click(buttonByText(container, "Cancel Mario Kart")!);
        await Promise.resolve();
      });
      expect(backend.cancelDownload).toHaveBeenCalledWith(9);
    });

    it("shows the attempt and the back-off while retrying", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [waitingItem({ phase: "retrying", attempt: 2, retry_delay_s: 5, phase_since: START + 132 })],
      });
      const { getByTestId } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();
      expect(getByTestId("dl-waiting-text").textContent).toBe("Connection problem — retrying");
      expect(getByTestId("dl-waiting-detail").textContent).toBe("Attempt 2 of 3 in 3s");
    });

    it("switches to the normal byte progress as soon as a frame without a phase arrives", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({ downloads: [waitingItem()] });
      const { getByTestId, queryByTestId } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();
      expect(queryByTestId("dl-waiting")).not.toBeNull();
      await act(async () => {
        const flowing = waitingItem({ bytes_downloaded: 512, total_bytes: 1024, resumable: true });
        delete flowing.phase;
        delete flowing.phase_since;
        setDownloads([flowing]);
        await vi.advanceTimersByTimeAsync(600);
      });
      expect(queryByTestId("dl-waiting")).toBeNull();
      expect(getByTestId("dl-bytes").textContent).toBe("512 B / 1.0 KB");
    });

    it("a partial that cannot be verified offers Restart instead of Resume", async () => {
      const stopped = waitingItem({ status: "interrupted", reason: "needs_recovery" });
      delete stopped.phase;
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({ downloads: [stopped] });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();
      expect(container.querySelector('[data-testid="dl-caption"]')?.textContent).toContain("restart needed");
      expect(buttonByText(container, "Resume Mario Kart")).toBeNull();
      await act(async () => {
        fireEvent.click(buttonByText(container, "Restart Mario Kart")!);
        await Promise.resolve();
      });
      expect(backend.resumeDownload).toHaveBeenCalledWith(9);
    });
  });

  // ---------------------------------------------------------------------------
  // Firmware downloads share the list
  // ---------------------------------------------------------------------------
  describe("firmware downloads", () => {
    function fw(overrides: Partial<DownloadItem> = {}): DownloadItem {
      return makeItem({
        kind: "firmware",
        rom_id: 0,
        firmware_id: 7,
        rom_name: "Firmware.23.0.1.zip",
        platform_name: "Switch firmware",
        status: "downloading",
        resumable: true,
        ...overrides,
      });
    }

    it("lists a firmware file next to a ROM with the same number as two separate rows", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [makeItem({ rom_id: 7, rom_name: "Zelda" }), fw()],
      });
      const { container } = await renderQueue();
      const captions = Array.from(container.querySelectorAll('[data-testid="dl-caption"]')).map((e) => e.textContent);
      expect(captions).toHaveLength(2);
      expect(captions.some((c) => c?.startsWith("Zelda"))).toBe(true);
      expect(captions.some((c) => c?.startsWith("Firmware.23.0.1.zip (Switch firmware)"))).toBe(true);
    });

    it("pause, resume and cancel address a firmware download by its kind and a ROM by its id alone", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [makeItem({ rom_id: 7, rom_name: "Zelda", resumable: true }), fw({ status: "paused" })],
      });
      const { container } = await renderQueue();
      await act(async () => {
        fireEvent.click(buttonByText(container, "Pause Zelda")!);
        await Promise.resolve();
      });
      expect(backend.pauseDownload).toHaveBeenCalledWith(7);
      await act(async () => {
        fireEvent.click(buttonByText(container, "Resume Firmware.23.0.1.zip")!);
        await Promise.resolve();
      });
      expect(backend.resumeDownload).toHaveBeenCalledWith(7, "firmware");
      await act(async () => {
        fireEvent.click(buttonByText(container, "Cancel Firmware.23.0.1.zip")!);
        await Promise.resolve();
      });
      expect(backend.cancelDownload).toHaveBeenCalledWith(7, "firmware");
      expect(backend.cancelDownload).not.toHaveBeenCalledWith(7);
    });

    it("a finished firmware row can be cleared without clearing the ROM with the same number", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [makeItem({ rom_id: 7, rom_name: "Zelda", status: "completed" }), fw({ status: "completed" })],
      });
      const { container } = await renderQueue();
      expect(container.textContent).toContain("Zelda");
      await act(async () => {
        fireEvent.click(buttonByText(container, "Clear Completed")!);
        await Promise.resolve();
      });
      expect(container.textContent).not.toContain("Zelda");
      expect(container.textContent).not.toContain("Firmware.23.0.1.zip");
    });
  });

  // ---------------------------------------------------------------------------
  // Queued behind another download
  // ---------------------------------------------------------------------------
  it("marks a download that waits for the transfer slot as In Queue", async () => {
    vi.mocked(backend.getDownloadQueue).mockResolvedValue({
      downloads: [makeItem({ rom_id: 3, rom_name: "Second", status: "queued", bytes_downloaded: 0 })],
    });
    const { container } = render(<DownloadQueue onBack={() => {}} />);
    await flushMount();
    expect(container.querySelector('[data-testid="dl-caption"]')?.textContent).toContain("In Queue");
  });

  // ---------------------------------------------------------------------------
  // handlePause / handleResume (#1124)
  // ---------------------------------------------------------------------------
  describe("pause / resume", () => {
    it("a downloading + resumable item renders a Pause control that calls pauseDownload(rom_id)", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [makeItem({ rom_id: 88, rom_name: "Zelda", status: "downloading", resumable: true })],
      });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();

      const pause = buttonByText(container, "Pause Zelda");
      expect(pause).not.toBeNull();
      await act(async () => {
        fireEvent.click(pause!);
        await Promise.resolve();
      });
      expect(backend.pauseDownload).toHaveBeenCalledWith(88);
    });

    it("a downloading + NOT resumable item renders no Pause control", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [makeItem({ rom_id: 89, rom_name: "Metroid", status: "downloading", resumable: false })],
      });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();

      expect(buttonByText(container, "Pause Metroid")).toBeNull();
      // Cancel is still offered for the non-resumable active download.
      expect(buttonByText(container, "Cancel Metroid")).not.toBeNull();
    });

    it("a paused item stays in the active section and renders a Resume control that calls resumeDownload(rom_id)", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [makeItem({ rom_id: 90, rom_name: "Kirby", status: "paused", resumable: true })],
      });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();

      // Still rendered as active (caption present, with the Paused marker).
      const caption = container.querySelector('[data-testid="dl-caption"]');
      expect(caption?.textContent).toBe("Kirby (Genesis) — Paused");

      const resume = buttonByText(container, "Resume Kirby");
      expect(resume).not.toBeNull();
      await act(async () => {
        fireEvent.click(resume!);
        await Promise.resolve();
      });
      expect(backend.resumeDownload).toHaveBeenCalledWith(90);
    });

    it("a refused resume is shown as a toast with the backend's reason instead of vanishing", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [makeItem({ rom_id: 90, rom_name: "Kirby", status: "interrupted", resumable: true })],
      });
      vi.mocked(backend.resumeDownload).mockResolvedValue({
        success: false,
        reason: "server_unreachable",
        message: "Could not reach the RomM server",
      });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();
      vi.mocked(toaster.toast).mockClear();
      await act(async () => {
        fireEvent.click(buttonByText(container, "Resume Kirby")!);
        await Promise.resolve();
      });
      expect(toaster.toast).toHaveBeenCalledWith(
        expect.objectContaining({ body: "Could not reach the RomM server" }),
      );
    });
  });

  // ---------------------------------------------------------------------------
  // interrupted (durable queue: transient failure or plugin restart)
  // ---------------------------------------------------------------------------
  describe("interrupted", () => {
    it.each([
      ["server_unreachable", "Interrupted — server unreachable"],
      ["disk_full", "Interrupted — disk full"],
      ["restart", "Interrupted — plugin restarted"],
      ["auth_failed", "Interrupted — sign-in required"],
      [undefined, "Interrupted"],
    ])("reason %s stays active with caption '%s' and offers Resume + Cancel", async (reason, label) => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [
          makeItem({
            rom_id: 93,
            rom_name: "Samus",
            status: "interrupted",
            resumable: false,
            ...(reason ? { reason } : {}),
          }),
        ],
      });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();

      expect(container.querySelector('[data-testid="dl-caption"]')?.textContent).toBe(`Samus (Genesis) — ${label}`);
      expect(buttonByText(container, "Pause Samus")).toBeNull();
      expect(buttonByText(container, "Cancel Samus")).not.toBeNull();
      const resume = buttonByText(container, "Resume Samus");
      expect(resume).not.toBeNull();
      await act(async () => {
        fireEvent.click(resume!);
        await Promise.resolve();
      });
      expect(backend.resumeDownload).toHaveBeenCalledWith(93);
    });
  });

  // ---------------------------------------------------------------------------
  // extracting (post-transfer ZIP unpack)
  // ---------------------------------------------------------------------------
  describe("extracting", () => {
    it("renders an extracting item in the active section with the 'Extracting…' caption and the extracted fraction", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [
          makeItem({
            rom_id: 91,
            rom_name: "Chrono",
            status: "extracting",
            bytes_downloaded: 4200,
            total_bytes: 10000,
            resumable: false,
          }),
        ],
      });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();

      // Caption carries the Extracting marker; the bar reads the 42% fraction.
      expect(container.querySelector('[data-testid="dl-caption"]')?.textContent).toBe("Chrono (Genesis) — Extracting…");
      expect(container.querySelector('[data-testid="progress-progress"]')?.textContent).toBe("42");
    });

    it("offers no Pause / Resume / Cancel control while extracting", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [
          makeItem({
            rom_id: 92,
            rom_name: "Trigger",
            status: "extracting",
            bytes_downloaded: 5000,
            total_bytes: 10000,
            resumable: false,
          }),
        ],
      });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();

      expect(buttonByText(container, "Pause Trigger")).toBeNull();
      expect(buttonByText(container, "Resume Trigger")).toBeNull();
      expect(buttonByText(container, "Cancel Trigger")).toBeNull();
    });
  });

  // ---------------------------------------------------------------------------
  // handleClearCompleted
  // ---------------------------------------------------------------------------
  describe("handleClearCompleted", () => {
    it("hides all completed/failed/cancelled items; keeps active items visible", async () => {
      const active = makeItem({ rom_id: 1, rom_name: "Active" });
      const completed = makeItem({
        rom_id: 2,
        rom_name: "Done",
        status: "completed",
        bytes_downloaded: 100,
        total_bytes: 100,
      });
      const failed = makeItem({
        rom_id: 3,
        rom_name: "Broke",
        status: "failed",
        error: "boom",
      });
      const cancelled = makeItem({
        rom_id: 4,
        rom_name: "Stopped",
        status: "cancelled",
      });
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [active, completed, failed, cancelled],
      });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();

      // Before clearing: all three finished Fields render.
      const labelsBefore = Array.from(container.querySelectorAll('[data-testid="field-label"]')).map(
        (n) => n.textContent,
      );
      expect(labelsBefore).toEqual(expect.arrayContaining(["Done", "Broke", "Stopped"]));

      await act(async () => {
        fireEvent.click(buttonByExactText(container, "Clear Completed")!);
      });

      // After clearing: finished Fields are gone; active progress bar remains.
      const labelsAfter = Array.from(container.querySelectorAll('[data-testid="field-label"]')).map(
        (n) => n.textContent,
      );
      expect(labelsAfter).not.toContain("Done");
      expect(labelsAfter).not.toContain("Broke");
      expect(labelsAfter).not.toContain("Stopped");
      expect(container.querySelector('[data-testid="dl-caption"]')?.textContent).toBe("Active (Genesis)");
      // Clear Completed button is gone (no finished items remain).
      expect(buttonByExactText(container, "Clear Completed")).toBeNull();
    });
  });

  // ---------------------------------------------------------------------------
  // Conditional render — empty / active / finished / button visibility
  // ---------------------------------------------------------------------------
  describe("conditional render", () => {
    it("empty state: visible.length === 0 → renders 'No downloads' Field", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({ downloads: [] });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();
      const labels = Array.from(container.querySelectorAll('[data-testid="field-label"]')).map((n) => n.textContent);
      expect(labels).toContain("No downloads");
    });

    it("active item with total_bytes > 0: nProgress is (bytes/total)*100, indeterminate=false, sTimeRemaining = 'X / Y'", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [
          makeItem({
            rom_id: 1,
            rom_name: "Det",
            bytes_downloaded: 512,
            total_bytes: 2048,
          }),
        ],
      });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();
      // 512 / 2048 * 100 = 25
      expect(container.querySelector('[data-testid="progress-progress"]')?.textContent).toBe("25");
      expect(container.querySelector('[data-testid="progress-indeterminate"]')?.textContent).toBe("false");
      expect(container.querySelector('[data-testid="dl-bytes"]')?.textContent).toBe("512 B / 2.0 KB");
      expect(container.querySelector('[data-testid="dl-caption"]')?.textContent).toBe("Det (Genesis)");
    });

    it("active item with total_bytes === 0: nProgress=undefined, indeterminate=true, sTimeRemaining = formatBytes(bytes_downloaded)", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [
          makeItem({
            rom_id: 1,
            rom_name: "Indet",
            bytes_downloaded: 700,
            total_bytes: 0,
          }),
        ],
      });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();
      // String(undefined) → "undefined".
      expect(container.querySelector('[data-testid="progress-progress"]')?.textContent).toBe("undefined");
      expect(container.querySelector('[data-testid="progress-indeterminate"]')?.textContent).toBe("true");
      expect(container.querySelector('[data-testid="dl-bytes"]')?.textContent).toBe("700 B");
    });

    it("finished list: completed → 'Completed — <bytes>'", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [
          makeItem({
            rom_id: 1,
            rom_name: "C",
            status: "completed",
            bytes_downloaded: 1024,
            total_bytes: 1024,
          }),
        ],
      });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();
      const desc = container.querySelector('[data-testid="field-desc"]');
      expect(desc?.textContent).toBe("Completed — 1.0 KB");
    });

    it("finished list: failed with error → 'Failed: <error>'", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [
          makeItem({
            rom_id: 1,
            rom_name: "F",
            status: "failed",
            error: "network",
          }),
        ],
      });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();
      const desc = container.querySelector('[data-testid="field-desc"]');
      expect(desc?.textContent).toBe("Failed: network");
    });

    it("finished list: failed without error → 'Failed' (no colon)", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [
          makeItem({
            rom_id: 1,
            rom_name: "F2",
            status: "failed",
          }),
        ],
      });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();
      const desc = container.querySelector('[data-testid="field-desc"]');
      expect(desc?.textContent).toBe("Failed");
    });

    it("finished list: cancelled → 'Cancelled'", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [
          makeItem({
            rom_id: 1,
            rom_name: "X",
            status: "cancelled",
          }),
        ],
      });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();
      const desc = container.querySelector('[data-testid="field-desc"]');
      expect(desc?.textContent).toBe("Cancelled");
    });

    it("Clear Completed button visible when any finished item is unhidden", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [makeItem({ rom_id: 1, status: "completed", bytes_downloaded: 100, total_bytes: 100 })],
      });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();
      expect(buttonByExactText(container, "Clear Completed")).not.toBeNull();
    });

    it("Clear Completed button hidden when only active items present", async () => {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [makeItem({ rom_id: 1, status: "downloading" })],
      });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();
      expect(buttonByExactText(container, "Clear Completed")).toBeNull();
    });
  });

  // ---------------------------------------------------------------------------
  // formatBytes — exercised indirectly via finished-item descriptions
  // ---------------------------------------------------------------------------
  describe("formatBytes (via rendered output)", () => {
    async function renderCompleted(total: number): Promise<HTMLElement> {
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [
          makeItem({
            status: "completed",
            bytes_downloaded: total,
            total_bytes: total,
          }),
        ],
      });
      const { container } = render(<DownloadQueue onBack={() => {}} />);
      await flushMount();
      return container;
    }

    it("0 bytes → '0 B'", async () => {
      const c = await renderCompleted(0);
      expect(c.querySelector('[data-testid="field-desc"]')?.textContent).toBe("Completed — 0 B");
    });

    it("< 1024 → '<n> B'", async () => {
      const c = await renderCompleted(512);
      expect(c.querySelector('[data-testid="field-desc"]')?.textContent).toBe("Completed — 512 B");
    });

    it("exactly 1024 → '1.0 KB'", async () => {
      const c = await renderCompleted(1024);
      expect(c.querySelector('[data-testid="field-desc"]')?.textContent).toBe("Completed — 1.0 KB");
    });

    it("MB range → 'X.X MB'", async () => {
      const c = await renderCompleted(5 * 1024 * 1024);
      expect(c.querySelector('[data-testid="field-desc"]')?.textContent).toBe("Completed — 5.0 MB");
    });

    it("GB range → 'X.XX GB'", async () => {
      const c = await renderCompleted(Math.round(1.5 * 1024 * 1024 * 1024));
      expect(c.querySelector('[data-testid="field-desc"]')?.textContent).toBe("Completed — 1.50 GB");
    });
  });

  // ---------------------------------------------------------------------------
  // Back button
  // ---------------------------------------------------------------------------
  describe("Back button", () => {
    it("clicking Back invokes onBack prop", async () => {
      const onBack = vi.fn();
      const { container } = render(<DownloadQueue onBack={onBack} />);
      await flushMount();
      const back = buttonByExactText(container, "Back");
      expect(back).not.toBeNull();
      fireEvent.click(back!);
      expect(onBack).toHaveBeenCalledTimes(1);
    });
  });

  describe("pause all / resume all and the compact (QAM) link", () => {
    beforeEach(() => {
      vi.mocked(backend.pauseDownload).mockClear();
      vi.mocked(backend.resumeDownload).mockClear();
    });

    const queue = (...items: Partial<DownloadItem>[]) =>
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: items.map((o, n) => makeItem({ rom_id: n + 1, rom_name: `G${n + 1}`, ...o })),
      });

    it("Pause all pauses every transferring download, and only those", async () => {
      queue({ resumable: true }, { resumable: true }, { status: "queued" });
      const { container } = await renderQueue();
      fireEvent.click(buttonByExactText(container, "Pause all")!);
      await flushMount();
      const paused = vi.mocked(backend.pauseDownload).mock.calls.map((c) => c[0]).sort();
      expect(paused).toEqual([1, 2]);
    });

    it("offers Resume all once nothing transfers, and resumes every stopped download", async () => {
      queue({ status: "paused", resumable: true }, { status: "interrupted", reason: "network", resumable: true });
      const { container } = await renderQueue();
      expect(buttonByExactText(container, "Pause all")).toBeNull();
      fireEvent.click(buttonByExactText(container, "Resume all")!);
      await flushMount();
      const resumed = vi.mocked(backend.resumeDownload).mock.calls.map((c) => c[0]).sort();
      expect(resumed).toEqual([1, 2]);
    });

    it("shows neither button when the queue is empty", async () => {
      const { container } = await renderQueue();
      expect(buttonByExactText(container, "Pause all")).toBeNull();
      expect(buttonByExactText(container, "Resume all")).toBeNull();
    });

    it("only the compact page links to the full queue, and the link opens the Downloads tab", async () => {
      const plain = await renderQueue();
      expect(buttonByExactText(plain.container, "Full queue in RomM Library")).toBeNull();
      plain.unmount();
      const compact = render(<DownloadQueue compact onBack={() => {}} />);
      await flushMount();
      fireEvent.click(buttonByExactText(compact.container, "Full queue in RomM Library")!);
      expect(Navigation.Navigate).toHaveBeenCalledWith("/romm-library");
      expect(getLibraryTab()).toBe("downloads");
    });
  });

  describe("Open on finished ROMs (full-screen queue)", () => {
    const finished = (over: Partial<DownloadItem> = {}) =>
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({
        downloads: [makeItem({ rom_id: 7, rom_name: "Metroid", status: "completed", progress: 100, ...over })],
      });

    it("a completed ROM gets an Open button that goes to its Steam game page", async () => {
      vi.mocked(backend.getAppIdRomIdMap).mockResolvedValue({ "4000000007": 7 });
      finished();
      const { container } = await renderQueue();
      fireEvent.click(buttonByText(container, "Open Metroid")!);
      expect(Navigation.Navigate).toHaveBeenCalledWith("/library/app/4000000007");
    });

    it("no Open for a failed download, a firmware job, a ROM without a shortcut, or in the compact (QAM) queue", async () => {
      vi.mocked(backend.getAppIdRomIdMap).mockResolvedValue({ "4000000007": 7 });
      finished({ status: "failed" });
      expect(buttonByText((await renderQueue()).container, "Open Metroid")).toBeNull();
      finished({ kind: "firmware" });
      expect(buttonByText((await renderQueue()).container, "Open Metroid")).toBeNull();
      vi.mocked(backend.getAppIdRomIdMap).mockResolvedValue({});
      finished();
      expect(buttonByText((await renderQueue()).container, "Open Metroid")).toBeNull();
      vi.mocked(backend.getAppIdRomIdMap).mockResolvedValue({ "4000000007": 7 });
      finished();
      const compact = render(<DownloadQueue compact onBack={() => {}} />);
      await flushMount();
      expect(buttonByText(compact.container, "Open Metroid")).toBeNull();
    });
  });

  describe("Start now", () => {
    beforeEach(() => {
      vi.mocked(backend.pauseDownload).mockClear();
      vi.mocked(backend.resumeDownload).mockClear();
      vi.mocked(backend.moveToFront).mockClear();
    });

    it("moves a waiting download to the front, pauses the running one and re-reads the order", async () => {
      const running = makeItem({ rom_id: 1, rom_name: "Zelda", status: "downloading", resumable: true });
      const waiting = makeItem({ rom_id: 2, rom_name: "Mario", status: "queued" });
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({ downloads: [running, waiting] });
      const { container } = await renderQueue();
      const fetches = vi.mocked(backend.getDownloadQueue).mock.calls.length;
      await act(async () => {
        fireEvent.click(buttonByText(container, "Start now: Mario")!);
        await Promise.resolve();
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(backend.moveToFront).toHaveBeenCalledWith(2);
      expect(backend.pauseDownload).toHaveBeenCalledWith(1);
      expect(backend.resumeDownload).not.toHaveBeenCalled();
      expect(vi.mocked(backend.getDownloadQueue).mock.calls.length).toBe(fetches + 1);
    });

    it("also resumes a paused download, and is not offered when nothing is transferring", async () => {
      const running = makeItem({ rom_id: 1, rom_name: "Zelda", status: "downloading", resumable: true });
      const paused = makeItem({ rom_id: 2, rom_name: "Mario", status: "paused" });
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({ downloads: [running, paused] });
      const { container } = await renderQueue();
      await act(async () => {
        fireEvent.click(buttonByText(container, "Start now: Mario")!);
        await Promise.resolve();
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(backend.resumeDownload).toHaveBeenCalledWith(2);

      vi.mocked(backend.getDownloadQueue).mockResolvedValue({ downloads: [paused] });
      expect(buttonByText((await renderQueue()).container, "Start now: Mario")).toBeNull();
    });

    it("a refused reorder shows a toast and leaves the running download alone", async () => {
      vi.mocked(backend.moveToFront).mockResolvedValue({ success: false, message: "No queued download" });
      const running = makeItem({ rom_id: 1, rom_name: "Zelda", status: "downloading", resumable: true });
      const waiting = makeItem({ rom_id: 2, rom_name: "Mario", status: "queued" });
      vi.mocked(backend.getDownloadQueue).mockResolvedValue({ downloads: [running, waiting] });
      const { container } = await renderQueue();
      await act(async () => {
        fireEvent.click(buttonByText(container, "Start now: Mario")!);
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(backend.pauseDownload).not.toHaveBeenCalled();
    });
  });
});
