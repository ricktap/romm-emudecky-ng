/** The states of EmuDeck's configuration; anything but `ok` stops downloads and installs. */
export type EmuDeckHealth = "ok" | "absent" | "unreadable" | "invalid" | "root_missing";

export interface EmuDeckHealthInfo {
  health: EmuDeckHealth;
  ok: boolean;
  /** One short sentence for the banner. */
  message: string;
  /** EmuDeck's `storagePath` (the parent of `Emulation/`) when it could be read. */
  storage_path: string | null;
}
