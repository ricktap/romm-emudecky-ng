import { FC, useEffect, useMemo, useState } from "react";
import { addEventListener, removeEventListener, toaster } from "@decky/api";
import { Focusable, Navigation } from "@decky/ui";
import { Icon } from "./rm";
import { getArtworkBase64, getPlatforms, getLibraryOverview, pauseDownload, resumeDownload, startDownload } from "../api/backend";
import { getDownloadState } from "../utils/downloadStore";
import {
  GRID_FILTERS,
  coverStatus,
  coverUrl,
  filterCounts,
  liveDownloads,
  platformTag,
  quickAction,
  tintFor,
  visibleRoms,
  type CoverStatus,
  type GridFilter,
  type QuickAction,
} from "../utils/libraryGrid";
import { getLibraryPlatformFilter, setLibraryPlatformFilter } from "../utils/libraryRoute";
import { detach } from "../utils/detach";
import { loadRommCollections, type RommCollection } from "../utils/romCollections";
import type { DownloadItem, LibraryOverview, LibraryOverviewRom, PlatformSyncSetting } from "../types";

type Loaded = Extract<LibraryOverview, { success: true }>;

const ACTION_TEXT: Record<Exclude<QuickAction, null>, string> = { download: "Download", pause: "Pause", resume: "Resume" };

const Badge: FC<{ status: CoverStatus }> = ({ status }) => {
  if (status.issue) {
    return (
      <span data-testid="badge-issue" className="rm-sbadge rm-sb-warn">
        <Icon name="bang" />
      </span>
    );
  }
  switch (status.state) {
    case "installed":
      return <span data-testid="badge-installed" className="rm-sbadge rm-sb-ok"><Icon name="check" /></span>;
    case "romm_only":
      return <span data-testid="badge-cloud" className="rm-sbadge rm-sb-cloud"><Icon name="cloud" /></span>;
    case "paused":
    case "interrupted":
      return <span data-testid="badge-paused" className="rm-sbadge rm-sb-warn"><Icon name="pause" size={12} /></span>;
    default:
      return <span data-testid="badge-downloading" className="rm-sbadge rm-sb-dl"><Icon name="down" /></span>;
  }
};

/** The caption under a cover: what state the game is in, in a few words. */
export function coverCaption(rom: LibraryOverviewRom, status: CoverStatus): { text: string; warn: boolean } {
  if (status.issue) {
    const first = rom.issues[0];
    if (first === "save_conflict") return { text: "Save conflict", warn: true };
    if (first === "bios_missing") return { text: "BIOS missing", warn: true };
    if (status.state === "interrupted") return { text: "Interrupted", warn: true };
    return { text: "Needs attention", warn: true };
  }
  switch (status.state) {
    case "installed":
      return { text: "Installed", warn: false };
    case "romm_only":
      return { text: "On RomM", warn: false };
    case "queued":
      return { text: "Queued", warn: false };
    case "downloading":
      return { text: `Downloading ${status.progress ?? 0}%`, warn: false };
    default:
      return { text: `Paused · ${status.progress ?? 0}%`, warn: false };
  }
}

const Cover: FC<{ rom: LibraryOverviewRom; status: CoverStatus; onAction: (a: Exclude<QuickAction, null>) => void }> = ({ rom, status, onAction }) => {
  const [broken, setBroken] = useState(false);
  const [fallback, setFallback] = useState<string | null>(null);
  const action = quickAction(status);
  // Steam has no portrait art for this shortcut (lost, or never applied): take the cover from RomM, show it, and
  // give it back to Steam so the game page and Steam's own library show it too.
  const healCover = () => {
    getArtworkBase64(rom.rom_id)
      .then(async (r) => {
        if (!r?.base64) return setBroken(true);
        setFallback(`data:image/png;base64,${r.base64}`);
        try {
          await SteamClient.Apps.SetCustomArtworkForApp(rom.app_id, r.base64, "png", 0);
        } catch {
          // showing it here is enough; Refresh Artwork will retry the Steam side
        }
      })
      .catch(() => setBroken(true));
  };
  const caption = coverCaption(rom, status);
  return (
    <Focusable noFocusRing
      className="rm-gcard"
      data-testid={`cover-${rom.rom_id}`}
      onActivate={() => Navigation.Navigate(`/library/app/${rom.app_id}`)}
      onSecondaryButton={() => action && onAction(action)}
      onOKActionDescription="Open"
      onSecondaryActionDescription={action ? ACTION_TEXT[action] : undefined}
    >
      <span className="rm-cover" style={{ background: tintFor(rom.name) }}>
        {!broken && (
          <img
            src={fallback ?? coverUrl(rom.app_id)}
            alt=""
            onError={() => (fallback ? setBroken(true) : healCover())}
            style={{ position: "absolute", inset: 0, width: "100%", height: "100%", objectFit: "cover", opacity: status.state === "romm_only" ? 0.85 : 1 }}
          />
        )}
        <span className="rm-ptag">{platformTag(rom.platform_slug)}</span>
        {broken && (
          <span data-testid="cover-fallback" className="rm-ctitle">
            {rom.name}
          </span>
        )}
        <Badge status={status} />
        {status.progress !== null && (
          <span className="rm-cbar">
            <span data-testid="cover-progress" style={{ width: `${status.progress}%`, background: status.state === "paused" || status.state === "interrupted" ? "#d4a72c" : "#1a9fff" }} />
          </span>
        )}
      </span>
      <span className="rm-gname">{rom.name}</span>
      <span className={`rm-gsub${caption.warn ? " rm-warn" : ""}`}>{caption.text}</span>
    </Focusable>
  );
};

/**
 * The Library tab: platform rail, filter chips, search and the cover grid. A on a cover opens its Steam game page, X
 * downloads / pauses / resumes. Everything is one call (`get_library_overview`); the live queue keeps the badges current.
 */
export const LibraryGrid: FC<{ search: string }> = ({ search }) => {
  const [overview, setOverview] = useState<LibraryOverview | null>(null);
  const [platforms, setPlatforms] = useState<PlatformSyncSetting[]>([]);
  const [downloads, setDownloads] = useState<DownloadItem[]>([]);
  const [platform, setPlatform] = useState<string | null>(getLibraryPlatformFilter());
  const [collections, setCollections] = useState<RommCollection[]>([]);
  const [collection, setCollection] = useState<string | null>(null);
  const [filter, setFilter] = useState<GridFilter>("all");

  const load = () => {
    getLibraryOverview()
      .then((o) => setOverview(o))
      .catch(() => setOverview({ success: false, message: "Could not load the library" }));
  };

  useEffect(() => {
    load();
    getPlatforms()
      .then((r) => r?.success && setPlatforms(r.platforms))
      .catch(() => undefined);
    loadRommCollections().then(setCollections).catch(() => undefined);
    const refresh = () => load();
    const onComplete = addEventListener("download_complete", refresh);
    const onSyncDone = addEventListener("sync_complete", refresh);
    globalThis.addEventListener("romm_rom_uninstalled", refresh);
    // The queue is a module store without subscriptions: a light poll keeps the badges live.
    const poll = setInterval(() => setDownloads([...getDownloadState()]), 1000);
    return () => {
      removeEventListener("download_complete", onComplete);
      removeEventListener("sync_complete", onSyncDone);
      globalThis.removeEventListener("romm_rom_uninstalled", refresh);
      clearInterval(poll);
    };
  }, []);

  const live = useMemo(() => liveDownloads(downloads), [downloads]);
  const roms = overview?.success ? overview.roms : [];
  const collectionApps = useMemo(
    () => collections.find((c) => c.name === collection)?.appIds ?? null,
    [collections, collection],
  );
  const query = { platform, collection: collectionApps, filter, search };
  const shown = useMemo(() => visibleRoms(roms, query, live), [roms, platform, collectionApps, filter, search, live]);
  const counts = useMemo(
    () => filterCounts(roms, { platform, collection: collectionApps, search }, live),
    [roms, platform, collectionApps, search, live],
  );

  if (!overview) return <div style={{ padding: "16px" }}>Loading library...</div>;
  if (!overview.success) return <div style={{ padding: "16px" }}>{overview.message ?? "Could not load the library"}</div>;
  const loaded: Loaded = overview;

  const names = new Map(platforms.map((p) => [p.slug, p]));
  // A platform and a collection are two ways to narrow the same list: choosing one clears the other.
  const choosePlatform = (slug: string | null) => {
    setLibraryPlatformFilter(slug);
    setPlatform(slug);
    setCollection(null);
  };
  const chooseCollection = (name: string) => {
    setLibraryPlatformFilter(null);
    setPlatform(null);
    setCollection(name);
  };

  const act = async (rom: LibraryOverviewRom, action: Exclude<QuickAction, null>) => {
    try {
      const result = await (action === "download" ? startDownload(rom.rom_id) : action === "pause" ? pauseDownload(rom.rom_id) : resumeDownload(rom.rom_id));
      if (result && !result.success) toaster.toast({ title: "EmuDecky RomM Manager", body: result.message || `Could not ${action} the download` });
    } catch {
      toaster.toast({ title: "EmuDecky RomM Manager", body: `Could not ${action} the download` });
    }
  };

  if (loaded.roms.length === 0) {
    return <div style={{ padding: "16px" }}>No games synced yet. Start a sync from the Quick Access Menu.</div>;
  }

  const platformLabel = collection ? collection : platform === null ? "All platforms" : (names.get(platform)?.name ?? platform);
  return (
    <Focusable noFocusRing flow-children="right" className="rm-lbody">
      <Focusable noFocusRing flow-children="down" data-testid="rail" className="rm-prail">
        <Focusable noFocusRing
          className={`rm-pbtn${platform === null && collection === null ? " rm-on" : ""}`}
          data-testid="rail-all"
          onActivate={() => choosePlatform(null)}
          onClick={() => choosePlatform(null)}
        >
          <span>All games</span>
          <span className="rm-n">{loaded.roms.length}</span>
        </Focusable>
        {loaded.platforms.map((p) => {
          const info = names.get(p.platform_slug);
          const off = info && !info.sync_enabled;
          return (
            <Focusable noFocusRing
              key={p.platform_slug}
              data-testid={`rail-${p.platform_slug}`}
              className={`rm-pbtn${platform === p.platform_slug ? " rm-on" : ""}${off ? " rm-off" : ""}`}
              {...(off ? { title: "Sync is off for this platform" } : {})}
              onActivate={() => choosePlatform(p.platform_slug)}
              onClick={() => choosePlatform(p.platform_slug)}
            >
              <span>{info?.name ?? p.platform_slug}</span>
              <span className="rm-n">{p.count}</span>
            </Focusable>
          );
        })}
        {collections.length > 0 && <div className="rm-qsec" style={{ paddingLeft: 12 }}>Collections</div>}
        {collections.map((c) => (
          <Focusable noFocusRing
            key={c.name}
            data-testid={`rail-collection-${c.name}`}
            className={`rm-pbtn${collection === c.name ? " rm-on" : ""}`}
            onActivate={() => chooseCollection(c.name)}
            onClick={() => chooseCollection(c.name)}
          >
            <span>{c.name}</span>
            <span className="rm-n">{loaded.roms.filter((r) => c.appIds.has(r.app_id)).length}</span>
          </Focusable>
        ))}
      </Focusable>
      <Focusable noFocusRing flow-children="down" className="rm-lmain">
        <Focusable noFocusRing flow-children="right" className="rm-chips">
          {GRID_FILTERS.map((f) => (
            <Focusable noFocusRing
              key={f.id}
              data-testid={`filter-${f.id}`}
              className={`rm-fchip${filter === f.id ? " rm-on" : ""}`}
              onActivate={() => setFilter(f.id)}
              onClick={() => setFilter(f.id)}
            >
              {f.label} · {counts[f.id]}
            </Focusable>
          ))}
          <span className="rm-muted" style={{ marginLeft: "auto" }}>
            {platformLabel} · {shown.length} shown
          </span>
        </Focusable>
        {shown.length === 0 ? (
          <div className="rm-muted" style={{ padding: "40px 0" }}>No games match this filter.</div>
        ) : (
          <Focusable noFocusRing flow-children="grid" className="rm-grid">
            {shown.map((rom) => (
              <Cover key={rom.rom_id} rom={rom} status={coverStatus(rom, live)} onAction={(a) => detach(act(rom, a))} />
            ))}
          </Focusable>
        )}
      </Focusable>
    </Focusable>
  );
};
