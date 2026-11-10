/**
 * Firmware types — the Switch firmware/keys status, and the available-cores selection presented in the UI.
 */

/**
 * One ES-DE `<command>` classified for launch-bakeability (#1210), the shape the
 * backend `get_emulator_options` / picker payloads carry. `kind` is `"libretro"`
 * (`core_so` set to the bare core name) or `"standalone"` (`core_so` null).
 * `bakeable` is false for the `needs_setup` (`reason: "inject"`) and un-bakeable
 * forms; `reason` names why the frontend can't offer it as a clickable pick.
 */
export interface EmulatorOption {
  label: string;
  kind: "libretro" | "standalone";
  core_so: string | null;
  is_default: boolean;
  bakeable: boolean;
  reason: string | null;
}

/**
 * Response shape of the `get_platform_core_info` callable — the dedicated
 * single-platform emulator-info path, decoupled from the per-game BIOS payload
 * (#923). The per-game detail page (`RomMPlaySection` / `RomMGameInfoPanel`)
 * reads emulator data from here. The System page's multi-platform overview
 * instead reads it off the `get_firmware_status` payload (`FirmwarePlatformExt`),
 * which enumerates every platform in one call — see that interface below.
 * `emulator_data_available` is false when `es_systems.xml` cannot be read
 * (no EmuDeck emulator detected), so the picker can say so instead of an empty list.
 */
export interface CoreInfo {
  emulators: EmulatorOption[];
  emulator_data_available: boolean;
  active_core: string | null;
  active_core_label: string | null;
  platform_core_label: string | null;
  has_game_override: boolean;
  /** The per-game pinned label (null when unpinned). When it names an emulator
   *  that is no longer bakeable (e.g. uninstalled), the game launches with the
   *  default and the picker says so. */
  game_override_label?: string | null;
}

/** How an emulator's copy of a key file compares with RomM's. */
export type KeyState = "missing" | "matches" | "differs" | "present";

export interface KeyEmulatorState {
  id: string;
  label: string;
  state: KeyState;
}

/** One Switch key file (`prod.keys`, `title.keys`, ...) with its state per installed emulator. */
export interface SwitchKeyFile {
  file_name: string;
  description: string;
  required: boolean;
  on_romm: boolean;
  firmware_id: number | null;
  size: number | null;
  /** Only the installed emulators that read this file. */
  emulators: KeyEmulatorState[];
}

/** One firmware archive RomM hosts (`Firmware.23.0.1.zip`). */
export interface SwitchFirmwareArchive {
  firmware_id: number | null;
  file_name: string;
  version: string;
  size: number | null;
  /** The version "Download required" fetches (the chosen one, else the newest RomM has). */
  selected: boolean;
  downloaded: boolean;
  local_size: number | null;
}

export interface SwitchEmulator {
  id: string;
  label: string;
  keys_dir: string;
}

/**
 * `get_firmware_status` (Switch only): the files RomM hosts for the Switch and how this machine compares.
 * Reading never writes anything.
 */
export interface FirmwareStatus {
  success: boolean;
  message?: string;
  /** RomM could not be reached: the last known list (or none) is shown. */
  server_offline?: boolean;
  platform_slug?: string;
  /** Where firmware archives are saved (`<Emulation>/bios/switch`). */
  archive_dir?: string;
  emulators?: SwitchEmulator[];
  keys?: SwitchKeyFile[];
  firmware?: SwitchFirmwareArchive[];
  /** The chosen firmware version (the newest when none is chosen or RomM no longer has it); `null` without firmware. */
  selected_version?: string | null;
  /** `true` when the user chose that version (and RomM still has it); `false` when it simply follows the newest. */
  version_pinned?: boolean;
  /** Files under `bios/Switch` that are neither keys nor firmware archives. */
  unrecognized?: number;
}

/** The answer of `download_required_firmware` / `download_all_firmware`: what was queued, not what finished. */
export interface FirmwareDownloadResult {
  success: boolean;
  message?: string;
  /** How many downloads were put in the download queue. */
  queued?: number;
  /** Key files that exist but differ from RomM's: kept, never replaced. */
  kept?: string[];
  reason?: string;
}
