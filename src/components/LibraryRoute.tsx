import { FC, useEffect, useRef, useState } from "react";
import { Focusable, GamepadButton, Navigation } from "@decky/ui";
import { getDownloadQueue, getSettings, getSyncStats } from "../api/backend";
import { getDownloadState, setDownloads } from "../utils/downloadStore";
import { isUnfinishedStatus } from "../utils/downloadStatus";
import { useRommConnectionState } from "../utils/connectionState";
import { formatTimeAgo } from "../utils/formatters";
import { getLibraryTab, setLibraryTab, type LibraryTab } from "../utils/libraryRoute";
import { SettingsPage } from "./SettingsPage";
import { SystemPage } from "./SystemPage";
import { DownloadQueue } from "./DownloadQueue";
import { LibraryGrid } from "./LibraryGrid";
import { AttRow, Glyph, Icon, RmButton, RmStyles } from "./rm";
import { emudeckTitle, useEmudeckHealth } from "../utils/emudeckHealth";

// The moved pages keep their "Back" button; inside the route it leaves the route (history back).
const leave = () => Navigation.NavigateBack();

const TABS: { id: LibraryTab; title: string }[] = [
  { id: "library", title: "Library" },
  { id: "downloads", title: "Downloads" },
  { id: "systems", title: "Systems & BIOS" },
  { id: "settings", title: "Settings" },
];

export function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

/** The header's status line: a dot (green connected, amber checking, red offline) and `host · synced 2h ago`. */
export function headerStatus(host: string, lastSync: string | null, connection: string): { dot: string; text: string } {
  const synced = lastSync ? formatTimeAgo(lastSync) : null;
  const dot = connection === "connected" ? "" : connection === "checking" ? " rm-amber" : " rm-red";
  const base = connection === "offline" ? `${host || "RomM"} · not connected` : host || "RomM";
  return { dot, text: synced ? `${base} · synced ${synced}` : base };
}

export const LibraryRoute: FC = () => {
  const [tab, setTabState] = useState<LibraryTab>(getLibraryTab());
  const [search, setSearch] = useState("");
  const [host, setHost] = useState("");
  const [lastSync, setLastSync] = useState<string | null>(null);
  const [queued, setQueued] = useState(0);
  const connection = useRommConnectionState();
  const emudeck = useEmudeckHealth();
  const searchRef = useRef<HTMLInputElement | null>(null);

  const setTab = (t: LibraryTab) => {
    setLibraryTab(t);
    setTabState(getLibraryTab());
  };

  useEffect(() => {
    getSettings()
      .then((s) => setHost(hostOf(s?.romm_url ?? "")))
      .catch(() => undefined);
    getSyncStats()
      .then((s) => setLastSync(s?.last_sync ?? null))
      .catch(() => undefined);
    // The queue store fills from live events; read the durable queue once so the badge is right on first open.
    if (getDownloadState().length === 0) {
      getDownloadQueue()
        .then((r) => r?.downloads && setDownloads(r.downloads))
        .catch(() => undefined);
    }
    const tick = () => setQueued(getDownloadState().filter((d) => isUnfinishedStatus(d.status)).length);
    tick();
    const poll = setInterval(tick, 1000);
    return () => clearInterval(poll);
  }, []);

  // L1 / R1 move between the tabs.
  const onButtonDown = (evt: { detail?: { button?: number } }) => {
    const at = TABS.findIndex((t) => t.id === tab);
    const button = evt?.detail?.button;
    if (button === GamepadButton.BUMPER_LEFT) setTab(TABS[Math.max(0, at - 1)]!.id);
    if (button === GamepadButton.BUMPER_RIGHT) setTab(TABS[Math.min(TABS.length - 1, at + 1)]!.id);
  };

  const status = headerStatus(host, lastSync, connection);

  return (
    <div className="rm-root rm-lib" data-testid="library-route">
      <RmStyles />
      <Focusable noFocusRing data-testid="library-root" onButtonDown={onButtonDown} style={{ display: "contents" }}>
        <div className="rm-lhead">
          <h1>RomM Library</h1>
          <span className="rm-status">
            <span className={`rm-dot${status.dot}`} />
            {status.text}
          </span>
          <Focusable noFocusRing className="rm-searchwrap" onActivate={() => searchRef.current?.focus()}>
            <Icon name="search" size={16} />
            <input
              ref={searchRef}
              className="rm-search"
              type="search"
              placeholder="Search games"
              aria-label="Search games"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
          </Focusable>
          <RmButton variant="sm" onActivate={() => Navigation.OpenQuickAccessMenu?.()} testId="header-sync">
            <Icon name="sync" size={16} />
            Sync
          </RmButton>
        </div>
        <Focusable noFocusRing flow-children="right" className="rm-ltabs">
          <Glyph side="left">L1</Glyph>
          {TABS.map((t) => (
            <Focusable noFocusRing
              key={t.id}
              data-testid={`tab-${t.id}`}
              className={`rm-ltab${tab === t.id ? " rm-on" : ""}`}
              onActivate={() => setTab(t.id)}
              onClick={() => setTab(t.id)}
            >
              {t.title}
              {t.id === "downloads" && queued > 0 && <span className="rm-badge">{queued}</span>}
            </Focusable>
          ))}
          <Glyph side="right">R1</Glyph>
        </Focusable>
        {emudeck && !emudeck.ok && (
          <div style={{ padding: "8px 28px 0" }} data-testid="emudeck-banner">
            <AttRow title={emudeckTitle(emudeck.health)} sub={emudeck.message} onActivate={() => setTab("systems")} />
          </div>
        )}
        {tab === "library" && <LibraryGrid search={search} />}
        {tab === "downloads" && (
          <div className="rm-lmain" style={{ maxWidth: 900 }}>
            <DownloadQueue onBack={leave} />
          </div>
        )}
        {tab === "systems" && (
          <div className="rm-lmain">
            <SystemPage onBack={leave} />
          </div>
        )}
        {tab === "settings" && (
          <div className="rm-lmain">
            <SettingsPage onBack={leave} />
          </div>
        )}
      </Focusable>
    </div>
  );
};
