import { FC, useEffect, useState } from "react";
import { toaster } from "@decky/api";
import { ButtonItem, Field, PanelSection, PanelSectionRow } from "@decky/ui";
import { downloadRequiredFirmware, getFirmwareStatus, getPlatformCoreInfo } from "../api/backend";
import { changeGameCore } from "../utils/gameCore";
import { missingRequiredKeys } from "../utils/missingKeys";
import { detach } from "../utils/detach";
import type { CoreInfo, FirmwareStatus, KeyState } from "../types";

interface BiosCoreTabProps {
  appId: number;
  romId: number;
  platformSlug: string;
}

const STATE_TEXT: Record<KeyState, string> = {
  missing: "missing",
  matches: "ready",
  differs: "differs from RomM",
  present: "present",
};

/**
 * BIOS & Core for one game: the files its platform needs (Switch: keys per emulator) with a one-tap download of the
 * required ones, and the emulator it launches with, with the per-game override.
 */
export const BiosCoreTab: FC<BiosCoreTabProps> = ({ appId, romId, platformSlug }) => {
  const [firmware, setFirmware] = useState<FirmwareStatus | null>(null);
  const [core, setCore] = useState<CoreInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const isSwitch = platformSlug === "switch";

  const loadFirmware = () => {
    if (!isSwitch) return;
    getFirmwareStatus()
      .then((f) => setFirmware(f ?? null))
      .catch(() => setFirmware(null));
  };
  const loadCore = () => {
    getPlatformCoreInfo(romId)
      .then((c) => setCore(c ?? null))
      .catch(() => setCore(null));
  };

  useEffect(() => {
    loadFirmware();
    loadCore();
    // A finished key download (romm_data_changed type "firmware") and a changed override (type "core_changed",
    // from here or from the play section's menu) both change what this tab shows.
    const onData = (e: Event) => {
      const type = (e as CustomEvent<{ type?: string }>).detail?.type;
      if (type === "firmware") loadFirmware();
      if (type === "core_changed") loadCore();
    };
    globalThis.addEventListener("romm_data_changed", onData);
    return () => globalThis.removeEventListener("romm_data_changed", onData);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- the loaders close over romId/platformSlug only
  }, [romId, platformSlug]);

  const getRequired = async () => {
    setBusy(true);
    try {
      const result = await downloadRequiredFirmware(platformSlug);
      toaster.toast({
        title: "EmuDecky RomM Manager",
        body: result.message || (result.success ? "Downloading keys — progress is in Downloads" : "Could not start the download"),
      });
    } catch {
      toaster.toast({ title: "EmuDecky RomM Manager", body: "Could not start the download" });
    }
    setBusy(false);
  };

  const change = async (kind: "pin" | "clear", label?: string) => {
    setBusy(true);
    await changeGameCore(appId, romId, platformSlug, kind === "pin" ? { kind, label: label ?? "" } : { kind });
    setBusy(false);
  };

  const missing = missingRequiredKeys(firmware);

  return (
    <div data-romm="true" style={{ padding: "0 2.8vw" }}>
      <PanelSection title="BIOS">
        {!isSwitch && (
          <PanelSectionRow>
            <Field label="No BIOS needed for this platform" />
          </PanelSectionRow>
        )}
        {isSwitch && !firmware?.success && (
          <PanelSectionRow>
            <Field label="Key status unavailable" description={firmware?.message} />
          </PanelSectionRow>
        )}
        {isSwitch &&
          firmware?.success &&
          (firmware.keys ?? []).map((key) => (
            <PanelSectionRow key={key.file_name}>
              <Field
                label={`${key.file_name}${key.required ? "" : " (optional)"}`}
                description={
                  key.emulators.length > 0
                    ? key.emulators.map((e) => `${e.label}: ${STATE_TEXT[e.state]}`).join(" · ")
                    : "No emulator reads this file"
                }
              />
            </PanelSectionRow>
          ))}
        {isSwitch && missing.length > 0 && (
          <PanelSectionRow>
            <ButtonItem layout="below" disabled={busy} onClick={() => detach(getRequired())}>
              Get required files
            </ButtonItem>
          </PanelSectionRow>
        )}
      </PanelSection>

      <PanelSection title="Emulator">
        {!core && (
          <PanelSectionRow>
            <Field label="Emulator info unavailable" />
          </PanelSectionRow>
        )}
        {core && !core.emulator_data_available && (
          <PanelSectionRow>
            <Field label="Emulator list unavailable" description="EmuDeck's emulator data could not be read." />
          </PanelSectionRow>
        )}
        {core && core.emulator_data_available && (
          <>
            <PanelSectionRow>
              <Field
                label={core.active_core_label ?? "Default"}
                description={core.has_game_override ? "Set for this game" : "Following the system emulator"}
              />
            </PanelSectionRow>
            {core.emulators.map((e) => {
              const active = e.label === core.active_core_label;
              return (
                <PanelSectionRow key={e.label}>
                  <ButtonItem
                    layout="below"
                    disabled={busy || !e.bakeable || active}
                    description={e.bakeable ? undefined : "Needs setup in EmuDeck"}
                    onClick={() => detach(change("pin", e.label))}
                  >
                    {e.label}
                    {active ? " ✓" : ""}
                  </ButtonItem>
                </PanelSectionRow>
              );
            })}
            {core.has_game_override && (
              <PanelSectionRow>
                <ButtonItem layout="below" disabled={busy} onClick={() => detach(change("clear"))}>
                  Use system emulator
                </ButtonItem>
              </PanelSectionRow>
            )}
          </>
        )}
      </PanelSection>
    </div>
  );
};
