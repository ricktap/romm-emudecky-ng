import { FC } from "react";
import { AttRow } from "./rm";
import { dismissSettingsResetNotice, logError } from "../api/backend";
import { setSettingsResetState } from "../utils/settingsResetStore";
import { detach } from "../utils/detach";

/** Title shared by the QAM banner and the game-detail card. */
export const SETTINGS_RESET_TITLE = "Settings were reset";

/**
 * Body text for the QAM corrupt-settings-reset banner. Names the backup file
 * when known so the user can recover the original bytes; falls back to a
 * generic line when the backup path is unavailable.
 */
export function settingsResetMessage(backedUpTo: string | null): string {
  const backup = backedUpTo
    ? `A backup of your old settings was saved to ${backedUpTo}.`
    : "A backup of your old settings was saved.";
  return `Your settings file was corrupt and has been reset. Re-enter your server URL and sign in again. ${backup}`;
}

/**
 * Body text for the game-detail card. The card is informational only — the
 * Dismiss control lives in the QAM — so the copy points the user there.
 */
export function settingsResetCardMessage(backedUpTo: string | null): string {
  const backup = backedUpTo
    ? `A backup of your old settings was saved to ${backedUpTo}.`
    : "A backup of your old settings was saved.";
  return `Your settings file was corrupt and has been reset. Re-enter your server URL and sign in again. ${backup} Open the EmuDecky RomM Manager menu (QAM) to dismiss this.`;
}

interface SettingsResetBannerProps {
  backedUpTo: string | null;
}

/** QAM attention row shown while a corrupt-settings reset is pending; pressing it dismisses the notice. */
export const SettingsResetBanner: FC<SettingsResetBannerProps> = ({ backedUpTo }) => {
  const handleDismiss = () => {
    detach(
      dismissSettingsResetNotice().then(
        // Clear the shared store on success so the banner AND every game-detail
        // card disappear immediately (both subscribe to the same store).
        () => setSettingsResetState({ pending: false, backedUpTo: null }),
        // On failure leave the banner up so the user can retry.
        (e) => logError(`Failed to dismiss settings reset notice: ${e}`),
      ),
    );
  };

  return (
    <AttRow
      title={SETTINGS_RESET_TITLE}
      sub={`${settingsResetMessage(backedUpTo)} Press to dismiss.`}
      onActivate={handleDismiss}
    />
  );
};
