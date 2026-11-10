/**
 * Temporary device experiment for the stable shortcut id (see docs/plans): what does THIS Steam build do with
 * `AddShortcut`, with a rename, and across a restart? Phase 1 creates test shortcuts and reports; phase 2 (after a
 * Steam restart) reports what became of them; cleanup removes them. All logic here is pure over an injected `Env`.
 */

import { forcedName, stableGameId, steamShortcutId, steamShortcutIdQuoted } from "../utils/shortcutId";

export interface ExperimentEnv {
  /** The executable path to create the shortcuts with (the plugin's launcher, like a real sync does). */
  exe: string;
  addShortcut(name: string, exe: string): Promise<number | null>;
  rename(appId: number, name: string): void;
  remove(appId: number): void;
  /** Steam's display name for an app id, or null when Steam does not know it. */
  displayName(appId: number): string | null;
  /** Every non-Steam shortcut app id Steam currently lists. */
  listShortcutIds(): number[];
  wait(ms: number): Promise<void>;
}

export interface ExperimentState {
  exe: string;
  plain: { created: number; renamedTo: string; formulaPlain: number; formulaQuoted: number };
  forced: { target: number; created: number; renamedTo: string; creationName: string };
}

const TEST_PREFIX = "romm-idtest";
const SETTLE_MS = 1500;

const hex = (n: number) => `0x${(n >>> 0).toString(16)}`;

/** Phase 1. Returns the report lines and the state phase 2 needs. */
export async function runPhase1(env: ExperimentEnv): Promise<{ lines: string[]; state: ExperimentState | null }> {
  const lines: string[] = [`exe: ${env.exe}`];
  const before = new Set(env.listShortcutIds());

  // A. Which formula does Steam use for the id of a shortcut created through the API?
  const nameA = `${TEST_PREFIX}-A`;
  const idA = await env.addShortcut(nameA, env.exe);
  if (!idA) return { lines: [...lines, "A: AddShortcut returned no id: stopping"], state: null };
  await env.wait(SETTLE_MS);
  const plain = steamShortcutId(env.exe, nameA);
  const quoted = steamShortcutIdQuoted(env.exe, nameA);
  lines.push(`A: AddShortcut(name, exe) gave id ${idA} (${hex(idA)})`);
  lines.push(`A: formula crc32(exe+name)|top bit = ${plain} -> ${idA === plain ? "MATCH" : "no match"}`);
  lines.push(`A: formula with quoted exe = ${quoted} -> ${idA === quoted ? "MATCH" : "no match"}`);

  // B. Does a rename keep the id, or make a new shortcut?
  const renamedA = `${TEST_PREFIX}-A-renamed`;
  env.rename(idA, renamedA);
  await env.wait(SETTLE_MS);
  const afterRename = steamShortcutId(env.exe, renamedA);
  const created = env.listShortcutIds().filter((id) => !before.has(id));
  lines.push(`B: after rename, old id ${idA} -> name "${env.displayName(idA)}"`);
  lines.push(`B: a shortcut with the id of the new name (${afterRename}) exists: ${created.includes(afterRename)}`);
  lines.push(`B: new shortcuts since the start: ${created.join(", ") || "none"}`);
  lines.push(
    created.includes(idA) && !created.includes(afterRename)
      ? "B: RESULT rename KEEPS the id"
      : "B: RESULT rename does NOT simply keep the id (see the lines above)",
  );

  // C. Force a target id through the creation name, then rename.
  const target = stableGameId("experiment-1");
  const creationName = forcedName(env.exe, `${TEST_PREFIX}-C-`, target);
  if (!creationName) return { lines: [...lines, "C: could not solve a name for the target: stopping"], state: null };
  const idC = await env.addShortcut(creationName, env.exe);
  await env.wait(SETTLE_MS);
  lines.push(`C: target id ${target} (${hex(target)}), created id ${idC} -> ${idC === target ? "EXACT MATCH" : "no match"}`);
  const renamedC = `${TEST_PREFIX}-C-renamed`;
  if (idC) {
    env.rename(idC, renamedC);
    await env.wait(SETTLE_MS);
    const known = env.listShortcutIds().includes(idC);
    lines.push(`C: after rename to "${renamedC}": id ${idC} still listed: ${known}, name "${env.displayName(idC)}"`);
    lines.push(`C: a shortcut with the id of the renamed name (${steamShortcutId(env.exe, renamedC)}) exists: ${env.listShortcutIds().includes(steamShortcutId(env.exe, renamedC))}`);
  }
  lines.push("", "Now RESTART STEAM completely, reopen the RomM Library > Settings tab and run phase 2.");

  return {
    lines,
    state: {
      exe: env.exe,
      plain: { created: idA, renamedTo: renamedA, formulaPlain: plain, formulaQuoted: quoted },
      forced: { target, created: idC ?? 0, renamedTo: renamedC, creationName },
    },
  };
}

/** Phase 2, after a Steam restart: what survived, under which id and name. */
export function runPhase2(env: ExperimentEnv, state: ExperimentState): string[] {
  const ids = new Set(env.listShortcutIds());
  const lines: string[] = [];
  const check = (label: string, id: number, expectedName: string) => {
    lines.push(`${label}: id ${id} listed: ${ids.has(id)}, name "${env.displayName(id)}" (expected "${expectedName}")`);
  };
  check("A (renamed)", state.plain.created, state.plain.renamedTo);
  check("C (forced, renamed)", state.forced.created, state.forced.renamedTo);
  const reCrc = [
    ["A", steamShortcutId(state.exe, state.plain.renamedTo)],
    ["C", steamShortcutId(state.exe, state.forced.renamedTo)],
  ] as const;
  for (const [label, id] of reCrc) lines.push(`${label}: a shortcut with the id of the renamed name (${id}) exists: ${ids.has(id)}`);
  lines.push(
    ids.has(state.forced.created) && state.forced.created === state.forced.target
      ? "RESULT: the forced id survived the restart under the real name."
      : "RESULT: the forced id did NOT survive as expected.",
  );
  return lines;
}

/** Remove every test shortcut (by the ids we know and by the test name prefix). */
export function cleanup(env: ExperimentEnv, state: ExperimentState | null): string[] {
  const remove = new Set<number>();
  // Only what we created: the ids we got back, plus anything still carrying the test name (a rename that made a new shortcut).
  if (state) for (const id of [state.plain.created, state.forced.created]) if (id) remove.add(id);
  for (const id of env.listShortcutIds()) if (env.displayName(id)?.startsWith(TEST_PREFIX)) remove.add(id);
  for (const id of remove) env.remove(id);
  return [`Removed ${remove.size} test shortcut(s): ${[...remove].join(", ") || "none"}`];
}
