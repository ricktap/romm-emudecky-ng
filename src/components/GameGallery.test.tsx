import { beforeEach, describe, expect, it, vi } from "vitest";
import { cloneElement, type ReactElement } from "react";
import { act, fireEvent, render } from "@testing-library/react";
import { showModal } from "@decky/ui";

vi.mock("../api/backend", () => ({
  getRomScreenshots: vi.fn(),
  getRomScreenshot: vi.fn(),
  getSgdbGallery: vi.fn(),
  getSgdbGalleryImage: vi.fn(),
}));
vi.mock("../utils/scrollHelpers", () => ({ scrollFocusedToCenter: vi.fn() }));

import { getRomScreenshot, getRomScreenshots, getSgdbGallery, getSgdbGalleryImage } from "../api/backend";
import { GameGallery, GalleryViewer, type GalleryItem } from "./GameGallery";

const flush = () =>
  act(async () => {
    for (let i = 0; i < 20; i++) await Promise.resolve();
  });

function counts(sgdb: number | Error, romm: number | Error) {
  const answer = (v: number | Error) =>
    v instanceof Error ? Promise.reject(v) : Promise.resolve({ success: true, count: v });
  vi.mocked(getSgdbGallery).mockImplementation(() => answer(sgdb));
  vi.mocked(getRomScreenshots).mockImplementation(() => answer(romm));
}

describe("GameGallery", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getSgdbGalleryImage).mockImplementation(async (_rom: number, i: number) => ({ base64: `art${i}`, mime: "image/png" }));
    vi.mocked(getRomScreenshot).mockImplementation(async (_rom: number, i: number) => ({ base64: `shot${i}`, mime: "image/jpeg" }));
  });

  it("shows nothing when neither SteamGridDB nor RomM has images (or both fail)", async () => {
    for (const [sgdb, romm] of [[0, 0], [new Error("x"), new Error("y")]] as const) {
      counts(sgdb, romm);
      const { queryByTestId, unmount } = render(<GameGallery romId={3} />);
      await flush();
      expect(queryByTestId("game-gallery")).toBeNull();
      unmount();
    }
    vi.mocked(getSgdbGallery).mockResolvedValue({ success: false, reason: "no_api_key" });
    vi.mocked(getRomScreenshots).mockResolvedValue({ success: false, reason: "server_unreachable" });
    const { queryByTestId } = render(<GameGallery romId={3} />);
    await flush();
    expect(queryByTestId("game-gallery")).toBeNull();
    expect(getSgdbGalleryImage).not.toHaveBeenCalled();
    expect(getRomScreenshot).not.toHaveBeenCalled();
  });

  it("puts the SteamGridDB images first, then the screenshots, loading them one by one", async () => {
    counts(2, 1);
    const { getByTestId, getByLabelText } = render(<GameGallery romId={3} />);
    await flush();
    expect(getByTestId("game-gallery").textContent).toContain("Gallery");
    const src = (n: number) => getByLabelText(`Image ${n}`).querySelector("img")?.getAttribute("src");
    expect([src(1), src(2), src(3)]).toEqual([
      "data:image/png;base64,art0",
      "data:image/png;base64,art1",
      "data:image/jpeg;base64,shot0",
    ]);
    expect(vi.mocked(getSgdbGalleryImage).mock.calls).toEqual([[3, 0], [3, 1]]);
    expect(vi.mocked(getRomScreenshot).mock.calls).toEqual([[3, 0]]);
  });

  it("one source failing still shows the other; a failed image stays a placeholder", async () => {
    counts(new Error("sgdb down"), 2);
    vi.mocked(getRomScreenshot).mockImplementation(async (_rom: number, i: number) => {
      if (i === 0) throw new Error("x");
      return { base64: `shot${i}`, mime: "image/jpeg" };
    });
    const { getByLabelText } = render(<GameGallery romId={3} />);
    await flush();
    expect(getByLabelText("Image 1").textContent).toBe("Loading...");
    expect(getByLabelText("Image 2").querySelector("img")?.getAttribute("src")).toBe("data:image/jpeg;base64,shot1");
  });

  it("selecting a thumbnail opens the viewer at that image", async () => {
    counts(1, 1);
    const { getByLabelText } = render(<GameGallery romId={3} />);
    await flush();
    fireEvent.click(getByLabelText("Image 2"));
    const calls = vi.mocked(showModal).mock.calls;
    const el = calls[calls.length - 1]?.[0] as ReactElement<{ romId: number; start: number; items: GalleryItem[]; shots: string[] }>;
    expect(el.props).toMatchObject({
      romId: 3,
      start: 1,
      items: [{ source: "sgdb", index: 0 }, { source: "romm", index: 0 }],
      shots: ["data:image/png;base64,art0", "data:image/jpeg;base64,shot0"],
    });
  });
});

describe("GalleryViewer", () => {
  const items: GalleryItem[] = [
    { source: "sgdb", index: 0 },
    { source: "sgdb", index: 1 },
    { source: "romm", index: 0 },
  ];
  beforeEach(() => vi.clearAllMocks());

  it("Previous and Next wrap around, and Close removes the modal (the prop Decky injects is closeModal)", () => {
    const closeModal = vi.fn();
    const viewer = <GalleryViewer romId={3} items={items} shots={["data:a", "data:b", "data:c"]} start={0} />;
    const { getByText, getByAltText } = render(cloneElement(viewer, { closeModal }));
    expect(getByText("1 / 3")).toBeTruthy();
    fireEvent.click(getByText("Previous"));
    expect(getByText("3 / 3")).toBeTruthy();
    expect(getByAltText("Image 3").getAttribute("src")).toBe("data:c");
    fireEvent.click(getByText("Next"));
    expect(getByAltText("Image 1").getAttribute("src")).toBe("data:a");
    fireEvent.click(getByText("Close"));
    expect(closeModal).toHaveBeenCalledTimes(1);
  });

  it("fetches an image whose thumbnail had not loaded yet, from the right source", async () => {
    vi.mocked(getRomScreenshot).mockResolvedValue({ base64: "late", mime: "image/webp" });
    const { getByText, findByAltText } = render(
      <GalleryViewer romId={3} items={items} shots={["data:a", "data:b", null]} start={2} />,
    );
    expect(getByText("Loading...")).toBeTruthy();
    expect((await findByAltText("Image 3")).getAttribute("src")).toBe("data:image/webp;base64,late");
    expect(getRomScreenshot).toHaveBeenCalledWith(3, 0);
    expect(getSgdbGalleryImage).not.toHaveBeenCalled();
  });

  it("a single image has no Previous / Next to press", () => {
    const { getByText } = render(<GalleryViewer romId={3} items={items.slice(0, 1)} shots={["data:a"]} start={0} />);
    expect((getByText("Previous") as HTMLButtonElement).disabled).toBe(true);
    expect((getByText("Next") as HTMLButtonElement).disabled).toBe(true);
  });
});
