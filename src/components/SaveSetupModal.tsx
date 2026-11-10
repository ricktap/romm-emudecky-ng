import { FC, useEffect, useState } from "react";
import { toaster } from "@decky/api";
import { DialogButton, ModalRoot, showModal } from "@decky/ui";
import { getSaveTitleInfo, setSaveProfile, setSaveTitleId } from "../api/backend";
import { formatBytes, formatTimeAgo } from "../utils/formatters";
import { detach } from "../utils/detach";
import type { SaveTitleInfo } from "../types";

const say = (body: string) => toaster.toast({ title: "EmuDecky RomM Manager", body });

interface Props {
  romId: number;
  onDone: (changed: boolean) => void;
  close?: () => void;
}

/**
 * Tells the plugin which save folder belongs to a game (and, with several emulator profiles, which profile): a
 * save is found by title ID, and the plugin never guesses one. Every row is a button, because the Steam UI can only
 * move between focusable elements.
 */
const SaveSetupModal: FC<Props> = ({ romId, onDone, close }) => {
  const [info, setInfo] = useState<SaveTitleInfo | null>(null);
  const [changed, setChanged] = useState(false);
  const [failed, setFailed] = useState(false);

  const load = () => {
    getSaveTitleInfo(romId)
      .then((i) => setInfo(i))
      .catch(() => setFailed(true));
  };
  useEffect(load, [romId]);

  const finish = () => {
    onDone(changed);
    close?.();
  };

  const choose = async (title: string, source: "manual" | "learned") => {
    try {
      const result = await setSaveTitleId(romId, title, source);
      if (result.success) {
        setChanged(true);
        say(`Save folder set: ${title}`);
        load();
      } else {
        say(result.message || "Could not set the save folder");
      }
    } catch {
      say("Could not set the save folder");
    }
  };

  const chooseProfile = async (profile: string) => {
    try {
      const result = await setSaveProfile(romId, profile);
      if (result.success) {
        setChanged(true);
        say("Profile set");
        load();
      } else {
        say(result.message || "Could not set the profile");
      }
    } catch {
      say("Could not set the profile");
    }
  };

  const needsProfile = info != null && info.profiles.length > 1 && info.profile === null;

  return (
    <ModalRoot closeModal={finish}>
      <div style={{ padding: "16px", minWidth: "360px" }}>
        <div style={{ fontSize: "16px", fontWeight: "bold", marginBottom: "8px" }}>Choose the save</div>
        {failed && <DialogButton onClick={load}>Could not load. Try again</DialogButton>}
        {!info && !failed && <div>Loading...</div>}
        {info && (
          <>
            {info.title_id && <div style={{ marginBottom: "8px", opacity: 0.8 }}>Save folder: {info.title_id}</div>}
            {needsProfile && (
              <>
                <div style={{ margin: "8px 0 4px", opacity: 0.8 }}>Whose saves are these?</div>
                {info.profiles.map((p) => (
                  <DialogButton key={p.id} style={{ marginBottom: "6px" }} onClick={() => detach(chooseProfile(p.id))}>
                    {p.name || p.id}
                  </DialogButton>
                ))}
              </>
            )}
            {info.proposal && (
              <DialogButton style={{ marginBottom: "6px" }} onClick={() => detach(choose(info.proposal as string, "learned"))}>
                Use {info.proposal} (saved while you played)
              </DialogButton>
            )}
            {!info.title_id && info.candidates.length === 0 && (
              <div style={{ margin: "8px 0", opacity: 0.8 }}>No save folders yet. Play the game once, then come back.</div>
            )}
            {info.candidates.map((c) => (
              <DialogButton key={c.title_id} style={{ marginBottom: "6px" }} onClick={() => detach(choose(c.title_id, "manual"))}>
                {c.title_id} · {c.files} files · {formatBytes(c.bytes)} · {formatTimeAgo(new Date(c.newest * 1000).toISOString()) ?? ""}
              </DialogButton>
            ))}
          </>
        )}
        <DialogButton style={{ marginTop: "8px" }} onClick={finish}>
          Done
        </DialogButton>
      </div>
    </ModalRoot>
  );
};

/** Open the chooser; resolves `true` when something was set. */
export function showSaveSetupModal(romId: number): Promise<boolean> {
  return new Promise((resolve) => {
    showModal(<SaveSetupModal romId={romId} onDone={resolve} />);
  });
}

export { SaveSetupModal };
