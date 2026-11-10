import { FC } from "react";
import { AttRow } from "./rm";
import { setPlaytimeScopeState } from "../utils/playtimeScopeStore";

/** Title of the account-wide QAM playtime-scope banner. */
export const PLAYTIME_SCOPE_TITLE = "Cross-device playtime";

/** Body text prompting the user to re-mint a scoped Client API Token. */
export const PLAYTIME_SCOPE_MESSAGE = "Sign in again to enable cross-device playtime sync.";

/**
 * QAM attention row shown while the Client API Token lacks the `roms.user.read`
 * scope needed to read cross-device playtime. Account-wide (no per-game card).
 *
 * Dismiss is local-only — pressing the row just clears the shared store for this
 * view. There is no backend dismiss callable: the durable flag clears itself
 * once a scoped token is minted (a fresh sign-in or a later successful
 * reconcile), so the next MainPage mount re-fetches the real state.
 */
export const PlaytimeScopeBanner: FC = () => (
  <AttRow
    title={PLAYTIME_SCOPE_TITLE}
    sub={`${PLAYTIME_SCOPE_MESSAGE} Press to dismiss.`}
    onActivate={() => setPlaytimeScopeState({ pending: false })}
  />
);
