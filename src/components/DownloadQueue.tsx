import { useState, useEffect, useRef, FC } from "react";
import { toaster } from "@decky/api";
import { Navigation } from "@decky/ui";
import {
  getDownloadQueue,
  cancelDownload,
  pauseDownload,
  resumeDownload,
  moveToFront,
  getAppIdRomIdMap,
} from "../api/backend";
import { recordBytes, formatSpeed, formatEta } from "../utils/downloadSpeed";
import { getDownloadState, setDownloads } from "../utils/downloadStore";
import { formatBytes } from "../utils/formatters";
import { detach } from "../utils/detach";
import {
  isStoppedStatus,
  isUnfinishedStatus,
  resumeActionLabel,
  interruptedLabel,
} from "../utils/downloadStatus";
import { phaseLines, useNowMs } from "../utils/downloadPhase";
import { callArgs, downloadKey } from "../utils/downloadKey";
import type { DownloadItem } from "../types";
import { openLibraryRoute } from "../utils/libraryRoute";
import { Icon, QamHeader, RmButton, RmButtonGroup, RmStyles } from "./rm";
import { platformAbbrev, tintFor } from "../utils/libraryGrid";

interface DownloadQueueProps {
  onBack: () => void;
  /** In the Quick Access Menu: adds the link to the full queue in the RomM Library. */
  compact?: boolean;
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

export const DownloadQueue: FC<DownloadQueueProps> = ({ onBack, compact }) => {
  const [downloads, setLocalDownloads] = useState<DownloadItem[]>([]); // NOSONAR(typescript:S6754) — setter intentionally renamed (local wrapper around global download state).
  const [cleared, setCleared] = useState<Set<string>>(new Set());
  // rom id -> Steam app id, to open a finished ROM's game page (full-screen queue only).
  const [appIds, setAppIds] = useState<Map<number, number>>(new Map());
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
    if (!compact) {
      getAppIdRomIdMap()
        .then((map) => setAppIds(new Map(Object.entries(map ?? {}).map(([appId, romId]) => [romId, Number(appId)]))))
        .catch(() => undefined);
    }
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

  // "Start now": first in line, and whatever else is transferring steps aside (it stays paused, not lost).
  const handleStartNow = async (item: DownloadItem, others: DownloadItem[]) => {
    try {
      const moved = await moveToFront(...callArgs(item));
      if (moved && !moved.success) {
        toaster.toast({ title: "EmuDecky RomM Manager", body: moved.message || "Could not reorder the queue" });
        return;
      }
      await Promise.all(
        others.filter((d) => d.status === "downloading" && downloadKey(d) !== downloadKey(item)).map(handlePause),
      );
      if (isStoppedStatus(item.status)) await handleResume(item);
      // The backend owns the order: re-read it so the list matches the new queue.
      const fresh = await getDownloadQueue();
      setDownloads(fresh.downloads);
      setLocalDownloads([...fresh.downloads]);
    } catch {
      toaster.toast({ title: "EmuDecky RomM Manager", body: "Could not start the download now" });
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

  // Speed and time left of the running transfer, from the byte counts seen on each refresh.
  const transferNote = (item: DownloadItem): string => {
    if (item.status !== "downloading") return "";
    const speed = recordBytes(downloadKey(item), item.bytes_downloaded, Date.now());
    const parts = [formatSpeed(speed), formatEta(item.total_bytes - item.bytes_downloaded, speed)].filter(Boolean);
    return parts.length ? ` · ${parts.join(" · ")}` : "";
  };

  const visible = downloads.filter((d) => !cleared.has(downloadKey(d)));
  // Paused, interrupted and extracting downloads stay in the active section —
  // they're not finished. Paused/interrupted hold the partial transfer for
  // resume; extracting is the post-transfer ZIP unpack (not cancellable, no
  // pause/resume offered).
  const active = visible.filter((d) => isUnfinishedStatus(d.status));
  const finished = visible.filter((d) => d.status === "completed" || d.status === "failed" || d.status === "cancelled");
  // "Pause all" pauses what is transferring; once nothing transfers, the same button resumes what is stopped.
  const pausable = active.filter((d) => d.status === "downloading" && (d.resumable || d.phase));
  const stopped = active.filter((d) => isStoppedStatus(d.status));
  const pauseAll = async () => {
    await Promise.all(pausable.map((d) => handlePause(d)));
  };
  const resumeAll = async () => {
    await Promise.all(stopped.map((d) => handleResume(d)));
  };
  const hasFinished = downloads.some(
    (d) => !cleared.has(downloadKey(d)) && (d.status === "completed" || d.status === "failed" || d.status === "cancelled"),
  );

  const queueCount = active.length;
  const paused = pausable.length === 0 && stopped.length > 0;
  const anyRunning = active.some((d) => d.status === "downloading");

  /** `Downloading 62%`, `Queued`, `Paused · 40%`, `Interrupted — server unreachable`. */
  const statusLabel = (d: DownloadItem): string => {
    const pct = d.total_bytes > 0 ? Math.floor((d.bytes_downloaded / d.total_bytes) * 100) : 0;
    switch (d.status) {
      case "queued":
        return "Queued";
      case "downloading":
        return `Downloading ${pct}%`;
      case "extracting":
        return "Extracting…";
      case "paused":
        return `Paused · ${pct}%`;
      case "interrupted":
        return interruptedLabel(d.reason);
      default:
        return d.status;
    }
  };
  const detailLine = (d: DownloadItem): string => {
    const size =
      d.total_bytes > 0
        ? `${formatBytes(d.bytes_downloaded)} / ${formatBytes(d.total_bytes)}`
        : formatBytes(d.bytes_downloaded);
    return [size + transferNote(d), d.platform_name].filter(Boolean).join(" · ");
  };
  /** The one control of a row: pause what runs, start now what waits, resume what stopped. */
  const rowAction = (d: DownloadItem): { label: string; run: () => void } | null => {
    if (d.status === "extracting") return null; // the unpack runs to completion
    if (d.status === "downloading") {
      return d.resumable || d.phase ? { label: "Pause", run: () => detach(handlePause(d)) } : null;
    }
    if (d.status === "queued") {
      return anyRunning ? { label: "Start now", run: () => detach(handleStartNow(d, active)) } : null;
    }
    if (isStoppedStatus(d.status)) {
      const label = resumeActionLabel(d.reason);
      return anyRunning
        ? { label: "Start now", run: () => detach(handleStartNow(d, active)) }
        : { label, run: () => detach(handleResume(d)) };
    }
    return null;
  };

  const Thumb: FC<{ item: DownloadItem; big?: boolean }> = ({ item, big }) => (
    <span
      className="rm-thumb"
      style={{ background: tintFor(item.rom_name), ...(big ? { width: 44, height: 60 } : {}) }}
    >
      {platformAbbrev(item.platform_name)}
    </span>
  );

  const queueBlock = active.map((item) => {
    const action = rowAction(item);
    const pct = item.total_bytes > 0 ? (item.bytes_downloaded / item.total_bytes) * 100 : 0;
    const waiting = item.status === "downloading" && Boolean(item.phase);
    const barStyle = { width: `${Math.min(100, pct)}%`, ...(isStoppedStatus(item.status) ? { background: "#d4a72c" } : {}) };
    return compact ? (
      <div className="rm-dlrow" key={downloadKey(item)} data-testid="dl-row">
        <Thumb item={item} />
        <div style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column", gap: 5 }}>
          <span data-testid="dl-caption" style={{ fontSize: 13, fontWeight: 600, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
            {item.rom_name}
          </span>
          <span className="rm-bar">
            <span style={barStyle} />
          </span>
          <span className="rm-muted" style={{ fontSize: 11 }}>
            {statusLabel(item)}
          </span>
        </div>
        {action && (
          <RmButton variant="sm icon" label={action.label} onActivate={action.run}>
            <Icon name={item.status === "downloading" ? "pause" : "play"} />
          </RmButton>
        )}
      </div>
    ) : (
      <div className="rm-dlrow" key={downloadKey(item)} data-testid="dl-row" style={{ gap: 14, padding: "14px 0" }}>
        <Thumb item={item} big />
        <div style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column", gap: 6 }}>
          <div style={{ display: "flex", justifyContent: "space-between", fontSize: 15 }}>
            <span data-testid="dl-caption" style={{ fontWeight: 600 }}>
              {item.rom_name}
            </span>
            <span className="rm-muted">{statusLabel(item)}</span>
          </div>
          {waiting && <WaitingNote item={item} />}
          <span className="rm-bar">
            <span style={barStyle} />
          </span>
          {waiting ? (
            <div className="rm-muted rm-small">{item.platform_name}</div>
          ) : (
            <div className="rm-muted rm-small" data-testid="dl-bytes">
              {detailLine(item)}
            </div>
          )}
        </div>
        <RmButtonGroup>
          {action && (
            <RmButton variant="sm" onActivate={action.run}>
              {action.label}
            </RmButton>
          )}
          {item.status !== "extracting" && (
            <RmButton variant="sm icon ghost" label="Cancel download" onActivate={() => detach(handleCancel(item))}>
              <Icon name="x" size={16} />
            </RmButton>
          )}
        </RmButtonGroup>
      </div>
    );
  });

  const allControl = (pausable.length > 0 || stopped.length > 0) && (
    <RmButton
      variant={compact ? "block ghost" : "sm ghost"}
      onActivate={() => detach(pausable.length > 0 ? pauseAll() : resumeAll())}
    >
      {paused || pausable.length === 0 ? "Resume all" : "Pause all"}
    </RmButton>
  );

  return (
    <div className={`rm-root${compact ? " rm-qam" : ""}`} style={{ background: "transparent" }}>
      {compact && <RmStyles />}
      {compact && <QamHeader title="Downloads" onBack={onBack} />}
      {!compact && (
        <div style={{ display: "flex", alignItems: "center", gap: 12, marginBottom: 8 }}>
          <h2 style={{ margin: 0, fontSize: 18, color: "#fff" }}>Queue</h2>
          <span className="rm-muted">{queueCount ? `${queueCount} item${queueCount > 1 ? "s" : ""}` : ""}</span>
          <span style={{ marginLeft: "auto" }}>{allControl}</span>
        </div>
      )}
      {queueCount === 0 ? (
        <div className="rm-muted" style={{ padding: compact ? "18px 0" : "24px 0" }}>
          {compact ? "No downloads running." : "Nothing downloading. Pick a game in the Library and press Download."}
        </div>
      ) : (
        queueBlock
      )}
      {compact && queueCount > 0 && <div style={{ marginTop: 12 }}>{allControl}</div>}

      {finished.length > 0 && !compact && (
        <>
          <h2 style={{ margin: "28px 0 4px", fontSize: 18, color: "#fff" }}>Finished</h2>
          {finished.map((item) => {
            const appId = item.status === "completed" && (item.kind ?? "rom") === "rom" ? appIds.get(item.rom_id) : undefined;
            return (
              <div className="rm-dlrow" key={downloadKey(item)} style={{ gap: 14 }} data-testid="dl-finished">
                <Thumb item={item} />
                <span style={{ flex: 1, fontSize: 15 }}>
                  {item.rom_name} <span className="rm-muted">· {formatFinishedDescription(item)}</span>
                </span>
                {appId !== undefined && (
                  <RmButton variant="sm play" onActivate={() => Navigation.Navigate(`/library/app/${appId}`)}>
                    Open
                  </RmButton>
                )}
              </div>
            );
          })}
          {hasFinished && (
            <div style={{ marginTop: 12 }}>
              <RmButton variant="sm ghost" onActivate={handleClearCompleted}>
                Clear completed
              </RmButton>
            </div>
          )}
        </>
      )}
      {compact && (
        <div style={{ marginTop: 8 }}>
          <RmButton variant="block" onActivate={() => openLibraryRoute("downloads")}>
            Full queue in RomM Library
          </RmButton>
        </div>
      )}
    </div>
  );
};
