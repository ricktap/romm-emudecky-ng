// CATCH-REJECTION ASSERTION RULE (applies to all orchestration shell tests):
// Every catch block with a setX(...) side effect MUST have its side effect
// asserted in the test (surfaced biosError / biosStatus string, debugLog spy,
// etc.). Asserting only that the rejecting call was invoked is vacuous — the
// rejection happens after the call returns so the test would pass with or
// without the .catch.
//
// SystemPage catch sites (all asserted below):
//   - refreshSystem try/catch → setBiosError(`Failed to fetch firmware status: ${e}`)
//   - refreshSystem failure branch → setBiosError(result.message || fallback)
//   - handleDownloadAll catch → setBiosStatus(`Download failed: ${e}`)
//   - handleDownloadRequired catch → setBiosStatus(`Download failed: ${e}`)

import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, fireEvent, act } from "@testing-library/react";
import { SystemPage } from "./SystemPage";
import * as backend from "../api/backend";
import type { FirmwarePlatformExt } from "../types";

// scrollToTop is a no-op in happy-dom; mock for cleanliness.
vi.mock("../utils/scrollHelpers", () => ({ scrollToTop: vi.fn() }));

vi.mock("@decky/ui", async () => {
  const { createElement: ce } = await import("react");
  type AnyProps = Record<string, unknown> & { children?: unknown };
  const passthrough = (tag: string) => (p: AnyProps) => ce(tag, {}, p.children as never);
  return {
    PanelSection: (p: AnyProps & { title?: unknown }) =>
      ce(
        "section",
        { "data-testid": "panel-section", "data-title": typeof p.title === "string" ? p.title : undefined },
        typeof p.title === "string" ? ce("h2", { "data-testid": "panel-title" }, p.title) : null,
        p.children as never,
      ),
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
    Focusable: passthrough("div"),
    Spinner: () => ce("div", { "data-testid": "spinner" }),
  };
});

// Flush mount-time + chained promise resolutions.
const flushAsync = () =>
  act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });

function makeBiosPlatform(overrides: Partial<FirmwarePlatformExt> = {}): FirmwarePlatformExt {
  const files = overrides.files ?? [];
  // Mirror the backend per-platform BIOS aggregates (_set_platform_bios_aggregates):
  // the real get_firmware_status payload always ships these, so the helper derives
  // them from the files unless a test overrides them explicitly.
  const serverCount = files.length;
  const localCount = files.filter((f) => f.downloaded).length;
  const requiredFiles = files.filter((f) => f.classification === "required");
  const requiredCount = requiredFiles.length;
  const requiredDownloaded = requiredFiles.filter((f) => f.downloaded).length;
  const biosLevel: "ok" | "partial" | "missing" =
    requiredDownloaded >= requiredCount ? "ok" : requiredDownloaded > 0 ? "partial" : "missing";
  return {
    platform_slug: "snes",
    files: [],
    has_games: true,
    server_count: serverCount,
    local_count: localCount,
    required_count: requiredCount,
    required_downloaded: requiredDownloaded,
    bios_level: biosLevel,
    ...overrides,
  };
}

describe("SystemPage", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    // Default callable behavior — tests override per case.
    vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
      success: true,
      platforms: [],
    });
    vi.mocked(backend.downloadAllFirmware).mockResolvedValue({ success: true });
    vi.mocked(backend.downloadRequiredFirmware).mockResolvedValue({
      success: true,
    });
  });

  // ------------------------------------------------------------------
  // Initial render — loads on mount (no tab click; this page IS the view)
  // ------------------------------------------------------------------
  describe("initial render", () => {
    it("calls getFirmwareStatus once on mount", async () => {
      render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      expect(vi.mocked(backend.getFirmwareStatus)).toHaveBeenCalledTimes(1);
    });

    it("renders the loading state before getFirmwareStatus resolves and removes it after", async () => {
      const { container } = render(<SystemPage onBack={vi.fn()} />);
      // Initial render — getFirmwareStatus not yet resolved
      expect(container.textContent).toContain("Loading firmware status...");
      await flushAsync();
      expect(container.textContent).not.toContain("Loading firmware status...");
    });
  });

  // ------------------------------------------------------------------
  // I. refreshSystem
  // ------------------------------------------------------------------
  describe("refreshSystem", () => {
    it("renders platforms and sets serverOffline on success", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        server_offline: false,
        platforms: [
          makeBiosPlatform({
            platform_slug: "snes",
            files: [
              {
                id: 1,
                file_name: "snes.rom",
                size: 100,
                md5: "x",
                downloaded: true,
                required: true,
                description: "BIOS",
                hash_valid: true,
                classification: "required",
              },
            ],
          }),
        ],
      });
      const { container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      expect(container.textContent).toContain("snes");
    });

    it("renders the server-offline banner when getFirmwareStatus reports server_offline", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        server_offline: true,
        platforms: [],
      });
      const { container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      expect(container.textContent).toContain("Server offline");
    });

    it("surfaces result.message when getFirmwareStatus returns success=false with a message", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: false,
        message: "Server is sad",
        platforms: [],
      });
      const { container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      // CATCH-REJECTION (failure branch): biosError = result.message
      expect(container.textContent).toContain("Server is sad");
    });

    it("falls back to 'Failed to fetch firmware status' when result.message is absent", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: false,
        platforms: [],
      });
      const { container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      expect(container.textContent).toContain("Failed to fetch firmware status");
    });

    it("sets biosError='Failed to fetch firmware status: <e>' when getFirmwareStatus throws", async () => {
      vi.mocked(backend.getFirmwareStatus).mockRejectedValue(new Error("network"));
      const { container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      // CATCH-REJECTION assert: rendered with the interpolated Error
      expect(container.textContent).toContain("Failed to fetch firmware status: Error: network");
    });

    it("renders the no-synced-systems empty state when platforms list is empty", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [],
      });
      const { container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      expect(container.textContent).toContain("No synced systems");
    });

    it("renders only currently-synced systems (has_games), hiding unsynced ones", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [
          makeBiosPlatform({ platform_slug: "snes", has_games: true }),
          makeBiosPlatform({ platform_slug: "ps2", has_games: false }),
        ],
      });
      const { container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      // The synced platform renders; the unsynced one is filtered out entirely.
      expect(container.textContent).toContain("snes");
      expect(container.textContent).not.toContain("ps2");
    });

    it("renders the no-synced-systems empty state when BIOS platforms exist but none are synced", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [
          makeBiosPlatform({ platform_slug: "snes", has_games: false }),
          makeBiosPlatform({ platform_slug: "ps2", has_games: false }),
        ],
      });
      const { container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      expect(container.textContent).toContain("No synced systems");
      // Neither unsynced platform is rendered as a section.
      expect(container.textContent).not.toContain("snes");
      expect(container.textContent).not.toContain("ps2");
    });
  });

  // ------------------------------------------------------------------
  // J. handleDownloadAll
  // ------------------------------------------------------------------
  describe("handleDownloadAll", () => {
    function biosPlatformWithMissingOptional(): FirmwarePlatformExt {
      return makeBiosPlatform({
        platform_slug: "snes",
        files: [
          {
            id: 1,
            file_name: "boot.rom",
            size: 100,
            md5: "x",
            downloaded: false,
            required: false,
            description: "Optional",
            hash_valid: null,
            classification: "optional",
          },
        ],
      });
    }

    it("calls downloadAllFirmware(slug) and then refreshes on success", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [biosPlatformWithMissingOptional()],
      });
      vi.mocked(backend.downloadAllFirmware).mockResolvedValue({
        success: true,
        downloaded: 1,
      });
      const { getByText } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      await act(async () => {
        fireEvent.click(getByText("Download All"));
        await Promise.resolve();
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(vi.mocked(backend.downloadAllFirmware)).toHaveBeenCalledWith("snes");
      // refreshSystem called once on mount + once after download
      expect(vi.mocked(backend.getFirmwareStatus)).toHaveBeenCalledTimes(2);
    });

    it("surfaces result.message when the download succeeds", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [biosPlatformWithMissingOptional()],
      });
      vi.mocked(backend.downloadAllFirmware).mockResolvedValue({
        success: true,
        message: "All good",
        downloaded: 1,
      });
      const { getByText, container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      await act(async () => {
        fireEvent.click(getByText("Download All"));
        await Promise.resolve();
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(container.textContent).toContain("All good");
    });

    it("surfaces 'Download failed' when result.success=false with no message", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [biosPlatformWithMissingOptional()],
      });
      vi.mocked(backend.downloadAllFirmware).mockResolvedValue({
        success: false,
      });
      const { getByText, container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      await act(async () => {
        fireEvent.click(getByText("Download All"));
        await Promise.resolve();
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(container.textContent).toContain("Download failed");
    });

    it("sets biosStatus='Download failed: <e>' when downloadAllFirmware throws", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [biosPlatformWithMissingOptional()],
      });
      vi.mocked(backend.downloadAllFirmware).mockRejectedValue(new Error("io"));
      const { getByText, container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      await act(async () => {
        fireEvent.click(getByText("Download All"));
        await Promise.resolve();
        await Promise.resolve();
        await Promise.resolve();
      });
      // CATCH-REJECTION assert: status string rendered
      expect(container.textContent).toContain("Download failed: Error: io");
    });
  });

  // ------------------------------------------------------------------
  // K. handleDownloadRequired
  // ------------------------------------------------------------------
  describe("handleDownloadRequired", () => {
    function biosPlatformWithMissingRequired(): FirmwarePlatformExt {
      return makeBiosPlatform({
        platform_slug: "snes",
        files: [
          {
            id: 1,
            file_name: "bios.rom",
            size: 100,
            md5: "x",
            downloaded: false,
            required: true,
            description: "Required BIOS",
            hash_valid: null,
            classification: "required",
          },
        ],
      });
    }

    it("calls downloadRequiredFirmware(slug) and refreshes on success", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [biosPlatformWithMissingRequired()],
      });
      vi.mocked(backend.downloadRequiredFirmware).mockResolvedValue({
        success: true,
        downloaded: 1,
      });
      const { getByText } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      await act(async () => {
        fireEvent.click(getByText("Download Required"));
        await Promise.resolve();
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(vi.mocked(backend.downloadRequiredFirmware)).toHaveBeenCalledWith("snes");
      expect(vi.mocked(backend.getFirmwareStatus)).toHaveBeenCalledTimes(2);
    });

    it("surfaces 'Download failed: <e>' when downloadRequiredFirmware throws", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [biosPlatformWithMissingRequired()],
      });
      vi.mocked(backend.downloadRequiredFirmware).mockRejectedValue(new Error("io"));
      const { getByText, container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      await act(async () => {
        fireEvent.click(getByText("Download Required"));
        await Promise.resolve();
        await Promise.resolve();
        await Promise.resolve();
      });
      // CATCH-REJECTION assert: status string rendered
      expect(container.textContent).toContain("Download failed: Error: io");
    });

    it("surfaces 'Download failed' fallback when result.success=false with no message", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [biosPlatformWithMissingRequired()],
      });
      vi.mocked(backend.downloadRequiredFirmware).mockResolvedValue({
        success: false,
      });
      const { getByText, container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      await act(async () => {
        fireEvent.click(getByText("Download Required"));
        await Promise.resolve();
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(container.textContent).toContain("Download failed");
    });
  });

  // ------------------------------------------------------------------
  // L. expand/collapse + hashIndicator + unknown summary
  // ------------------------------------------------------------------
  describe("expand/collapse and file rendering", () => {
    it("expands files on Show Files click and collapses on the same button", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [
          makeBiosPlatform({
            platform_slug: "snes",
            files: [
              {
                id: 1,
                file_name: "ok.bin",
                size: 100,
                md5: "x",
                downloaded: true,
                required: false,
                description: "OK File",
                hash_valid: true,
                classification: "optional",
              },
            ],
          }),
        ],
      });
      const { getByText, container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      // Initially collapsed — file name not rendered
      expect(container.textContent).not.toContain("OK File");
      await act(async () => {
        fireEvent.click(getByText("Show Files (1)"));
        await Promise.resolve();
      });
      expect(container.textContent).toContain("OK File");
      // Now collapse
      await act(async () => {
        fireEvent.click(getByText("Hide Files"));
        await Promise.resolve();
      });
      expect(container.textContent).not.toContain("OK File");
    });

    it("renders hashIndicator ' ✓' for downloaded files with hash_valid=true", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [
          makeBiosPlatform({
            platform_slug: "snes",
            files: [
              {
                id: 1,
                file_name: "good.rom",
                size: 100,
                md5: "x",
                downloaded: true,
                required: false,
                description: "Good",
                hash_valid: true,
                classification: "optional",
              },
            ],
          }),
        ],
      });
      const { getByText, container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      await act(async () => {
        fireEvent.click(getByText("Show Files (1)"));
        await Promise.resolve();
      });
      expect(container.textContent).toContain("good.rom ✓");
    });

    it("renders hashIndicator ' ⚠' for downloaded files with hash_valid=false", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [
          makeBiosPlatform({
            platform_slug: "snes",
            files: [
              {
                id: 1,
                file_name: "bad.rom",
                size: 100,
                md5: "x",
                downloaded: true,
                required: false,
                description: "Bad",
                hash_valid: false,
                classification: "optional",
              },
            ],
          }),
        ],
      });
      const { getByText, container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      await act(async () => {
        fireEvent.click(getByText("Show Files (1)"));
        await Promise.resolve();
      });
      expect(container.textContent).toContain("bad.rom ⚠");
    });

    it("renders hashIndicator ' —' for downloaded files with hash_valid=null", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [
          makeBiosPlatform({
            platform_slug: "snes",
            files: [
              {
                id: 1,
                file_name: "unk.rom",
                size: 100,
                md5: "x",
                downloaded: true,
                required: false,
                description: "Unk",
                hash_valid: null,
                classification: "optional",
              },
            ],
          }),
        ],
      });
      const { getByText, container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      await act(async () => {
        fireEvent.click(getByText("Show Files (1)"));
        await Promise.resolve();
      });
      expect(container.textContent).toContain("unk.rom —");
    });

    it("renders a missing required file (red dot branch) when expanded", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [
          makeBiosPlatform({
            platform_slug: "snes",
            files: [
              {
                id: 1,
                file_name: "missing-req.rom",
                size: 100,
                md5: "x",
                downloaded: false,
                required: true,
                description: "ReqMissing",
                hash_valid: null,
                classification: "required",
              },
            ],
          }),
        ],
      });
      const { getByText, container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      await act(async () => {
        fireEvent.click(getByText("Show Files (1)"));
        await Promise.resolve();
      });
      // Missing required → "Missing" suffix and red dot branch
      expect(container.textContent).toContain("missing-req.rom — Missing");
    });

    it("renders a missing optional file (gray dot branch) when expanded", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [
          makeBiosPlatform({
            platform_slug: "snes",
            files: [
              {
                id: 1,
                file_name: "missing-opt.rom",
                size: 100,
                md5: "x",
                downloaded: false,
                required: false,
                description: "OptMissing",
                hash_valid: null,
                classification: "optional",
              },
            ],
          }),
        ],
      });
      const { getByText, container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      await act(async () => {
        fireEvent.click(getByText("Show Files (1)"));
        await Promise.resolve();
      });
      // Missing optional → "Missing" suffix and gray dot branch
      expect(container.textContent).toContain("missing-opt.rom — Missing");
    });

    it("renders the unrecognized-file footer when unknown files are present", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [
          makeBiosPlatform({
            platform_slug: "snes",
            files: [
              {
                id: 1,
                file_name: "mystery.rom",
                size: 100,
                md5: "x",
                downloaded: true,
                required: false,
                description: "?",
                hash_valid: null,
                classification: "unknown",
              },
            ],
          }),
        ],
      });
      const { getByText, container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      await act(async () => {
        fireEvent.click(getByText("Show Files (1)"));
        await Promise.resolve();
      });
      expect(container.textContent).toContain("1 file(s) not recognized");
    });
  });

  // ------------------------------------------------------------------
  // M. getBiosSummary indirect coverage via rendering
  // ------------------------------------------------------------------
  describe("summary text", () => {
    it("shows 'X / Y required' + 'All required ready' when all required are done and no optional missing", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [
          makeBiosPlatform({
            platform_slug: "snes",
            files: [
              {
                id: 1,
                file_name: "req.rom",
                size: 100,
                md5: "x",
                downloaded: true,
                required: true,
                description: "Req",
                hash_valid: true,
                classification: "required",
              },
            ],
          }),
        ],
      });
      const { container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      expect(container.textContent).toContain("1 / 1 required");
      expect(container.textContent).toContain("All required ready");
    });

    it("shows 'N optional missing' when all required are done but optional is missing", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [
          makeBiosPlatform({
            platform_slug: "snes",
            files: [
              {
                id: 1,
                file_name: "req.rom",
                size: 100,
                md5: "x",
                downloaded: true,
                required: true,
                description: "Req",
                hash_valid: true,
                classification: "required",
              },
              {
                id: 2,
                file_name: "opt.rom",
                size: 100,
                md5: "x",
                downloaded: false,
                required: false,
                description: "Opt",
                hash_valid: null,
                classification: "optional",
              },
            ],
          }),
        ],
      });
      const { container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      expect(container.textContent).toContain("All required ready (1 optional missing)");
    });

    it("shows 'N required missing — games may not launch' when required is incomplete", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [
          makeBiosPlatform({
            platform_slug: "snes",
            files: [
              {
                id: 1,
                file_name: "req.rom",
                size: 100,
                md5: "x",
                downloaded: false,
                required: true,
                description: "Req",
                hash_valid: null,
                classification: "required",
              },
            ],
          }),
        ],
      });
      const { container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      expect(container.textContent).toContain("1 required missing");
    });

    it("falls back to 'X / Y files' summary when there are no required files", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [
          makeBiosPlatform({
            platform_slug: "snes",
            files: [
              {
                id: 1,
                file_name: "opt.rom",
                size: 100,
                md5: "x",
                downloaded: true,
                required: false,
                description: "Opt",
                hash_valid: true,
                classification: "optional",
              },
            ],
          }),
        ],
      });
      const { container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      expect(container.textContent).toContain("1 / 1 files");
      expect(container.textContent).toContain("All downloaded");
    });

    it("shows 'N missing' suffix when not all files are downloaded and no required files exist", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [
          makeBiosPlatform({
            platform_slug: "snes",
            files: [
              {
                id: 1,
                file_name: "opt.rom",
                size: 100,
                md5: "x",
                downloaded: false,
                required: false,
                description: "Opt",
                hash_valid: null,
                classification: "optional",
              },
            ],
          }),
        ],
      });
      const { container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      expect(container.textContent).toContain("0 / 1 files");
      expect(container.textContent).toContain("1 missing");
    });
  });

  // ------------------------------------------------------------------
  // O. Back button
  // ------------------------------------------------------------------
  describe("active core (read-only)", () => {
    it("shows the active core label as plain text with no picker button", async () => {
      vi.mocked(backend.getFirmwareStatus).mockResolvedValue({
        success: true,
        platforms: [makeBiosPlatform({ platform_slug: "snes", active_core_label: "snes9x" })],
      });
      const { container } = render(<SystemPage onBack={vi.fn()} />);
      await flushAsync();
      expect(container.textContent).toContain("Emulator Core");
      expect(container.textContent).toContain("snes9x");
      expect(
        [...container.querySelectorAll("button")].some((b) => b.textContent.startsWith("Emulator Core:")),
      ).toBe(false);
    });
  });

  describe("back button", () => {
    it("invokes onBack when the Back button is clicked", async () => {
      const onBack = vi.fn();
      const { getByText } = render(<SystemPage onBack={onBack} />);
      await flushAsync();
      fireEvent.click(getByText("Back"));
      expect(onBack).toHaveBeenCalledTimes(1);
    });
  });
});
