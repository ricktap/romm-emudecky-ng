/**
 * Artwork and controller settings: the SteamGridDB API key (with a verify
 * button) and the Steam Input mode for ROM shortcuts. Pure renderer: the parent
 * owns the masked key, the verifying flag, the mode and the status messages.
 */

import { FC } from "react";
import { showModal } from "@decky/ui";
import { TextInputModal } from "./TextInputModal";
import { RmButton, RmCard, RmChoice, RmRow, RmValueRow, RmButtonGroup } from "../rm";

const INPUT_MODES = [
  { data: "default", label: "Default" },
  { data: "force_on", label: "Force On" },
  { data: "force_off", label: "Force Off" },
];

interface SteamGridDBSectionProps {
  sgdbApiKey: string;
  sgdbStatus: string;
  sgdbVerifying: boolean;
  onSubmitKey: (value: string) => void;
  onVerifyKey: () => void;
  steamInputMode: string;
  steamInputStatus: string;
  loading: boolean;
  onModeChange: (mode: string) => void;
  onApplyMode: () => void;
}

export const SteamGridDBSection: FC<SteamGridDBSectionProps> = ({
  sgdbApiKey,
  sgdbStatus,
  sgdbVerifying,
  onSubmitKey,
  onVerifyKey,
  steamInputMode,
  steamInputStatus,
  loading,
  onModeChange,
  onApplyMode,
}) => (
  <RmCard title="Artwork & controller">
    <RmRow label="SteamGridDB API key" sub={sgdbApiKey ? undefined : "Needed for hero, logo and wide artwork"}>
      <RmButtonGroup>
        <RmButton
          variant="sm ghost"
          testId="edit-sgdb-key"
          onActivate={() =>
            showModal(<TextInputModal label="SteamGridDB API Key" value="" bIsPassword onSubmit={onSubmitKey} />)
          }
        >
          {sgdbApiKey ? "•••• Edit" : "Set key"}
        </RmButton>
        <RmButton variant="sm ghost" testId="verify-sgdb-key" disabled={sgdbVerifying || !sgdbApiKey} onActivate={onVerifyKey}>
          {sgdbVerifying ? "Verifying..." : "Verify"}
        </RmButton>
      </RmButtonGroup>
    </RmRow>
    {sgdbStatus && <RmValueRow label={sgdbStatus} />}
    <RmRow label="Steam Input for ROM shortcuts">
      <RmChoice label="Steam Input" value={steamInputMode} options={INPUT_MODES} onChange={onModeChange} />
    </RmRow>
    <RmRow label="Apply to all shortcuts" sub="Writes the mode above to every RomM shortcut">
      <RmButton variant="sm ghost" testId="apply-steam-input" disabled={loading} onActivate={onApplyMode}>
        Apply
      </RmButton>
    </RmRow>
    {steamInputStatus && <RmValueRow label={steamInputStatus} />}
  </RmCard>
);
