import { beforeEach, describe, expect, it, vi } from "vitest";
import { cloneElement, type ReactElement } from "react";
import { act, fireEvent, render } from "@testing-library/react";
import { toaster } from "@decky/api";
import { showModal } from "@decky/ui";

vi.mock("../api/backend", () => ({ getRomFiles: vi.fn(), downloadRomFiles: vi.fn(), removeRomFiles: vi.fn() }));

import { downloadRomFiles, getRomFiles, removeRomFiles, type RomFileInfo } from "../api/backend";
import { RomFilesModal, actionable, preselected, showRomFilesModal } from "./RomFilesModal";

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
  file(1, "MK8.xci", "present", { base: true }),
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
    vi.mocked(removeRomFiles).mockResolvedValue({ success: true, removed: 1, errors: [], message: "1 file removed" });
  });

  it("preselects what is missing or changed, never what is present or already downloading", () => {
    expect([...preselected(FILES)].sort()).toEqual([2, 3]);
  });

  it("lists every file with size and state; one already downloading cannot be picked", async () => {
    const { getByText } = await open();
    expect(getRomFiles).toHaveBeenCalledWith(30);
    expect(getByText("Mario Kart")).toBeTruthy();
    expect(row(getByText, "MK8.xci").textContent).toBe("☐ MK8.xci · 1.0 MB · on this device · base game");
    expect(row(getByText, "dlc/Booster.nsp").textContent).toContain("☑");
    expect(row(getByText, "dlc/Booster.nsp").textContent).toContain("not on this device");
    expect(row(getByText, "update/MK8 v3.nsp").textContent).toContain("changed on RomM");
    expect(row(getByText, "update/MK8 v2.nsp").textContent).toContain("downloading");
    expect(row(getByText, "update/MK8 v2.nsp").disabled).toBe(true);
    expect(getByText("Download selected (2)")).toBeTruthy();
  });

  it("downloads only the ticked files that are missing or changed, then closes", async () => {
    const { getByText, closeModal } = await open();
    fireEvent.click(row(getByText, "dlc/Booster.nsp"));
    fireEvent.click(row(getByText, "MK8.xci"));
    expect(row(getByText, "dlc/Booster.nsp").textContent).toContain("\u2610");
    // MK8.xci is ticked but already here: nothing to download for it (and as the base game it is never removed).
    await act(async () => {
      fireEvent.click(getByText("Download selected (1)"));
    });
    expect(vi.mocked(downloadRomFiles).mock.calls[0]).toEqual([30, [3]]);
    expect(toaster.toast).toHaveBeenCalledWith(expect.objectContaining({ body: "2 files queued" }));
    expect(closeModal).toHaveBeenCalledTimes(1);
  });

  it("what a ticked file counts for: download when missing/changed, removal when on this device and not the base game", () => {
    const files = [
      file(1, "base.xci", "present", { base: true }),
      file(2, "dlc/a.nsp", "present"),
      file(3, "update/b.nsp", "changed"),
      file(4, "update/c.nsp", "missing"),
      file(5, "update/d.nsp", "present", { downloading: true }),
    ];
    expect(actionable(files, new Set([1, 2, 3, 4, 5]))).toEqual({ download: [3, 4], remove: [2, 3] });
    expect(actionable(files, new Set())).toEqual({ download: [], remove: [] });
  });

  it("removes the ticked files after a confirmation, then reloads the list and stays open", async () => {
    const { getByText, closeModal } = await open();
    // Untick the preselected downloads, tick the changed update for removal only.
    fireEvent.click(row(getByText, "dlc/Booster.nsp"));
    expect(getByText("Remove selected (1)")).toBeTruthy();
    fireEvent.click(getByText("Remove selected (1)"));
    const calls = vi.mocked(showModal).mock.calls;
    const confirm = calls[calls.length - 1]?.[0] as ReactElement<{ strTitle: string; onOK: () => void }>;
    expect(confirm.props.strTitle).toBe("Remove 1 file from this device?");
    expect(removeRomFiles).not.toHaveBeenCalled();
    await act(async () => {
      confirm.props.onOK();
    });
    await flush();
    expect(vi.mocked(removeRomFiles).mock.calls[0]).toEqual([30, [3]]);
    expect(toaster.toast).toHaveBeenCalledWith(expect.objectContaining({ body: "1 file removed" }));
    expect(getRomFiles).toHaveBeenCalledTimes(2);
    expect(closeModal).not.toHaveBeenCalled();
  });

  it("a refused or failed removal says why", async () => {
    vi.mocked(removeRomFiles).mockResolvedValueOnce({ success: false, reason: "downloading", message: "This file is downloading; cancel the download first" });
    const { getByText } = await open();
    fireEvent.click(getByText("Remove selected (1)"));
    let calls = vi.mocked(showModal).mock.calls;
    await act(async () => {
      (calls[calls.length - 1]?.[0] as ReactElement<{ onOK: () => void }>).props.onOK();
    });
    expect(toaster.toast).toHaveBeenCalledWith(expect.objectContaining({ body: "This file is downloading; cancel the download first" }));
    expect(getRomFiles).toHaveBeenCalledTimes(1);
    vi.mocked(removeRomFiles).mockRejectedValueOnce(new Error("socket"));
    fireEvent.click(getByText("Remove selected (1)"));
    calls = vi.mocked(showModal).mock.calls;
    await act(async () => {
      (calls[calls.length - 1]?.[0] as ReactElement<{ onOK: () => void }>).props.onOK();
    });
    expect(toaster.toast).toHaveBeenCalledWith(expect.objectContaining({ body: "Could not remove the files" }));
  });

  it("nothing chosen: both buttons are disabled", async () => {
    const { getByText } = await open();
    fireEvent.click(row(getByText, "dlc/Booster.nsp"));
    fireEvent.click(row(getByText, "update/MK8 v3.nsp"));
    expect((getByText("Download selected (0)") as HTMLButtonElement).disabled).toBe(true);
    expect((getByText("Remove selected (0)") as HTMLButtonElement).disabled).toBe(true);
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
