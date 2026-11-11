import { beforeEach, describe, expect, it, vi } from "vitest";

const doc = document;
vi.mock("./deckyUiInternals", () => ({ findSP: () => ({ window: { document: doc } }) }));

import { hideNativePlaySection } from "./styleInjector";

describe("hideNativePlaySection styles", () => {
  beforeEach(() => {
    doc.head.innerHTML = "";
  });

  it("leaves room below whatever takes the focus on the game page, so the footer bar never covers it", () => {
    hideNativePlaySection("PlaySection");
    const css = doc.getElementById("romm-focus-styles")?.textContent ?? "";
    expect(css).toMatch(/\[data-romm\], \[data-romm\] \* \{[^}]*scroll-margin-bottom: 64px/);
  });

  it("hides the native play section but not ours, once", () => {
    hideNativePlaySection("PlaySection");
    hideNativePlaySection("PlaySection");
    expect(doc.querySelectorAll("#romm-hide-native-play")).toHaveLength(1);
    expect(doc.getElementById("romm-hide-native-play")?.textContent).toContain(".PlaySection:not([data-romm])");
  });
});
