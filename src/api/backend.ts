import { callable } from "@decky/api";
import { unportedCallable } from "./unportedCallable";
import { detach } from "../utils/detach";
import type {
  UpdateCheckResult,
  PluginSettings,
  SyncStats,
  SyncProgress,
  DownloadItem,
  InstalledRom,
  LibraryOverview,
  PlatformSyncSetting,
  CollectionSyncSetting,
  CollectionKind,
  RegistryPlatform,
  FirmwareStatus,
  DownloadKind,
  FirmwareDownloadResult,
  CoreInfo,
  RomMetadata,
  SaveSyncSettings,
  SaveStatus,
  SaveSyncDisplay,
  SyncConflict,
  RommErrorCode,
  SyncPreview,
  SaveSlotSummary,
  SaveSetupInfo,
  SlotSavesResponse,
  SwitchSlotResponse,
  LaunchVerdict,
  SlotDeleteInfo,
  DeleteSlotResult,
  RollbackStatus,
  ListFileVersionsResult,
  ListDevicesResponse,
} from "../types";

export interface BackendResult {
  success: boolean;
  message: string;
  reason?: RommErrorCode;
  romm_version?: string;
  /** Set when a callable was rejected because a RetroDECK migration is pending. */
  blocked_by_migration?: boolean;
}

export interface CachedGameDetail {
  found: boolean;
  rom_id?: number;
  rom_name?: string;
  platform_slug?: string;
  platform_name?: string;
  installed?: boolean;
  save_sync_enabled?: boolean;
  save_status?: {
    files: Array<{ filename: string; status: string; last_sync_at?: string }>;
    last_sync_check_at?: string;
    conflicts?: SyncConflict[];
  } | null;

  metadata?: Record<string, unknown> | null;
  rom_file?: string;
  save_sync_display?: SaveSyncDisplay | null;
  stale_fields?: string[];
  // Version metadata (ADR-0021) — the RomM sibling-group dimensions of the
  // active version, rendered read-only as the Region/Languages rows in
  // RomMGameInfoPanel's GAME INFO section (the version switcher is separate).
  regions?: string[];
  languages?: string[];
  revision?: string;
  tags?: string[];
  is_main_sibling?: boolean;
}

// get_cached_game_detail wiring lives in utils/cachedGameDetailStore.ts so the
// module-scope cache + invalidation surface is in one place. Re-exported here
// for back-compat with existing import sites.
export { getCachedGameDetail, invalidateCachedGameDetail } from "../utils/cachedGameDetailStore";
export const getSettings = callable<[], PluginSettings>("get_settings");
export const saveServerUrl = callable<[string, boolean], BackendResult>("save_server_url");
export const connectWithCredentials = callable<[string, string, string, boolean], BackendResult>(
  "connect_with_credentials",
);
export const connectWithToken = callable<[string, string, boolean], BackendResult>("connect_with_token");
export const connectWithPairingCode = callable<[string, string, boolean], BackendResult>("connect_with_pairing_code");
export const signOut = callable<[], BackendResult>("sign_out");

export interface WhitelistSettings {
  disabled_defaults: string[];
  custom_names: string[];
}
export const getWhitelistSettings = callable<[], WhitelistSettings>("get_whitelist_settings");
export const updateWhitelistSettings = callable<[string[], string[]], { success: boolean; message?: string }>(
  "update_whitelist_settings",
);

export const testConnection = callable<[], BackendResult>("test_connection");
export const startSync = callable<[], BackendResult>("start_sync");
export const cancelSync = callable<[string], BackendResult>("cancel_sync");
export const syncHeartbeat = callable<[], { success: boolean }>("sync_heartbeat");
export const syncPreview = callable<[], SyncPreview>("sync_preview");
export const syncApplyDelta = callable<[string], BackendResult>("sync_apply_delta");
export const syncCancelPreview = callable<[], BackendResult>("sync_cancel_preview");
export const getSyncStatus = callable<[], SyncProgress>("get_sync_status");
export const clearSyncCache = callable<[], BackendResult>("clear_sync_cache");
export const getSyncStats = callable<[], SyncStats>("get_sync_stats");
// The optional second argument is the download kind ("rom" by default); firmware downloads pass "firmware".
export const startDownload = callable<[number, DownloadKind?], BackendResult>("start_download");
export const cancelDownload = callable<[number, DownloadKind?], BackendResult>("cancel_download");
export const pauseDownload = callable<[number, DownloadKind?], BackendResult>("pause_download");
export const resumeDownload = callable<[number, DownloadKind?], BackendResult>("resume_download");
export const getDownloadQueue = callable<[], { downloads: DownloadItem[] }>("get_download_queue");
export const getInstalledRom = callable<[number], InstalledRom | null>("get_installed_rom");
export const evaluateLaunch = callable<[number], LaunchVerdict>("evaluate_launch");
export const checkLocalDrift = callable<[number], { drifted: boolean; rom_id: number }>("check_local_drift");
export const getRomRelaunchOptions = callable<[number], { app_id: number; launch_options: string } | null>(
  "get_rom_relaunch_options",
);
export const probeReachability = callable<[], { online: boolean }>("probe_reachability");
export const refreshSaveStatus = unportedCallable<[number], { success: boolean }>("refresh_save_status");
export const removeRom = callable<[number], BackendResult>("remove_rom");
export const getPlatforms = callable<[], { success: boolean; platforms: PlatformSyncSetting[] }>("get_platforms");
export const savePlatformSync = callable<[number, boolean], { success: boolean; message: string }>(
  "save_platform_sync",
);
export const setAllPlatformsSync = callable<[boolean], { success: boolean; message: string }>("set_all_platforms_sync");
export const getCollections = callable<
  [],
  { success: boolean; collections: CollectionSyncSetting[]; message?: string; reason?: RommErrorCode }
>("get_collections");
export const saveCollectionSync = callable<[string, CollectionKind, boolean], { success: boolean; message?: string }>(
  "save_collection_sync",
);
export const setAllCollectionsSync = callable<
  [boolean, "my" | "smart" | "franchise" | null],
  { success: boolean; message?: string }
>("set_all_collections_sync");
export const saveCollectionPlatformGroups = callable<[boolean], { success: boolean }>(
  "save_collection_platform_groups",
);
export const getRegistryPlatforms = callable<[], { platforms: RegistryPlatform[] }>("get_registry_platforms");
export const removePlatformShortcuts = callable<
  [string],
  {
    success: boolean;
    // The success path returns success/app_ids/rom_ids/platform_name; the
    // @migration_blocked gate short-circuits to success/message/
    // blocked_by_migration, omitting app_ids/rom_ids. Every field below the
    // discriminant is therefore path-dependent (mirrors removeAllShortcuts).
    app_ids?: number[];
    rom_ids?: (string | number)[];
    platform_name?: string;
    message?: string;
    blocked_by_migration?: boolean;
  }
>("remove_platform_shortcuts");
export const removeAllShortcuts = callable<
  [],
  {
    success: boolean;
    // The success path returns only success/app_ids/rom_ids; the
    // @migration_blocked gate short-circuits to success/message/
    // blocked_by_migration, omitting app_ids/rom_ids. Every field below the
    // discriminant is therefore path-dependent.
    message?: string;
    app_ids?: number[];
    rom_ids?: (string | number)[];
    blocked_by_migration?: boolean;
  }
>("remove_all_shortcuts");
export const getArtworkBase64 = callable<[number], { base64: string | null }>("get_artwork_base64");
// Cache-first per-ROM cover fetch for the version picker (#1346, ADR-0021).
// Keyed by RomM ID: a cache hit returns the cached bytes, a miss downloads the
// ROM's cover from RomM into the cache. Works for a group version with no local
// DB row (the picker lists not-yet-synced siblings). Every failure — offline, no
// cover, read error — returns { base64: null } silently; it never re-downloads a
// cached cover.
export const fetchCoverBase64 = callable<[number], { base64: string | null }>("fetch_cover_base64");
export const refreshCoverArtwork = callable<
  [number],
  { success: boolean; reason?: string; message: string; cover_path?: string }
>("refresh_cover_artwork");
export const getSgdbArtworkBase64 = callable<[number, number], { base64: string | null; no_api_key?: boolean }>(
  "get_sgdb_artwork_base64",
);

/** A single SGDB game candidate for the manual picker. */
export interface SgdbCandidate {
  id: number;
  name: string;
  release_year: number | null;
  thumb_url: string | null;
}

/** Discriminated outcome of the SGDB artwork resolution cascade. */
export type SgdbResolution =
  | { decision: "no_api_key" }
  | { decision: "resolved"; sgdb_id: number }
  | { decision: "needs_pick"; candidates: SgdbCandidate[] };

/** Result of a manual SGDB name search. */
export interface SgdbSearchResult {
  success: boolean;
  games: SgdbCandidate[];
}

export const getSgdbResolution = callable<[number], SgdbResolution>("get_sgdb_resolution");
export const searchSgdbGames = callable<[string], SgdbSearchResult>("search_sgdb_games");
export const applySgdbGameId = callable<[number, number], { success: boolean }>("apply_sgdb_game_id");
export const reportUnitResults = callable<
  [Record<string, number>, string, number | string],
  { success: boolean; count: number; ignored?: boolean }
>("report_unit_results");
export const reportRemovalResults = callable<[(string | number)[]], { success: boolean; message: string }>(
  "report_removal_results",
);
export const reconcileShortcuts = callable<
  [number[]],
  { success: boolean; reason?: string; message: string; unbound_count?: number }
>("reconcile_shortcuts");
export const uninstallAllRoms = callable<
  [],
  { success: boolean; removed_count: number; errors: { rom_id: string; error: string }[]; app_ids: number[] }
>("uninstall_all_roms");
export const saveSgdbApiKey = callable<[string], { success: boolean; message: string }>("save_sgdb_api_key");
export const verifySgdbApiKey = callable<[string], { success: boolean; message: string }>("verify_sgdb_api_key");
export const saveSteamInputSetting = callable<[string], { success: boolean }>("save_steam_input_setting");
export const applySteamInputSetting = callable<[], { success: boolean; message: string }>("apply_steam_input_setting");
export const getFirmwareStatus = callable<[], FirmwareStatus>("get_firmware_status");
export const downloadAllFirmware = callable<[string], FirmwareDownloadResult>("download_all_firmware");
export const downloadRequiredFirmware = callable<[string], FirmwareDownloadResult>("download_required_firmware");
/**
 * Replace one emulator's key file that differs from RomM's with RomM's verified copy (the user's file is kept as
 * `<name>.romm-backup`). Arguments: the key's firmware id and the emulator id.
 */
export const replaceFirmwareKey = callable<
  [number, string],
  { success: boolean; message?: string; reason?: string; backup_path?: string }
>("replace_firmware_key");
/** Save the firmware version to download (`""` = the newest RomM has). */
export const saveFirmwareVersion = callable<[string], { success: boolean; message?: string; reason?: string }>(
  "save_firmware_version",
);
/**
 * Result of pinning / clearing a per-game emulator override. On success for an
 * installed + bound ROM, the backend re-bakes and returns the fresh
 * `launch_options` (the `-e`-wrapped command for a pin, the plain command for a
 * clear) plus the shortcut's `app_id` — the frontend confirm-sets them via
 * `setLaunchOptionsConfirmed`. Both are absent/None when the ROM is uninstalled
 * or unbound (no shortcut to update). An unresolvable label hard-fails with
 * `{success: false, reason: "core_unavailable", message}`.
 */
export interface GameCoreApplyResult {
  success: boolean;
  launch_options?: string;
  app_id?: number | null;
  reason?: string;
  message?: string;
}

// Per-game override (epic #945). Keyed by rom_id — the DB pin survives
// uninstall/reinstall (roms.emulator_override). set_game_core pins a label;
// clear_game_core drops the pin (follow default — triggered by picking the
// default-marked core in the menu).
export const setGameCore = callable<[number, string], GameCoreApplyResult>("set_game_core");
export const clearGameCore = callable<[number], GameCoreApplyResult>("clear_game_core");
// Dedicated core-info path (#923) — active core + available cores for a ROM,
// decoupled from the BIOS firmware status. Keyed by rom_id (#945): the active
// core reflects the per-game DB override when one is pinned, else the platform
// default.
export const getPlatformCoreInfo = callable<[number], CoreInfo>("get_platform_core_info");

/** One launchable disc image within a multi-disc ROM's install directory. */
export interface Disc {
  filename: string;
  label: string;
  index: number;
}

/**
 * Disc-picker state for a ROM (#865). `multi_disc` is `false` when the ROM is
 * unknown, not installed, single-file, or has fewer than two discs — the picker
 * renders nothing. When `true` the remaining fields are present: `discs` in disc
 * order, `selected` the persisted `roms.selected_disc` (null when following the
 * default), and `default` describing the NULL-selection target (the `.m3u`
 * playlist, or disc 1).
 */
export interface DiscSelection {
  multi_disc: boolean;
  discs?: Disc[];
  selected?: string | null;
  /** `"file"`: a launch-file pick (Switch folder: largest `.xci`/`.nsp` by default), not a disc. */
  default?: { kind: "m3u" | "disc" | "file"; label: string; filename: string };
}

/**
 * Result of pinning / clearing a disc selection. On success the backend persists
 * the pick (or NULL when clearing back to the default) and re-bakes the
 * `launch_options` for the now-selected disc — the frontend confirm-sets it via
 * `setLaunchOptionsConfirmed`. `selected` echoes the now-effective pin (null when
 * cleared). A failure carries the canonical `{success: false, reason, message}`
 * shape (`not_found` for an unknown filename, `not_installed` / `unsupported`
 * when the ROM is not a multi-disc install).
 */
export interface SelectDiscResult {
  success: boolean;
  launch_options?: string;
  selected?: string | null;
  reason?: string;
  message?: string;
}

// Per-game disc pick (#865). Keyed by rom_id — the DB pin survives
// uninstall/reinstall (roms.selected_disc). select_disc(rom_id, filename) pins a
// disc by basename; select_disc(rom_id, null) clears the pin (follow the default
// — the m3u playlist or disc 1).
export const getDiscSelection = callable<[number], DiscSelection>("get_disc_selection");
export const selectDisc = callable<[number, string | null], SelectDiscResult>("select_disc");

/**
 * One version (RomM sibling) of a game in the version picker (#1297, ADR-0021).
 * `label` is the fs_name_no_ext-derived display text; the dimensions are the
 * version's own region/language/revision/tag attributes. `synced` is false for a
 * version that exists on the server but has no local row yet (selecting it
 * persists one); `installed` marks a downloaded version; `active` is the bound
 * version the Download button fetches; `is_default` marks the version the
 * resolution chain + Preferred-region setting would pick as the default.
 * `switchable` is false for a RomM sibling whose metadata match conflicts with
 * this game's — a locally-synced ROM under a different group key (#1359) or a
 * not-yet-synced sibling carrying a different id at the group's canonical source
 * (#1360) — the picker lists it but disables the row, because switch_version would
 * reject it; every other row is switchable (#1368).
 */
export interface VersionInfo {
  rom_id: number;
  name: string;
  label: string;
  regions: string[];
  languages: string[];
  revision: string;
  tags: string[];
  synced: boolean;
  installed: boolean;
  active: boolean;
  is_default: boolean;
  switchable: boolean;
}

/**
 * Version-picker state for the group bound to an appId. `multi_version` is
 * `false` when the appId is unknown/unbound or the group has a single version —
 * the picker renders nothing. When `true`, `versions` lists every version
 * (local + server-only) with markers; `server_query_failed` is `true` when the
 * live `sibling_roms` view could not be fetched, so the list is local-only
 * (partial-success carve-out).
 */
export interface VersionList {
  multi_version: boolean;
  versions?: VersionInfo[];
  server_query_failed?: boolean;
}

/**
 * Success outcome of a version switch (uniform across all switch paths, #1298).
 * The binding moved to `rom_id`; `launch_options` is the target install's full
 * Steam launch command when `target_installed`, or `""` for an uninstalled
 * target (the ADR-0009 placeholder). The frontend confirm-writes `launch_options`
 * onto the shortcut identified by `app_id` — even when blank, so the shortcut
 * never keeps the old version's command.
 */
export interface SwitchVersionSuccess {
  success: true;
  rom_id: number;
  target_installed: boolean;
  launch_options: string;
  app_id: number;
}

/**
 * Soft-block: switching away from a downloaded version whose local saves have
 * unsynced changes (#1298). `server_reachable` decides which confirm modal to
 * show (T4 reachable offers "Sync now & switch", T5 offline does not);
 * `unsynced_rom_id` / `unsynced_version_name` name the version that would be
 * stranded. The user resolves it with the `allow_stranded` override.
 */
export interface SwitchVersionUnsyncedSaves {
  success: false;
  reason: "unsynced_saves";
  message: string;
  server_reachable: boolean;
  unsynced_rom_id: number;
  unsynced_version_name: string;
}

/**
 * Other canonical `{success, reason, message}` failures — `not_found` (unknown
 * appId), `not_in_group` (the target's metadata match conflicts with this game's),
 * `bound_elsewhere` (a grandfathered duplicate),
 * `invalid_target` (a server-only target the aggregate rejects),
 * `download_in_progress` (a group download is running — cancel it first), or
 * `server_unreachable`. All surface via the picker's toast fallback
 * (`result.message`). The literal reason union excludes `unsynced_saves` so the
 * soft-block variant narrows cleanly.
 */
export interface SwitchVersionFailure {
  success: false;
  reason:
    "not_found" | "not_in_group" | "bound_elsewhere" | "invalid_target" | "download_in_progress" | "server_unreachable";
  message: string;
}

export type SwitchVersionResult = SwitchVersionSuccess | SwitchVersionUnsyncedSaves | SwitchVersionFailure;

// Version picker (#1297, ADR-0021). Keyed by the Steam appId (the group's
// shortcut). get_version_list reads the group's versions; switch_version moves
// the active-version binding to a target rom_id.
export const getVersionList = callable<[number], VersionList>("get_version_list");
export const switchVersion = callable<[number, number, boolean], SwitchVersionResult>("switch_version");

export const saveLogLevel = callable<[string], { success: boolean }>("save_log_level");
// Preferred sibling-group region (ADR-0021 §3). "auto" = build-time default
// order; any RomM region string heads the ranking on the next sync.
export const savePreferredRegion = callable<[string], { success: boolean }>("save_preferred_region");
// Distinct region values present in the locally synced library — the non-anchor
// options for the Preferred-region dropdown. Pure local DB read, no server call.
export const getKnownRegions = callable<[], string[]>("get_known_regions");
export const debugLog = callable<[string], void>("debug_log");
const frontendLog = callable<[string, string], void>("frontend_log");
export const logInfo = (msg: string) => {
  detach(frontendLog("info", msg));
};
export const logWarn = (msg: string) => {
  detach(frontendLog("warn", msg));
};
export const logError = (msg: string) => {
  detach(frontendLog("error", msg));
};
export const getRomMetadata = callable<[number], RomMetadata>("get_rom_metadata");
export const getAllMetadataCache = callable<[], Record<string, RomMetadata>>("get_all_metadata_cache");
export const getAppIdRomIdMap = callable<[], Record<string, number>>("get_app_id_rom_id_map");
export const getLibraryOverview = callable<[], LibraryOverview>("get_library_overview");
export const getInstalledRelaunchOptions = callable<[], { app_id: number; launch_options: string }[]>(
  "get_installed_relaunch_options",
);

// Icon files are saved by the backend and applied through SteamClient.
export const saveShortcutIcon = callable<[number, string], { success: boolean; icon_path?: string }>("save_shortcut_icon");

// Save sync callables
export const ensureDeviceRegistered = unportedCallable<[], { success: boolean; device_id: string; device_name: string }>(
  "ensure_device_registered",
);

export const listDevices = unportedCallable<[], ListDevicesResponse>("list_devices");
export const getSaveStatus = unportedCallable<[number], SaveStatus>("get_save_status");
export const preLaunchSync = unportedCallable<
  [number],
  { success: boolean; message: string; synced?: number; errors?: string[]; conflicts?: SyncConflict[]; reason?: string }
>("pre_launch_sync");
export const syncRomSaves = unportedCallable<
  [number],
  { success: boolean; message: string; synced: number; errors?: string[]; conflicts?: SyncConflict[]; reason?: string }
>("sync_rom_saves");
export const syncAllSaves = unportedCallable<
  [],
  { success: boolean; message: string; synced: number; conflicts: number; reason?: string }
>("sync_all_saves");
export const resolveSyncConflict = unportedCallable<
  [number, string, number, "keep_local" | "use_server"],
  { success: boolean; message?: string; reason?: "stale_conflict"; action?: "keep_local" | "use_server" }
>("resolve_sync_conflict");
export const recordSessionStart = callable<[number], { success: boolean }>("record_session_start");
export const getSaveSyncSettings = unportedCallable<[], SaveSyncSettings>("get_save_sync_settings");
export const updateSaveSyncSettings = unportedCallable<[SaveSyncSettings], { success: boolean }>("update_save_sync_settings");
export const getSaveSlots = unportedCallable<
  [number],
  { success: boolean; slots: SaveSlotSummary[]; active_slot: string; reason?: string; message?: string }
>("get_save_slots");
export const getSlotSaves = unportedCallable<[number, string], SlotSavesResponse>("get_slot_saves");
export const switchSlot = unportedCallable<[number, string], SwitchSlotResponse>("switch_slot");

export const getSlotDeleteInfo = unportedCallable<[number, string], SlotDeleteInfo>("get_slot_delete_info");
export const deleteSlot = unportedCallable<[number, string], DeleteSlotResult>("delete_slot");

export const isSaveTrackingConfigured = unportedCallable<[number], { configured: boolean; active_slot: string | null }>(
  "is_save_tracking_configured",
);
export const getSaveSetupInfo = unportedCallable<[number], SaveSetupInfo>("get_save_setup_info");
// confirm_slot_choice(rom_id, chosen_slot, migrate, migrate_from_slot):
// `chosen_slot` must be a non-empty named slot — legacy `slot:null` confirmation
// is retired (#1276), so an empty/`null` target is rejected by the backend's
// `invalid_slot_name` guard. `migrate` is an explicit boolean — the
// non-destructive paths pass `false`; `migrate_from_slot` is `null` unless
// migrating (then the source slot, with `null` meaning the legacy no-slot source).
export const confirmSlotChoice = unportedCallable<
  [number, string, boolean, string | null],
  { success: boolean; needs_conflict_resolution?: boolean; message: string }
>("confirm_slot_choice");
export const checkCoreChange = callable<
  [number],
  { changed: boolean; old_core?: string; new_core?: string; old_label?: string; new_label?: string }
>("check_core_change");

// Bulk playtime for plugin-load UI update. last_played is the ISO end time of
// the newest recorded/reconciled session (null until one exists).
export const getAllPlaytime = callable<
  [],
  { playtime: Record<string, { total_seconds: number; session_count: number; last_played: string | null }> }
>("get_all_playtime");

// Pull-only playtime reconcile-on-view — folds RomM's native play-session
// history in (monotonic max) so a session played on another device shows up the
// moment the detail page is opened. Restores total_seconds, session_count AND
// last_played across a device cutover (#903, ADR-0018). server_query_failed=true
// means the server was unreachable and these are the local fallback.
export const reconcilePlaytime = callable<
  [number],
  { total_seconds: number; session_count: number; last_played: string | null; server_query_failed: boolean }
>("reconcile_playtime");

// Persistent corrupt-settings-reset notice. When settings.json was unparseable
// at boot it is backed up to settings.json.corrupt-<ts> and reset to defaults,
// and a marker is persisted into the fresh settings.json. This read is
// non-consuming: it reports pending:true with the backup filename until the
// user explicitly dismisses it in the QAM, so the banner survives reloads.
export const getSettingsResetNotice = callable<[], { pending: boolean; backed_up_to: string | null }>(
  "get_settings_reset_notice",
);

// Acknowledge the corrupt-settings reset — pops the persistent marker and
// persists, so the QAM banner + game-detail cards stay down across reloads.
export const dismissSettingsResetNotice = callable<[], { success: boolean }>("dismiss_settings_reset_notice");

// Durable "re-sign-in for cross-device playtime" notice. The backend persists a
// flag when a playtime reconcile is rejected because the Client API Token lacks
// the `roms.user.read` scope; pending:true means the user should sign in again
// to mint a scoped token. The flag clears itself once the scope is present (a
// later successful reconcile, or a fresh sign-in), so this read is non-consuming
// and pull-only — no backend dismiss callable, the QAM banner's Dismiss is local.
export const getPlaytimeScopeNotice = callable<[], { pending: boolean }>("get_playtime_scope_notice");

// End-of-session orchestration — collapses recordSessionEnd + postExitSync into a single backend round-trip.
// See SessionLifecycleService in py_modules/services/session_lifecycle.py.
interface SessionFinalizeSyncResult {
  offline: boolean;
  success: boolean;
  synced: number | null;
  conflicts: SyncConflict[];
  toast_title: string | null;
  toast_body: string | null;
  conflicts_toast: string | null;
}

export interface SessionFinalizeResult {
  total_seconds: number | null;
  sync: SessionFinalizeSyncResult;
}

export const finalizeGameSession = callable<[number], SessionFinalizeResult>("finalize_game_session");

// Delete operations
export const deleteLocalSaves = unportedCallable<[number], { success: boolean; deleted_count: number; message: string }>(
  "delete_local_saves",
);
export const deletePlatformSaves = unportedCallable<[string], { success: boolean; deleted_count: number; message: string }>(
  "delete_platform_saves",
);

// Save version history callables
export const savesListFileVersions = unportedCallable<[number, string, string], ListFileVersionsResult>(
  "saves_list_file_versions",
);
export const savesRollbackToVersion = unportedCallable<[number, string, number], RollbackStatus>("saves_rollback_to_version");

// Self-update: checks GitHub for a newer release (throttled to every 6 h unless
// force=true). Never installs — see utils/selfUpdate.ts for the Decky handoff.
export const checkForUpdate = callable<[boolean], UpdateCheckResult>("check_for_update");
