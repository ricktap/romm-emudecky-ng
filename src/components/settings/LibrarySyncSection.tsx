import { FC, useEffect, useState } from "react";
import { PanelSection, PanelSectionRow, ButtonItem, Field, ToggleField } from "@decky/ui";
import { clearSyncCache } from "../../api/backend";
import { getSkipPreview, setSkipPreview, onSyncPreferencesChange } from "../../utils/syncPreferences";
import { detach } from "../../utils/detach";

/** Library sync behaviour: whether a sync shows its preview, and the full re-fetch (moved here from the QAM home). */
export const LibrarySyncSection: FC = () => {
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
    <PanelSection title="Library sync">
      <PanelSectionRow>
        <ToggleField
          label="Skip sync preview"
          description="Apply changes without showing the preview first"
          checked={skipPreview}
          onChange={setSkipPreview}
        />
      </PanelSectionRow>
      <PanelSectionRow>
        <ButtonItem
          layout="below"
          description="Clear cached sync data so the next sync re-fetches every platform"
          onClick={() => detach(forceFullSync())}
        >
          Force full sync
        </ButtonItem>
      </PanelSectionRow>
      {note && (
        <PanelSectionRow>
          <Field label={note} />
        </PanelSectionRow>
      )}
    </PanelSection>
  );
};
