import { FC, useEffect, useState } from "react";
import { addEventListener, removeEventListener, toaster } from "@decky/api";
import { DialogButton, Focusable } from "@decky/ui";
import { downloadRequiredFirmware, getFirmwareStatus, getInstalledRom, getSaveStatus } from "../api/backend";
import { getDownloadState } from "../utils/downloadStore";
import { biosChip, romChip, savesChip, type ReadinessChip } from "../utils/readiness";
import { detach } from "../utils/detach";
import type { DownloadItem, FirmwareStatus, InstalledRom } from "../types";
import { handleConflicts } from "./SyncConflictModal";

interface ReadinessStripProps {
  romId: number;
  platformSlug: string;
  saveSyncEnabled: boolean;
  saveSyncStatus: "synced" | "conflict" | "none" | null;
  saveSyncLabel: string;
  activeCoreLabel: string | null;
}

const TONE_COLOR = { ok: "#59bf40", warn: "#d4a72c", neutral: "#8b929a" } as const;

/**
 * Three chips under the play row: ROM, BIOS and Saves, each with a one-line state and, where there is something to
 * fix, a button. The primary button already downloads and plays; the chips say what else the game needs.
 */
export const ReadinessStrip: FC<ReadinessStripProps> = ({
  romId,
  platformSlug,
  saveSyncEnabled,
  saveSyncStatus,
  saveSyncLabel,
  activeCoreLabel,
}) => {
  const [installed, setInstalled] = useState<InstalledRom | null>(null);
  const [download, setDownload] = useState<DownloadItem | null>(null);
  const [firmware, setFirmware] = useState<FirmwareStatus | null>(null);

  const refreshInstalled = () => {
    getInstalledRom(romId)
      .then((rom) => setInstalled(rom ?? null))
      .catch(() => undefined);
  };
  const refreshFirmware = () => {
    if (platformSlug !== "switch") return;
    getFirmwareStatus()
      .then((f) => setFirmware(f ?? null))
      .catch(() => setFirmware(null));
  };

  useEffect(() => {
    refreshInstalled();
    refreshFirmware();
    // A finished transfer changes what is on disk: ROMs through download_complete, key files through the
    // plugin-wide romm_data_changed event (type "firmware"), an uninstall through romm_rom_uninstalled.
    const onComplete = addEventListener<[{ rom_id?: number }]>("download_complete", (evt) => {
      if (evt?.rom_id === romId) refreshInstalled();
    });
    const onUninstall = (e: Event) => {
      if ((e as CustomEvent<{ rom_id?: number }>).detail?.rom_id === romId) setInstalled(null);
    };
    const onData = (e: Event) => {
      if ((e as CustomEvent<{ type?: string }>).detail?.type === "firmware") refreshFirmware();
    };
    globalThis.addEventListener("romm_rom_uninstalled", onUninstall);
    globalThis.addEventListener("romm_data_changed", onData);
    // The queue lives in a module store without subscriptions; a light poll keeps the ROM chip live.
    const poll = setInterval(() => {
      setDownload(getDownloadState().find((d) => d.rom_id === romId && (d.kind ?? "rom") === "rom") ?? null);
    }, 1000);
    return () => {
      removeEventListener("download_complete", onComplete);
      globalThis.removeEventListener("romm_rom_uninstalled", onUninstall);
      globalThis.removeEventListener("romm_data_changed", onData);
      clearInterval(poll);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- the refreshers close over romId/platformSlug only
  }, [romId, platformSlug]);

  const getBios = async () => {
    try {
      const result = await downloadRequiredFirmware(platformSlug);
      toaster.toast({
        title: "EmuDecky RomM Manager",
        body: result.message || (result.success ? "Downloading keys — progress is in Downloads" : "Could not start the download"),
      });
    } catch {
      toaster.toast({ title: "EmuDecky RomM Manager", body: "Could not start the download" });
    }
  };

  const resolveConflict = async () => {
    try {
      const status = await getSaveStatus(romId);
      const outcome = await handleConflicts(status.conflicts ?? []);
      if (outcome === "resolved") {
        globalThis.dispatchEvent(new CustomEvent("romm_data_changed", { detail: { type: "save_sync", rom_id: romId } }));
      }
    } catch {
      toaster.toast({ title: "EmuDecky RomM Manager", body: "Could not load the save conflict" });
    }
  };

  const chips: ReadinessChip[] = [
    romChip({ installed: installed != null, fileName: installed?.file_name, download }),
    biosChip({ platformSlug, firmware, activeCoreLabel }),
    savesChip({ saveSyncEnabled, status: saveSyncStatus, label: saveSyncLabel }),
  ];

  return (
    <Focusable
      data-romm="true"
      className="romm-readiness-strip"
      flow-children="right"
      style={{ display: "flex", gap: "12px", padding: "8px 2.8vw 0", boxSizing: "border-box" }}
    >
      {chips.map((chip) => (
        <div
          key={chip.id}
          data-testid={`chip-${chip.id}`}
          style={{
            flex: 1,
            minWidth: 0,
            display: "flex",
            alignItems: "center",
            gap: "10px",
            padding: "8px 12px",
            borderRadius: "4px",
            background: chip.tone === "warn" ? "rgba(212, 167, 44, 0.15)" : "rgba(14, 20, 27, 0.33)",
            border: `1px solid ${chip.tone === "warn" ? "#4a3f1d" : "transparent"}`,
          }}
        >
          <span style={{ width: "8px", height: "8px", borderRadius: "4px", flex: "none", background: TONE_COLOR[chip.tone] }} />
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontSize: "11px", fontWeight: 700, letterSpacing: "0.06em", opacity: 0.7 }}>
              {chip.title.toUpperCase()}
            </div>
            <div
              data-testid={`chip-${chip.id}-detail`}
              style={{ fontSize: "12px", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
            >
              {chip.detail}
            </div>
          </div>
          {chip.fix && (
            <DialogButton
              style={{ minWidth: "auto", width: "auto", padding: "4px 12px", height: "28px", fontSize: "12px" }}
              onClick={() => detach(chip.fix === "get_bios" ? getBios() : resolveConflict())}
            >
              {chip.fixLabel}
            </DialogButton>
          )}
        </div>
      ))}
    </Focusable>
  );
};
