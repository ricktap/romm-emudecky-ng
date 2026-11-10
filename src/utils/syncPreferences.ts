/**
 * Plugin-side sync preferences kept in the frontend (the backend has no setting for them).
 * "Skip preview" applies changes without showing the preview first. It lives in localStorage so the QAM home and
 * the full-screen Settings tab share one value across reopen and restart; storage can be unavailable, so every
 * access is guarded and the default (preview on) applies.
 */

const SKIP_PREVIEW_KEY = "romm-emudecky-ng:skip-sync-preview";

let _listeners: Array<() => void> = [];

export function getSkipPreview(): boolean {
  try {
    return globalThis.localStorage?.getItem(SKIP_PREVIEW_KEY) === "1";
  } catch {
    return false;
  }
}

export function setSkipPreview(value: boolean): void {
  try {
    globalThis.localStorage?.setItem(SKIP_PREVIEW_KEY, value ? "1" : "0");
  } catch {
    // Storage unavailable: the choice then only lasts until the next reload (listeners still see the old value).
  }
  _listeners.forEach((fn) => fn());
}

export function onSyncPreferencesChange(fn: () => void): () => void {
  _listeners.push(fn);
  return () => {
    _listeners = _listeners.filter((l) => l !== fn);
  };
}
