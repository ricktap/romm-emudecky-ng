/**
 * RomMGameInfoPanel — metadata and actions panel injected below the PlaySection
 * on RomM game detail pages.
 *
 * Layout:
 *   Game Info:    Platform, Description, Developer/Publisher, Genre tags, Release date
 *   ROM File:     Filename (only when installed)
 *   Save Sync:    Status (only when save sync enabled)
 *   Purely informational — all actions live in RomMPlaySection gear menu.
 *
 * Uses createElement throughout (no JSX) to match the RomMPlaySection pattern.
 * CSS classes prefixed with `romm-panel-` are injected separately by styleInjector.
 */

import { useState, useEffect, useRef, FC, createElement } from "react";
import { addEventListener, removeEventListener } from "@decky/api";
import { DialogButton, Focusable } from "@decky/ui";
// DialogButton is natively focusable by Steam's gamepad engine (unlike Focusable
// wrappers around non-interactive content, which don't register in this injection
// context). Style as content sections, not buttons.
import {
  getCachedGameDetail,
  invalidateCachedGameDetail,
  getRomMetadata,
  getInstalledRom,
  getSaveStatus,
  getArtworkBase64,
  getSaveSlots,
  isSaveTrackingConfigured,
  debugLog,
} from "../api/backend";
import { SlotSetupWizard } from "./SlotSetupWizard";
import { SavesTab } from "./SavesTab";
import type {
  RomMetadata,
  InstalledRom,
  SaveStatus,
  SyncConflict,
  SaveSlotSummary,
  DownloadCompleteEvent,
} from "../types";
import type { RommDataChangedDetail } from "../types/events";
import { getSettingsResetState, onSettingsResetChange } from "../utils/settingsResetStore";
import { scrollFocusedToCenter } from "../utils/scrollHelpers";
import { reportServerReachable, setServerRetryProgress, useRommConnectionState } from "../utils/connectionState";
import { applyLoadSlotsResult, applyRefreshSlotResult } from "../utils/slotState";
import { VersionErrorCard, useVersionError } from "./VersionErrorCard";
import { SettingsResetCard } from "./SettingsResetCard";
import { detach } from "../utils/detach";

interface RomMGameInfoPanelProps {
  appId: number;
}

interface PanelState {
  loading: boolean;
  romId: number | null;
  romName: string;
  platformName: string;
  platformSlug: string;
  installed: boolean;
  installedRom: InstalledRom | null;
  metadata: RomMetadata | null;
  coverBase64: string | null;
  saveSyncEnabled: boolean;
  saveStatus: SaveStatus | null;
  conflicts: SyncConflict[];
  error: boolean;
  activeTab: string;
  slotConfirmed: boolean;
  activeSlot: string | null;
  availableSlots: SaveSlotSummary[];
  slotsLoading: boolean;
  // Region / Languages of the ACTIVE version (ADR-0021), rendered as GAME INFO
  // rows; empty arrays hide their row. Refreshed on a version switch.
  regions: string[];
  languages: string[];
}

/** Format a Unix timestamp (seconds) as a release date string (e.g. "15 Mar 2003") */
function formatReleaseDate(timestamp: number | null): string | null {
  if (!timestamp || timestamp <= 0) return null;
  const date = new Date(timestamp * 1000);
  const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  return `${date.getDate()} ${months[date.getMonth()]} ${date.getFullYear()}`;
}

/** Refresh slot configuration and available slots — extracted to reduce nesting depth. */
function refreshSlotState(romId: number, setter: React.Dispatch<React.SetStateAction<PanelState>>): void {
  isSaveTrackingConfigured(romId)
    .then((result) => setter((prev) => ({ ...prev, slotConfirmed: result.configured })))
    .catch(() => {});
  getSaveSlots(romId)
    .then((slotResult) => applyRefreshSlotResult<PanelState>(slotResult, setter))
    .catch(() => {});
}

/** Fire-and-forget installed-rom fetch — kept at module scope to avoid nesting. */
function refreshInstalledRomInBackground(
  romId: number,
  cancelled: () => boolean,
  setter: React.Dispatch<React.SetStateAction<PanelState>>,
): Promise<void> {
  return getInstalledRom(romId)
    .then((installed) => {
      if (!cancelled() && installed) {
        setter((prev) => ({ ...prev, installedRom: installed }));
      }
    })
    .catch(() => {});
}

/** Fire-and-forget cover-art fetch — kept at module scope to avoid nesting. */
function refreshCoverArtInBackground(
  romId: number,
  cancelled: () => boolean,
  setter: React.Dispatch<React.SetStateAction<PanelState>>,
): Promise<void> {
  return getArtworkBase64(romId)
    .then((result) => {
      if (!cancelled() && result.base64) {
        setter((prev) => ({ ...prev, coverBase64: result.base64 }));
      }
    })
    .catch(() => {});
}

/** Fire-and-forget metadata fetch — kept at module scope to avoid nesting. */
function refreshMetadataInBackground(
  romId: number,
  cancelled: () => boolean,
  setter: React.Dispatch<React.SetStateAction<PanelState>>,
): Promise<void> {
  return getRomMetadata(romId)
    .then((meta) => {
      if (!cancelled()) {
        setter((prev) => ({ ...prev, metadata: meta }));
      }
    })
    .catch(() => {});
}

/** Build a `SaveStatus` from a cached game detail's `save_status` field. */
function saveStatusFromCache(
  romId: number,
  cachedSave: NonNullable<Awaited<ReturnType<typeof getCachedGameDetail>>["save_status"]> | null | undefined,
): SaveStatus | null {
  if (!cachedSave) return null;
  return {
    rom_id: romId,
    files: cachedSave.files.map((f) => ({
      filename: f.filename,
      status: f.status as "skip" | "download" | "upload" | "conflict",
      local_path: null,
      local_hash: null,
      local_mtime: null,
      local_size: null,
      server_save_id: null,
      server_file_name: null,
      server_emulator: null,
      server_updated_at: null,
      server_size: null,
      last_sync_at: f.last_sync_at ?? null,
    })),
    playtime: {
      total_seconds: 0,
      session_count: 0,
      last_session_start: null,
      last_session_duration_sec: null,
      last_played: null,
    },
    device_id: "",
    last_sync_check_at: cachedSave.last_sync_check_at ?? null,
  };
}

/** Kick off background fetches that fill in fields not present in the cache:
 *  installed-rom details, cover art, and fresh metadata (if stale or missing). */
function startBackgroundRefreshes(
  cached: Awaited<ReturnType<typeof getCachedGameDetail>>,
  romId: number,
  cancelled: () => boolean,
  setter: React.Dispatch<React.SetStateAction<PanelState>>,
): Promise<void[]> {
  const bgPromises: Promise<void>[] = [];

  if (cached.installed) {
    bgPromises.push(refreshInstalledRomInBackground(romId, cancelled, setter));
  }

  bgPromises.push(refreshCoverArtInBackground(romId, cancelled, setter));

  const metaStale = cached.stale_fields?.includes("metadata") ?? true;
  if (!cached.metadata || metaStale) {
    bgPromises.push(refreshMetadataInBackground(romId, cancelled, setter));
  }

  return Promise.all(bgPromises);
}

/** Cache-first initial render. Resolves the cached game detail for this appId,
 *  pushes it into PanelState, and fires the background refresh tasks whose
 *  results are merged in later. Module-scope so the FC body stays focused on
 *  rendering. */
async function loadData(
  appId: number,
  cancelled: () => boolean,
  romIdRef: React.MutableRefObject<number | null>,
  setter: React.Dispatch<React.SetStateAction<PanelState>>,
): Promise<void> {
  try {
    // Phase 1: Cache-first — render instantly from cached data
    const cached = await getCachedGameDetail(appId);
    if (cancelled()) return;
    if (!cached.found) {
      setter((prev) => ({ ...prev, loading: false, error: true }));
      return;
    }

    const romId = cached.rom_id!;
    const romName = cached.rom_name || "";
    const platformName = cached.platform_name || "";
    const platformSlug = cached.platform_slug || "";

    romIdRef.current = romId;

    const saveStatus = saveStatusFromCache(romId, cached.save_status);
    const conflicts: SyncConflict[] = cached.save_status?.conflicts ?? [];

    // Render immediately with cached data (metadata may be null — that's OK)
    setter({
      loading: false,
      romId,
      romName,
      platformName,
      platformSlug,
      installed: cached.installed ?? false,
      installedRom: null, // Will be filled by background fetch if installed
      metadata: cached.metadata as RomMetadata | null,
      coverBase64: null, // Will be filled by background fetch
      saveSyncEnabled: cached.save_sync_enabled ?? false,
      saveStatus,
      conflicts,
      error: false,
      activeTab: "info",
      slotConfirmed: false,
      activeSlot: "default",
      availableSlots: [],
      slotsLoading: false,
      regions: cached.regions ?? [],
      languages: cached.languages ?? [],
    });

    if (cached.save_sync_enabled) {
      refreshSlotState(romId, setter);
    }

    // Phase 2: Background fetch for data not available in cache
    await startBackgroundRefreshes(cached, romId, cancelled, setter);
  } catch (e) {
    detach(debugLog(`RomMGameInfoPanel: loadData error: ${e}`));
    if (!cancelled()) setter((prev) => ({ ...prev, loading: false, error: true }));
  }
}

// S3776 is raised on the declaration line, so its NOSONAR must stay there. prettier-ignore stops
// Prettier from relocating the trailing comment into the body (which would break the suppression).
// prettier-ignore
export const RomMGameInfoPanel: FC<RomMGameInfoPanelProps> = ({ appId }) => { // NOSONAR(typescript:S3776) — React FC fan-out; decomposed in #387/#391/Phase 7. Further split scatters handlers.
  // Subscribe to version error — re-renders when global state changes
  const versionError = useVersionError();
  // Subscribe to the shared connection state so the saves-tab slot load can take
  // the known-offline fast path and re-load automatically on reconnect (#1345).
  const isOffline = useRommConnectionState() === "offline";

  const [state, setState] = useState<PanelState>({
    loading: true,
    romId: null,
    romName: "",
    platformName: "",
    platformSlug: "",
    installed: false,
    installedRom: null,
    metadata: null,
    coverBase64: null,
    saveSyncEnabled: false,
    saveStatus: null,
    conflicts: [],
    error: false,
    activeTab: "info",
    slotConfirmed: false,
    activeSlot: "default",
    availableSlots: [],
    slotsLoading: false,
    regions: [],
    languages: [],
  });
  const romIdRef = useRef<number | null>(null);
  // Load-once gate for the lazy-loaded SAVES tab data. A version switch resets
  // it (in handleVersionSwitched) so the tab data re-fetches for the
  // newly-bound rom_id instead of lingering from the previous version.
  const slotsLoadedRef = useRef(false);
  // Monotonic load counter (#1345 F2): a load only clears the shared
  // serverRetryProgress store on settle if it is still the latest load — a
  // stale torn-down slot fetch resolving late must not wipe a newer load's
  // live "(attempt N/M)" frame.
  const loadGenRef = useRef(0);
  const [settingsReset, setSettingsReset] = useState(getSettingsResetState());

  useEffect(() => {
    const unsubSettingsReset = onSettingsResetChange(() => setSettingsReset(getSettingsResetState()));
    return () => {
      unsubSettingsReset();
    };
  }, []);

  useEffect(() => {
    let cancelled = false;

    detach(loadData(appId, () => cancelled, romIdRef, setState));

    // Listen for uninstall events to update state (uses ref to avoid stale closure)
    const onUninstall = (e: Event) => {
      const detail = (e as CustomEvent).detail;
      if (detail?.rom_id === romIdRef.current) {
        setState((prev) => ({ ...prev, installed: false, installedRom: null }));
      }
    };
    globalThis.addEventListener("romm_rom_uninstalled", onUninstall);

    // Listen for download completion — the install counterpart to onUninstall.
    // The "ROM File" section is gated on installed + installedRom; a fresh
    // download must flip both so the section (with the local path) appears
    // without a detail-page re-mount (#1340). Decky backend event (@decky/api),
    // not a DOM CustomEvent — mirrors DiscSelector's download_complete wiring.
    const onDownloadComplete = addEventListener<[DownloadCompleteEvent]>(
      "download_complete",
      (evt: DownloadCompleteEvent) => {
        if (evt.rom_id !== romIdRef.current) return;
        // Invalidate the cached detail so a fast re-mount inside the 3s TTL
        // doesn't briefly re-serve the stale installed:false.
        invalidateCachedGameDetail(appId);
        setState((prev) => ({ ...prev, installed: true }));
        detach(refreshInstalledRomInBackground(evt.rom_id, () => cancelled, setState));
      },
    );

    // Per-event-type handlers — each owns one branch of the data-changed dispatch.
    // Defined inside useEffect to share the cancelled/appId/romIdRef/setState closure.
    const handleSaveSyncSettingsChange = async (
      detail: Extract<RommDataChangedDetail, { type: "save_sync_settings" }>,
    ) => {
      const enabled = detail.save_sync_enabled;
      if (!enabled) {
        setState((prev) => ({ ...prev, saveSyncEnabled: false }));
        return;
      }
      const romId = romIdRef.current;
      if (!romId) return;
      const updatedStatus = await getSaveStatus(romId).catch((): SaveStatus | null => null);
      const conflicts: SyncConflict[] = updatedStatus?.conflicts ?? [];
      setState((prev) => ({
        ...prev,
        saveSyncEnabled: true,
        saveStatus: updatedStatus,
        conflicts,
      }));
    };

    const handleSaveSyncChange = async (detail: Extract<RommDataChangedDetail, { type: "save_sync" }>) => {
      if (detail.rom_id && detail.rom_id !== romIdRef.current) return;
      const romId = romIdRef.current;
      if (!romId) return;
      const updatedStatus: SaveStatus | null =
        detail.save_status ?? (await getSaveStatus(romId).catch((): SaveStatus | null => null));
      const conflicts: SyncConflict[] = updatedStatus?.conflicts ?? [];
      setState((prev) => ({
        ...prev,
        saveStatus: updatedStatus,
        conflicts,
      }));
      // Also re-check slot configuration + refresh slot data
      refreshSlotState(romId, setState);
    };

    const handleVersionSwitched = async (detail: Extract<RommDataChangedDetail, { type: "version_switched" }>) => {
      // A version switch moved the group's binding to a new rom_id. Re-read the
      // cached detail (invalidated by the picker) so the panel reflects the new
      // active version — its RomM name (the injected panel title), Region /
      // Languages rows, and cover — while the Steam hero title stays sticky.
      //
      // The per-rom TAB data must follow the new active version too. The SAVES
      // tab loads once behind slotsLoadedRef and holds per-rom state; without
      // re-keying, it keeps showing the previous version's data. Reset the
      // load-once gate and re-key the tab state off
      // the fresh cache (mirroring a fresh mount) so the tab-activation effects
      // re-fetch for the new rom_id — covering both the sitting-on-a-tab case
      // (romId changes → the effect re-runs) and the open-a-tab-later case.
      if (detail.app_id !== appId) return;
      const cached = await getCachedGameDetail(appId);
      if (cancelled || !cached.found) return;
      const newRomId = cached.rom_id ?? romIdRef.current;
      romIdRef.current = newRomId;
      slotsLoadedRef.current = false;
      const saveStatus = newRomId != null ? saveStatusFromCache(newRomId, cached.save_status) : null;
      setState((prev) => ({
        ...prev,
        romId: newRomId,
        romName: cached.rom_name || prev.romName,
        installed: cached.installed ?? false,
        regions: cached.regions ?? [],
        languages: cached.languages ?? [],
        // Re-key per-rom tab state so nothing lingers from the previous version.
        saveSyncEnabled: cached.save_sync_enabled ?? false,
        saveStatus,
        conflicts: cached.save_status?.conflicts ?? [],
        activeSlot: "default",
        availableSlots: [],
        slotsLoading: false,
      }));
      // Re-fetch slot configuration (slotConfirmed) + slots for the new rom_id,
      // mirroring loadData's save-sync branch — this is the authority that keeps
      // the SlotSetupWizard-vs-SavesTab gate correct across the switch.
      if (cached.save_sync_enabled && newRomId != null) {
        refreshSlotState(newRomId, setState);
      }
      if (newRomId) await refreshCoverArtInBackground(newRomId, () => cancelled, setState);
    };

    const handleMetadataChange = async (detail: Extract<RommDataChangedDetail, { type: "metadata" }>) => {
      if (detail.rom_id !== romIdRef.current) return;
      const romId = romIdRef.current;
      if (!romId) return;
      const meta = await getRomMetadata(romId).catch((): RomMetadata | null => null);
      setState((prev) => ({ ...prev, metadata: meta }));
    };

    const handleCoverRefreshed = async (detail: Extract<RommDataChangedDetail, { type: "cover_refreshed" }>) => {
      if (detail.rom_id !== romIdRef.current) return;
      const romId = romIdRef.current;
      if (!romId) return;
      // Re-fetch the cover image. Backend has already patched the registry's
      // cover_path; getArtworkBase64 will now resolve the freshly-downloaded
      // file. Catch swallowed because the .then handles the success path —
      // the rejection branch surfaces via the empty-cover render.
      await refreshCoverArtInBackground(romId, () => cancelled, setState);
    };

    const onDataChanged = (e: Event) => {
      detach(
        (async () => {
          try {
            const detail = (e as CustomEvent).detail;
            if (!romIdRef.current) return;
            switch (detail?.type) {
              case "save_sync_settings":
                await handleSaveSyncSettingsChange(detail);
                break;
              case "save_sync":
                await handleSaveSyncChange(detail);
                break;
              case "metadata":
                await handleMetadataChange(detail);
                break;
              case "cover_refreshed":
                await handleCoverRefreshed(detail);
                break;
              case "version_switched":
                await handleVersionSwitched(detail);
                break;
            }
          } catch (err) {
            detach(debugLog(`RomMGameInfoPanel: onDataChanged error: ${err}`));
          }
        })(),
      );
    };
    globalThis.addEventListener("romm_data_changed", onDataChanged);

    const onTabSwitch = (e: Event) => {
      const tab = (e as CustomEvent).detail?.tab;
      if (tab) setState((prev) => ({ ...prev, activeTab: tab }));
    };
    globalThis.addEventListener("romm_tab_switch", onTabSwitch);

    return () => {
      cancelled = true;
      globalThis.removeEventListener("romm_rom_uninstalled", onUninstall);
      removeEventListener("download_complete", onDownloadComplete);
      globalThis.removeEventListener("romm_data_changed", onDataChanged);
      globalThis.removeEventListener("romm_tab_switch", onTabSwitch);
    };
  }, [appId]);

  useEffect(() => {
    if (state.activeTab !== "saves" || !state.saveSyncEnabled || !state.romId) return;
    if (slotsLoadedRef.current) return;

    // Known-offline fast path (#1345): the server slot fetch runs through the
    // retry+backoff ladder, so on a known-unreachable server it would hang
    // "Loading slots…" for tens of seconds before the local degraded view (the
    // per-slot "Server unreachable" notices) renders. Skip the fetch entirely —
    // slotsLoading is still false here (the slotsLoadedRef guard above means the
    // connected path never set it), so SavesTab renders the degraded view now.
    // slotsLoadedRef stays false, so a flip back to connected re-runs this effect
    // (isOffline dep) and loads.
    if (isOffline) return;
    slotsLoadedRef.current = true;

    const gen = ++loadGenRef.current;
    let cancelled = false;
    let settled = false;

    async function loadSlots() {
      // Drop stale retry progress from a prior load so the SavesTab's
      // ConnectingIndicator starts at plain "Connecting to RomM…" (#1345).
      setServerRetryProgress(null);
      setState((prev) => ({ ...prev, slotsLoading: true }));
      try {
        if (!state.romId) return;
        const result = await getSaveSlots(state.romId);
        if (cancelled) return;
        settled = true;
        // A completed slot fetch is a reachability signal (#1345): a success
        // proves the server is reachable; an unreachable server drives the store
        // offline (which then renders the fast path above on the next dependent
        // run). Any OTHER failure reason is a server-side "no" (the server
        // answered), not a connectivity verdict — leave the store untouched.
        if (result.success) {
          reportServerReachable(true);
        } else if (result.reason === "server_unreachable") {
          reportServerReachable(false);
        }
        applyLoadSlotsResult<PanelState>(result, setState, slotsLoadedRef, (msg) => {
          detach(debugLog(msg));
        });
      } catch (e) {
        detach(debugLog(`Failed to load save slots: ${e}`));
        if (!cancelled) {
          settled = true;
          slotsLoadedRef.current = false;
          setState((prev) => ({ ...prev, slotsLoading: false }));
        }
      } finally {
        // Clear-on-settle in addition to clear-on-start (#1345 F2), but only if
        // this is still the latest load — a newer load may already own the
        // shared frame.
        if (loadGenRef.current === gen) setServerRetryProgress(null);
      }
    }

    detach(loadSlots());
    return () => {
      cancelled = true;
      // Torn down mid-flight (e.g. a concurrent call flipped the store offline,
      // re-running this effect) — release the load-once gate and drop the spinner
      // so the re-run isn't wedged behind a stuck slotsLoading/slotsLoadedRef, and
      // a later reconnect reliably reloads (#1345 F2). A settled load already set
      // its own final state, so leave it alone.
      if (!settled) {
        slotsLoadedRef.current = false;
        setState((prev) => (prev.slotsLoading ? { ...prev, slotsLoading: false } : prev));
      }
    };
  }, [state.activeTab, state.saveSyncEnabled, state.romId, isOffline]);

  // --- Render helpers ---

  /** A labeled info row: LABEL on the left, value on the right */
  const infoRow = (key: string, label: string, value: string) =>
    createElement(
      "div",
      { key, className: "romm-panel-info-row" },
      createElement("span", { className: "romm-panel-label" }, label),
      createElement("span", { className: "romm-panel-value" }, value),
    );

  /** A section with a title and children — uses DialogButton (not Focusable)
   *  because DialogButton is natively focusable by Steam's gamepad engine.
   *  Styled to look like a content section, not a button.
   *  Steam's outer scroll container auto-scrolls to focused elements. */
  const section = (key: string, title: string | null, ...children: (ReturnType<typeof createElement> | null)[]) =>
    createElement(
      DialogButton,
      {
        key,
        className: "romm-panel-section",
        style: {
          background: "transparent",
          border: "none",
          padding: "12px 0",
          textAlign: "left" as const,
          width: "100%",
          cursor: "default",
          display: "block",
        },
        noFocusRing: false,
        onFocus: scrollFocusedToCenter,
      },
      title ? createElement("div", { className: "romm-panel-section-title" }, title) : null,
      ...children.filter(Boolean),
    );

  // --- Version mismatch — replace entire panel with polished error card ---
  if (versionError) {
    return createElement("div", { "data-romm": "true" }, createElement(VersionErrorCard, { message: versionError }));
  }

  // --- Corrupt-settings reset — surface the reason instead of a bare "offline" ---
  if (settingsReset.pending) {
    return createElement(
      "div",
      { "data-romm": "true" },
      createElement(SettingsResetCard, { backedUpTo: settingsReset.backedUpTo, compact: true }),
    );
  }

  // --- Loading state ---
  // Use minHeight so Steam's scroll container allocates enough space
  // before async data loads and expands the panel.
  if (state.loading) {
    return createElement(
      "div",
      {
        "data-romm": "true",
        className: "romm-panel-container",
        style: { minHeight: "500px" },
      },
      createElement("div", { className: "romm-panel-loading" }, "Loading..."),
    );
  }

  // --- Error / not found state ---
  if (state.error || !state.romId) {
    return null;
  }

  const romId = state.romId;
  const meta = state.metadata;

  // --- Game Info section ---
  const gameInfoChildren: ReturnType<typeof createElement>[] = [];

  // The RomM game name (distinct from the Steam shortcut hero title, which can differ).
  if (state.romName) {
    gameInfoChildren.push(createElement("div", { key: "rom-name", className: "romm-panel-rom-name" }, state.romName));
  }

  // Region / Languages of the active version (ADR-0021) — omitted when empty.
  if (state.regions.length > 0) {
    gameInfoChildren.push(infoRow("regions", "Region", state.regions.join("/")));
  }
  if (state.languages.length > 0) {
    gameInfoChildren.push(infoRow("languages", "Languages", state.languages.join(", ")));
  }

  if (meta) {
    if (meta.summary) {
      gameInfoChildren.push(createElement("div", { key: "summary", className: "romm-panel-summary" }, meta.summary));
    }

    // Platform after description
    if (state.platformName) {
      gameInfoChildren.push(infoRow("platform", "Platform", state.platformName));
    }

    if (meta.companies.length > 0) {
      gameInfoChildren.push(infoRow("companies", "Developer / Publisher", meta.companies.join(", ")));
    }

    if (meta.genres.length > 0) {
      gameInfoChildren.push(
        createElement(
          "div",
          { key: "genres", className: "romm-panel-info-row" },
          createElement("span", { className: "romm-panel-label" }, "Genres"),
          createElement(
            "div",
            { className: "romm-panel-tags" },
            ...meta.genres.map((g) => createElement("span", { key: g, className: "romm-panel-tag" }, g)),
          ),
        ),
      );
    }

    const releaseDate = formatReleaseDate(meta.first_release_date);
    if (releaseDate) {
      gameInfoChildren.push(infoRow("release-date", "Release Date", releaseDate));
    }

    if (meta.game_modes.length > 0) {
      gameInfoChildren.push(infoRow("game-modes", "Game Modes", meta.game_modes.join(", ")));
    }

    if (meta.player_count) {
      gameInfoChildren.push(infoRow("players", "Players", meta.player_count));
    }

    if (meta.average_rating != null && meta.average_rating > 0) {
      gameInfoChildren.push(infoRow("rating", "Rating", `${Math.round(meta.average_rating)}%`));
    }
  } else if (state.platformName) {
    // No metadata — still show platform
    gameInfoChildren.push(infoRow("platform", "Platform", state.platformName));
  }

  // "No metadata available" fires only when NO descriptive row was added (name,
  // region/languages, metadata, or platform). The version switcher no longer lives
  // here — it moved to the play-button section (#1297) — so this is a plain count
  // of the descriptive rows.
  if (gameInfoChildren.length === 0) {
    gameInfoChildren.push(createElement("div", { key: "no-meta", className: "romm-panel-muted" }, "No metadata available"));
  }
  const gameInfoContent = gameInfoChildren;

  const gameInfoSection = state.coverBase64
    ? section(
        "game-info",
        null,
        createElement(
          "div",
          {
            key: "game-info-row",
            style: { display: "flex", gap: "16px", alignItems: "flex-start" },
          },
          createElement("img", {
            key: "cover",
            src: `data:image/png;base64,${state.coverBase64}`,
            style: { width: "120px", borderRadius: "4px", flexShrink: 0, objectFit: "cover" as const },
          }),
          createElement("div", { key: "details", style: { flex: 1 } }, ...gameInfoContent),
        ),
      )
    : section("game-info", null, ...gameInfoContent);

  // --- ROM File section (only when installed) ---
  const romFileSection =
    state.installed && state.installedRom
      ? section("rom-file", "ROM File", infoRow("filename", "Filename", state.installedRom.file_name))
      : null;

  // --- Tab bar ---
  const tabs: { id: string; label: string; visible: boolean }[] = [
    { id: "info", label: "GAME INFO", visible: true },
    { id: "saves", label: "SAVES", visible: state.saveSyncEnabled },
  ];

  const tabBar = createElement(
    Focusable,
    {
      className: "romm-tab-bar",
      "flow-children": "right",
      "data-romm": "true",
    },
    ...tabs
      .filter((t) => t.visible)
      .map((t) =>
        createElement(
          DialogButton,
          {
            key: `tab-${t.id}`,
            className: `romm-tab ${state.activeTab === t.id ? "romm-tab-active" : ""}`,
            onClick: () => setState((prev) => ({ ...prev, activeTab: t.id })),
            style: {
              background: "transparent",
              border: "none",
              borderBottom: state.activeTab === t.id ? "2px solid #1a9fff" : "2px solid transparent",
              padding: "10px 16px",
              minWidth: "auto",
              width: "auto",
            },
            noFocusRing: false,
          },
          t.label,
        ),
      ),
  );

  // --- Determine active tab content ---
  let activeTabContent: ReturnType<typeof createElement> | null = null;
  if (state.activeTab === "info") {
    activeTabContent = createElement("div", { key: "tab-info" }, gameInfoSection, romFileSection);
  } else if (state.activeTab === "saves") {
    if (state.saveSyncEnabled && !state.slotConfirmed) {
      activeTabContent = createElement(SlotSetupWizard, {
        romId: state.romId,
        onComplete: () => {
          // Refresh: mark as configured and reload save status
          setState((prev) => ({ ...prev, slotConfirmed: true }));
          globalThis.dispatchEvent(
            new CustomEvent("romm_data_changed", {
              detail: { type: "save_sync", rom_id: state.romId },
            }),
          );
        },
      });
    } else {
      activeTabContent = createElement(SavesTab, {
        appId,
        romId,
        saveStatus: state.saveStatus,
        conflicts: state.conflicts,
        activeSlot: state.activeSlot,
        availableSlots: state.availableSlots,
        slotsLoading: state.slotsLoading,
        onSlotSwitched: (newSlot, newStatus) => {
          setState((prev) => ({
            ...prev,
            activeSlot: newSlot === "" ? null : newSlot,
            saveStatus: newStatus,
            conflicts: newStatus.conflicts ?? [],
          }));
          globalThis.dispatchEvent(
            new CustomEvent("romm_data_changed", {
              detail: { type: "save_sync", rom_id: state.romId },
            }),
          );
        },
      });
    }
  }

  return createElement(
    "div",
    { "data-romm": "true" },
    tabBar,
    createElement(
      Focusable,
      {
        noFocusRing: true,
        className: "romm-tab-content",
        style: { paddingBottom: "48px" },
      },
      activeTabContent,
    ),
  );
};
