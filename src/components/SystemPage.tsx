import { useState, useEffect, FC } from "react";
import { PanelSection, PanelSectionRow, ButtonItem, Field, Focusable } from "@decky/ui";
import { getFirmwareStatus, downloadAllFirmware, downloadRequiredFirmware } from "../api/backend";
import type { FirmwarePlatformExt } from "../types";
import { scrollToTop } from "../utils/scrollHelpers";
import { detach } from "../utils/detach";

/**
 * Build the per-platform summary label/description from the backend BIOS
 * aggregates. The ok/partial/missing DECISION is the backend's `bios_level`
 * (`compute_bios_level`) — `requiredReady` is `bios_level === "ok"`, so the
 * required-files threshold is no longer re-compared here. `requiredCount` still
 * selects the phrasing axis (required vs. plain file counts), and the
 * optional-missing breakdown stays a local computation passed in by the caller.
 */
function getBiosSummary(
  requiredCount: number,
  requiredDone: number,
  requiredReady: boolean,
  optionalMissing: number,
  done: number,
  total: number,
  allDone: boolean,
) {
  if (requiredCount > 0 && requiredReady) {
    return {
      summaryLabel: `${requiredDone} / ${requiredCount} required`,
      summaryDescription:
        optionalMissing > 0 ? `All required ready (${optionalMissing} optional missing)` : "All required ready",
    };
  }
  if (requiredCount > 0) {
    return {
      summaryLabel: `${requiredDone} / ${requiredCount} required`,
      summaryDescription: `${requiredCount - requiredDone} required missing — games may not launch`,
    };
  }
  return {
    summaryLabel: `${done} / ${total} files`,
    summaryDescription: allDone ? "All downloaded" : `${total - done} missing`,
  };
}

function hashIndicator(hv: boolean | null): string {
  if (hv === true) return " ✓";
  if (hv === false) return " ⚠";
  return " —";
}

interface SystemPageProps {
  onBack: () => void;
}

/**
 * Top-level QAM destination for per-system firmware: the firmware files each
 * synced platform needs and their download state, from the
 * `get_firmware_status` multi-platform overview. The active emulator core is
 * shown read-only (per-system core selection and BIOS deletion were removed
 * until v0.90 — see REMOVED_TILL_0_90_0.md).
 */
export const SystemPage: FC<SystemPageProps> = ({ onBack }) => {
  const [biosPlatforms, setBiosPlatforms] = useState<FirmwarePlatformExt[]>([]);
  const [biosLoading, setBiosLoading] = useState(true);
  const [biosError, setBiosError] = useState("");
  const [serverOffline, setServerOffline] = useState(false);
  const [downloading, setDownloading] = useState<string | null>(null);
  const [biosStatus, setBiosStatus] = useState("");
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});

  async function refreshSystem() {
    setBiosLoading(true);
    setBiosError("");
    try {
      const result = await getFirmwareStatus();
      if (result.success) {
        setBiosPlatforms(result.platforms);
        setServerOffline(result.server_offline ?? false);
      } else {
        setBiosError(result.message || "Failed to fetch firmware status");
      }
    } catch (e) {
      setBiosError(`Failed to fetch firmware status: ${e}`);
    }
    setBiosLoading(false);
  }

  // Load System data (core + BIOS) on mount — this page IS the System view.
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- initial async data loads on mount are the standard React pattern; the rule is overzealous here
    detach(refreshSystem());
  }, []);

  const handleDownloadAll = async (platformSlug: string) => {
    setDownloading(platformSlug);
    setBiosStatus("");
    try {
      const result = await downloadAllFirmware(platformSlug);
      if (result.success) {
        setBiosStatus(result.message || `Downloaded ${result.downloaded} files`);
        await refreshSystem();
      } else {
        setBiosStatus(result.message || "Download failed");
      }
    } catch (e) {
      setBiosStatus(`Download failed: ${e}`);
    }
    setDownloading(null);
  };

  const handleDownloadRequired = async (platformSlug: string) => {
    setDownloading(platformSlug);
    setBiosStatus("");
    try {
      const result = await downloadRequiredFirmware(platformSlug);
      if (result.success) {
        setBiosStatus(result.message || `Downloaded ${result.downloaded} required files`);
        await refreshSystem();
      } else {
        setBiosStatus(result.message || "Download failed");
      }
    } catch (e) {
      setBiosStatus(`Download failed: ${e}`);
    }
    setDownloading(null);
  };

  // Only currently-synced systems are shown (#956): a platform counts as synced
  // when it has at least one ROM bound to a Steam shortcut (has_games).
  const syncedPlatforms = biosPlatforms.filter((p) => p.has_games);

  const renderBiosPlatform = (platform: FirmwarePlatformExt) => {
    const unknownFiles = platform.files.filter((f) => f.classification === "unknown");
    // Display counts come from the backend aggregates (computed from the same
    // core-aware files); fall back to local derivation only if a payload omits
    // them. The optional-missing breakdown stays a local file-level axis — the
    // 3-state bios_level doesn't model it.
    const total = platform.server_count ?? platform.files.length;
    const done = platform.local_count ?? platform.files.filter((f) => f.downloaded).length;
    const allDone = done === total;
    const isDownloading = downloading === platform.platform_slug;
    const isExpanded = expanded[platform.platform_slug] ?? false;

    const requiredFiles = platform.files.filter((f) => f.classification === "required");
    const requiredCount = platform.required_count ?? requiredFiles.length;
    const requiredDone = platform.required_downloaded ?? requiredFiles.filter((f) => f.downloaded).length;
    const optionalMissing = platform.files.filter((f) => f.classification === "optional" && !f.downloaded).length;

    // The ok/partial/missing DECISION is the backend's bios_level — "ready"
    // means all required files present (bios_level === "ok"). Fall back to the
    // local count comparison only when the level is absent from the payload.
    const requiredReady = platform.bios_level == null ? requiredDone === requiredCount : platform.bios_level === "ok";

    const needsAttention = platform.has_games && requiredCount > 0 && !requiredReady;
    const { summaryLabel, summaryDescription } = getBiosSummary(
      requiredCount,
      requiredDone,
      requiredReady,
      optionalMissing,
      done,
      total,
      allDone,
    );
    const hasRequiredMissing = requiredCount > 0 && !requiredReady;
    const hasOptionalMissing = optionalMissing > 0;

    return (
      <PanelSection
        key={platform.platform_slug}
        title={`${platform.platform_slug}${needsAttention ? " — BIOS needed" : ""}`}
      >
        {platform.active_core_label && (
          <PanelSectionRow>
            <Field label="Emulator Core" description={platform.active_core_label} />
          </PanelSectionRow>
        )}
        <PanelSectionRow>
          <Field label={summaryLabel} description={summaryDescription} />
        </PanelSectionRow>
        <PanelSectionRow>
          <ButtonItem
            layout="below"
            onClick={() =>
              setExpanded((prev) => ({
                ...prev,
                [platform.platform_slug]: !prev[platform.platform_slug],
              }))
            }
          >
            {isExpanded ? "Hide Files" : `Show Files (${total})`}
          </ButtonItem>
        </PanelSectionRow>
        {isExpanded && (
          <Focusable>
            {platform.files.map((file) => {
              let dotColor: string;
              if (file.classification === "unknown") {
                dotColor = "#d4a72c";
              } else if (file.downloaded) {
                dotColor = "#5ba32b";
              } else if (file.classification === "required") {
                dotColor = "#d94126";
              } else {
                dotColor = "#8f98a0";
              }
              return (
                <PanelSectionRow key={file.id}>
                  <Field
                    label={
                      <span style={{ display: "flex", alignItems: "center", gap: "6px" }}>
                        <span
                          style={{
                            display: "inline-block",
                            width: "8px",
                            height: "8px",
                            borderRadius: "50%",
                            backgroundColor: dotColor,
                            flexShrink: 0,
                          }}
                        />
                        {`${file.description || file.file_name} (${file.classification})`}
                      </span>
                    }
                    description={
                      file.downloaded
                        ? `${file.file_name}${hashIndicator(file.hash_valid)}`
                        : `${file.file_name} — Missing`
                    }
                  />
                </PanelSectionRow>
              );
            })}
            {unknownFiles.length > 0 && (
              <PanelSectionRow>
                <Field
                  label={`${unknownFiles.length} file(s) not recognized`}
                  description="Report at github.com/danielcopper/decky-romm-sync/issues if needed."
                />
              </PanelSectionRow>
            )}
          </Focusable>
        )}
        {hasRequiredMissing && !serverOffline && (
          <PanelSectionRow>
            <ButtonItem
              layout="below"
              onClick={() => {
                detach(handleDownloadRequired(platform.platform_slug));
              }}
              disabled={isDownloading}
            >
              {isDownloading ? "Downloading..." : "Download Required"}
            </ButtonItem>
          </PanelSectionRow>
        )}
        {!allDone && (hasOptionalMissing || hasRequiredMissing) && !serverOffline && (
          <PanelSectionRow>
            <ButtonItem
              layout="below"
              onClick={() => {
                detach(handleDownloadAll(platform.platform_slug));
              }}
              disabled={isDownloading}
            >
              {isDownloading ? "Downloading..." : "Download All"}
            </ButtonItem>
          </PanelSectionRow>
        )}
      </PanelSection>
    );
  };

  return (
    <>
      <PanelSection>
        <PanelSectionRow>
          <ButtonItem
            layout="below"
            onClick={onBack}
            // @ts-expect-error onFocus works at runtime; not in Decky's ButtonItem types
            onFocus={scrollToTop}
          >
            Back
          </ButtonItem>
        </PanelSectionRow>
      </PanelSection>

      <PanelSection title="System">
        <PanelSectionRow>
          <div style={{ fontSize: "11px", color: "#8f98a0", padding: "0 16px 4px" }}>
            Per-system firmware files. The active core determines which files a system needs.
          </div>
        </PanelSectionRow>
        {biosLoading && (
          <PanelSectionRow>
            <Field label="Loading firmware status..." />
          </PanelSectionRow>
        )}

        {biosError && (
          <PanelSectionRow>
            <Field label="Error" description={biosError} />
          </PanelSectionRow>
        )}

        {serverOffline && (
          <PanelSectionRow>
            <Field
              label="Server offline"
              description="RomM server is unreachable. Firmware downloads are unavailable."
            />
          </PanelSectionRow>
        )}

        {!biosLoading && !biosError && syncedPlatforms.length === 0 && (
          <PanelSectionRow>
            <Field label="No synced systems" description="Sync some games to manage their firmware files here." />
          </PanelSectionRow>
        )}

        {biosStatus && (
          <PanelSectionRow>
            <Field label={biosStatus} />
          </PanelSectionRow>
        )}
      </PanelSection>

      {syncedPlatforms.map(renderBiosPlatform)}
    </>
  );
};
