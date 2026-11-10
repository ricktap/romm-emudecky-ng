import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, fireEvent } from "@testing-library/react";

vi.mock("../../utils/updateStore", () => {
  const state = { currentVersion: "0.52.0", info: null as null | Record<string, unknown>, error: null as string | null, checking: false, checkedAt: null as string | null };
  return {
    getUpdateState: () => state,
    onUpdateStateChange: () => () => undefined,
    refreshUpdateState: vi.fn().mockResolvedValue(undefined),
    __state: state,
  };
});
vi.mock("../UpdateBanner", () => ({ UpdateNotice: () => <div data-testid="update-notice" /> }));

import * as store from "../../utils/updateStore";
import { PluginSection, updateStatusText } from "./PluginSection";

const mockState = (store as unknown as { __state: Record<string, unknown> }).__state;

describe("updateStatusText", () => {
  it("covers checking, error, never checked, update, up to date and no release", () => {
    const base = { currentVersion: "1", info: null, error: null, checking: false, checkedAt: null } as unknown as Record<string, unknown>;
    expect(updateStatusText({ ...base, checking: true } as never)).toBe("Checking…");
    expect(updateStatusText({ ...base, error: "offline" } as never)).toBe("offline");
    expect(updateStatusText(base as never)).toBe("Not checked yet");
    expect(updateStatusText({ ...base, info: { update_available: true, latest_version: "2.0" } } as never)).toBe("Update available: v2.0");
    expect(updateStatusText({ ...base, info: { update_available: false, latest_version: "1.0" } } as never)).toBe("Up to date (latest release v1.0)");
    expect(updateStatusText({ ...base, info: { update_available: false, latest_version: "" } } as never)).toBe("No installable release found");
  });
});

describe("PluginSection", () => {
  beforeEach(() => {
    Object.assign(mockState, { info: null, error: null, checking: false, checkedAt: null });
    vi.mocked(store.refreshUpdateState).mockClear();
  });

  it("shows the installed version and checks for updates on demand", () => {
    const { container, getByTestId } = render(<PluginSection logLevel="info" onLogLevelChange={vi.fn()} dangerOpen={false} onToggleDanger={vi.fn()} />);
    expect(container.textContent).toContain("v0.52.0");
    fireEvent.click(getByTestId("check-updates"));
    expect(store.refreshUpdateState).toHaveBeenCalledWith(true);
  });

  it("shows the update notice when one is available", () => {
    mockState.info = { update_available: true, latest_version: "0.53.0" };
    const { getByTestId } = render(<PluginSection logLevel="info" onLogLevelChange={vi.fn()} dangerOpen={false} onToggleDanger={vi.fn()} />);
    expect(getByTestId("update-notice")).toBeTruthy();
  });

  it("the log level cycles and the danger zone button toggles", () => {
    const onLogLevelChange = vi.fn();
    const onToggleDanger = vi.fn();
    const { getByTestId, rerender } = render(<PluginSection logLevel="info" onLogLevelChange={onLogLevelChange} dangerOpen={false} onToggleDanger={onToggleDanger} />);
    fireEvent.click(getByTestId("choice-Log level"));
    expect(onLogLevelChange).toHaveBeenCalledWith("debug");
    expect(getByTestId("danger-zone").textContent).toBe("Open");
    fireEvent.click(getByTestId("danger-zone"));
    expect(onToggleDanger).toHaveBeenCalledTimes(1);
    rerender(<PluginSection logLevel="info" onLogLevelChange={onLogLevelChange} dangerOpen onToggleDanger={onToggleDanger} />);
    expect(getByTestId("danger-zone").textContent).toBe("Close");
  });
});
