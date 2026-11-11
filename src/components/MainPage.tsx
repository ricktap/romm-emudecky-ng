import { useState, useEffect, useRef, FC, ReactNode } from "react";
import { Focusable } from "@decky/ui";
import { AttRow, Icon, NavRow, RmButton, RmStyles, RmButtonGroup, ActionRow } from "./rm";
import { emudeckTitle, movedSummary, useEmudeckHealth } from "../utils/emudeckHealth";
import { openLibraryRoute } from "../utils/libraryRoute";
import { getSkipPreview, onSyncPreferencesChange } from "../utils/syncPreferences";
import { missingRequiredKeys } from "../utils/missingKeys";
import { serverHost } from "../utils/serverUrl";
import {
  testConnection,
  cancelSync,
  getSyncStats,
  getSettings,
  startSync,
  syncPreview,
  syncApplyDelta,
  syncCancelPreview,
  getSyncStatus,
  getFirmwareStatus,
  logError,
} from "../api/backend";
import { getSyncProgress, setSyncProgress as setStoredSyncProgress, onSyncProgressChange } from "../utils/syncProgress";
import { getDownloadState } from "../utils/downloadStore";
import { isUnfinishedStatus } from "../utils/downloadStatus";
import { getSettingsResetState, onSettingsResetChange } from "../utils/settingsResetStore";
import { getPlaytimeScopeState, onPlaytimeScopeChange, fetchPlaytimeScopeState } from "../utils/playtimeScopeStore";
import { reconcileStaleShortcuts, requestSyncCancel, isCancelRequested, resetSyncCancel } from "../utils/syncManager";
import { setVersionError } from "../utils/connectionState";
import { VersionErrorCard, useVersionError } from "./VersionErrorCard";
import { SettingsResetBanner } from "./SettingsResetBanner";
import { PlaytimeScopeBanner } from "./PlaytimeScopeBanner";
import { UpdateBanner } from "./UpdateBanner";
import { getUpdateState, onUpdateStateChange, refreshUpdateState } from "../utils/updateStore";
import type {
  SyncProgress,
  SyncStage,
  SyncStats,
  SyncPreview,
  SyncPreviewSummary,
  DownloadItem,
} from "../types";
import { detach } from "../utils/detach";
import { withTimeout } from "../utils/withTimeout";

type Page = "settings" | "platforms" | "downloads";

interface MainPageProps {
  onNavigate: (page: Page) => void;
}

// The connection probe races each `test_connection()` attempt against a deadline
// (the callable never times out on its own and hangs forever when the backend
// isn't up yet) and retries across the slow-cold-boot window. A call that
// resolves — success OR "not connected" — is authoritative and ends the probe;
// only an exhausted retry budget means the plugin backend never came up
// (bootstrap aborted), which the connection row surfaces explicitly instead of
// an eternal "Checking…" spinner (#1045). The schedule mirrors the metadata init
// loop's tuned window in index.tsx (#1203).
const CONNECTION_RETRY_DELAYS = [2000, 5000, 10000, 15000, 20000];
const CONNECTION_CALLABLE_TIMEOUT = 5000;

/** Backend never answered after the retry budget — distinct from `false` ("not connected"). */
type BackendFailed = "backend_failed";

function formatChanges(pairs: [number, string][]): string {
  return pairs
    .filter(([n]) => n > 0)
    .map(([n, label]) => `${n} ${label}`)
    .join(", ");
}

const ConnectionIndicator: FC<{ connected: boolean | null | BackendFailed }> = ({ connected }) => {
  if (connected === "backend_failed") {
    return (
      <>
        <span className="rm-dot rm-amber" />
        <span>Backend error</span>
      </>
    );
  }
  if (connected === null) {
    return (
      <>
        <span className="rm-dot rm-grey" />
        <span>Checking...</span>
      </>
    );
  }
  if (connected) {
    return (
      <>
        <span className="rm-dot" />
        <span>Connected</span>
      </>
    );
  }
  return (
    <>
      <span className="rm-dot rm-red" />
      <span>Not connected</span>
    </>
  );
};

const TERMINAL_STAGES: ReadonlySet<SyncStage> = new Set<SyncStage>(["done", "cancelled", "error"]);

function isTerminalStage(stage: SyncProgress["stage"]): boolean {
  return !!stage && TERMINAL_STAGES.has(stage);
}

const STAGE_LABELS: Record<SyncStage, string> = {
  discovering: "Discovering platforms",
  fetching: "Fetching library",
  applying: "Applying shortcuts",
  finalizing: "Finalizing",
  done: "Done",
  cancelled: "Cancelled",
  error: "Error",
};

function stageLabel(stage: SyncProgress["stage"]): string {
  return stage ? STAGE_LABELS[stage] : "Syncing";
}

function formatProgressText(progress: SyncProgress | null): string {
  if (!progress) return "Syncing...";
  const step = progress.step && progress.totalSteps ? `[${progress.step}/${progress.totalSteps}] ` : "";
  const msg = progress.message || "Syncing...";
  // Truncate to ~40 chars to prevent multi-line jumping in the QAM panel
  const maxLen = 40 - step.length;
  const truncated = msg.length > maxLen ? msg.slice(0, maxLen - 1) + "\u2026" : msg;
  return step + truncated;
}

function formatLastSync(iso: string | null): string {
  if (!iso) return "Never";
  try {
    const d = new Date(iso);
    const now = new Date();
    const diffMs = now.getTime() - d.getTime();
    const diffMins = Math.floor(diffMs / 60000);
    if (diffMins < 1) return "Just now";
    if (diffMins < 60) return `${diffMins}m ago`;
    const diffHours = Math.floor(diffMins / 60);
    if (diffHours < 24) return `${diffHours}h ago`;
    const diffDays = Math.floor(diffHours / 24);
    return `${diffDays}d ago`;
  } catch {
    return iso;
  }
}

function formatPreviewDescription(s: SyncPreviewSummary): string {
  const sections: string[] = [];
  const romChanges = formatChanges([
    [s.new_count, "added"],
    [s.changed_count, "updated"],
    [s.remove_count, "removed"],
  ]);
  if (romChanges) sections.push(`Games: ${romChanges}`);
  const p = s.platform_collection_diff;
  if (p?.has_changes) {
    const platChanges = formatChanges([
      [p.added_count, "added"],
      [p.removed_count, "removed"],
    ]);
    if (platChanges) sections.push(`Platforms: ${platChanges}`);
  }
  const d = s.collection_diff;
  if (d?.has_changes) {
    const collChanges = formatChanges([
      [d.added.length, "added"],
      [d.removed.length, "removed"],
    ]);
    if (collChanges) sections.push(`Collections: ${collChanges}`);
  }
  return sections.length > 0 ? sections.join("; ") : "Everything is up to date.";
}

export const MainPage: FC<MainPageProps> = ({ onNavigate }) => {
  const emudeck = useEmudeckHealth();
  const [stats, setStats] = useState<SyncStats | null>(null);
  const [connected, setConnected] = useState<boolean | null | BackendFailed>(null);
  const versionError = useVersionError();
  const [syncing, setSyncing] = useState(false);
  // Disarmed "Cancelling…" state during the backend's RUNNING→CANCELLING→IDLE
  // drain. The Sync/Cancel button stays disabled until the terminal
  // sync_progress stage re-arms it, so a quick re-press can't hit the
  // sync_in_progress reject and look like an instant finish (#1202, RC-B).
  const [cancelling, setCancelling] = useState(false);
  const [syncProgress, setSyncProgress] = useState<SyncProgress | null>(null);
  const [status, setStatus] = useState("");
  const [preview, setPreview] = useState<SyncPreview | null>(null);
  const [skipPreview, setSkipPreviewState] = useState(getSkipPreview());
  const [missingKeys, setMissingKeys] = useState<string[]>([]);
  const [hostName, setHostName] = useState("");
  const [loading, setLoading] = useState(false);
  const [settingsReset, setSettingsReset] = useState(getSettingsResetState());
  const [playtimeScope, setPlaytimeScope] = useState(getPlaytimeScopeState());
  const [updateState, setUpdateState] = useState(getUpdateState());
  const [downloads, setDownloads] = useState<DownloadItem[]>([]);
  const statusTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const downloadPollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const showTransientStatus = (msg: string) => {
    if (statusTimeoutRef.current) clearTimeout(statusTimeoutRef.current);
    setStatus(msg);
    statusTimeoutRef.current = setTimeout(() => setStatus(""), 8000);
  };

  useEffect(() => {
    // Guards for the async connection probe below: `cancelled` stops the retry
    // loop from touching state after unmount; the timer ref lets cleanup clear a
    // pending backoff delay.
    let cancelled = false;
    let connectionRetryTimer: ReturnType<typeof setTimeout> | null = null;

    getSyncStats()
      .then(setStats)
      .catch((e) => logError(`Failed to load sync stats: ${e}`));

    // Probe the backend for the connection row. Each attempt has a deadline
    // because the callable hangs (rather than rejects) while the backend is
    // still starting, and the retries ride out a slow cold boot. A resolved call
    // ends the probe — "not connected" (success:false) is an authoritative
    // answer, not a failure. Only an exhausted retry budget means the backend
    // never came up (bootstrap aborted); surface that explicitly (#1045).
    const probeConnection = async (isCancelled: () => boolean) => {
      for (let attempt = 0; !isCancelled(); attempt++) {
        try {
          const r = await withTimeout(testConnection(), CONNECTION_CALLABLE_TIMEOUT);
          if (isCancelled()) return;
          setConnected(r.success);
          setVersionError(r.reason === "version_error" ? r.message : null);
          return;
        } catch {
          if (isCancelled()) return;
          if (attempt >= CONNECTION_RETRY_DELAYS.length) {
            // Retry budget exhausted. test_connection() also waits out the
            // server round-trip — a hanging RomM server keeps the backend's
            // retrying heartbeat busy for up to ~90s, far past our per-attempt
            // deadline — so an exhausted budget alone can't tell a dead backend
            // from an unreachable server. Ping get_settings (a pure in-memory
            // read that resolves iff the backend RPC bridge is alive) to decide:
            // alive ⇒ the server is merely unreachable ("Not connected");
            // dead ⇒ the backend never came up ("Backend error").
            try {
              await withTimeout(getSettings(), CONNECTION_CALLABLE_TIMEOUT);
              if (isCancelled()) return;
              setConnected(false);
            } catch (pingErr) {
              if (isCancelled()) return;
              setConnected("backend_failed");
              // logError is itself a callable and would hang against a dead
              // backend — log to the console instead.
              console.error("[RomM] backend RPC bridge unreachable (get_settings ping failed):", pingErr);
            }
            return;
          }
          await new Promise<void>((resolve) => {
            connectionRetryTimer = setTimeout(resolve, CONNECTION_RETRY_DELAYS[attempt]);
          });
        }
      }
    };
    detach(probeConnection(() => cancelled));

    // Cross-device playtime scope notice. The backend sets a durable flag when a
    // playtime reconcile is rejected for a token missing `roms.user.read`; it
    // self-clears once a scoped token is minted, so we re-read it on every mount.
    fetchPlaytimeScopeState().catch((e) => logError(`Failed to check playtime scope notice: ${e}`));

    // Self-update notice. Non-forced: the backend reuses its last check for up
    // to 6 h, so opening the QAM does not hit GitHub every time. Failures fold
    // into the store (never reject).
    refreshUpdateState(false).catch(() => undefined);

    // Host for the status line, and the required keys no emulator has yet (the Needs attention list).
    getSettings()
      .then((s) => !cancelled && setHostName(serverHost(s.romm_url)))
      .catch(() => undefined);
    getFirmwareStatus()
      .then((f) => !cancelled && setMissingKeys(missingRequiredKeys(f)))
      .catch(() => undefined);

    // Backend is authoritative for in-flight sync state. Seed the module
    // store from get_sync_status() so a QAM close/reopen recovers the live
    // run rather than guessing from the event-fed store alone.
    getSyncStatus()
      .then((progress) => {
        setStoredSyncProgress(progress);
        if (progress.running) {
          setSyncing(true);
          setLoading(true);
          setSyncProgress(progress);
        }
      })
      .catch((e) => logError(`Failed to query sync status: ${e}`));

    // Subscribe to the module store — every backend sync_progress event and
    // every frontend updateSyncProgress notifies, driving a re-render. The
    // in-progress UI is torn down ONLY on a terminal stage, never on a bare
    // running:false (which can transiently race a fresh run's first event).
    const unsubProgress = onSyncProgressChange(() => {
      const progress = getSyncProgress();
      setSyncProgress(progress);
      if (isTerminalStage(progress.stage)) {
        setSyncing(false);
        setLoading(false);
        // True terminal reached — re-arm the button out of any "Cancelling…"
        // drain state (#1202, RC-B).
        setCancelling(false);
        showTransientStatus(progress.message || "Sync finished");
        getSyncStats()
          .then(setStats)
          .catch((e) => logError(`Failed to refresh sync stats: ${e}`));
      }
    });

    // Poll download state for inline display
    downloadPollRef.current = setInterval(() => {
      setDownloads([...getDownloadState()]);
    }, 1000);

    const unsubSettingsReset = onSettingsResetChange(() => setSettingsReset(getSettingsResetState()));
    const unsubPlaytimeScope = onPlaytimeScopeChange(() => setPlaytimeScope(getPlaytimeScopeState()));
    const unsubUpdate = onUpdateStateChange(() => setUpdateState(getUpdateState()));
    const unsubPrefs = onSyncPreferencesChange(() => setSkipPreviewState(getSkipPreview()));
    return () => {
      unsubPrefs();
      cancelled = true;
      if (connectionRetryTimer) clearTimeout(connectionRetryTimer);
      unsubProgress();
      unsubSettingsReset();
      unsubPlaytimeScope();
      unsubUpdate();
      if (statusTimeoutRef.current) clearTimeout(statusTimeoutRef.current);
      if (downloadPollRef.current) clearInterval(downloadPollRef.current);
    };
  }, []);

  // A start/apply call never reached a running backend sync (rejected up
  // front or threw). Reset both the local UI and the MODULE store so the
  // store mirrors reality — the optimistic running:true must not linger.
  const abortOptimisticSync = (msg: string) => {
    setStatus(msg);
    setSyncing(false);
    setLoading(false);
    setCancelling(false);
    setStoredSyncProgress({ running: false, stage: "" });
  };

  const handleSync = async () => {
    // Clear any stale cancel flag from a prior run BEFORE this sync starts, so a
    // fresh sync never begins pre-cancelled (#1198). Run identity for a Cancel
    // click comes from the backend-fed sync_progress store now (#1202).
    resetSyncCancel();
    // Optimistically disable the button and show the in-progress UI before
    // the backend's first sync_progress event lands — writing running:true
    // into the MODULE store (the single source of truth the subscription
    // reads), not a shadowing local state.
    setLoading(true);
    setSyncing(true);
    setCancelling(false);
    setStatus("");
    setPreview(null);
    setStoredSyncProgress({ running: true, stage: "fetching", message: "Fetching library..." });
    try {
      // Reconcile shortcuts the user deleted via Steam's own UI BEFORE the work
      // queue is built (both sync paths fetch through it): unbind any dead
      // binding so the incremental skip re-fetches the platform and recreates
      // the missing shortcut (#1046). Best-effort — never blocks the sync.
      await reconcileStaleShortcuts();
      // Skip Preview takes the per-unit pipeline (start_sync) — incremental
      // shortcut delivery, per-unit crash safety, no upfront full library
      // fetch. The legacy preview/apply path remains for users who want to
      // review changes before they apply.
      if (skipPreview) {
        const startResult = await startSync();
        if (!startResult.success) {
          abortOptimisticSync(startResult.message);
        }
        // On success the store subscription drives the UI from here.
        return;
      }
      const result = await syncPreview();
      if (!result.success) {
        abortOptimisticSync(result.message || "Preview failed");
        return;
      }
      // RC-CANCEL-PREVIEW (#1202): a Cancel can land in the sub-second window
      // while syncPreview() is in flight. The backend returns a cancelled
      // result for a preview cancelled mid-loop, but a preview that finished
      // just before the cancel still resolves success — re-check the flag
      // before showing the phantom "Apply Sync". On a cancel, clear the flag
      // (so the next sync doesn't start pre-cancelled) and abort to idle.
      if (isCancelRequested()) {
        resetSyncCancel();
        abortOptimisticSync("Sync cancelled");
        return;
      }
      setPreview(result);
      setSyncing(false);
      setLoading(false);
    } catch {
      abortOptimisticSync("Failed to start sync");
    }
  };

  const handleApply = async () => {
    if (!preview) return;
    const previewId = preview.preview_id;
    // Clear any stale cancel flag before the apply run starts (#1198). A Cancel
    // in the apply window reads "" from the sync_progress store until the
    // backend stamps the run id, which the backend treats as an unconditional
    // cancel (#1202).
    resetSyncCancel();
    setPreview(null);
    setLoading(true);
    setSyncing(true);
    setCancelling(false);
    setStoredSyncProgress({ running: true, stage: "applying", message: "Applying changes..." });
    try {
      const result = await syncApplyDelta(previewId);
      if (!result.success) {
        abortOptimisticSync(result.message);
      }
      // On success the store subscription drives the UI from here.
    } catch {
      abortOptimisticSync("Failed to apply sync");
    }
  };

  const handleDismiss = async () => {
    setPreview(null);
    setStatus("");
    try {
      await syncCancelPreview();
    } catch {
      // ignore
    }
  };

  const finishCancelWithStatus = (msg: string) => {
    setCancelling(false);
    setSyncing(false);
    setLoading(false);
    showTransientStatus(msg);
  };

  const handleCancel = async () => {
    if (preview) {
      await handleDismiss();
      setSyncing(false);
      setLoading(false);
      return;
    }
    // RC-B (#1202): do NOT re-arm the Sync button here. The backend drains
    // RUNNING → CANCELLING → IDLE asynchronously; flipping back to enabled now
    // lets a quick re-press hit the sync_in_progress reject and look like an
    // "instant finish". Disarm into the "Cancelling…" state and wait for the
    // terminal sync_progress stage (the store subscription) to re-arm.
    setCancelling(true);
    requestSyncCancel();
    try {
      // Scope the cancel to the active run via the backend-fed run id; "" in the
      // pre-progress window → the backend's unconditional cancel (#1202).
      await cancelSync(getSyncProgress().runId ?? "");
      // Success: stay disarmed; the terminal stage tears the UI down and
      // re-arms, surfacing the backend's final message — no status here, so no
      // instant-finish flash during the drain.
    } catch {
      // The cancel call itself failed — no terminal will arrive from a cancel
      // that never landed, so re-arm and surface the failure for a retry.
      finishCancelWithStatus("Failed to cancel sync");
    }
  };

  // Two-level progress. The main determinate bar tracks COARSE unit
  // progress (step / totalSteps); 0/0 means the run hasn't reached a unit
  // yet, so the bar goes indeterminate. Steam's ProgressBarWithInfo
  // nProgress uses percentage (0-100), not fraction (0-1).
  const coarseFraction = syncProgress?.totalSteps
    ? ((syncProgress.step ?? 0) / syncProgress.totalSteps) * 100
    : undefined;
  const hasFineDetail = !!(syncProgress?.total && syncProgress.message);

  const activeDownloads = downloads.filter((d) => isUnfinishedStatus(d.status));
  // Sync is unavailable when the server test failed OR the plugin backend never
  // started — both gate the Sync buttons off.
  const connectionUnavailable = connected === false || connected === "backend_failed";

  if (versionError) {
    return <VersionErrorCard message={versionError} compact />;
  }

  let syncBody: ReactNode;
  if (preview) {
    const hasChanges =
      preview.summary.new_count + preview.summary.changed_count + preview.summary.remove_count > 0 ||
      !!(preview.summary.collection_diff?.added.length || preview.summary.collection_diff?.removed.length) ||
      preview.summary.platform_collection_diff?.has_changes;
    syncBody = (
      <>
        <h3>{hasChanges ? "Changes on RomM" : "Library is up to date"}</h3>
        <div className="rm-muted" style={{ marginBottom: 8 }} data-testid="preview-summary">
          {formatPreviewDescription(preview.summary)}
        </div>
        {hasChanges ? (
          <RmButtonGroup style={{ marginTop: 12 }}>
            <RmButton variant="primary" className="rm-grow" onActivate={() => detach(handleApply())}>
              Apply
            </RmButton>
            <RmButton onActivate={() => detach(handleDismiss())}>Cancel</RmButton>
          </RmButtonGroup>
        ) : (
          <RmButton variant="block" onActivate={() => detach(handleDismiss())}>
            Dismiss
          </RmButton>
        )}
      </>
    );
  } else if (syncing) {
    const stepText = syncProgress?.totalSteps ? `${syncProgress.step ?? 0}/${syncProgress.totalSteps}` : "";
    syncBody = (
      <>
        <h3>Syncing…</h3>
        <div className="rm-muted" style={{ marginBottom: 10, display: "flex", justifyContent: "space-between" }}>
          <span data-testid="sync-stage">{stageLabel(syncProgress?.stage)}</span>
          {stepText && <span data-testid="sync-step">{stepText}</span>}
        </div>
        <span className="rm-bar">
          <span
            style={{ width: coarseFraction === undefined ? "30%" : `${Math.min(100, coarseFraction)}%` }}
            data-testid="sync-bar"
          />
        </span>
        {hasFineDetail && (
          <div className="rm-muted rm-small" style={{ marginTop: 8 }}>
            {formatProgressText(syncProgress)}
          </div>
        )}
        <div style={{ marginTop: 10 }}>
          <RmButton variant="sm ghost" disabled={cancelling} onActivate={() => detach(handleCancel())}>
            {cancelling ? "Cancelling…" : "Cancel"}
          </RmButton>
        </div>
      </>
    );
  } else {
    const syncedBefore = Boolean(status) && !syncing;
    syncBody = (
      <>
        {status ? (
          <h3 data-testid="sync-status">{status}</h3>
        ) : (
          stats && (
            <h3>
              {stats.roms > 0
                ? `${stats.roms} games${stats.platforms > 0 ? ` · ${stats.platforms} platforms` : ""}`
                : "No games synced yet"}
            </h3>
          )
        )}
        {stats && (
          <div className="rm-muted" style={{ marginBottom: 12 }}>
            Last sync {formatLastSync(stats.last_sync)}
          </div>
        )}
        <RmButton
          variant={syncedBefore ? "block" : "primary block"}
          disabled={loading || connectionUnavailable}
          onActivate={() => detach(handleSync())}
        >
          {syncedBefore ? "Sync again" : "Sync library"}
        </RmButton>
        {connectionUnavailable && (
          <div className="rm-muted rm-small" style={{ marginTop: 8 }}>
            Sync needs a connection to the RomM server
          </div>
        )}
      </>
    );
  }

  const attention: ReactNode[] = [];
  // EmuDeck's storage comes first: without it nothing can be downloaded or installed.
  if (emudeck && !emudeck.ok) {
    attention.push(
      <AttRow
        key="emudeck"
        title={emudeckTitle(emudeck.health)}
        sub={emudeck.message}
        onActivate={() => openLibraryRoute("systems")}
      />,
    );
  } else if (movedSummary(emudeck)) {
    attention.push(
      <AttRow
        key="emudeck-moved"
        title="EmuDeck storage moved"
        sub={movedSummary(emudeck) ?? ""}
        onActivate={() => openLibraryRoute("systems")}
      />,
    );
  }
  if (settingsReset.pending) attention.push(<SettingsResetBanner key="reset" backedUpTo={settingsReset.backedUpTo} />);
  if (playtimeScope.pending) attention.push(<PlaytimeScopeBanner key="playtime" />);
  if (updateState.info?.update_available) attention.push(<UpdateBanner key="update" info={updateState.info} />);
  if (missingKeys.length > 0) {
    attention.push(
      <AttRow
        key="keys"
        title="Switch keys missing"
        sub={missingKeys.join(", ")}
        onActivate={() => openLibraryRoute("systems")}
      />,
    );
  }

  return (
    <div className="rm-root rm-qam" style={{ background: "transparent" }}>
      <RmStyles />
      <Focusable noFocusRing className="rm-status" style={{ margin: "10px 0 4px" }} data-testid="status-line">
        <ConnectionIndicator connected={connected} />
        {connected === true && hostName && (
          <span data-testid="status-host">· {hostName}</span>
        )}
      </Focusable>
      {connected === "backend_failed" && (
        <div className="rm-muted rm-small">Plugin backend failed to start — check Decky logs.</div>
      )}

      <div className="rm-qsec">Library</div>
      <div className="rm-card">{syncBody}</div>

      {attention.length > 0 && (
        <>
          <div className="rm-qsec">Needs attention</div>
          {attention}
        </>
      )}

      <div className="rm-qsec">Manage</div>
      <NavRow label="Downloads" badge={activeDownloads.length} onActivate={() => onNavigate("downloads")} />
      <NavRow label="Platforms & collections" onActivate={() => onNavigate("platforms")} />
      <NavRow label="Settings" onActivate={() => onNavigate("settings")} />
      <ActionRow
        label="Check for updates"
        testId="check-updates"
        value={updateState.checking ? "Checking…" : updateState.info ? (updateState.info.update_available ? `v${updateState.info.latest_version} available` : "Up to date") : updateState.error ? "Check failed" : ""}
        disabled={updateState.checking}
        onActivate={() => {
          // refreshUpdateState folds every failure into the store: it never rejects.
          refreshUpdateState(true).catch(() => undefined);
        }}
      />
      <div style={{ marginTop: 18 }}>
        <RmButton variant="block" className="rm-openlib" onActivate={() => openLibraryRoute()}>
          <Icon name="grid" size={18} />
          Open RomM Library
        </RmButton>
      </div>
    </div>
  );
};
