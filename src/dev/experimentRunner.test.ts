import { describe, expect, it } from "vitest";
import { cleanup, runPhase1, runPhase2, type ExperimentEnv } from "./experimentRunner";
import { steamShortcutId } from "../utils/shortcutId";

/** A fake Steam that behaves as the experiment hopes: the id is fixed at creation, a rename only changes the name. */
function fakeSteam(renameMakesNewShortcut: boolean) {
  const apps = new Map<number, string>();
  const removed: number[] = [];
  const exe = "/home/deck/homebrew/plugins/romm-emudecky-ng/bin/rom-launcher-ng";
  const env: ExperimentEnv = {
    exe,
    addShortcut: async (name, path) => {
      const id = steamShortcutId(path, name);
      apps.set(id, name);
      return id;
    },
    rename: (id, name) => {
      if (renameMakesNewShortcut) {
        apps.set(steamShortcutId(exe, name), name);
        apps.delete(id);
      } else apps.set(id, name);
    },
    remove: (id) => {
      removed.push(id);
      apps.delete(id);
    },
    displayName: (id) => apps.get(id) ?? null,
    listShortcutIds: () => [...apps.keys()],
    wait: async () => {},
  };
  return { env, apps, removed };
}

describe("shortcut id experiment", () => {
  it("phase 1 reports an exact forced id that survives a rename, and phase 2 confirms it", async () => {
    const { env } = fakeSteam(false);
    const { lines, state } = await runPhase1(env);
    const text = lines.join("\n");
    expect(text).toContain("A: formula crc32(exe+name)|top bit");
    expect(text).toMatch(/-> MATCH/);
    expect(text).toContain("B: RESULT rename KEEPS the id");
    expect(text).toContain("EXACT MATCH");
    expect(text).toContain("still listed: true");
    expect(state).not.toBeNull();
    const phase2 = runPhase2(env, state!).join("\n");
    expect(phase2).toContain("RESULT: the forced id survived the restart under the real name.");
    expect(phase2).toContain('name "romm-idtest-C-renamed"');
  });

  it("detects a Steam where a rename creates a new shortcut", async () => {
    const { env } = fakeSteam(true);
    const { lines, state } = await runPhase1(env);
    expect(lines.join("\n")).toContain("B: RESULT rename does NOT simply keep the id");
    expect(lines.join("\n")).toContain("still listed: false");
    expect(runPhase2(env, state!).join("\n")).toContain("did NOT survive");
  });

  it("stops when AddShortcut gives no id", async () => {
    const { env } = fakeSteam(false);
    const out = await runPhase1({ ...env, addShortcut: async () => null });
    expect(out.state).toBeNull();
    expect(out.lines.join("\n")).toContain("AddShortcut returned no id");
  });

  it("cleanup removes the test shortcuts and nothing else", async () => {
    const { env, apps, removed } = fakeSteam(true);
    apps.set(111, "Zelda");
    const { state } = await runPhase1(env);
    const out = cleanup(env, state);
    expect(out[0]).toMatch(/^Removed \d+ test shortcut/);
    expect(removed).not.toContain(111);
    expect(apps.get(111)).toBe("Zelda");
    expect([...apps.values()].some((n) => n.startsWith("romm-idtest"))).toBe(false);
  });

  it("cleanup without a stored state still removes shortcuts carrying the test name", () => {
    const { env, apps } = fakeSteam(false);
    apps.set(5, "romm-idtest-leftover");
    apps.set(6, "Real game");
    cleanup(env, null);
    expect([...apps.keys()]).toEqual([6]);
  });
});
