/**
 * Pure helpers for reading a download's queue status in the UI.
 *
 * `paused` and `interrupted` are both "stopped but unfinished": the partial
 * transfer is kept, the download stays in the active section, and Resume is
 * offered. `interrupted` additionally carries a short reason slug that is
 * rendered as a human label.
 */

import type { DownloadStatus } from "../types";

/** Statuses of a download that is not finished (shown in the active section). */
export function isUnfinishedStatus(status: string): boolean {
  return (
    status === "queued" ||
    status === "downloading" ||
    status === "extracting" ||
    status === "paused" ||
    status === "interrupted"
  );
}

/** True for a stopped-but-resumable download (user pause or interruption). */
export function isStoppedStatus(status: string): status is Extract<DownloadStatus, "paused" | "interrupted"> {
  return status === "paused" || status === "interrupted";
}

const REASON_LABELS: Record<string, string> = {
  server_unreachable: "server unreachable",
  timeout: "server did not respond",
  needs_recovery: "restart needed",
  auth_failed: "sign-in required",
  disk_full: "disk full",
  restart: "plugin restarted",
};

/** Short label for an interrupted download, e.g. "Interrupted — disk full". */
export function interruptedLabel(reason?: string | null): string {
  const detail = reason ? REASON_LABELS[reason] : undefined;
  return detail ? `Interrupted — ${detail}` : "Interrupted";
}

/** Caption suffix for a download that is stopped or still waiting for the transfer slot ("" otherwise). */
export function stoppedCaptionSuffix(status: string, reason?: string | null): string {
  if (status === "queued") return " — In Queue";
  if (status === "paused") return " — Paused";
  if (status === "interrupted") return ` — ${interruptedLabel(reason)}`;
  return "";
}

/**
 * Label of the Resume control. A partial that cannot be verified is never continued: the user's
 * Resume is an explicit restart of that download, so the button says so.
 */
export function resumeActionLabel(reason?: string | null): "Resume" | "Restart" {
  return reason === "needs_recovery" ? "Restart" : "Resume";
}
