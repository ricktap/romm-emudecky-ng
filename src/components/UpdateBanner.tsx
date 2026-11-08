import { FC, useState } from "react";
import { PanelSection, PanelSectionRow, ButtonItem } from "@decky/ui";
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
    <>
      <PanelSectionRow>
        <div
          style={{
            padding: "8px 12px",
            backgroundColor: "rgba(26, 159, 255, 0.15)",
            borderLeft: "3px solid #1a9fff",
            borderRadius: "4px",
          }}
        >
          <div style={{ fontSize: "13px", fontWeight: "bold", color: "#1a9fff", marginBottom: "4px" }}>
            {updateTitle(info)}
          </div>
          <div style={{ fontSize: "12px", color: "rgba(255, 255, 255, 0.7)" }}>
            Installed: v{info.current_version}
          </div>
          {notes && (!longNotes || showNotes) && (
            <div
              data-testid="update-notes"
              style={{
                marginTop: "6px",
                fontSize: "12px",
                color: "rgba(255, 255, 255, 0.85)",
                lineHeight: 1.4,
                whiteSpace: "pre-wrap",
                maxHeight: "220px",
                overflowY: "auto",
              }}
            >
              {notes}
            </div>
          )}
          {failed && (
            <div style={{ marginTop: "6px", fontSize: "12px", color: "#d4a72c" }}>{updateFallbackMessage(info)}</div>
          )}
        </div>
      </PanelSectionRow>
      {longNotes && (
        <PanelSectionRow>
          <ButtonItem layout="below" onClick={() => setShowNotes(!showNotes)}>
            {showNotes ? "Hide release notes" : "Show release notes"}
          </ButtonItem>
        </PanelSectionRow>
      )}
      <PanelSectionRow>
        <ButtonItem
          layout="below"
          disabled={starting}
          onClick={() => {
            handleUpdate().catch(() => setFailed(true));
          }}
        >
          {starting ? "Starting update…" : "Update"}
        </ButtonItem>
      </PanelSectionRow>
      {failed && (
        <PanelSectionRow>
          <ButtonItem layout="below" onClick={() => openReleasePage(info.html_url)}>
            Download v{info.latest_version}
          </ButtonItem>
        </PanelSectionRow>
      )}
    </>
  );
};

/** QAM main-page banner shown while a newer release is available. */
export const UpdateBanner: FC<UpdateNoticeProps> = ({ info }) => (
  <PanelSection title="Update">
    <UpdateNotice info={info} />
  </PanelSection>
);
