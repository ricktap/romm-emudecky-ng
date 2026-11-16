/**
 * The "Eden" entry in the Steam library: a non-Steam shortcut that opens Eden itself (its window, no game), so its
 * settings are reachable from Game Mode. Its exe is Eden itself, never `bin/rom-launcher-ng`, so library sync never
 * treats it as a ROM. Made once when Eden is installed (at plugin start and after a library sync); an "Eden" entry
 * that already exists (e.g. from Steam ROM Manager) is adopted instead of duplicated, and a shortcut the user removed
 * is not made again.
 */

import { getEmulatorStarter, logError, logInfo, rememberEmulatorStarter } from "../api/backend";

export type StarterOutcome = "created" | "adopted" | "kept" | "removed_by_user" | "not_installed" | "unavailable";

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

function nonSteamAppIds(): number[] {
  try {
    return [...(collectionStore?.deckDesktopApps?.apps?.keys() ?? [])];
  } catch {
    return [];
  }
}

function overviewOf(appId: number): SteamAppOverview | null {
  try {
    return appStore.GetAppOverviewByAppID(appId);
  } catch {
    return null;
  }
}

function findByName(name: string): number | null {
  const wanted = name.toLowerCase();
  return nonSteamAppIds().find((id) => overviewOf(id)?.display_name.toLowerCase() === wanted) ?? null;
}

async function run(settleMs: number): Promise<StarterOutcome> {
  if (typeof SteamClient === "undefined" || typeof appStore === "undefined") return "unavailable";
  const info = await getEmulatorStarter();
  if (!info.success || !info.exe) return "not_installed";
  if (info.app_id != null) return overviewOf(info.app_id) ? "kept" : "removed_by_user";
  const existing = findByName(info.name);
  if (existing != null) {
    await rememberEmulatorStarter(existing);
    logInfo(`Adopted the existing ${info.name} shortcut (appId ${existing})`);
    return "adopted";
  }
  // AddShortcut ignores most of its parameters: name, exe and start dir are set after Steam registered the app.
  const appId = await SteamClient.Apps.AddShortcut(info.name, info.exe, "", "");
  if (!appId) return "unavailable";
  await sleep(settleMs);
  SteamClient.Apps.SetShortcutName(appId, info.name);
  SteamClient.Apps.SetShortcutExe(appId, info.exe);
  if (info.start_dir) SteamClient.Apps.SetShortcutStartDir(appId, info.start_dir);
  await rememberEmulatorStarter(appId);
  logInfo(`Added the ${info.name} shortcut to the Steam library (appId ${appId})`);
  return "created";
}

let running: Promise<StarterOutcome> | null = null;

/** Make sure the Eden entry exists; concurrent calls share one run. Never throws. */
export function ensureEmulatorStarter(settleMs = 500): Promise<StarterOutcome> {
  running ??= run(settleMs)
    .catch((e: unknown): StarterOutcome => {
      logError(`Could not add the Eden shortcut: ${e}`);
      return "unavailable";
    })
    .finally(() => {
      running = null;
    });
  return running;
}
