import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, fireEvent } from "@testing-library/react";
import type { ReactElement } from "react";
import { SaveSyncSection } from "./SaveSyncSection";
import { showModal } from "@decky/ui";
import type { SaveSyncSettings } from "../../types";

const settings = (o: Partial<SaveSyncSettings> = {}): SaveSyncSettings =>
  ({ save_sync_enabled: true, sync_before_launch: true, sync_after_exit: false, default_slot: "default", autocleanup_limit: 10, ...o }) as SaveSyncSettings;

function props(over: Partial<React.ComponentProps<typeof SaveSyncSection>> = {}) {
  return {
    saveSyncSettings: settings(),
    saveSyncToggleKey: 0,
    deviceInfo: { device_id: "d1", device_name: "steamdeck" },
    syncing: false,
    syncStatus: "",
    onToggleSaveSync: vi.fn(),
    onSettingChange: vi.fn(),
    onDefaultSlotSubmit: vi.fn(),
    onResetDefaultSlot: vi.fn(),
    onSyncAll: vi.fn(),
    ...over,
  };
}

function lastOf<T>(items: T[]): T {
  return items[items.length - 1]!;
}

describe("SaveSyncSection", () => {
  beforeEach(() => vi.mocked(showModal).mockClear());

  it("shows Loading until the settings arrive", () => {
    expect(render(<SaveSyncSection {...props({ saveSyncSettings: null })} />).container.textContent).toContain("Loading...");
  });

  it("off: only the master switch and a hint; turning it on is reported", () => {
    const p = props({ saveSyncSettings: settings({ save_sync_enabled: false }) });
    const { getByLabelText, container, queryByLabelText } = render(<SaveSyncSection {...p} />);
    expect(container.textContent).toContain("Save sync is off");
    expect(queryByLabelText("Sync before launch")).toBeNull();
    fireEvent.click(getByLabelText("Save sync"));
    expect(p.onToggleSaveSync).toHaveBeenCalledWith(true);
  });

  it("on: the two switches report their own change, and the device is named", () => {
    const p = props();
    const { getByLabelText, container } = render(<SaveSyncSection {...p} />);
    expect(container.textContent).toContain('"steamdeck"');
    fireEvent.click(getByLabelText("Sync before launch"));
    expect(p.onSettingChange).toHaveBeenLastCalledWith({ sync_before_launch: false });
    fireEvent.click(getByLabelText("Sync after exit"));
    expect(p.onSettingChange).toHaveBeenLastCalledWith({ sync_after_exit: true });
  });

  it("the slot button opens the slot modal; Reset appears only for a changed slot", () => {
    const first = props();
    const a = render(<SaveSyncSection {...first} />);
    expect(a.queryByTestId("reset-default-slot")).toBeNull();
    fireEvent.click(a.getByTestId("edit-default-slot"));
    const el = vi.mocked(showModal).mock.calls[0]![0] as ReactElement<Record<string, unknown>>;
    expect(el.props).toMatchObject({ label: "Default Save Slot", value: "default", onSubmit: first.onDefaultSlotSubmit });
    a.unmount();
    const second = props({ saveSyncSettings: settings({ default_slot: "mine" }) });
    const b = render(<SaveSyncSection {...second} />);
    fireEvent.click(b.getByTestId("reset-default-slot"));
    expect(second.onResetDefaultSlot).toHaveBeenCalledTimes(1);
  });

  it("the history limit cycles through 5, 10, 20, 50 and reports a number", () => {
    const p = props();
    const { getByTestId } = render(<SaveSyncSection {...p} />);
    expect(getByTestId("choice-Save history limit").textContent).toBe("10");
    fireEvent.click(getByTestId("choice-Save history limit"));
    expect(p.onSettingChange).toHaveBeenLastCalledWith({ autocleanup_limit: 20 });
  });

  it("Sync now runs, is disabled while syncing, and the result line shows", () => {
    const p = props({ syncStatus: "12 saves synced" });
    const { getByTestId, container } = render(<SaveSyncSection {...p} />);
    expect(container.textContent).toContain("12 saves synced");
    fireEvent.click(getByTestId("sync-all-saves"));
    expect(p.onSyncAll).toHaveBeenCalledTimes(1);
    const busy = props({ syncing: true });
    const second = render(<SaveSyncSection {...busy} />);
    expect(lastOf(second.getAllByTestId("sync-all-saves")).textContent).toBe("Syncing...");
    fireEvent.click(lastOf(second.getAllByTestId("sync-all-saves")));
    expect(busy.onSyncAll).not.toHaveBeenCalled();
  });
});
