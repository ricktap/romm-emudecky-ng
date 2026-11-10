import { useState, useEffect, FC } from "react";
import { PanelSection, PanelSectionRow, ButtonItem, Field } from "@decky/ui";
import { getFirmwareStatus, downloadAllFirmware, downloadRequiredFirmware } from "../api/backend";
import type { FirmwareStatus, KeyState, SwitchKeyFile } from "../types";
import { formatBytes } from "../utils/formatters";
import { scrollToTop } from "../utils/scrollHelpers";
import { detach } from "../utils/detach";

const KEY_STATE_LABEL: Record<KeyState, string> = {
  matches: "matches RomM",
  differs: "differs from RomM — kept as is",
  present: "present",
  missing: "missing",
};

/** `Eden: matches RomM · Ryujinx: missing` — one line per key file. */
export function keyStateSummary(key: SwitchKeyFile): string {
  if (key.emulators.length === 0) return "No Switch emulator found";
  return key.emulators.map((e) => `${e.label}: ${KEY_STATE_LABEL[e.state]}`).join(" · ");
}

interface SystemPageProps {
  onBack: () => void;
}

/**
 * Top-level QAM destination for the Switch firmware and keys RomM hosts: the key files with their state in every
 * installed emulator, and the firmware versions. Reading never changes anything; an existing key file that differs
 * from RomM's is reported, not replaced. Downloads arrive with the firmware queue work.
 */
export const SystemPage: FC<SystemPageProps> = ({ onBack }) => {
  const [status, setStatus] = useState<FirmwareStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [downloading, setDownloading] = useState(false);
  const [note, setNote] = useState("");

  async function refresh() {
    setLoading(true);
    setError("");
    try {
      const result = await getFirmwareStatus();
      if (result.success) {
        setStatus(result);
      } else {
        setError(result.message || "Failed to fetch firmware status");
      }
    } catch (e) {
      setError(`Failed to fetch firmware status: ${e}`);
    }
    setLoading(false);
  }

  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- initial async data load on mount
    detach(refresh());
  }, []);

  const runDownload = async (download: (slug: string) => Promise<{ success: boolean; message?: string }>) => {
    setDownloading(true);
    setNote("");
    try {
      const result = await download("switch");
      setNote(result.message || (result.success ? "Done" : "Download failed"));
      if (result.success) await refresh();
    } catch (e) {
      setNote(`Download failed: ${e}`);
    }
    setDownloading(false);
  };

  const keys = status?.keys ?? [];
  const firmware = status?.firmware ?? [];
  const requiredMissing = keys.filter((k) => k.required && k.emulators.some((e) => e.state === "missing")).length;
  const onRomm = keys.filter((k) => k.on_romm).length + firmware.length;

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

      <PanelSection title="Switch firmware and keys">
        <PanelSectionRow>
          <div style={{ fontSize: "11px", color: "#8f98a0", padding: "0 16px 4px" }}>
            Keys are read from each emulator&apos;s own folder. A key file you already have is never replaced.
          </div>
        </PanelSectionRow>

        {loading && (
          <PanelSectionRow>
            <Field label="Loading firmware status..." />
          </PanelSectionRow>
        )}

        {error && (
          <PanelSectionRow>
            <Field label="Error" description={error} />
          </PanelSectionRow>
        )}

        {status?.server_offline && (
          <PanelSectionRow>
            <Field
              label="Server offline"
              description="RomM is unreachable. Showing what is on this machine; downloads are unavailable."
            />
          </PanelSectionRow>
        )}

        {status && (status.emulators?.length ?? 0) === 0 && (
          <PanelSectionRow>
            <Field label="No Switch emulator found" description="Install Eden, yuzu or Ryujinx with EmuDeck first." />
          </PanelSectionRow>
        )}

        {status && !loading && onRomm === 0 && !status.server_offline && (
          <PanelSectionRow>
            <Field label="No Switch firmware on RomM" description="Upload firmware and keys to RomM's bios/Switch folder." />
          </PanelSectionRow>
        )}

        {note && (
          <PanelSectionRow>
            <Field label={note} />
          </PanelSectionRow>
        )}
      </PanelSection>

      {status && keys.length > 0 && (
        <PanelSection title={requiredMissing > 0 ? "Keys — required keys missing" : "Keys"}>
          {keys.map((key) => (
            <PanelSectionRow key={key.file_name}>
              <Field
                label={`${key.file_name}${key.required ? "" : " (optional)"}${key.on_romm ? "" : " — not on RomM"}`}
                description={keyStateSummary(key)}
              />
            </PanelSectionRow>
          ))}
        </PanelSection>
      )}

      {status && firmware.length > 0 && (
        <PanelSection title="Firmware">
          {firmware.map((fw) => (
            <PanelSectionRow key={fw.file_name}>
              <Field
                label={`${fw.version}${fw.selected ? " (newest)" : ""}`}
                description={`${fw.size != null ? formatBytes(fw.size) : "size unknown"} — ${
                  fw.downloaded ? "downloaded" : "not downloaded"
                }`}
              />
            </PanelSectionRow>
          ))}
          <PanelSectionRow>
            <div style={{ fontSize: "11px", color: "#8f98a0", padding: "0 16px 4px" }}>
              Firmware is saved to {status.archive_dir}. Install it from Eden&apos;s or Ryujinx&apos;s own Install
              Firmware menu.
            </div>
          </PanelSectionRow>
        </PanelSection>
      )}

      {status && !status.server_offline && onRomm > 0 && (
        <PanelSection>
          <PanelSectionRow>
            <ButtonItem layout="below" disabled={downloading} onClick={() => detach(runDownload(downloadRequiredFirmware))}>
              {downloading ? "Downloading..." : "Download Required"}
            </ButtonItem>
          </PanelSectionRow>
          <PanelSectionRow>
            <ButtonItem layout="below" disabled={downloading} onClick={() => detach(runDownload(downloadAllFirmware))}>
              {downloading ? "Downloading..." : "Download All"}
            </ButtonItem>
          </PanelSectionRow>
        </PanelSection>
      )}
    </>
  );
};
