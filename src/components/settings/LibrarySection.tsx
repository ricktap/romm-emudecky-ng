/**
 * Library-wide sync preferences, as one settings card: the preferred region
 * (ADR-0021 §3: which region wins when a game has several dumps and the plugin
 * must pick one to bind and to name the Steam shortcut after), whether a sync
 * shows its preview, and the full re-fetch. Pure renderer for the region; the
 * sync rows read and drive the shared sync preferences themselves.
 */

import { FC, useEffect, useState } from "react";
import { clearSyncCache } from "../../api/backend";
import { getSkipPreview, setSkipPreview, onSyncPreferencesChange } from "../../utils/syncPreferences";
import { detach } from "../../utils/detach";
import { RmButton, RmCard, RmChoice, RmRow, RmToggle, RmValueRow } from "../rm";

interface LibrarySectionProps {
  preferredRegion: string;
  // Distinct region values found in the locally synced library (from the
  // backend get_known_regions read). Appended after the fixed anchors.
  libraryRegions: string[];
  onPreferredRegionChange: (region: string) => void;
}

// The internal sentinel for "no preference — use the fixed build-time order".
export const AUTO_REGION = "auto";

// The fixed anchor regions, in the build-time default order. MIRRORS the backend
// constant DEFAULT_REGION_PRIORITY — keep the two in sync. This is a fixed order,
// NOT language/system detection.
export const ANCHOR_REGIONS: readonly string[] = ["World", "USA", "Europe", "Japan"];

// The default option's label states the order explicitly so it never reads as
// auto-detection.
export const DEFAULT_REGION_LABEL = "Default (World > USA > Europe)";

/**
 * Build the region options: the "Default" sentinel + the fixed anchors, then
 * every OTHER region found in the local library (sorted, de-duped against the
 * anchors). The currently-selected value is always included so a preference for
 * a region no longer in the library still renders as selected.
 */
export function buildRegionOptions(libraryRegions: string[], selected: string): { data: string; label: string }[] {
  const options: { data: string; label: string }[] = [
    { data: AUTO_REGION, label: DEFAULT_REGION_LABEL },
    ...ANCHOR_REGIONS.map((r) => ({ data: r, label: r })),
  ];
  const known = new Set<string>([AUTO_REGION, ...ANCHOR_REGIONS]);
  const extras = Array.from(new Set(libraryRegions))
    .filter((r) => r && !known.has(r))
    .sort((a, b) => a.localeCompare(b));
  for (const r of extras) {
    options.push({ data: r, label: r });
    known.add(r);
  }
  if (selected !== AUTO_REGION && !known.has(selected)) {
    options.push({ data: selected, label: selected });
  }
  return options;
}

export const LibrarySection: FC<LibrarySectionProps> = ({ preferredRegion, libraryRegions, onPreferredRegionChange }) => {
  const [skipPreview, setSkipPreviewState] = useState(getSkipPreview());
  const [note, setNote] = useState("");

  useEffect(() => onSyncPreferencesChange(() => setSkipPreviewState(getSkipPreview())), []);

  const forceFullSync = async () => {
    try {
      const result = await clearSyncCache();
      setNote(result.message);
    } catch {
      setNote("Failed to clear sync cache");
    }
  };

  return (
    <RmCard title="Library sync">
      <RmRow label="Preferred region" sub="Picks the shortcut's version when a game has several (default: World > USA > Europe). Applies from the next sync.">
        <RmChoice
          label="Preferred region"
          value={preferredRegion}
          options={buildRegionOptions(libraryRegions, preferredRegion).map((o) => (o.data === AUTO_REGION ? { ...o, label: "Default" } : o))}
          onChange={onPreferredRegionChange}
        />
      </RmRow>
      <RmRow label="Skip preview" sub="Apply changes without review">
        <RmToggle label="Skip preview" checked={skipPreview} onChange={setSkipPreview} />
      </RmRow>
      <RmRow label="Full resync" sub="Clears cached data, re-fetches all platforms">
        <RmButton variant="sm ghost" testId="full-resync" onActivate={() => detach(forceFullSync())}>
          Run
        </RmButton>
      </RmRow>
      {note && <RmValueRow label={note} />}
    </RmCard>
  );
};
