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
    // The RomM-styled controls are Focusables; here each one is a button so tests can click it by its text or label.
    Focusable: ({ children, onClick, className, ...rest }: AnyProps & { onClick?: () => void; className?: string }) =>
      ce(
        className?.includes("rm-btn") ? "button" : "div",
        { onClick, className, "aria-label": rest["aria-label"] as string | undefined },
        children as never,
      ),
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

/** The queue row (`dl-row`) of the download called `name`. */
function row(container: HTMLElement, name: string): HTMLElement {
  const found = Array.from(container.querySelectorAll<HTMLElement>('[data-testid="dl-row"]')).find((r) =>
    r.textContent.includes(name),
  );
  if (!found) throw new Error(`no row for ${name}`);
  return found;
}

/** A button inside a row, found by its text or its aria-label. */
function rowButton(container: HTMLElement, name: string, text: string): HTMLButtonElement | null {
  const btn = Array.from(row(container, name).querySelectorAll("button")).find(
    (b) => b.textContent === text || b.getAttribute("aria-label") === text,
  );
  return (btn as HTMLButtonElement | undefined) ?? null;
}

function buttonByText(container: HTMLElement, text: string): HTMLButtonElement | null {
  const btn = Array.from(container.querySelectorAll("button")).find(
    (b) => b.textContent.includes(text) || b.getAttribute("aria-label")?.includes(text),
  );
  return (btn as HTMLButtonElement | undefined) ?? null;
}

async function settle(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

async function click(el: Element | null): Promise<void> {
  expect(el).not.toBeNull();
  await act(async () => {
    await vi.advanceTimersByTimeAsync(300); // a second press of the same button within 250 ms counts once
    fireEvent.click(el!);
    await Promise.resolve();
    await Promise.resolve();
  });
}

async function renderQueue(props: { compact?: boolean; onBack?: () => void } = {}) {
  const utils = render(<DownloadQueue onBack={props.onBack ?? (() => {})} {...(props.compact ? { compact: true } : {})} />);
  await settle();
  return utils;
}

const queueOf = (...downloads: DownloadItem[]) => vi.mocked(backend.getDownloadQueue).mockResolvedValue({ downloads });

describe("DownloadQueue", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    setDownloads([]);
    vi.mocked(backend.getDownloadQueue).mockResolvedValue({ downloads: [] });
    for (const fn of [backend.cancelDownload, backend.pauseDownload, backend.resumeDownload, backend.moveToFront]) {
      vi.mocked(fn).mockReset().mockResolvedValue({ success: true, message: "" });
    }
    vi.mocked(backend.getAppIdRomIdMap).mockResolvedValue({});
    vi.mocked(toaster.toast).mockClear();
    vi.mocked(Navigation.Navigate).mockClear();
  });

  afterEach(() => {
    vi.useRealTimers();
    setDownloads([]);
  });

  describe("loading and polling", () => {
    it("seeds the store and the list from the backend on mount", async () => {
      queueOf(makeItem({ rom_id: 5, rom_name: "Tetris" }));
      const { container } = await renderQueue();
      expect(getDownloadState()).toHaveLength(1);
      expect(row(container, "Tetris")).toBeTruthy();
    });

    it("falls back to the store when the backend read fails", async () => {
      setDownloads([makeItem({ rom_name: "Cached Game" })]);
      vi.mocked(backend.getDownloadQueue).mockRejectedValue(new Error("net"));
      const { container } = await renderQueue();
      expect(row(container, "Cached Game")).toBeTruthy();
    });

    it("re-reads the store every 500 ms and stops when unmounted", async () => {
      const spy = vi.spyOn(globalThis, "clearInterval");
      const { container, unmount } = await renderQueue();
      expect(container.textContent).toContain("Nothing downloading");
      await act(async () => {
        setDownloads([makeItem({ rom_name: "Late Arrival" })]);
        await vi.advanceTimersByTimeAsync(600);
      });
      expect(row(container, "Late Arrival")).toBeTruthy();
      unmount();
      expect(spy).toHaveBeenCalled();
      spy.mockRestore();
    });

    it("says what to do when nothing is downloading (full) and keeps it short (compact)", async () => {
      expect((await renderQueue()).container.textContent).toContain("Nothing downloading. Pick a game in the Library");
      const compact = await renderQueue({ compact: true });
      expect(compact.container.textContent).toContain("No downloads running.");
    });
  });

  describe("row controls", () => {
    it("Pause on a resumable running download; none when it cannot pause", async () => {
      queueOf(makeItem({ rom_id: 3, rom_name: "Alpha", resumable: true }), makeItem({ rom_id: 4, rom_name: "Bravo", resumable: false }));
      const { container } = await renderQueue();
      await click(rowButton(container, "Alpha", "Pause"));
      expect(backend.pauseDownload).toHaveBeenCalledWith(3);
      expect(rowButton(container, "Bravo", "Pause")).toBeNull();
    });

    it("Resume a paused download; a refusal is shown as a toast", async () => {
      queueOf(makeItem({ rom_id: 3, rom_name: "Alpha", status: "paused" }));
      const { container } = await renderQueue();
      await click(rowButton(container, "Alpha", "Resume"));
      expect(backend.resumeDownload).toHaveBeenCalledWith(3);
      vi.mocked(backend.resumeDownload).mockResolvedValue({ success: false, message: "Server unreachable" });
      await click(rowButton(container, "Alpha", "Resume"));
      expect(toaster.toast).toHaveBeenCalledWith(expect.objectContaining({ body: "Server unreachable" }));
    });

    it("an unverifiable partial offers Restart instead of Resume", async () => {
      queueOf(makeItem({ rom_id: 9, rom_name: "Kart", status: "interrupted", reason: "needs_recovery" }));
      const { container } = await renderQueue();
      expect(rowButton(container, "Kart", "Resume")).toBeNull();
      await click(rowButton(container, "Kart", "Restart"));
      expect(backend.resumeDownload).toHaveBeenCalledWith(9);
    });

    it.each([
      ["server_unreachable", "Interrupted — server unreachable"],
      ["disk_full", "Interrupted — disk full"],
      ["restart", "Interrupted — plugin restarted"],
      ["auth_failed", "Interrupted — sign-in required"],
      [undefined, "Interrupted"],
    ])("an interrupted download (%s) says why and can be resumed or cancelled", async (reason, label) => {
      queueOf(makeItem({ rom_id: 3, rom_name: "A", status: "interrupted", ...(reason ? { reason } : {}) }));
      const { container } = await renderQueue();
      expect(row(container, "A").textContent).toContain(label);
      expect(rowButton(container, "A", "Resume")).not.toBeNull();
      expect(rowButton(container, "A", "Cancel download")).not.toBeNull();
    });

    it("Cancel calls cancelDownload; a rejection leaves the list intact", async () => {
      queueOf(makeItem({ rom_id: 77, rom_name: "Mario" }));
      const { container } = await renderQueue();
      await click(rowButton(container, "Mario", "Cancel download"));
      expect(backend.cancelDownload).toHaveBeenCalledWith(77);
      vi.mocked(backend.cancelDownload).mockRejectedValue(new Error("nope"));
      await click(rowButton(container, "Mario", "Cancel download"));
      expect(row(container, "Mario")).toBeTruthy();
    });

    it("an extracting download has no controls and says so", async () => {
      queueOf(makeItem({ rom_id: 3, rom_name: "A", status: "extracting" }));
      const { container } = await renderQueue();
      expect(row(container, "A").textContent).toContain("Extracting…");
      expect(row(container, "A").querySelectorAll("button")).toHaveLength(0);
    });

    it("a queued download shows Queued and Start now only while another one is running", async () => {
      queueOf(makeItem({ rom_id: 1, rom_name: "Q", status: "queued" }));
      expect(rowButton((await renderQueue()).container, "Q", "Start now")).toBeNull();
      queueOf(makeItem({ rom_id: 2, rom_name: "Run", resumable: true }), makeItem({ rom_id: 1, rom_name: "Q", status: "queued" }));
      const { container } = await renderQueue();
      expect(row(container, "Q").textContent).toContain("Queued");
      expect(rowButton(container, "Q", "Start now")).not.toBeNull();
    });
  });

  describe("Start now", () => {
    const running = () => makeItem({ rom_id: 1, rom_name: "Zelda", resumable: true });

    it("moves a waiting download to the front, pauses the running one and re-reads the order", async () => {
      queueOf(running(), makeItem({ rom_id: 2, rom_name: "Mario", status: "queued" }));
      const { container } = await renderQueue();
      const fetches = vi.mocked(backend.getDownloadQueue).mock.calls.length;
      await click(rowButton(container, "Mario", "Start now"));
      await settle();
      expect(backend.moveToFront).toHaveBeenCalledWith(2);
      expect(backend.pauseDownload).toHaveBeenCalledWith(1);
      expect(backend.resumeDownload).not.toHaveBeenCalled();
      expect(vi.mocked(backend.getDownloadQueue).mock.calls.length).toBe(fetches + 1);
    });

    it("also resumes a paused download", async () => {
      queueOf(running(), makeItem({ rom_id: 2, rom_name: "Mario", status: "paused" }));
      const { container } = await renderQueue();
      await click(rowButton(container, "Mario", "Start now"));
      await settle();
      expect(backend.resumeDownload).toHaveBeenCalledWith(2);
    });

    it("a refused reorder shows a toast and leaves the running download alone", async () => {
      vi.mocked(backend.moveToFront).mockResolvedValue({ success: false, message: "No queued download" });
      queueOf(running(), makeItem({ rom_id: 2, rom_name: "Mario", status: "queued" }));
      const { container } = await renderQueue();
      await click(rowButton(container, "Mario", "Start now"));
      expect(toaster.toast).toHaveBeenCalledWith(expect.objectContaining({ body: "No queued download" }));
      expect(backend.pauseDownload).not.toHaveBeenCalled();
    });
  });

  describe("pause all / resume all", () => {
    it("pauses what is running, and resumes what is stopped once nothing runs", async () => {
      queueOf(makeItem({ rom_id: 1, rom_name: "A", resumable: true }), makeItem({ rom_id: 2, rom_name: "B", resumable: true }));
      const first = await renderQueue();
      await click(buttonByText(first.container, "Pause all"));
      expect(backend.pauseDownload).toHaveBeenCalledTimes(2);
      first.unmount();
      queueOf(makeItem({ rom_id: 1, rom_name: "A", status: "paused" }), makeItem({ rom_id: 2, rom_name: "B", status: "interrupted" }));
      const second = await renderQueue();
      await click(buttonByText(second.container, "Resume all"));
      expect(backend.resumeDownload).toHaveBeenCalledTimes(2);
    });
  });

  describe("progress", () => {
    it("shows bytes, platform and the status in percent", async () => {
      queueOf(makeItem({ rom_id: 3, rom_name: "A", bytes_downloaded: 512, total_bytes: 1024 }));
      const { container } = await renderQueue();
      expect(row(container, "A").textContent).toContain("Downloading 50%");
      expect(container.querySelector('[data-testid="dl-bytes"]')?.textContent).toContain("512 B / 1.0 KB");
      expect(container.querySelector('[data-testid="dl-bytes"]')?.textContent).toContain("Genesis");
    });

    it("an unknown total shows the bytes so far and no invented percentage", async () => {
      queueOf(makeItem({ rom_id: 3, rom_name: "A", bytes_downloaded: 2048, total_bytes: 0 }));
      const { container } = await renderQueue();
      expect(container.querySelector('[data-testid="dl-bytes"]')?.textContent).toContain("2.0 KB");
      expect(row(container, "A").textContent).toContain("Downloading 0%");
    });

    it("a paused download keeps its percentage", async () => {
      queueOf(makeItem({ rom_id: 3, rom_name: "A", status: "paused", bytes_downloaded: 256, total_bytes: 1024 }));
      expect(row((await renderQueue()).container, "A").textContent).toContain("Paused · 25%");
    });
  });

  describe("waiting for the server", () => {
    const START = 1_700_000_000;
    const waiting = (o: Partial<DownloadItem> = {}) =>
      makeItem({ rom_id: 9, rom_name: "Kart", resumable: false, bytes_downloaded: 0, total_bytes: 0, phase: "waiting", phase_since: START, attempt: 1, max_attempts: 3, ...o });

    beforeEach(() => {
      vi.setSystemTime(new Date((START + 134) * 1000));
    });

    it("shows an honest waiting state with elapsed time and no byte counter", async () => {
      queueOf(waiting());
      const { getByTestId, queryByTestId } = await renderQueue();
      expect(getByTestId("dl-waiting-text").textContent).toBe("Waiting for RomM to respond. A download archive may be preparing…");
      expect(getByTestId("dl-waiting-detail").textContent).toBe("Elapsed: 2m 14s");
      expect(queryByTestId("dl-bytes")).toBeNull();
    });

    it("counts the elapsed time locally without asking the server again", async () => {
      queueOf(waiting());
      const { getByTestId } = await renderQueue();
      const fetches = vi.mocked(backend.getDownloadQueue).mock.calls.length;
      await act(async () => {
        await vi.advanceTimersByTimeAsync(5000);
      });
      expect(getByTestId("dl-waiting-detail").textContent).toBe("Elapsed: 2m 19s");
      expect(vi.mocked(backend.getDownloadQueue).mock.calls.length).toBe(fetches);
    });

    it("keeps Pause and Cancel available while waiting", async () => {
      queueOf(waiting());
      const { container } = await renderQueue();
      await click(rowButton(container, "Kart", "Pause"));
      expect(backend.pauseDownload).toHaveBeenCalledWith(9);
      await click(rowButton(container, "Kart", "Cancel download"));
      expect(backend.cancelDownload).toHaveBeenCalledWith(9);
    });

    it("shows the attempt and the back-off while retrying", async () => {
      queueOf(waiting({ phase: "retrying", attempt: 2, retry_delay_s: 5, phase_since: START + 132 }));
      const { getByTestId } = await renderQueue();
      expect(getByTestId("dl-waiting-text").textContent).toBe("Connection problem — retrying");
      expect(getByTestId("dl-waiting-detail").textContent).toBe("Attempt 2 of 3 in 3s");
    });

    it("switches to byte progress as soon as a frame without a phase arrives", async () => {
      queueOf(waiting());
      const { getByTestId, queryByTestId } = await renderQueue();
      expect(queryByTestId("dl-waiting")).not.toBeNull();
      await act(async () => {
        const flowing = waiting({ bytes_downloaded: 512, total_bytes: 1024, resumable: true });
        delete flowing.phase;
        delete flowing.phase_since;
        setDownloads([flowing]);
        await vi.advanceTimersByTimeAsync(600);
      });
      expect(queryByTestId("dl-waiting")).toBeNull();
      expect(getByTestId("dl-bytes").textContent).toContain("512 B / 1.0 KB");
    });
  });

  describe("firmware downloads share the list", () => {
    const fw = (o: Partial<DownloadItem> = {}) =>
      makeItem({ kind: "firmware", rom_id: 0, firmware_id: 7, rom_name: "Firmware.23.0.1.zip", platform_name: "Switch firmware", resumable: true, ...o });

    it("lists a firmware file next to a ROM with the same number as two rows", async () => {
      queueOf(makeItem({ rom_id: 7, rom_name: "Zelda" }), fw());
      const { container } = await renderQueue();
      expect(container.querySelectorAll('[data-testid="dl-row"]')).toHaveLength(2);
    });

    it("pause, resume and cancel address firmware by kind and a ROM by its id alone", async () => {
      queueOf(makeItem({ rom_id: 7, rom_name: "Zelda", resumable: true }), fw({ status: "paused" }));
      const { container } = await renderQueue();
      await click(rowButton(container, "Zelda", "Pause"));
      expect(backend.pauseDownload).toHaveBeenCalledWith(7);
      await click(rowButton(container, "Firmware.23.0.1.zip", "Start now"));
      await settle();
      expect(backend.moveToFront).toHaveBeenCalledWith(7, "firmware");
      await click(rowButton(container, "Firmware.23.0.1.zip", "Cancel download"));
      expect(backend.cancelDownload).toHaveBeenCalledWith(7, "firmware");
    });
  });

  describe("finished downloads", () => {
    const done = (o: Partial<DownloadItem> = {}) => makeItem({ rom_id: 7, rom_name: "Metroid", status: "completed", total_bytes: 2048, ...o });
    const finished = (container: HTMLElement) => Array.from(container.querySelectorAll('[data-testid="dl-finished"]'));

    it("describes completed, failed and cancelled downloads", async () => {
      queueOf(done(), done({ rom_id: 8, rom_name: "F1", status: "failed", error: "disk full" }), done({ rom_id: 9, rom_name: "F2", status: "failed" }), done({ rom_id: 10, rom_name: "C", status: "cancelled" }));
      const { container } = await renderQueue();
      const text = finished(container).map((e) => e.textContent);
      expect(text.join("|")).toContain("Metroid · Completed — 2.0 KB");
      expect(text.join("|")).toContain("F1 · Failed: disk full");
      expect(text.join("|")).toMatch(/F2 · Failed(?!:)/);
      expect(text.join("|")).toContain("C · Cancelled");
    });

    it("a completed ROM gets an Open button to its Steam page; failed, firmware, no shortcut and compact do not", async () => {
      vi.mocked(backend.getAppIdRomIdMap).mockResolvedValue({ "4000000007": 7 });
      queueOf(done());
      const first = await renderQueue();
      await click(buttonByText(first.container, "Open"));
      expect(Navigation.Navigate).toHaveBeenCalledWith("/library/app/4000000007");
      first.unmount();
      queueOf(done({ status: "failed" }));
      expect(buttonByText((await renderQueue()).container, "Open")).toBeNull();
      queueOf(done({ kind: "firmware" }));
      expect(buttonByText((await renderQueue()).container, "Open")).toBeNull();
      vi.mocked(backend.getAppIdRomIdMap).mockResolvedValue({});
      queueOf(done());
      expect(buttonByText((await renderQueue()).container, "Open")).toBeNull();
      vi.mocked(backend.getAppIdRomIdMap).mockResolvedValue({ "4000000007": 7 });
      queueOf(done());
      expect(buttonByText((await renderQueue({ compact: true })).container, "Open")).toBeNull();
    });

    it("Clear completed hides finished items, and a restarted download shows again", async () => {
      queueOf(done(), makeItem({ rom_id: 2, rom_name: "Running", resumable: true }));
      const { container } = await renderQueue();
      expect(finished(container)).toHaveLength(1);
      await click(buttonByText(container, "Clear completed"));
      expect(finished(container)).toHaveLength(0);
      expect(row(container, "Running")).toBeTruthy();
      // the same ROM starts again: it is not hidden by the earlier clear
      await act(async () => {
        setDownloads([makeItem({ rom_id: 7, rom_name: "Metroid", status: "queued" }), makeItem({ rom_id: 2, rom_name: "Running" })]);
        await vi.advanceTimersByTimeAsync(600);
      });
      expect(row(container, "Metroid")).toBeTruthy();
    });

    it("clearing a firmware row does not clear the ROM with the same number", async () => {
      queueOf(done({ rom_id: 7, rom_name: "Zelda" }), done({ kind: "firmware", rom_id: 0, firmware_id: 7, rom_name: "Fw.zip", status: "failed" }));
      const { container } = await renderQueue();
      expect(finished(container)).toHaveLength(2);
      await click(buttonByText(container, "Clear completed"));
      expect(finished(container)).toHaveLength(0);
    });
  });

  describe("compact (Quick Access Menu)", () => {
    it("has a Back button, icon controls and the link to the full queue", async () => {
      const onBack = vi.fn();
      queueOf(makeItem({ rom_id: 3, rom_name: "A", resumable: true }));
      const { container } = await renderQueue({ compact: true, onBack });
      await click(buttonByText(container, "Back"));
      expect(onBack).toHaveBeenCalledTimes(1);
      await click(rowButton(container, "A", "Pause"));
      expect(backend.pauseDownload).toHaveBeenCalledWith(3);
      await click(buttonByText(container, "Full queue in RomM Library"));
      expect(getLibraryTab()).toBe("downloads");
      expect(Navigation.Navigate).toHaveBeenCalledWith("/romm-library");
    });

    it("the full queue has no Back button", async () => {
      queueOf(makeItem({ rom_id: 3, rom_name: "A" }));
      expect(buttonByText((await renderQueue()).container, "Back")).toBeNull();
    });
  });
});
