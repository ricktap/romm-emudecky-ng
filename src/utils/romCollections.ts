/**
 * The RomM collections (not platforms) this device carries as Steam collections, read back for the library grid.
 * The sync creates them as "RomM: [Name] (hostname)"; only this device's are listed.
 */

import { getHostname } from "./collections";

export interface RommCollection {
  name: string;
  /** Steam app ids of the shortcuts in it. */
  appIds: ReadonlySet<number>;
}

/** The collection name inside `RomM: [Name] (hostname)`, or null for any other collection. */
export function parseCollectionName(displayName: string, hostname: string): string | null {
  const prefix = "RomM: [";
  const suffix = `] (${hostname})`;
  if (!displayName.startsWith(prefix) || !displayName.endsWith(suffix)) return null;
  const name = displayName.slice(prefix.length, displayName.length - suffix.length);
  return name === "" ? null : name;
}

export function readRommCollections(
  userCollections: readonly { displayName: string; allApps: readonly { appid: number }[] }[],
  hostname: string,
): RommCollection[] {
  const found: RommCollection[] = [];
  for (const c of userCollections) {
    const name = parseCollectionName(c.displayName, hostname);
    if (name !== null) found.push({ name, appIds: new Set(c.allApps.map((a) => a.appid)) });
  }
  return found.sort((a, b) => a.name.localeCompare(b.name));
}

/** This device's RomM collections from Steam; empty when Steam's collection store is not available. */
export async function loadRommCollections(): Promise<RommCollection[]> {
  if (typeof collectionStore === "undefined") return [];
  try {
    return readRommCollections(collectionStore.userCollections, await getHostname());
  } catch {
    return [];
  }
}
