import { FC, useState } from "react";
import { AttRow, RmButton, RmButtonGroup } from "./rm";
import { openLibraryRoute } from "../utils/libraryRoute";
import type { UpdateCheckSuccess } from "../types";
import { startSelfUpdate, openReleasePage } from "../utils/selfUpdate";

/** Notes longer than this start collapsed behind "Show release notes". */
const SHORT_NOTES_CHARS = 160;

export function updateTitle(info: UpdateCheckSuccess): string {
  return `Update available: v${info.latest_version}`;
}

export function updateFallbackMessage(info: UpdateCheckSuccess): string {
  return `Couldn't start the update — download v${info.latest_version}`;
}

interface UpdateNoticeProps {
  info: UpdateCheckSuccess;
}

/**
 * The update rows (title, notes, Update button, manual-download fallback).
 * Rendered inside the QAM banner and the Settings "Updates" section. The
 * notice always shows; only the install path can fall back.
 */
export const UpdateNotice: FC<UpdateNoticeProps> = ({ info }) => {
  const [showNotes, setShowNotes] = useState(false);
  const [starting, setStarting] = useState(false);
  const [failed, setFailed] = useState(false);

  const notes = info.notes.trim();
  const longNotes = notes.length > SHORT_NOTES_CHARS;

  const handleUpdate = async () => {
    setStarting(true);
    const ok = await startSelfUpdate(info);
    setStarting(false);
    setFailed(!ok);
  };

  return (
    <div className="rm-card" data-testid="update-notice-card">
      <h3>{updateTitle(info)}</h3>
      <div className="rm-muted">Installed: v{info.current_version}</div>
      {notes && (!longNotes || showNotes) && (
        <div data-testid="update-notes" className="rm-notes" style={{ marginTop: 6 }}>
          {notes}
        </div>
      )}
      {failed && (
        <div style={{ marginTop: 6, fontSize: 12, color: "#d4a72c" }}>{updateFallbackMessage(info)}</div>
      )}
      <RmButtonGroup style={{ marginTop: 10, flexWrap: "wrap" }}>
        {longNotes && (
          <RmButton variant="sm ghost" onActivate={() => setShowNotes(!showNotes)}>
            {showNotes ? "Hide release notes" : "Show release notes"}
          </RmButton>
        )}
        <RmButton
          variant="sm primary"
          disabled={starting}
          onActivate={() => {
            handleUpdate().catch(() => setFailed(true));
          }}
        >
          {starting ? "Starting update…" : "Update"}
        </RmButton>
        {failed && (
          <RmButton variant="sm ghost" onActivate={() => openReleasePage(info.html_url)}>
            Download v{info.latest_version}
          </RmButton>
        )}
      </RmButtonGroup>
    </div>
  );
};

/** QAM attention row shown while a newer release is available; it opens the Plugin card in Settings, which has the Update button. */
export const UpdateBanner: FC<UpdateNoticeProps> = ({ info }) => (
  <AttRow
    title={updateTitle(info)}
    sub={`Installed v${info.current_version} · open Settings to update`}
    onActivate={() => openLibraryRoute("settings")}
  />
);
