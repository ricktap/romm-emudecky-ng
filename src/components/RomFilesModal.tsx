import { FC, useEffect, useState } from "react";
import { toaster } from "@decky/api";
import { ConfirmModal, DialogButton, Focusable, ModalRoot, showModal } from "@decky/ui";
import { downloadRomFiles, getRomFiles, removeRomFiles, type RomFileInfo } from "../api/backend";
import { formatBytes } from "../utils/formatters";
import { detach } from "../utils/detach";

const say = (body: string) => toaster.toast({ title: "EmuDecky RomM Manager", body });

const STATE_LABEL: Record<RomFileInfo["state"], string> = {
  missing: "not on this device",
  changed: "changed on RomM",
  present: "on this device",
};

/** Files worth fetching: not on this device, or a different size than on RomM; never one already downloading. */
export function preselected(files: RomFileInfo[]): Set<number> {
  return new Set(files.filter((f) => f.state !== "present" && !f.downloading).map((f) => f.file_id));
}

/** Of the ticked files, those a download fetches (missing or changed) and those a removal deletes (on this device, never the base game). */
export function actionable(files: RomFileInfo[], chosen: Set<number>): { download: number[]; remove: number[] } {
  const ticked = files.filter((f) => chosen.has(f.file_id) && !f.downloading);
  return {
    download: ticked.filter((f) => f.state !== "present").map((f) => f.file_id),
    remove: ticked.filter((f) => f.state !== "missing" && !f.base).map((f) => f.file_id),
  };
}

interface Props {
  romId: number;
  romName: string;
  /** Injected by showModal; the modal stays on screen until it is called. */
  closeModal?: () => void;
}

/**
 * The files of an installed multi-file game as RomM lists them (base game, updates, DLC), to download single ones
 * without the whole game (an update or DLC added on RomM after the install, or a file that changed there) or to
 * remove ones no longer wanted from this device. Every row is a button, because the Steam UI can only move between
 * focusable elements.
 */
export const RomFilesModal: FC<Props> = ({ romId, romName, closeModal }) => {
  const [files, setFiles] = useState<RomFileInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [chosen, setChosen] = useState<Set<number>>(new Set());
  const [busy, setBusy] = useState(false);

  const load = () => {
    setError(null);
    getRomFiles(romId)
      .then((result) => {
        if (result.success && result.files) {
          setFiles(result.files);
          setChosen(preselected(result.files));
        } else {
          setError(result.message || "Could not load the game's files");
        }
      })
      .catch(() => setError("Could not load the game's files"));
  };
  useEffect(load, [romId]);

  const toggle = (id: number) =>
    setChosen((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const { download: toDownload, remove: toRemove } = actionable(files ?? [], chosen);

  const download = async () => {
    setBusy(true);
    try {
      const result = await downloadRomFiles(romId, toDownload);
      say(result.message || (result.success ? "Download started" : "Could not start the download"));
      if (result.success) {
        closeModal?.();
        return;
      }
    } catch {
      say("Could not start the download");
    }
    setBusy(false);
  };

  const remove = async (ids: number[]) => {
    setBusy(true);
    try {
      const result = await removeRomFiles(romId, ids);
      say(result.message || (result.success ? "Removed" : "Could not remove the files"));
      if (result.success) load();
    } catch {
      say("Could not remove the files");
    }
    setBusy(false);
  };

  const confirmRemove = () => {
    const ids = toRemove;
    showModal(
      <ConfirmModal
        strTitle={ids.length === 1 ? "Remove 1 file from this device?" : `Remove ${ids.length} files from this device?`}
        strDescription="RomM keeps them; you can download them again here."
        strOKButtonText="Remove"
        strCancelButtonText="Cancel"
        onOK={() => detach(remove(ids))}
      />,
    );
  };

  return (
    <ModalRoot closeModal={closeModal}>
      <div style={{ padding: "16px", minWidth: "420px" }}>
        <div style={{ fontSize: "16px", fontWeight: "bold", marginBottom: "4px" }}>Game files</div>
        <div style={{ marginBottom: "10px", opacity: 0.7 }}>{romName}</div>
        {error && <DialogButton onClick={load}>{`${error}. Try again`}</DialogButton>}
        {!files && !error && <div>Loading...</div>}
        {files && files.length === 0 && <div style={{ margin: "8px 0", opacity: 0.8 }}>RomM lists no files for this game.</div>}
        {files && (
          <Focusable flow-children="down" style={{ display: "flex", flexDirection: "column", gap: "6px" }}>
            {files.map((f) => {
              const on = chosen.has(f.file_id);
              const status = (f.downloading ? "downloading" : STATE_LABEL[f.state]) + (f.base ? " \u00b7 base game" : "");
              return (
                <DialogButton
                  key={f.file_id}
                  disabled={f.downloading || busy}
                  onClick={() => toggle(f.file_id)}
                  style={{ textAlign: "left", opacity: f.state === "present" && !on ? 0.7 : 1 }}
                >
                  {`${on ? "☑" : "☐"} ${f.path} · ${formatBytes(f.size)} · ${status}`}
                </DialogButton>
              );
            })}
          </Focusable>
        )}
        <Focusable flow-children="right" style={{ display: "flex", gap: "8px", marginTop: "12px" }}>
          <DialogButton disabled={busy || toDownload.length === 0} onClick={() => detach(download())}>
            {`Download selected (${toDownload.length})`}
          </DialogButton>
          <DialogButton disabled={busy || toRemove.length === 0} onClick={confirmRemove}>
            {`Remove selected (${toRemove.length})`}
          </DialogButton>
          <DialogButton onClick={() => closeModal?.()}>Close</DialogButton>
        </Focusable>
      </div>
    </ModalRoot>
  );
};

export function showRomFilesModal(romId: number, romName: string): void {
  showModal(<RomFilesModal romId={romId} romName={romName} />);
}
