import { useState, useEffect, useMemo, useRef, FC } from "react";
import { QamHeader, RmButton, RmRow, RmSegments, RmStyles, RmToggle, RmValueRow, RmButtonGroup } from "./rm";
import {
  getPlatforms,
  savePlatformSync,
  setAllPlatformsSync,
  getCollections,
  saveCollectionSync,
  setAllCollectionsSync,
  saveCollectionPlatformGroups,
  getSettings,
} from "../api/backend";
import type { PlatformSyncSetting, CollectionSyncSetting, CollectionKind, CollectionScope } from "../types";
import { detach } from "../utils/detach";

type CollectionSubTab = "my" | "smart" | "franchise";

const SUB_TAB_ORDER: readonly CollectionSubTab[] = ["my", "smart", "franchise"];

const SUB_TAB_LABELS: Record<CollectionSubTab, string> = {
  my: "My",
  smart: "Smart",
  franchise: "Franchise",
};

const SUB_TAB_HEADERS: Record<CollectionSubTab, string> = {
  my: "MY COLLECTIONS",
  smart: "SMART COLLECTIONS",
  franchise: "FRANCHISE",
};

function filterCollectionsBySubTab(
  collections: CollectionSyncSetting[],
  subTab: CollectionSubTab,
  // When the favorites toggle isn't shown (zero or >1 favorites), the "My"
  // sub-tab includes favorites too so they remain reachable. Defaults to
  // false because the optimistic-update callsite in handleSetAllCollections
  // doesn't care — it only ever inspects the favorites-excluded "My" set,
  // and the favorites toggle owns favorites mutations independently.
  includeFavoritesInMy = false,
): CollectionSyncSetting[] {
  switch (subTab) {
    case "my":
      return collections.filter((c) => c.kind === "user" && (includeFavoritesInMy || !c.is_favorite));
    case "smart":
      return collections.filter((c) => c.kind === "smart");
    case "franchise":
      return collections.filter((c) => c.kind === "franchise");
  }
}

function favoritesDescription(romCount: number): string {
  if (romCount === 1) return "Includes 1 favorited ROM";
  return `Includes ${romCount} favorited ROMs`;
}

interface LibraryPageProps {
  onBack: () => void;
}

export const LibraryPage: FC<LibraryPageProps> = ({ onBack }) => {
  const [activeTab, setActiveTab] = useState<"platforms" | "collections">("platforms");

  // --- Platforms tab state ---
  const [syncPlatforms, setSyncPlatforms] = useState<PlatformSyncSetting[]>([]);
  const [syncLoading, setSyncLoading] = useState(true);
  const [syncError, setSyncError] = useState(false);

  // --- Collections tab state ---
  const [collections, setCollections] = useState<CollectionSyncSetting[]>([]);
  const [collectionsLoading, setCollectionsLoading] = useState(true);
  const [collectionsError, setCollectionsError] = useState(false);
  const collectionsLoaded = useRef(false);
  const [platformGroups, setPlatformGroups] = useState(false);
  const [activeSubTab, setActiveSubTab] = useState<CollectionSubTab>("my");

  // The favorites collection (a user collection with is_favorite=true) is
  // promoted to a top-level toggle. RomM's schema theoretically allows more
  // than one — if that ever happens, drop the toggle and let the "My" sub-tab
  // surface them all, since a single toggle can't represent the set.
  const favoritesCollection = useMemo(() => {
    const favs = collections.filter((c) => c.kind === "user" && c.is_favorite);
    if (favs.length === 0) return null;
    if (favs.length > 1) {
      console.warn(
        `romm-emudecky-ng: expected at most one favorites collection, got ${favs.length}. ` +
          `Falling back to listing them in the My sub-tab.`,
      );
      return null;
    }
    return favs[0] ?? null;
  }, [collections]);

  // Load sync platforms on mount
  useEffect(() => {
    getPlatforms()
      .then((result) => {
        if (result.success) {
          setSyncPlatforms(result.platforms);
        } else {
          setSyncError(true);
        }
      })
      .catch(() => setSyncError(true))
      .finally(() => setSyncLoading(false));
  }, []);

  // Load collections data lazily on first switch to collections tab.
  // Sub-tab is reset to "my" in the tab-click handler (not here);
  // that's an event-driven concern, not state synchronisation.
  useEffect(() => {
    if (activeTab === "collections" && !collectionsLoaded.current) {
      collectionsLoaded.current = true;
      Promise.all([getCollections(), getSettings()])
        .then(([collResult, settingsResult]) => {
          if (collResult.success) {
            setCollections(collResult.collections);
          } else {
            setCollectionsError(true);
          }
          setPlatformGroups(!!settingsResult.collection_create_platform_groups);
        })
        .catch(() => setCollectionsError(true))
        .finally(() => setCollectionsLoading(false));
    }
  }, [activeTab]);

  // Reset the collections sub-tab on every entry into the Collections tab
  // so the user lands on a predictable view (no persistence).
  const handleCollectionsTabClick = () => {
    setActiveSubTab("my");
    setActiveTab("collections");
  };

  // --- Platforms tab handlers ---
  const handleToggle = async (id: number, enabled: boolean) => {
    setSyncPlatforms((prev) => prev.map((p) => (p.id === id ? { ...p, sync_enabled: enabled } : p)));
    try {
      await savePlatformSync(id, enabled);
    } catch {
      setSyncPlatforms((prev) => prev.map((p) => (p.id === id ? { ...p, sync_enabled: !enabled } : p)));
    }
  };

  const handleSetAll = async (enabled: boolean) => {
    const previous = syncPlatforms.map((p) => ({ ...p }));
    setSyncPlatforms((prev) => prev.map((p) => ({ ...p, sync_enabled: enabled })));
    try {
      await setAllPlatformsSync(enabled);
    } catch {
      setSyncPlatforms(previous);
    }
  };

  // --- Collections tab handlers ---
  const handleCollectionToggle = async (id: string, kind: CollectionKind, enabled: boolean) => {
    setCollections((prev) => prev.map((c) => (c.id === id && c.kind === kind ? { ...c, sync_enabled: enabled } : c)));
    try {
      await saveCollectionSync(id, kind, enabled);
    } catch {
      setCollections((prev) =>
        prev.map((c) => (c.id === id && c.kind === kind ? { ...c, sync_enabled: !enabled } : c)),
      );
    }
  };

  const handleSetAllCollections = async (enabled: boolean, scope: CollectionScope) => {
    const previous = collections.map((c) => ({ ...c }));
    // Optimistically flip only the entries in the active sub-tab.
    setCollections((prev) =>
      prev.map((c) => (filterCollectionsBySubTab([c], scope).length > 0 ? { ...c, sync_enabled: enabled } : c)),
    );
    try {
      await setAllCollectionsSync(enabled, scope);
    } catch {
      setCollections(previous);
    }
  };

  // --- Platforms tab content ---
  const renderPlatformsContent = () => {
    if (syncLoading) return <div className="rm-muted" data-testid="loading">Loading…</div>;
    if (syncError) {
      return (
        <RmButton variant="block" onActivate={onBack}>
          Failed to load platforms
        </RmButton>
      );
    }
    return (
      <>
        <div className="rm-muted rm-small" style={{ marginBottom: 8 }}>Changes apply on the next sync</div>
        <RmButtonGroup style={{ marginBottom: 8 }}>
          <RmButton variant="sm ghost" className="rm-grow" onActivate={() => detach(handleSetAll(true))}>
            Enable All
          </RmButton>
          <RmButton variant="sm ghost" className="rm-grow" onActivate={() => detach(handleSetAll(false))}>
            Disable All
          </RmButton>
        </RmButtonGroup>
        <div className="rm-scard">
          {syncPlatforms.map((platform) => (
            <RmRow key={platform.id} label={platform.name} sub={`${platform.rom_count} ROMs`}>
              <RmToggle
                label={platform.name}
                checked={platform.sync_enabled}
                onChange={(value) => detach(handleToggle(platform.id, value))}
              />
            </RmRow>
          ))}
        </div>
      </>
    );
  };

  // --- Collections tab content ---
  const renderCollectionsContent = () => {
    if (collectionsLoading) return <div className="rm-muted" data-testid="loading">Loading…</div>;
    if (collectionsError) {
      return <RmValueRow label="Failed to load collections" sub="Check your connection and try again" />;
    }
    if (collections.length === 0) {
      return <RmValueRow label="No collections found" sub="Create collections in RomM to sync them here" />;
    }

    // When the favorites toggle isn't rendered (zero or multi-favorites case),
    // include any favorites in the "My" sub-tab so they stay reachable.
    const includeFavoritesInMy = favoritesCollection === null;
    const visible = filterCollectionsBySubTab(collections, activeSubTab, includeFavoritesInMy);
    const activeLabel = SUB_TAB_LABELS[activeSubTab];
    const sectionTitle = `${SUB_TAB_HEADERS[activeSubTab]} (${visible.length})`;

    return (
      <>
        <div className="rm-scard">
          <RmRow label="Show collection games in platform groups" sub="When syncing a collection, also add its games to their platform-specific Steam group.">
            <RmToggle
              label="Show collection games in platform groups"
              checked={platformGroups}
              onChange={(value) => {
                setPlatformGroups(value);
                detach(
                  (async () => {
                    try {
                      await saveCollectionPlatformGroups(value);
                    } catch {
                      setPlatformGroups(!value);
                    }
                  })(),
                );
              }}
            />
          </RmRow>
          {favoritesCollection && (
            <RmRow label="Sync RomM favorites" sub={favoritesDescription(favoritesCollection.rom_count)}>
              <RmToggle
                label="Sync RomM favorites"
                checked={favoritesCollection.sync_enabled}
                onChange={(value) => detach(handleCollectionToggle(favoritesCollection.id, favoritesCollection.kind, value))}
              />
            </RmRow>
          )}
        </div>
        <RmSegments
          testPrefix="subtab"
          items={SUB_TAB_ORDER.map((sub) => ({ id: sub, label: SUB_TAB_LABELS[sub] }))}
          value={activeSubTab}
          onChange={(sub) => setActiveSubTab(sub as CollectionSubTab)}
        />
        <div className="rm-qsec" style={{ marginTop: 0 }}>{sectionTitle}</div>
        <RmButtonGroup style={{ marginBottom: 8 }}>
          <RmButton variant="sm ghost" className="rm-grow" onActivate={() => detach(handleSetAllCollections(true, activeSubTab))}>
            Enable All
          </RmButton>
          <RmButton variant="sm ghost" className="rm-grow" onActivate={() => detach(handleSetAllCollections(false, activeSubTab))}>
            Disable All
          </RmButton>
        </RmButtonGroup>
        {visible.length === 0 ? (
          <RmValueRow label={`No ${activeLabel.toLowerCase()} collections`} />
        ) : (
          <div className="rm-scard">
            {visible.map((collection) => (
              <RmRow key={`${collection.kind}:${collection.id}`} label={collection.name} sub={`${collection.rom_count} ROMs`}>
                <RmToggle
                  label={collection.name}
                  checked={collection.sync_enabled}
                  onChange={(value) => detach(handleCollectionToggle(collection.id, collection.kind, value))}
                />
              </RmRow>
            ))}
          </div>
        )}
      </>
    );
  };

  // --- Render ---
  return (
    <div className="rm-root rm-qam" style={{ background: "transparent" }}>
      <RmStyles />
      <QamHeader title="Platforms & collections" onBack={onBack} />
      <RmSegments
        testPrefix="tab"
        items={[
          { id: "platforms", label: "Platforms" },
          { id: "collections", label: "Collections" },
        ]}
        value={activeTab}
        onChange={(id) => (id === "collections" ? handleCollectionsTabClick() : setActiveTab("platforms"))}
      />
      {activeTab === "platforms" ? renderPlatformsContent() : renderCollectionsContent()}
    </div>
  );
};
