import { beforeEach, describe, expect, it, vi } from "vitest";
import { cloneElement, type ReactElement } from "react";
import { act, fireEvent, render } from "@testing-library/react";
import { showModal } from "@decky/ui";

vi.mock("../api/backend", () => ({ getRomScreenshots: vi.fn(), getRomScreenshot: vi.fn() }));
vi.mock("../utils/scrollHelpers", () => ({ scrollFocusedToCenter: vi.fn() }));

import { getRomScreenshot, getRomScreenshots } from "../api/backend";
import { ScreenshotGallery, ScreenshotViewer } from "./ScreenshotGallery";

const flush = () =>
  act(async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });

describe("ScreenshotGallery", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getRomScreenshot).mockImplementation(async (_rom: number, i: number) => ({ base64: `img${i}`, mime: "image/png" }));
  });

  it("shows nothing when RomM has no screenshots, cannot be reached, or the call fails", async () => {
    for (const outcome of [
      () => vi.mocked(getRomScreenshots).mockResolvedValue({ success: true, count: 0 }),
      () => vi.mocked(getRomScreenshots).mockResolvedValue({ success: false, reason: "server_unreachable" }),
      () => vi.mocked(getRomScreenshots).mockRejectedValue(new Error("x")),
    ]) {
      outcome();
      const { queryByTestId, unmount } = render(<ScreenshotGallery romId={3} />);
      await flush();
      expect(queryByTestId("screenshot-gallery")).toBeNull();
      unmount();
    }
    expect(getRomScreenshot).not.toHaveBeenCalled();
  });

  it("loads the thumbnails one by one with their MIME type; a failed image stays a placeholder", async () => {
    vi.mocked(getRomScreenshots).mockResolvedValue({ success: true, count: 3 });
    vi.mocked(getRomScreenshot).mockImplementation(async (_rom: number, i: number) => {
      if (i === 1) throw new Error("x");
      return { base64: `img${i}`, mime: "image/png" };
    });
    const { getByTestId, getByLabelText } = render(<ScreenshotGallery romId={3} />);
    await flush();
    expect(getByTestId("screenshot-gallery").textContent).toContain("Screenshots");
    expect(vi.mocked(getRomScreenshot).mock.calls).toEqual([[3, 0], [3, 1], [3, 2]]);
    expect(getByLabelText("Screenshot 1").querySelector("img")?.getAttribute("src")).toBe("data:image/png;base64,img0");
    expect(getByLabelText("Screenshot 2").querySelector("img")).toBeNull();
    expect(getByLabelText("Screenshot 2").textContent).toBe("Loading...");
  });

  it("selecting a thumbnail opens the viewer at that screenshot", async () => {
    vi.mocked(getRomScreenshots).mockResolvedValue({ success: true, count: 2 });
    const { getByLabelText } = render(<ScreenshotGallery romId={3} />);
    await flush();
    fireEvent.click(getByLabelText("Screenshot 2"));
    const calls = vi.mocked(showModal).mock.calls;
    const el = calls[calls.length - 1]?.[0] as ReactElement<{ romId: number; start: number; shots: string[] }>;
    expect(el.props.romId).toBe(3);
    expect(el.props.start).toBe(1);
    expect(el.props.shots).toEqual(["data:image/png;base64,img0", "data:image/png;base64,img1"]);
  });
});

describe("ScreenshotViewer", () => {
  beforeEach(() => vi.clearAllMocks());

  it("Previous and Next wrap around, and Close removes the modal (the prop Decky injects is closeModal)", async () => {
    const closeModal = vi.fn();
    const shots = ["data:a", "data:b", "data:c"];
    const { getByText, getByAltText } = render(cloneElement(<ScreenshotViewer romId={3} shots={shots} start={0} />, { closeModal }));
    expect(getByText("1 / 3")).toBeTruthy();
    fireEvent.click(getByText("Previous"));
    expect(getByText("3 / 3")).toBeTruthy();
    expect(getByAltText("Screenshot 3").getAttribute("src")).toBe("data:c");
    fireEvent.click(getByText("Next"));
    expect(getByAltText("Screenshot 1").getAttribute("src")).toBe("data:a");
    fireEvent.click(getByText("Close"));
    expect(closeModal).toHaveBeenCalledTimes(1);
    expect(getRomScreenshot).not.toHaveBeenCalled();
  });

  it("fetches a screenshot whose thumbnail had not loaded yet", async () => {
    vi.mocked(getRomScreenshot).mockResolvedValue({ base64: "late", mime: "image/webp" });
    const { getByText, findByAltText } = render(<ScreenshotViewer romId={3} shots={["data:a", null]} start={1} />);
    expect(getByText("Loading...")).toBeTruthy();
    expect((await findByAltText("Screenshot 2")).getAttribute("src")).toBe("data:image/webp;base64,late");
    expect(getRomScreenshot).toHaveBeenCalledWith(3, 1);
  });

  it("a single screenshot has no Previous / Next to press", () => {
    const { getByText } = render(<ScreenshotViewer romId={3} shots={["data:a"]} start={0} />);
    expect((getByText("Previous") as HTMLButtonElement).disabled).toBe(true);
    expect((getByText("Next") as HTMLButtonElement).disabled).toBe(true);
  });
});
