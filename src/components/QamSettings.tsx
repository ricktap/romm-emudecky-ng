import { FC, useEffect, useState } from "react";
import { PanelSection, PanelSectionRow, ButtonItem, Field, ToggleField } from "@decky/ui";
import { getSaveSyncSettings, updateSaveSyncSettings, getSettings } from "../api/backend";
import type { SaveSyncSettings } from "../types";
import { getSkipPreview, setSkipPreview, onSyncPreferencesChange } from "../utils/syncPreferences";
import { serverHost } from "../utils/serverUrl";
import { scrollToTop } from "../utils/scrollHelpers";
import { detach } from "../utils/detach";
import { openLibraryRoute } from "../utils/libraryRoute";

interface QamSettingsProps {
  onBack: () => void;
}

/**
 * The few settings worth a glance in the Quick Access Menu: save sync on/off and its triggers, whether a sync
 * shows its preview, and the server host. Everything else lives in the full-screen Settings tab.
 */
export const QamSettings: FC<QamSettingsProps> = ({ onBack }) => {
  const [saveSync, setSaveSync] = useState<SaveSyncSettings | null>(null);
  const [saveSyncUnavailable, setSaveSyncUnavailable] = useState(false);
  const [skipPreview, setSkipPreviewState] = useState(getSkipPreview());
  const [host, setHost] = useState("");

  useEffect(() => {
    let alive = true;
    getSaveSyncSettings()
      .then((s) => alive && setSaveSync(s))
      // Save sync is not implemented yet (0.55.0): the read rejects, say so instead of showing toggles that do nothing.
      .catch(() => alive && setSaveSyncUnavailable(true));
    getSettings()
      .then((s) => alive && setHost(serverHost(s.romm_url)))
      .catch(() => undefined);
    const unsub = onSyncPreferencesChange(() => setSkipPreviewState(getSkipPreview()));
    return () => {
      alive = false;
      unsub();
    };
  }, []);

  const changeSaveSync = async (partial: Partial<SaveSyncSettings>) => {
    if (!saveSync) return;
    const previous = saveSync;
    const updated = { ...saveSync, ...partial };
    setSaveSync(updated);
    try {
      await updateSaveSyncSettings(updated);
      if ("save_sync_enabled" in partial) {
        globalThis.dispatchEvent(
          new CustomEvent("romm_data_changed", {
            detail: { type: "save_sync_settings", save_sync_enabled: updated.save_sync_enabled },
          }),
        );
      }
    } catch {
      setSaveSync(previous);
    }
  };

  return (
    <>
      <PanelSection>
        <PanelSectionRow>
          <ButtonItem
            layout="below"
            onClick={onBack}
            // @ts-expect-error onFocus works at runtime; not in Decky's ButtonItem types
            onFocus={scrollToTop}
          >
            Back
          </ButtonItem>
        </PanelSectionRow>
      </PanelSection>

      <PanelSection title="Save sync">
        {saveSyncUnavailable && (
          <PanelSectionRow>
            <Field label="Save sync is not implemented yet" description="Planned for 0.55.0" focusable />
          </PanelSectionRow>
        )}
        {saveSync && (
          <>
            <PanelSectionRow>
              <ToggleField
                label="Enable save sync"
                checked={saveSync.save_sync_enabled}
                onChange={(v) => detach(changeSaveSync({ save_sync_enabled: v }))}
              />
            </PanelSectionRow>
            {saveSync.save_sync_enabled && (
              <>
                <PanelSectionRow>
                  <ToggleField
                    label="Sync before launch"
                    checked={saveSync.sync_before_launch}
                    onChange={(v) => detach(changeSaveSync({ sync_before_launch: v }))}
                  />
                </PanelSectionRow>
                <PanelSectionRow>
                  <ToggleField
                    label="Sync after exit"
                    checked={saveSync.sync_after_exit}
                    onChange={(v) => detach(changeSaveSync({ sync_after_exit: v }))}
                  />
                </PanelSectionRow>
              </>
            )}
          </>
        )}
      </PanelSection>

      <PanelSection title="Library sync">
        <PanelSectionRow>
          <ToggleField
            label="Skip sync preview"
            description="Apply changes without showing the preview first"
            checked={skipPreview}
            onChange={setSkipPreview}
          />
        </PanelSectionRow>
      </PanelSection>

      <PanelSection title="Server">
        <PanelSectionRow>
          <Field label="Host" description={host || "Not set"} focusable />
        </PanelSectionRow>
        <PanelSectionRow>
          <ButtonItem layout="below" onClick={() => openLibraryRoute("settings")}>
            All settings
          </ButtonItem>
        </PanelSectionRow>
      </PanelSection>
    </>
  );
};
