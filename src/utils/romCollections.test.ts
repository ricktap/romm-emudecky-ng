import { describe, it, expect } from "vitest";
import { parseCollectionName, readRommCollections } from "./romCollections";

describe("romCollections", () => {
  it("parses only this device's bracketed collections", () => {
    expect(parseCollectionName("RomM: [Favourites] (deck)", "deck")).toBe("Favourites");
    expect(parseCollectionName("RomM: [Favourites] (bazzite)", "deck")).toBeNull();
    expect(parseCollectionName("RomM: Switch (deck)", "deck")).toBeNull();
    expect(parseCollectionName("RomM: [] (deck)", "deck")).toBeNull();
    expect(parseCollectionName("Other", "deck")).toBeNull();
  });

  it("reads sorted collections with their app ids and skips the rest", () => {
    const result = readRommCollections(
      [
        { displayName: "RomM: [Zelda] (deck)", allApps: [{ appid: 3 }, { appid: 4 }] },
        { displayName: "RomM: Switch (deck)", allApps: [{ appid: 9 }] },
        { displayName: "RomM: [Arcade] (deck)", allApps: [{ appid: 1 }] },
      ],
      "deck",
    );
    expect(result.map((c) => c.name)).toEqual(["Arcade", "Zelda"]);
    expect([...result[1]!.appIds]).toEqual([3, 4]);
  });
});
