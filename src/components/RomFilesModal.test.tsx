import { beforeEach, describe, expect, it, vi } from "vitest";
import { cloneElement, type ReactElement } from "react";
import { act, fireEvent, render } from "@testing-library/react";
import { toaster } from "@decky/api";
import { showModal } from "@decky/ui";

vi.mock("../api/backend", () => ({ getRomFiles: vi.fn(), downloadRomFiles: vi.fn() }));

import { downloadRomFiles, getRomFiles, type RomFileInfo } from "../api/backend";
import { RomFilesModal, preselected, showRomFilesModal } from "./RomFilesModal";

const file = (file_id: number, path: string, state: RomFileInfo["state"], over: Partial<RomFileInfo> = {}): RomFileInfo => ({
  file_id,
  path,
  category: path.split("/")[0] ?? "game",
  size: 1024 * 1024,
  state,
  downloading: false,
  ...over,
});

const FILES = [
  file(1, "MK8.xci", "present"),
  file(2, "dlc/Booster.nsp", "missing"),
  file(3, "update/MK8 v3.nsp", "changed"),
  file(4, "update/MK8 v2.nsp", "missing", { downloading: true }),
];

const flush = () =>
  act(async () => {
    for (let i = 0; i < 5; i++) await Promise.resolve();
  });

async function open(closeModal = vi.fn()) {
  const utils = render(<RomFilesModal romId={30} romName="Mario Kart" closeModal={closeModal} />);
  await flush();
  return { ...utils, closeModal };
}

const row = (getByText: (t: RegExp) => HTMLElement, path: string) =>
  getByText(new RegExp(path.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))) as HTMLButtonElement;

describe("RomFilesModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getRomFiles).mockResolvedValue({ success: true, rom_id: 30, files: FILES });
    vi.mocked(downloadRomFiles).mockResolvedValue({ success: true, started: 2, errors: [], message: "2 files queued" });
  });

  it("preselects what is missing or changed, never what is present or already downloading", () => {
    expect([...preselected(FILES)].sort()).toEqual([2, 3]);
  });

  it("lists every file with size and state; one already downloading cannot be picked", async () => {
    const { getByText } = await open();
    expect(getRomFiles).toHaveBeenCalledWith(30);
    expect(getByText("Mario Kart")).toBeTruthy();
    expect(row(getByText, "MK8.xci").textContent).toBe("☐ MK8.xci · 1.0 MB · on this device");
    expect(row(getByText, "dlc/Booster.nsp").textContent).toContain("☑");
    expect(row(getByText, "dlc/Booster.nsp").textContent).toContain("not on this device");
    expect(row(getByText, "update/MK8 v3.nsp").textContent).toContain("changed on RomM");
    expect(row(getByText, "update/MK8 v2.nsp").textContent).toContain("downloading");
    expect(row(getByText, "update/MK8 v2.nsp").disabled).toBe(true);
    expect(getByText("Download selected (2)")).toBeTruthy();
  });

  it("toggles files and downloads exactly the chosen ones, then closes", async () => {
    const { getByText, closeModal } = await open();
    fireEvent.click(row(getByText, "dlc/Booster.nsp"));
    fireEvent.click(row(getByText, "MK8.xci"));
    expect(row(getByText, "dlc/Booster.nsp").textContent).toContain("☐");
    await act(async () => {
      fireEvent.click(getByText("Download selected (2)"));
    });
    expect(vi.mocked(downloadRomFiles).mock.calls[0]?.[0]).toBe(30);
    expect([...(vi.mocked(downloadRomFiles).mock.calls[0]?.[1] ?? [])].sort()).toEqual([1, 3]);
    expect(toaster.toast).toHaveBeenCalledWith(expect.objectContaining({ body: "2 files queued" }));
    expect(closeModal).toHaveBeenCalledTimes(1);
  });

  it("nothing chosen: the download button is disabled", async () => {
    const { getByText } = await open();
    fireEvent.click(row(getByText, "dlc/Booster.nsp"));
    fireEvent.click(row(getByText, "update/MK8 v3.nsp"));
    expect((getByText("Download selected (0)") as HTMLButtonElement).disabled).toBe(true);
  });

  it("a refused or failed download says why and keeps the modal open", async () => {
    vi.mocked(downloadRomFiles).mockResolvedValueOnce({ success: false, reason: "insufficient_space", message: "Not enough disk space" });
    const { getByText, closeModal } = await open();
    await act(async () => {
      fireEvent.click(getByText("Download selected (2)"));
    });
    expect(toaster.toast).toHaveBeenCalledWith(expect.objectContaining({ body: "Not enough disk space" }));
    expect(closeModal).not.toHaveBeenCalled();
    vi.mocked(downloadRomFiles).mockRejectedValueOnce(new Error("socket"));
    await act(async () => {
      fireEvent.click(getByText("Download selected (2)"));
    });
    expect(toaster.toast).toHaveBeenCalledWith(expect.objectContaining({ body: "Could not start the download" }));
    expect((getByText("Download selected (2)") as HTMLButtonElement).disabled).toBe(false);
  });

  it("a game without a folder or an unreachable server shows the reason with a retry", async () => {
    vi.mocked(getRomFiles).mockResolvedValueOnce({ success: false, reason: "not_installed", message: "Only a game installed as a folder has separate files" });
    const { getByText } = await open();
    const retry = getByText("Only a game installed as a folder has separate files. Try again");
    vi.mocked(getRomFiles).mockRejectedValueOnce(new Error("x"));
    await act(async () => {
      fireEvent.click(retry);
    });
    await flush();
    await act(async () => {
      fireEvent.click(getByText("Could not load the game's files. Try again"));
    });
    await flush();
    expect(getByText("Download selected (2)")).toBeTruthy();
  });

  it("opened through showModal, Close removes it (the prop Decky injects is closeModal)", async () => {
    showRomFilesModal(30, "Mario Kart");
    const calls = vi.mocked(showModal).mock.calls;
    const el = calls[calls.length - 1]?.[0] as ReactElement<{ closeModal?: () => void }>;
    const closeModal = vi.fn();
    const { getByText } = render(cloneElement(el, { closeModal }));
    await flush();
    fireEvent.click(getByText("Close"));
    expect(closeModal).toHaveBeenCalledTimes(1);
  });
});
