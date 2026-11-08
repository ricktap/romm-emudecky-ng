/**
 * Module-level self-update state shared by the QAM banner and the Settings
 * "Updates" section.
 *
 * `info` keeps the last successful check, so a later failed check (offline,
 * rate-limited) does not hide an update the user was already told about;
 * `error` carries the failed check's message.
 */

import { checkForUpdate } from "../api/backend";
import type { UpdateCheckSuccess } from "../types";

export interface UpdateState {
  info: UpdateCheckSuccess | null;
  error: string | null;
  currentVersion: string | null;
  checkedAt: string | null;
  checking: boolean;
}

const INITIAL: UpdateState = { info: null, error: null, currentVersion: null, checkedAt: null, checking: false };

let _state: UpdateState = { ...INITIAL };
let _listeners: Array<() => void> = [];

function setState(next: UpdateState): void {
  _state = next;
  _listeners.forEach((fn) => fn());
}

export function getUpdateState(): UpdateState {
  return _state;
}

export function onUpdateStateChange(fn: () => void): () => void {
  _listeners.push(fn);
  return () => {
    _listeners = _listeners.filter((l) => l !== fn);
  };
}

/** Test helper — reset to the initial empty state. */
export function resetUpdateState(): void {
  setState({ ...INITIAL });
}

/**
 * Run a check (`force` bypasses the backend's 6 h throttle) and fold the
 * result into the store. Rejections are folded into `error` too, so callers
 * never need their own catch.
 */
export async function refreshUpdateState(force: boolean): Promise<UpdateState> {
  setState({ ..._state, checking: true });
  try {
    const result = await checkForUpdate(force);
    if (!result) {
      setState({ ..._state, checking: false });
    } else if (result.success) {
      setState({
        info: result,
        error: null,
        currentVersion: result.current_version,
        checkedAt: result.checked_at,
        checking: false,
      });
    } else {
      setState({
        ..._state,
        error: result.message || "Could not check for updates",
        currentVersion: result.current_version || _state.currentVersion,
        checkedAt: result.checked_at || _state.checkedAt,
        checking: false,
      });
    }
  } catch (e) {
    setState({ ..._state, error: `Could not check for updates: ${e}`, checking: false });
  }
  return _state;
}
