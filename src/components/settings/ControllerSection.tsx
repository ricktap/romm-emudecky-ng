/**
 * Controller settings — Steam Input mode dropdown and the "Apply to All
 * Shortcuts" trigger. Pure renderer: parent owns the mode value and status.
 */

import { FC } from "react";
import { PanelSection, PanelSectionRow, ButtonItem, Field, DropdownItem } from "@decky/ui";

interface ControllerSectionProps {
  steamInputMode: string;
  steamInputStatus: string;
  loading: boolean;
  onModeChange: (mode: string) => void;
  onApplyMode: () => void;
}

export const ControllerSection: FC<ControllerSectionProps> = ({
  steamInputMode,
  steamInputStatus,
  loading,
  onModeChange,
  onApplyMode,
}) => {
  return (
    <PanelSection title="Controller">
      <PanelSectionRow>
        <DropdownItem
          label="Steam Input Mode"
          description="Controls how Steam handles controller input for ROM shortcuts"
          rgOptions={[
            { data: "default", label: "Default (Recommended)" },
            { data: "force_on", label: "Force On" },
            { data: "force_off", label: "Force Off" },
          ]}
          selectedOption={steamInputMode}
          onChange={(option) => onModeChange(option.data)}
        />
      </PanelSectionRow>
      <PanelSectionRow>
        <ButtonItem layout="below" onClick={onApplyMode} disabled={loading}>
          Apply to All Shortcuts
        </ButtonItem>
      </PanelSectionRow>
      {steamInputStatus && (
        <PanelSectionRow>
          <Field label={steamInputStatus} focusable />
        </PanelSectionRow>
      )}
    </PanelSection>
  );
};
