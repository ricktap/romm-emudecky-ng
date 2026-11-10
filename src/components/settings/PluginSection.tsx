/**
 * The plugin card: installed version and update check, log level, and the way
 * into the danger zone. Reads and drives the shared update store.
 */

import { FC, useEffect, useState } from "react";
import { getUpdateState, onUpdateStateChange, refreshUpdateState, type UpdateState } from "../../utils/updateStore";
import { UpdateNotice } from "../UpdateBanner";
import { RmButton, RmCard, RmChoice, RmRow, RmValueRow } from "../rm";

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

const LOG_LEVELS = [
  { data: "error", label: "Error" },
  { data: "warn", label: "Warn" },
  { data: "info", label: "Info" },
  { data: "debug", label: "Debug" },
];

interface PluginSectionProps {
  logLevel: string;
  onLogLevelChange: (level: string) => void;
  dangerOpen: boolean;
  onToggleDanger: () => void;
}

export const PluginSection: FC<PluginSectionProps> = ({ logLevel, onLogLevelChange, dangerOpen, onToggleDanger }) => {
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
    <RmCard title="Plugin">
      <RmValueRow
        label="Version"
        sub={checkedAt ? `Checked ${checkedAt}` : undefined}
        value={`${state.currentVersion ? `v${state.currentVersion}` : "—"} · ${updateStatusText(state)}`}
      />
      <RmRow label="Check for updates">
        <RmButton
          variant="sm ghost"
          testId="check-updates"
          disabled={state.checking}
          onActivate={() => {
            // refreshUpdateState folds every failure into the store — never rejects.
            refreshUpdateState(true).catch(() => undefined);
          }}
        >
          Check
        </RmButton>
      </RmRow>
      {state.info?.update_available && <UpdateNotice info={state.info} />}
      <RmRow label="Log level" sub="How much detail the plugin logs">
        <RmChoice label="Log level" value={logLevel} options={LOG_LEVELS} onChange={onLogLevelChange} />
      </RmRow>
      <RmRow label="Danger zone" sub="Remove shortcuts, delete ROMs, reset">
        <RmButton variant="sm danger" testId="danger-zone" onActivate={onToggleDanger}>
          {dangerOpen ? "Close" : "Open"}
        </RmButton>
      </RmRow>
    </RmCard>
  );
};
