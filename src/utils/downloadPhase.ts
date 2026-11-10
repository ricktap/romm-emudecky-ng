/**
 * Wording and timing for a download that is waiting on the server.
 *
 * Before the response headers arrive there is no byte progress to show. For a multipart ROM the wait may be
 * RomM preparing a large archive, but silence alone does not prove that, so the text says "may be". Elapsed
 * time is counted locally from the backend's `phase_since`; nothing is sent to the server to refresh it.
 */

import { useEffect, useState } from "react";
import type { DownloadItem } from "../types";

/** The part of a download item that carries a live phase. */
export type PhaseFields = Pick<DownloadItem, "phase" | "phase_since" | "attempt" | "max_attempts" | "retry_delay_s">;

export const WAITING_TEXT = "Waiting for RomM to respond. A download archive may be preparing…";

/** `45s`, `2m 14s`, `1h 02m`. */
export function formatElapsed(totalSeconds: number): string {
  const s = Math.max(0, Math.floor(totalSeconds));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${String(s % 60).padStart(2, "0")}s`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, "0")}m`;
}

/** Seconds since the phase began (0 when unknown). */
export function phaseElapsedSeconds(item: PhaseFields, nowMs: number): number {
  if (item.phase_since === undefined) return 0;
  return Math.max(0, Math.floor(nowMs / 1000 - item.phase_since));
}

/** `attempt 2 of 3` when this is a retry, "" for the first attempt. */
export function attemptText(item: PhaseFields): string {
  return item.attempt !== undefined && item.attempt > 1 && item.max_attempts
    ? `attempt ${item.attempt} of ${item.max_attempts}`
    : "";
}

/** Seconds left of a back-off pause (never negative). */
export function retryRemainingSeconds(item: PhaseFields, nowMs: number): number {
  return Math.max(0, Math.ceil((item.retry_delay_s ?? 0) - phaseElapsedSeconds(item, nowMs)));
}

/** Full text for the download queue: one or two lines. */
export function phaseLines(item: PhaseFields, nowMs: number): string[] {
  if (item.phase === "retrying") {
    const next = `${attemptText(item) || "next attempt"} in ${retryRemainingSeconds(item, nowMs)}s`;
    return ["Connection problem — retrying", next.charAt(0).toUpperCase() + next.slice(1)];
  }
  const lines = [WAITING_TEXT, `Elapsed: ${formatElapsed(phaseElapsedSeconds(item, nowMs))}`];
  const attempt = attemptText(item);
  if (attempt) lines[1] += ` · ${attempt}`;
  return lines;
}

/** Short label for the narrow game-page button. */
export function phaseButtonLabel(item: PhaseFields, nowMs: number): string {
  if (item.phase === "retrying") {
    return `Retrying (${item.attempt ?? 2}/${item.max_attempts ?? 3}) in ${retryRemainingSeconds(item, nowMs)}s`;
  }
  return `Waiting for RomM… ${formatElapsed(phaseElapsedSeconds(item, nowMs))}`;
}

/** The current time in ms, refreshed every second while `active` (local only, no backend calls). */
export function useNowMs(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- sync the clock when the phase starts
    setNow(Date.now());
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [active]);
  return now;
}
