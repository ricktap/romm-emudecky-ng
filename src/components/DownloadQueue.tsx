import { useState, useEffect, useRef, FC, Fragment } from "react";
import { toaster } from "@decky/api";
import { PanelSection, PanelSectionRow, ButtonItem, Field, ProgressBar } from "@decky/ui";
import { getDownloadQueue, cancelDownload, pauseDownload, resumeDownload } from "../api/backend";
import { getDownloadState, setDownloads } from "../utils/downloadStore";
import { formatBytes } from "../utils/formatters";
import { scrollToTop } from "../utils/scrollHelpers";
import { detach } from "../utils/detach";
import {
  isStoppedStatus,
  isUnfinishedStatus,
  resumeActionLabel,
  stoppedCaptionSuffix,
} from "../utils/downloadStatus";
import { phaseLines, useNowMs } from "../utils/downloadPhase";
import { callArgs, downloadKey } from "../utils/downloadKey";
import type { DownloadItem } from "../types";

interface DownloadQueueProps {
  onBack: () => void;
}

function formatFinishedDescription(item: DownloadItem): string {
  if (item.status === "completed") return `Completed — ${formatBytes(item.total_bytes)}`;
  if (item.status === "failed") {
    const detail = item.error ? `: ${item.error}` : "";
    return `Failed${detail}`;
  }
  return "Cancelled";
}

/**
 * The honest waiting state: the request is out and no response headers have arrived (a multipart archive may
 * still be preparing on RomM). Elapsed time ticks locally every second; no request is sent to refresh it.
 */
const WaitingNote: FC<{ item: DownloadItem }> = ({ item }) => {
  const now = useNowMs(true);
  const [headline, detail] = phaseLines(item, now);
  return (
    <div data-testid="dl-waiting" style={{ fontSize: "12px", opacity: 0.85, marginBottom: "4px" }}>
      <div data-testid="dl-waiting-text">{headline}</div>
      <div data-testid="dl-waiting-detail">{detail}</div>
    </div>
  );
};

export const DownloadQueue: FC<DownloadQueueProps> = ({ onBack }) => {
  const [downloads, setLocalDownloads] = useState<DownloadItem[]>([]); // NOSONAR(typescript:S6754) — setter intentionally renamed (local wrapper around global download state).
  const [cleared, setCleared] = useState<Set<string>>(new Set());
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const stopPolling = () => {
    if (pollRef.current) {
      clearInterval(pollRef.current);
      pollRef.current = null;
    }
  };

  // Un-clear rom_ids that have new active downloads (re-download case)
  const unclearRestarted =
    (current: DownloadItem[]) =>
    (prev: Set<string>): Set<string> => {
      const restarted = current.filter((d) => isUnfinishedStatus(d.status) && prev.has(downloadKey(d)));
      if (restarted.length === 0) return prev;
      const next = new Set(prev);
      for (const d of restarted) next.delete(downloadKey(d));
      return next;
    };

  const pollTick = () => {
    const current = getDownloadState();
    setCleared(unclearRestarted(current));
    setLocalDownloads([...current]);
  };

  const startPolling = () => {
    stopPolling();
    pollRef.current = setInterval(pollTick, 500);
  };

  useEffect(() => {
    // Seed from backend on mount, then poll the store
    getDownloadQueue()
      .then((result) => {
        setDownloads(result.downloads);
        setLocalDownloads([...result.downloads]);
      })
      .catch(() => {
        // Fall back to whatever is in the store already
        setLocalDownloads([...getDownloadState()]);
      });
    startPolling();
    return () => stopPolling();
  }, []);

  // A ROM is addressed by its id alone; firmware also names its kind (see downloadKey).
  const handleCancel = async (item: DownloadItem) => {
    try {
      await cancelDownload(...callArgs(item));
    } catch {
      // ignore
    }
  };

  const handlePause = async (item: DownloadItem) => {
    try {
      await pauseDownload(...callArgs(item));
    } catch {
      // ignore
    }
  };

  const handleResume = async (item: DownloadItem) => {
    try {
      const result = await resumeDownload(...callArgs(item));
      // A refused resume (server unreachable, sign-in rejected, superseded, ...) used to vanish silently.
      if (result && !result.success) {
        toaster.toast({ title: "EmuDecky RomM Manager", body: result.message || "Could not resume the download" });
      }
    } catch {
      toaster.toast({ title: "EmuDecky RomM Manager", body: "Could not resume the download" });
    }
  };

  const handleClearCompleted = () => {
    const finishedIds = downloads
      .filter((d) => d.status === "completed" || d.status === "failed" || d.status === "cancelled")
      .map(downloadKey);
    setCleared((prev) => {
      const next = new Set(prev);
      finishedIds.forEach((id) => next.add(id));
      return next;
    });
  };

  const visible = downloads.filter((d) => !cleared.has(downloadKey(d)));
  // Paused, interrupted and extracting downloads stay in the active section —
  // they're not finished. Paused/interrupted hold the partial transfer for
  // resume; extracting is the post-transfer ZIP unpack (not cancellable, no
  // pause/resume offered).
  const active = visible.filter((d) => isUnfinishedStatus(d.status));
  const finished = visible.filter((d) => d.status === "completed" || d.status === "failed" || d.status === "cancelled");
  const hasFinished = downloads.some(
    (d) => !cleared.has(downloadKey(d)) && (d.status === "completed" || d.status === "failed" || d.status === "cancelled"),
  );

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

      <PanelSection title="Downloads">
        {visible.length === 0 ? (
          <PanelSectionRow>
            <Field label="No downloads" />
          </PanelSectionRow>
        ) : (
          <>
            {active.map((item) => (
              <PanelSectionRow key={downloadKey(item)}>
                {/* Own the caption in a full-width row and use the bare ProgressBar.
                    ProgressBarWithInfo is a Steam Field (label column | bar column);
                    with the rom name in sOperationText the empty bar column gets
                    squeezed into the right half and clips (#751). The bare
                    ProgressBar is just the bar and spans the full panel width
                    (mirrors the sync-progress fix in MainPage). */}
                <div style={{ width: "100%" }}>
                  <div
                    style={{
                      display: "flex",
                      justifyContent: "space-between",
                      fontSize: "12px",
                      marginBottom: "4px",
                    }}
                  >
                    <span
                      data-testid="dl-caption"
                      style={{ minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
                    >
                      {item.rom_name} ({item.platform_name}){stoppedCaptionSuffix(item.status, item.reason)}
                      {item.status === "extracting" ? " — Extracting…" : ""}
                    </span>
                    {!(item.status === "downloading" && item.phase) && (
                      <span data-testid="dl-bytes" style={{ flexShrink: 0 }}>
                        {item.total_bytes > 0
                          ? `${formatBytes(item.bytes_downloaded)} / ${formatBytes(item.total_bytes)}`
                          : formatBytes(item.bytes_downloaded)}
                      </span>
                    )}
                  </div>
                  {item.status === "downloading" && item.phase && <WaitingNote item={item} />}
                  <ProgressBar
                    indeterminate={item.total_bytes === 0 || (item.status === "downloading" && Boolean(item.phase))}
                    {...(item.total_bytes > 0 ? { nProgress: (item.bytes_downloaded / item.total_bytes) * 100 } : {})}
                  />
                </div>
              </PanelSectionRow>
            ))}
            {active.map((item) =>
              // Extraction is not cancellable and can't pause/resume — the
              // post-transfer unpack runs to completion. Offer no action row,
              // matching the game-detail button's disabled throbber.
              item.status === "extracting" ? null : (
                <Fragment key={`actions-${downloadKey(item)}`}>
                  {item.status === "downloading" && (item.resumable || item.phase) && (
                    <PanelSectionRow>
                      <ButtonItem
                        layout="below"
                        onClick={() => {
                          detach(handlePause(item));
                        }}
                      >
                        Pause {item.rom_name}
                      </ButtonItem>
                    </PanelSectionRow>
                  )}
                  {isStoppedStatus(item.status) && (
                    <PanelSectionRow>
                      <ButtonItem
                        layout="below"
                        onClick={() => {
                          detach(handleResume(item));
                        }}
                      >
                        {resumeActionLabel(item.reason)} {item.rom_name}
                      </ButtonItem>
                    </PanelSectionRow>
                  )}
                  <PanelSectionRow>
                    <ButtonItem
                      layout="below"
                      onClick={() => {
                        detach(handleCancel(item));
                      }}
                    >
                      Cancel {item.rom_name}
                    </ButtonItem>
                  </PanelSectionRow>
                </Fragment>
              ),
            )}

            {finished.map((item) => (
              <PanelSectionRow key={downloadKey(item)}>
                <Field label={item.rom_name} description={formatFinishedDescription(item)} />
              </PanelSectionRow>
            ))}

            {hasFinished && (
              <PanelSectionRow>
                <ButtonItem layout="below" onClick={handleClearCompleted}>
                  Clear Completed
                </ButtonItem>
              </PanelSectionRow>
            )}
          </>
        )}
      </PanelSection>
    </>
  );
};
