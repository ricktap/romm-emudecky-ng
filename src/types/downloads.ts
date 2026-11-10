/**
 * Per-ROM download types — queue entries and progress/completion events
 * for individual ROM file downloads triggered from the UI.
 */

/**
 * Queue status of one ROM download. `paused` (user) and `interrupted` (a
 * transient failure — server unreachable, sign-in, full disk — or a plugin
 * restart) are both stopped-but-unfinished: the partial transfer is kept on
 * disk and the download can be resumed. Only `interrupted` downloads are
 * auto-resumed when the plugin starts.
 */
export type DownloadStatus =
  "queued" | "downloading" | "extracting" | "completed" | "failed" | "cancelled" | "paused" | "interrupted";

export interface DownloadItem {
  rom_id: number;
  rom_name: string;
  platform_name: string;
  file_name: string;
  status: DownloadStatus;
  progress: number;
  bytes_downloaded: number;
  total_bytes: number;
  /**
   * Whether the server honoured the HTTP Range probe for this transfer, so a
   * paused download continues from its partial instead of restarting. Set live
   * once the response headers arrive (single-file and multi-file ZIP alike);
   * false behind Cloudflare or a server that ignores Range. The frontend offers
   * Pause only when this is true; Resume of a stopped download is always offered.
   */
  resumable: boolean;
  /**
   * Why an `interrupted` download stopped: `server_unreachable`, `timeout`, `auth_failed`, `disk_full`,
   * `restart`, or `needs_recovery` (a partial that cannot be verified; Resume then restarts it).
   */
  reason?: string;
  error?: string;
  /**
   * Live transfer phase while `downloading` and no byte has arrived: `waiting` (request out, no response
   * headers yet — for a multipart ROM RomM may be preparing the archive) or `retrying` (backing off).
   * Absent while bytes flow.
   */
  phase?: DownloadPhase;
  /** Epoch seconds when the phase began; the UI counts elapsed time locally from it. */
  phase_since?: number;
  attempt?: number;
  max_attempts?: number;
  /** Back-off before the next attempt, in seconds (`retrying` only). */
  retry_delay_s?: number;
}

export type DownloadPhase = "waiting" | "retrying";

export interface DownloadProgressEvent {
  rom_id: number;
  rom_name: string;
  platform_name: string;
  file_name: string;
  status: string;
  progress: number;
  bytes_downloaded: number;
  total_bytes: number;
  /** Server's Range-support verdict; carried live once response headers arrive. */
  resumable?: boolean;
  /** Present on `interrupted` frames: why the download stopped. */
  reason?: string;
  /** Live phase fields, present only while waiting for the server (see `DownloadItem`). */
  phase?: DownloadPhase;
  phase_since?: number;
  attempt?: number;
  max_attempts?: number;
  retry_delay_s?: number;
}

export interface DownloadCompleteEvent {
  rom_id: number;
  rom_name: string;
  platform_name: string;
  file_path: string;
  /**
   * Bound Steam `app_id` for this ROM, or `null` when the ROM isn't synced
   * yet (no shortcut). Resolved on the backend so the handler confirm-sets
   * launch options on the exact shortcut without a full-library scan.
   */
  app_id: number | null;
  /** Full launch command for the just-downloaded ROM (now installed/launchable). */
  launch_options: string;
  /** Whether the just-finished transfer was resumable (carried for store parity). */
  resumable?: boolean;
  /**
   * `true` when no download ran: the ROM's files were already on disk exactly
   * where a download would put them (RomM file name + size) and were adopted as
   * installed. Launch options are applied the same way; the download queue and
   * the "Downloaded" toast are skipped.
   */
  adopted?: boolean;
}

export interface DownloadFailedEvent {
  rom_id: number;
  rom_name: string;
  platform_name: string;
  error_message: string;
}
