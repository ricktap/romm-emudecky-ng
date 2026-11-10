/**
 * Save-sync settings card — master enable, sync-before-launch, sync-after-exit,
 * default save slot, history limit and "Sync all". Pure renderer: the parent
 * owns SaveSyncSettings, deviceInfo and the sync status.
 */

import { FC } from "react";
import { showModal } from "@decky/ui";
import type { SaveSyncSettings as SaveSyncSettingsType } from "../../types";
import { TextInputModal } from "./TextInputModal";
import { RmButton, RmCard, RmChoice, RmRow, RmToggle, RmValueRow } from "../rm";

interface SaveSyncSectionProps {
  saveSyncSettings: SaveSyncSettingsType | null;
  saveSyncToggleKey: number;
  deviceInfo: { device_id: string; device_name: string } | null;
  syncing: boolean;
  syncStatus: string;
  onToggleSaveSync: (value: boolean) => void;
  onSettingChange: (partial: Partial<SaveSyncSettingsType>) => void;
  onDefaultSlotSubmit: (value: string) => void;
  onResetDefaultSlot: () => void;
  onSyncAll: () => void;
}

const HISTORY_LIMITS = [
  { data: "5", label: "5" },
  { data: "10", label: "10" },
  { data: "20", label: "20" },
  { data: "50", label: "50" },
];

export const SaveSyncSection: FC<SaveSyncSectionProps> = ({
  saveSyncSettings,
  saveSyncToggleKey,
  deviceInfo,
  syncing,
  syncStatus,
  onToggleSaveSync,
  onSettingChange,
  onDefaultSlotSubmit,
  onResetDefaultSlot,
  onSyncAll,
}) => {
  const saveSyncEnabled = saveSyncSettings?.save_sync_enabled ?? false;
  if (!saveSyncSettings) {
    return (
      <RmCard title="Save sync">
        <RmValueRow label="Loading..." />
      </RmCard>
    );
  }
  return (
    <RmCard title="Save sync">
      <RmRow label="Save sync" sub="Keep saves on RomM, shared between your devices">
        <RmToggle key={saveSyncToggleKey} label="Save sync" checked={saveSyncEnabled} onChange={onToggleSaveSync} />
      </RmRow>
      {!saveSyncEnabled && <RmValueRow label="Save sync is off" sub="Turn it on to set the options below" />}
      {saveSyncEnabled && (
        <>
          {deviceInfo && <RmValueRow label="This device" value={`"${deviceInfo.device_name}"`} />}
          <RmRow label="Download newer saves before launch">
            <RmToggle
              label="Sync before launch"
              checked={saveSyncSettings.sync_before_launch}
              onChange={(value) => onSettingChange({ sync_before_launch: value })}
            />
          </RmRow>
          <RmRow label="Upload saves after exit">
            <RmToggle
              label="Sync after exit"
              checked={saveSyncSettings.sync_after_exit}
              onChange={(value) => onSettingChange({ sync_after_exit: value })}
            />
          </RmRow>
          <RmRow label="Default save slot" sub="For new games and games without their own slot">
            <span style={{ display: "flex", gap: 8 }}>
              <RmButton
                variant="sm ghost"
                testId="edit-default-slot"
                onActivate={() =>
                  showModal(
                    <TextInputModal
                      label="Default Save Slot"
                      value={saveSyncSettings.default_slot ?? ""}
                      onSubmit={onDefaultSlotSubmit}
                    />,
                  )
                }
              >
                {saveSyncSettings.default_slot || "(no slot)"}
              </RmButton>
              {saveSyncSettings.default_slot !== "default" && (
                <RmButton variant="sm ghost" testId="reset-default-slot" onActivate={onResetDefaultSlot}>
                  Reset
                </RmButton>
              )}
            </span>
          </RmRow>
          <RmRow label="Versions kept per slot" sub="Older versions on the server are removed">
            <RmChoice
              label="Save history limit"
              value={String(saveSyncSettings.autocleanup_limit)}
              options={HISTORY_LIMITS}
              onChange={(value) => onSettingChange({ autocleanup_limit: Number(value) })}
            />
          </RmRow>
          <RmRow label="Sync all saves now">
            <RmButton variant="sm ghost" testId="sync-all-saves" disabled={syncing} onActivate={onSyncAll}>
              {syncing ? "Syncing..." : "Sync now"}
            </RmButton>
          </RmRow>
          {syncStatus && <RmValueRow label={syncStatus} />}
        </>
      )}
    </RmCard>
  );
};
