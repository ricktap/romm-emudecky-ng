import { FC, useState } from "react";
import { Navigation, Tabs } from "@decky/ui";
import { getLibraryTab, setLibraryTab, type LibraryTab } from "../utils/libraryRoute";
import { SettingsPage } from "./SettingsPage";
import { SystemPage } from "./SystemPage";
import { DangerZone } from "./DangerZone";
import { DownloadQueue } from "./DownloadQueue";
import { LibraryGrid } from "./LibraryGrid";
import { LibrarySyncSection } from "./settings/LibrarySyncSection";

// The moved pages keep their "Back" button; inside the route it leaves the route (history back).
const leave = () => Navigation.NavigateBack();

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
          { id: "library", title: "Library", content: <LibraryGrid /> },
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
