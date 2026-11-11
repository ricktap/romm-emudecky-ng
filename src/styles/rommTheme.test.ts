import { describe, expect, it } from "vitest";
import { ROMM_CSS } from "./rommTheme";

describe("ROMM_CSS footer room", () => {
  it("every control leaves room above and below it when focus scrolls it into view, and the QAM pages end above the footer", () => {
    expect(ROMM_CSS).toMatch(/\.rm-root \*\{[^}]*scroll-margin-top:64px;scroll-margin-bottom:64px/);
    expect(ROMM_CSS).toMatch(/\.rm-lmain,\.rm-prail\{scroll-padding-top:64px;scroll-padding-bottom:64px/);
    expect(ROMM_CSS).toMatch(/\.rm-qam\{padding-bottom:48px\}/);
    expect(ROMM_CSS).toMatch(/\.rm-lib\{[^}]*padding-bottom:40px/);
  });

  it("a focused switch gets a ring, and a focused selected chip or platform no longer disappears into its own white or blue", () => {
    expect(ROMM_CSS).toMatch(/\.rm-sw:hover,\.rm-sw\.gpfocus\{outline:3px solid #fff/);
    expect(ROMM_CSS).toMatch(/\.rm-fchip\.rm-on\.gpfocus\{background:#5cbaff/);
    expect(ROMM_CSS).toMatch(/\.rm-pbtn\.rm-on\.gpfocus\{background:#5cbaff/);
  });
});
