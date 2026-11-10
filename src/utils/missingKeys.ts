import type { FirmwareStatus } from "../types";

/**
 * Required Switch key files RomM hosts that at least one installed emulator lacks: the "BIOS missing" item of the
 * Needs attention list. Empty when the status could not be read, so a failed read never raises a false alarm.
 */
export function missingRequiredKeys(status: FirmwareStatus | null | undefined): string[] {
  if (!status?.success) return [];
  return (status.keys ?? [])
    .filter((k) => k.required && k.on_romm && k.emulators.some((e) => e.state === "missing"))
    .map((k) => k.file_name);
}
