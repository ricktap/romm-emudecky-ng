import { FC, useEffect, useState } from "react";
import { Navigation, Tabs } from "@decky/ui";
import { getLibraryTab, setLibraryTab, type LibraryTab } from "../utils/libraryRoute";
import { getLibraryOverview } from "../api/backend";
import type { LibraryOverview } from "../types";
import { SettingsPage } from "./SettingsPage";
import { SystemPage } from "./SystemPage";
import { DangerZone } from "./DangerZone";
import { DownloadQueue } from "./DownloadQueue";
import { LibrarySyncSection } from "./settings/LibrarySyncSection";

// The moved pages keep their "Back" button; inside the route it leaves the route (history back).
const leave = () => Navigation.NavigateBack();

/** Placeholder until the cover grid (0.49.0): proves the overview callable and shows the per-platform counts. */
const LibraryTabContent: FC = () => {
  const [overview, setOverview] = useState<LibraryOverview | null>(null);

  useEffect(() => {
    let alive = true;
    getLibraryOverview()
      .then((o) => alive && setOverview(o))
      .catch(() => alive && setOverview({ success: false, message: "Could not load the library" }));
    return () => {
      alive = false;
    };
  }, []);

  if (!overview) return <div style={{ padding: "16px" }}>Loading library...</div>;
  if (!overview.success) {
    return <div style={{ padding: "16px" }}>{overview.message ?? "Could not load the library"}</div>;
  }
  const installed = overview.roms.filter((r) => r.installed).length;
  return (
    <div style={{ padding: "16px" }}>
      <div>
        {overview.roms.length} games, {installed} installed
      </div>
      {overview.platforms.map((p) => (
        <div key={p.platform_slug} style={{ opacity: 0.7 }}>
          {p.platform_slug}: {p.count} games, {p.installed} installed
        </div>
      ))}
      <div style={{ marginTop: "12px", opacity: 0.6 }}>The cover grid arrives in a later release.</div>
    </div>
  );
};

export const LibraryRoute: FC = () => {
  const [tab, setTabState] = useState<LibraryTab>(getLibraryTab());
  const setTab = (t: string) => {
    setLibraryTab(t as LibraryTab);
    setTabState(getLibraryTab());
  };

  return (
    <div style={{ marginTop: "40px", height: "calc(100% - 40px)" }}>
      <Tabs
        activeTab={tab}
        onShowTab={setTab}
        tabs={[
          { id: "library", title: "Library", content: <LibraryTabContent /> },
          { id: "downloads", title: "Downloads", content: <DownloadQueue onBack={leave} /> },
          { id: "systems", title: "Systems & BIOS", content: <SystemPage onBack={leave} /> },
          {
            id: "settings",
            title: "Settings",
            content: (
              <>
                <SettingsPage onBack={leave} />
                <LibrarySyncSection />
                <DangerZone onBack={leave} />
              </>
            ),
          },
        ]}
      />
    </div>
  );
};
