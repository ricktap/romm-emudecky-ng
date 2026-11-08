/**
 * Hands a validated release to Decky Loader's own plugin installer.
 *
 * Decky exposes its websocket router to plugin frontends as the global
 * `window.DeckyBackend`. Its internal `utilities/install_plugin` route shows
 * Decky's install confirmation modal; on confirm Decky downloads the zip,
 * verifies the sha256, replaces the installed plugin folder (matched by
 * `plugin.json` "name") and reloads the plugin. The route is undocumented, so
 * every failure here resolves to `false` and the caller shows a manual
 * download link instead.
 */

import { Navigation } from "@decky/ui";

/** Decky's `PluginInstallType.UPDATE`. */
export const DECKY_INSTALL_TYPE_UPDATE = 2;

export const DECKY_INSTALL_ROUTE = "utilities/install_plugin";

type DeckyRouteCall = (route: string, ...args: unknown[]) => Promise<unknown>;

interface DeckyBackendRouter {
  call?: DeckyRouteCall;
  callable?: (route: string) => (...args: unknown[]) => Promise<unknown>;
}

declare global {
  interface Window {
    DeckyBackend?: DeckyBackendRouter;
  }
}

export interface SelfUpdateTarget {
  asset_url: string;
  plugin_name: string;
  latest_version: string | null;
  sha256: string;
}

/**
 * Ask Decky to install `target` as an update. Resolves `true` once Decky
 * accepted the request (its confirmation modal takes over from there), `false`
 * when Decky's router is unavailable, the target is incomplete, or the call
 * threw.
 */
export async function startSelfUpdate(target: SelfUpdateTarget): Promise<boolean> {
  const { asset_url, plugin_name, latest_version, sha256 } = target;
  if (!asset_url || !plugin_name || !latest_version || !sha256) return false;
  const router = typeof window === "undefined" ? undefined : window.DeckyBackend;
  if (!router) return false;
  const args = [asset_url, plugin_name, latest_version, sha256, DECKY_INSTALL_TYPE_UPDATE];
  try {
    if (typeof router.call === "function") {
      await router.call(DECKY_INSTALL_ROUTE, ...args);
      return true;
    }
    if (typeof router.callable === "function") {
      await router.callable(DECKY_INSTALL_ROUTE)(...args);
      return true;
    }
  } catch (e) {
    console.error("[RomM] Decky install_plugin call failed:", e);
  }
  return false;
}

/** Open the release page in Steam's browser (manual-download fallback). */
export function openReleasePage(url: string): void {
  try {
    Navigation.NavigateToExternalWeb(url);
  } catch (e) {
    console.error("[RomM] could not open release page:", e);
  }
}
