import { FC, useEffect, useMemo, useState } from "react";
import { addEventListener, removeEventListener, toaster } from "@decky/api";
import { DialogButton, Focusable, Navigation, TextField } from "@decky/ui";
import { FaCheck, FaCloud, FaDownload, FaExclamationTriangle, FaPause } from "react-icons/fa";
import { getPlatforms, getLibraryOverview, pauseDownload, resumeDownload, startDownload } from "../api/backend";
import { getDownloadState } from "../utils/downloadStore";
import {
  GRID_FILTERS,
  coverStatus,
  coverUrl,
  filterCounts,
  liveDownloads,
  quickAction,
  visibleRoms,
  type CoverStatus,
  type GridFilter,
  type QuickAction,
} from "../utils/libraryGrid";
import { getLibraryPlatformFilter, setLibraryPlatformFilter } from "../utils/libraryRoute";
import { detach } from "../utils/detach";
import type { DownloadItem, LibraryOverview, LibraryOverviewRom, PlatformSyncSetting } from "../types";

type Loaded = Extract<LibraryOverview, { success: true }>;

const ACTION_TEXT: Record<Exclude<QuickAction, null>, string> = { download: "Download", pause: "Pause", resume: "Resume" };

const Badge: FC<{ status: CoverStatus }> = ({ status }) => {
  const style = { position: "absolute", top: "6px", right: "6px", width: "22px", height: "22px", borderRadius: "11px", display: "flex", alignItems: "center", justifyContent: "center", fontSize: "11px", color: "#fff" } as const;
  if (status.issue) {
    return (
      <span data-testid="badge-issue" style={{ ...style, background: "#d4a72c" }}>
        <FaExclamationTriangle size={11} />
      </span>
    );
  }
  switch (status.state) {
    case "installed":
      return <span data-testid="badge-installed" style={{ ...style, background: "#59bf40" }}><FaCheck size={11} /></span>;
    case "romm_only":
      return <span data-testid="badge-cloud" style={{ ...style, background: "#4b5563" }}><FaCloud size={11} /></span>;
    case "paused":
      return <span data-testid="badge-paused" style={{ ...style, background: "#d4a72c" }}><FaPause size={10} /></span>;
    default:
      return <span data-testid="badge-downloading" style={{ ...style, background: "#1a9fff" }}><FaDownload size={10} /></span>;
  }
};

const Cover: FC<{ rom: LibraryOverviewRom; status: CoverStatus; onAction: (a: Exclude<QuickAction, null>) => void }> = ({ rom, status, onAction }) => {
  const [broken, setBroken] = useState(false);
  const action = quickAction(status);
  return (
    <Focusable
      data-testid={`cover-${rom.rom_id}`}
      onActivate={() => Navigation.Navigate(`/library/app/${rom.app_id}`)}
      onSecondaryButton={() => action && onAction(action)}
      onOKActionDescription="Open"
      onSecondaryActionDescription={action ? ACTION_TEXT[action] : undefined}
      style={{ width: "150px", flex: "none", display: "flex", flexDirection: "column", gap: "4px" }}
    >
      <div style={{ position: "relative", width: "150px", height: "200px", borderRadius: "4px", overflow: "hidden", background: "#171c23" }}>
        {!broken && (
          <img
            src={coverUrl(rom.app_id)}
            alt=""
            onError={() => setBroken(true)}
            style={{ width: "100%", height: "100%", objectFit: "cover", opacity: status.state === "romm_only" ? 0.6 : 1 }}
          />
        )}
        {broken && (
          <div data-testid="cover-fallback" style={{ padding: "8px", fontSize: "12px", opacity: 0.7 }}>
            {rom.name}
          </div>
        )}
        <Badge status={status} />
        {status.progress !== null && (
          <div style={{ position: "absolute", left: 0, right: 0, bottom: 0, height: "5px", background: "rgba(0,0,0,0.5)" }}>
            <div data-testid="cover-progress" style={{ width: `${status.progress}%`, height: "100%", background: status.state === "paused" || status.state === "interrupted" ? "#d4a72c" : "#1a9fff" }} />
          </div>
        )}
      </div>
      <div style={{ fontSize: "12px", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{rom.name}</div>
    </Focusable>
  );
};

/**
 * The Library tab: platform rail, filter chips, search and the cover grid. A on a cover opens its Steam game page, X
 * downloads / pauses / resumes. Everything is one call (`get_library_overview`); the live queue keeps the badges current.
 */
export const LibraryGrid: FC = () => {
  const [overview, setOverview] = useState<LibraryOverview | null>(null);
  const [platforms, setPlatforms] = useState<PlatformSyncSetting[]>([]);
  const [downloads, setDownloads] = useState<DownloadItem[]>([]);
  const [platform, setPlatform] = useState<string | null>(getLibraryPlatformFilter());
  const [filter, setFilter] = useState<GridFilter>("all");
  const [search, setSearch] = useState("");

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
  const query = { platform, filter, search };
  const shown = useMemo(() => visibleRoms(roms, query, live), [roms, platform, filter, search, live]);
  const counts = useMemo(() => filterCounts(roms, { platform, search }, live), [roms, platform, search, live]);

  if (!overview) return <div style={{ padding: "16px" }}>Loading library...</div>;
  if (!overview.success) return <div style={{ padding: "16px" }}>{overview.message ?? "Could not load the library"}</div>;
  const loaded: Loaded = overview;

  const names = new Map(platforms.map((p) => [p.slug, p]));
  const choosePlatform = (slug: string | null) => {
    setLibraryPlatformFilter(slug);
    setPlatform(slug);
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

  return (
    <div style={{ display: "flex", gap: "16px", padding: "8px 16px" }}>
      <Focusable data-testid="rail" style={{ width: "200px", flex: "none", display: "flex", flexDirection: "column", gap: "4px" }}>
        <DialogButton
          style={{ opacity: platform === null ? 1 : 0.8, textAlign: "left" }}
          onClick={() => choosePlatform(null)}
        >
          All games ({loaded.roms.length})
        </DialogButton>
        {loaded.platforms.map((p) => {
          const info = names.get(p.platform_slug);
          return (
            <DialogButton
              key={p.platform_slug}
              data-testid={`rail-${p.platform_slug}`}
              {...(info && !info.sync_enabled ? { title: "Sync is off for this platform" } : {})}
              style={{ opacity: info && !info.sync_enabled ? 0.45 : platform === p.platform_slug ? 1 : 0.8, textAlign: "left" }}
              onClick={() => choosePlatform(p.platform_slug)}
            >
              {info?.name ?? p.platform_slug} ({p.count})
            </DialogButton>
          );
        })}
      </Focusable>
      <div style={{ flex: 1, minWidth: 0 }}>
        <TextField label="Search" value={search} onChange={(e: { target: { value: string } }) => setSearch(e.target.value)} />
        <Focusable flow-children="right" style={{ display: "flex", gap: "8px", margin: "8px 0" }}>
          {GRID_FILTERS.map((f) => (
            <DialogButton
              key={f.id}
              data-testid={`filter-${f.id}`}
              style={{ minWidth: "auto", width: "auto", padding: "4px 12px", opacity: filter === f.id ? 1 : 0.7 }}
              onClick={() => setFilter(f.id)}
            >
              {f.label} {counts[f.id]}
            </DialogButton>
          ))}
        </Focusable>
        {shown.length === 0 ? (
          <div style={{ padding: "8px 0" }}>No games match.</div>
        ) : (
          <Focusable flow-children="grid" style={{ display: "flex", flexWrap: "wrap", gap: "14px" }}>
            {shown.map((rom) => (
              <Cover key={rom.rom_id} rom={rom} status={coverStatus(rom, live)} onAction={(a) => detach(act(rom, a))} />
            ))}
          </Focusable>
        )}
      </div>
    </div>
  );
};
