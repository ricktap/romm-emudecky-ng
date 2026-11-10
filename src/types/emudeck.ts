/** The states of EmuDeck's configuration; anything but `ok` stops downloads and installs. */
export type EmuDeckHealth = "ok" | "absent" | "unreadable" | "invalid" | "root_missing";

export interface EmuDeckHealthInfo {
  health: EmuDeckHealth;
  ok: boolean;
  /** One short sentence for the banner. */
  message: string;
  /** EmuDeck's `storagePath` (the parent of `Emulation/`) when it could be read. */
  storage_path: string | null;
  /** `true` when the storage location differs from the one the user last accepted (EmuDeck moved to another drive). */
  moved?: boolean;
  /** The location that was accepted before; present with `moved`. */
  accepted_path?: string;
  /** Recorded installs, and how many of their files are no longer on disk; present with `moved`. */
  installed?: number;
  missing?: number;
}
