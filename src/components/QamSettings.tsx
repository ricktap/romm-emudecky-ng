import { FC, useEffect, useState } from "react";
import { QamHeader, RmButton, RmRow, RmStyles, RmToggle, RmValueRow } from "./rm";
import { getSaveSyncSettings, updateSaveSyncSettings, getSettings } from "../api/backend";
import type { SaveSyncSettings } from "../types";
import { getSkipPreview, setSkipPreview, onSyncPreferencesChange } from "../utils/syncPreferences";
import { serverHost } from "../utils/serverUrl";
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
      // The read failed (backend not ready): say so instead of showing toggles that do nothing.
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
    <div className="rm-root" style={{ background: "transparent" }}>
      <RmStyles />
      <QamHeader title="Settings" onBack={onBack} />
      <div className="rm-scard">
        <h3>Save sync</h3>
        {saveSyncUnavailable && (
          <RmValueRow label="Save sync settings unavailable" sub="The plugin backend did not answer" />
        )}
        {saveSync && (
          <>
            <RmRow label="Save sync">
              <RmToggle label="Save sync" checked={saveSync.save_sync_enabled} onChange={(v) => detach(changeSaveSync({ save_sync_enabled: v }))} />
            </RmRow>
            {saveSync.save_sync_enabled && (
              <>
                <RmRow label="Sync before launch">
                  <RmToggle label="Sync before launch" checked={saveSync.sync_before_launch} onChange={(v) => detach(changeSaveSync({ sync_before_launch: v }))} />
                </RmRow>
                <RmRow label="Sync after exit">
                  <RmToggle label="Sync after exit" checked={saveSync.sync_after_exit} onChange={(v) => detach(changeSaveSync({ sync_after_exit: v }))} />
                </RmRow>
              </>
            )}
          </>
        )}
      </div>
      <div className="rm-scard" style={{ marginTop: 12 }}>
        <h3>Library sync</h3>
        <RmRow label="Skip sync preview" sub="Apply changes without review">
          <RmToggle label="Skip sync preview" checked={skipPreview} onChange={setSkipPreview} />
        </RmRow>
        <RmValueRow label="Server" value={host || "Not set"} />
      </div>
      <div style={{ marginTop: 12 }}>
        <RmButton variant="block" onActivate={() => openLibraryRoute("settings")}>
          All settings
        </RmButton>
      </div>
    </div>
  );
};
