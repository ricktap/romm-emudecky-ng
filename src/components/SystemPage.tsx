import { useState, useEffect, FC, createElement } from "react";
import { ConfirmModal, showModal } from "@decky/ui";
import {
  getFirmwareStatus,
  getPlatforms,
  savePlatformSync,
  downloadAllFirmware,
  downloadRequiredFirmware,
  saveFirmwareVersion,
  replaceFirmwareKey,
} from "../api/backend";
import type { FirmwareStatus, KeyEmulatorState, KeyState, PlatformSyncSetting, SwitchKeyFile } from "../types";
import { RmButton, RmCard, RmChoice, RmRow, RmStyles, RmToggle, RmValueRow } from "./rm";
import { formatBytes } from "../utils/formatters";
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
  onBack?: () => void;
}

const games = (n: number) => `${n} game${n === 1 ? "" : "s"}`;
const isSwitch = (p: PlatformSyncSetting) => p.slug === "switch" || p.slug === "nintendo_switch";

/**
 * The Systems & BIOS tab: one table row per platform (sync switch, BIOS state, emulator), and below it the Switch
 * firmware and keys RomM hosts — the key files with their state in every installed emulator, and the firmware
 * versions. The buttons put downloads in the shared download queue (progress, pause and cancel are in the Downloads
 * tab). Downloads only fill gaps: an existing key file that differs from RomM's is reported and kept, never replaced.
 */
export const SystemPage: FC<SystemPageProps> = () => {
  const [status, setStatus] = useState<FirmwareStatus | null>(null);
  const [platforms, setPlatforms] = useState<PlatformSyncSetting[]>([]);
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
    getPlatforms()
      .then((r) => r?.success && setPlatforms(r.platforms))
      .catch(() => undefined);
    // A finished firmware download (announced by the global listener) changes what is on disk.
    const onChanged = (e: Event) => {
      if ((e as CustomEvent<{ type?: string }>).detail?.type === "firmware") detach(refresh());
    };
    globalThis.addEventListener("romm_data_changed", onChanged);
    return () => globalThis.removeEventListener("romm_data_changed", onChanged);
  }, []);

  const toggleSync = async (platform: PlatformSyncSetting, enabled: boolean) => {
    setPlatforms((all) => all.map((p) => (p.id === platform.id ? { ...p, sync_enabled: enabled } : p)));
    try {
      const result = await savePlatformSync(platform.id, enabled);
      if (!result?.success) throw new Error(result?.message);
      setNote("Changes apply on the next sync");
    } catch {
      setPlatforms((all) => all.map((p) => (p.id === platform.id ? { ...p, sync_enabled: !enabled } : p)));
      setNote("Could not save the platform setting");
    }
  };

  const chooseVersion = async (version: string) => {
    setNote("");
    try {
      const result = await saveFirmwareVersion(version);
      if (!result.success) setNote(result.message || "Could not save the firmware version");
      await refresh();
    } catch (e) {
      setNote(`Could not save the firmware version: ${e}`);
    }
  };

  const runDownload = async (
    download: (slug: string) => Promise<{ success: boolean; message?: string; queued?: number }>,
  ) => {
    setDownloading(true);
    setNote("");
    try {
      const result = await download("switch");
      const text = result.message || (result.success ? "Done" : "Download failed");
      // The transfers run in the Downloads list; point there instead of implying they are done.
      setNote(result.queued ? `${text} — progress is in Downloads` : text);
    } catch (e) {
      setNote(`Download failed: ${e}`);
    }
    setDownloading(false);
  };

  // Replacing is explicit, per file and emulator, and asks first: the user's own file is kept as <name>.romm-backup.
  const askReplace = (key: SwitchKeyFile, emulator: KeyEmulatorState) => {
    if (key.firmware_id == null) return;
    const firmwareId = key.firmware_id;
    showModal(
      createElement(ConfirmModal, {
        strTitle: `Replace ${emulator.label}'s ${key.file_name}?`,
        strDescription: `${emulator.label}'s ${key.file_name} differs from the one on RomM. RomM's file is downloaded and verified first, and your current file is kept next to it as ${key.file_name}.romm-backup.`,
        strOKButtonText: "Replace",
        strCancelButtonText: "Keep mine",
        onOK: () => {
          detach(
            (async () => {
              setNote("");
              try {
                const result = await replaceFirmwareKey(firmwareId, emulator.id);
                setNote(result.message || (result.success ? "Replaced" : "Could not replace the key"));
                if (result.success) await refresh();
              } catch (e) {
                setNote(`Could not replace the key: ${e}`);
              }
            })(),
          );
        },
      }),
    );
  };

  const keys = status?.keys ?? [];
  const firmware = status?.firmware ?? [];
  const emulators = status?.emulators ?? [];
  const requiredMissing = keys.filter((k) => k.required && k.emulators.some((e) => e.state === "missing")).length;
  const onRomm = keys.filter((k) => k.on_romm).length + firmware.length;
  const offline = Boolean(status?.server_offline);
  const canDownload = status !== null && !offline && onRomm > 0;

  // The Switch row's BIOS cell: a colour dot, one line of state and the first file name.
  const switchBios = ((): { dot: string; text: string; file: string } => {
    if (!status) return { dot: "rm-grey", text: loading ? "Checking…" : "Unknown", file: "" };
    if (emulators.length === 0) return { dot: "rm-grey", text: "No Switch emulator found", file: "" };
    if (requiredMissing > 0) return { dot: "rm-red", text: "Missing required keys", file: keys.find((k) => k.required)?.file_name ?? "" };
    return { dot: "", text: "Ready", file: keys[0]?.file_name ?? "" };
  })();

  const rows: { key: string; platform: PlatformSyncSetting | null; name: string; sub: string; isSwitchRow: boolean }[] =
    platforms.filter((p) => !isSwitch(p)).map((p) => ({ key: p.slug, platform: p, name: p.name, sub: games(p.rom_count), isSwitchRow: false }));
  const switchPlatform = platforms.find(isSwitch) ?? null;
  rows.push({
    key: "switch",
    platform: switchPlatform,
    name: "Nintendo Switch",
    sub: switchPlatform ? `${games(switchPlatform.rom_count)} · firmware and keys` : "Firmware and keys",
    isSwitchRow: true,
  });

  return (
    <div className="rm-root" style={{ background: "transparent" }}>
      <RmStyles />
      {loading && !status && <div className="rm-muted" style={{ padding: "8px 0" }}>Loading firmware status...</div>}
      {error && <RmValueRow label="Error" sub={error} />}
      {offline && <RmValueRow label="Server offline" sub="RomM is unreachable. Showing what is on this machine; downloads are unavailable." />}
      {status && !loading && onRomm === 0 && !offline && (
        <RmValueRow label="No Switch firmware on RomM" sub="Upload firmware and keys to RomM's bios/Switch folder." />
      )}

      <table className="rm-tbl" data-testid="systems-table">
        <thead>
          <tr>
            <th>Platform</th>
            <th>Sync</th>
            <th>BIOS</th>
            <th>Emulator</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr key={r.key} data-testid={`system-${r.key}`}>
              <td>
                <div style={{ fontWeight: 600, color: "#fff" }}>{r.name}</div>
                <div className="rm-muted rm-small">{r.sub}</div>
              </td>
              <td>
                {r.platform && (
                  <RmToggle
                    label={`Sync ${r.name}`}
                    checked={r.platform.sync_enabled}
                    onChange={(value) => detach(toggleSync(r.platform!, value))}
                  />
                )}
              </td>
              <td>
                {r.isSwitchRow ? (
                  <>
                    <span style={{ display: "inline-flex", alignItems: "center", gap: 8 }}>
                      <span className={`rm-dot ${switchBios.dot}`} />
                      {switchBios.text}
                    </span>
                    <div className="rm-muted rm-small" style={{ marginLeft: 16 }}>{switchBios.file}</div>
                  </>
                ) : (
                  <span style={{ display: "inline-flex", alignItems: "center", gap: 8 }}>
                    <span className="rm-dot rm-grey" />
                    No BIOS check yet
                  </span>
                )}
              </td>
              <td className="rm-muted">
                {r.isSwitchRow
                  ? emulators.length > 0
                    ? emulators.map((e) => e.label).join(", ")
                    : status
                      ? "Install Eden, yuzu or Ryujinx with EmuDeck first"
                      : ""
                  : ""}
              </td>
              <td style={{ textAlign: "right" }}>
                {r.isSwitchRow && canDownload && requiredMissing > 0 && (
                  <RmButton variant="sm primary" testId="download-required" disabled={downloading} onActivate={() => detach(runDownload(downloadRequiredFirmware))}>
                    {downloading ? "Downloading..." : "Download BIOS"}
                  </RmButton>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      {note && <div className="rm-muted" style={{ marginTop: 14 }} data-testid="systems-note">{note}</div>}

      {status && keys.length > 0 && (
        <div style={{ marginTop: 18, maxWidth: 640 }}>
          <RmCard title={requiredMissing > 0 ? "Switch keys — required keys missing" : "Switch keys"}>
            {keys.map((key) => (
              <div key={key.file_name}>
                <RmValueRow
                  label={`${key.file_name}${key.required ? "" : " (optional)"}${key.on_romm ? "" : " — not on RomM"}`}
                  sub={keyStateSummary(key)}
                />
                {key.on_romm &&
                  !offline &&
                  key.emulators
                    .filter((e) => e.state === "differs")
                    .map((e) => (
                      <RmRow key={`${key.file_name}-${e.id}`} label={`${e.label}'s ${key.file_name} differs`}>
                        <RmButton variant="sm ghost" testId={`replace-${key.file_name}-${e.id}`} onActivate={() => askReplace(key, e)}>
                          Replace with RomM's
                        </RmButton>
                      </RmRow>
                    ))}
              </div>
            ))}
          </RmCard>
        </div>
      )}

      {status && firmware.length > 0 && (
        <div style={{ marginTop: 18, maxWidth: 640 }}>
          <RmCard title="Switch firmware">
            <RmRow label="Firmware version" sub="The version Download Required fetches">
              <RmChoice
                label="Firmware version"
                value={status.version_pinned ? (status.selected_version ?? "") : ""}
                options={[
                  { data: "", label: `Newest available (${firmware[0]?.version ?? ""})` },
                  ...firmware.map((fw) => ({ data: fw.version, label: fw.version })),
                ]}
                onChange={(version) => detach(chooseVersion(version))}
              />
            </RmRow>
            {firmware.map((fw) => (
              <RmValueRow
                key={fw.file_name}
                label={`${fw.version}${fw.selected ? " (selected)" : ""}`}
                value={`${fw.size != null ? formatBytes(fw.size) : "size unknown"} — ${fw.downloaded ? "downloaded" : "not downloaded"}`}
              />
            ))}
            <RmValueRow
              label="Where it is saved"
              sub={`${status.archive_dir ?? ""} — install it from Eden's or Ryujinx's own Install Firmware menu`}
            />
            {canDownload && (
              <RmRow label="Download everything RomM has">
                <RmButton variant="sm ghost" testId="download-all" disabled={downloading} onActivate={() => detach(runDownload(downloadAllFirmware))}>
                  Download all
                </RmButton>
              </RmRow>
            )}
          </RmCard>
        </div>
      )}

      <p className="rm-muted" style={{ marginTop: 18 }}>
        Keys are read from each emulator&apos;s own folder. A key file you already have is never replaced.
      </p>
    </div>
  );
};
