import { FC, useState } from "react";
import { ButtonItem, Field, PanelSection, PanelSectionRow } from "@decky/ui";
import { logInfo } from "../api/backend";
import { findRomMLauncherExe } from "../utils/steamShortcuts";
import { cleanup, runPhase1, runPhase2, type ExperimentEnv, type ExperimentState } from "./experimentRunner";

const STORAGE_KEY = "romm-idtest-state";
const FALLBACK_EXE = "/usr/bin/env";

const loadState = (): ExperimentState | null => {
  try {
    const raw = globalThis.localStorage?.getItem(STORAGE_KEY);
    return raw ? (JSON.parse(raw) as ExperimentState) : null;
  } catch {
    return null;
  }
};
const saveState = (state: ExperimentState | null) => {
  try {
    if (state) globalThis.localStorage?.setItem(STORAGE_KEY, JSON.stringify(state));
    else globalThis.localStorage?.removeItem(STORAGE_KEY);
  } catch {
    // The result is still shown on screen; phase 2 then needs the lines from phase 1.
  }
};

async function realEnv(): Promise<ExperimentEnv> {
  const exe = (await findRomMLauncherExe()) ?? FALLBACK_EXE;
  return {
    exe,
    addShortcut: async (name, path) => (await SteamClient.Apps.AddShortcut(name, path, "", "")) || null,
    rename: (id, name) => SteamClient.Apps.SetShortcutName(id, name),
    remove: (id) => SteamClient.Apps.RemoveShortcut(id),
    displayName: (id) => appStore.GetAppOverviewByAppID(id)?.display_name ?? null,
    listShortcutIds: () => Array.from(collectionStore.deckDesktopApps?.apps?.keys() ?? []),
    wait: (ms) => new Promise((r) => setTimeout(r, ms)),
  };
}

/** Temporary developer panel: the stable shortcut id experiment (remove together with the plan's step 0). */
export const ShortcutIdExperiment: FC = () => {
  const [lines, setLines] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);

  const show = (out: string[]) => {
    setLines(out);
    for (const line of out) logInfo(`[shortcut-id-test] ${line}`);
  };
  const run = (job: () => Promise<string[]>) => {
    setBusy(true);
    job()
      .then(show)
      .catch((e) => show([`Failed: ${e}`]))
      .finally(() => setBusy(false));
  };

  return (
    <PanelSection title="Developer: stable shortcut id test (temporary)">
      <PanelSectionRow>
        <Field label="Creates a few test shortcuts named romm-idtest-…, then removes them with Clean up." />
      </PanelSectionRow>
      <PanelSectionRow>
        <ButtonItem
          layout="below"
          disabled={busy}
          onClick={() =>
            run(async () => {
              const result = await runPhase1(await realEnv());
              saveState(result.state);
              return result.lines;
            })
          }
        >
          Phase 1: run the test
        </ButtonItem>
      </PanelSectionRow>
      <PanelSectionRow>
        <ButtonItem
          layout="below"
          disabled={busy}
          onClick={() =>
            run(async () => {
              const state = loadState();
              if (!state) return ["No phase 1 result stored: run phase 1 first."];
              return runPhase2(await realEnv(), state);
            })
          }
        >
          Phase 2: check after a Steam restart
        </ButtonItem>
      </PanelSectionRow>
      <PanelSectionRow>
        <ButtonItem
          layout="below"
          disabled={busy}
          onClick={() =>
            run(async () => {
              const out = cleanup(await realEnv(), loadState());
              saveState(null);
              return out;
            })
          }
        >
          Clean up test shortcuts
        </ButtonItem>
      </PanelSectionRow>
      {lines.map((line, i) => (
        <PanelSectionRow key={i}>
          <div style={{ fontSize: "12px", whiteSpace: "pre-wrap", wordBreak: "break-all" }}>{line}</div>
        </PanelSectionRow>
      ))}
    </PanelSection>
  );
};
