/**
 * Settings "Updates" section: installed version, the last check's result, a
 * forced "Check for updates" button, and the update notice when one exists.
 * Reads and drives the shared update store; no props.
 */

import { FC, useEffect, useState } from "react";
import { PanelSection, PanelSectionRow, ButtonItem, Field } from "@decky/ui";
import { getUpdateState, onUpdateStateChange, refreshUpdateState, type UpdateState } from "../../utils/updateStore";
import { UpdateNotice } from "../UpdateBanner";

function formatCheckedAt(iso: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

export function updateStatusText(state: UpdateState): string {
  if (state.checking) return "Checking…";
  if (state.error) return state.error;
  const info = state.info;
  if (!info) return "Not checked yet";
  if (info.update_available) return `Update available: v${info.latest_version}`;
  if (info.latest_version) return `Up to date (latest release v${info.latest_version})`;
  return "No installable release found";
}

export const UpdatesSection: FC = () => {
  const [state, setState] = useState<UpdateState>(getUpdateState());

  useEffect(() => {
    const unsub = onUpdateStateChange(() => setState(getUpdateState()));
    // The QAM main page normally ran a check already; if not, fetch the
    // (backend-throttled) status so the installed version shows.
    const current = getUpdateState();
    if (!current.info && !current.error && !current.checking) {
      refreshUpdateState(false).catch(() => undefined);
    }
    return unsub;
  }, []);

  const checkedAt = formatCheckedAt(state.checkedAt);

  return (
    <PanelSection title="Updates">
      <PanelSectionRow>
        <Field label="Installed version" description={state.currentVersion ? `v${state.currentVersion}` : "—"} focusable />
      </PanelSectionRow>
      <PanelSectionRow>
        <Field
          label="Last check"
          description={checkedAt ? `${updateStatusText(state)} · ${checkedAt}` : updateStatusText(state)} focusable />
      </PanelSectionRow>
      <PanelSectionRow>
        <ButtonItem
          layout="below"
          disabled={state.checking}
          onClick={() => {
            // refreshUpdateState folds every failure into the store — never rejects.
            refreshUpdateState(true).catch(() => undefined);
          }}
        >
          Check for updates
        </ButtonItem>
      </PanelSectionRow>
      {state.info?.update_available && <UpdateNotice info={state.info} />}
    </PanelSection>
  );
};
