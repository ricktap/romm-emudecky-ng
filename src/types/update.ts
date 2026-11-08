/**
 * Self-update check result (`check_for_update` callable).
 *
 * Success carries everything Decky's installer needs (`asset_url`,
 * `plugin_name`, `latest_version`, `sha256`) plus the notes and release page.
 * Failure uses the canonical `{success: false, reason, message}` shape and
 * still reports the installed version.
 */
export interface UpdateCheckSuccess {
  success: true;
  current_version: string;
  latest_version: string | null;
  update_available: boolean;
  notes: string;
  asset_url: string;
  sha256: string;
  html_url: string;
  /** `plugin.json` "name" — Decky matches the installed plugin by it. */
  plugin_name: string;
  checked_at: string;
}

export interface UpdateCheckFailure {
  success: false;
  reason: "server_unreachable" | "rate_limited" | "unknown" | string;
  message: string;
  current_version: string;
  checked_at: string;
}

export type UpdateCheckResult = UpdateCheckSuccess | UpdateCheckFailure;
