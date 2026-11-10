import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, fireEvent } from "@testing-library/react";
import type { ReactElement } from "react";
import { SteamGridDBSection } from "./SteamGridDBSection";
import { showModal } from "@decky/ui";

function props(over: Partial<React.ComponentProps<typeof SteamGridDBSection>> = {}) {
  return {
    sgdbApiKey: "",
    sgdbStatus: "",
    sgdbVerifying: false,
    onSubmitKey: vi.fn(),
    onVerifyKey: vi.fn(),
    steamInputMode: "default",
    steamInputStatus: "",
    loading: false,
    onModeChange: vi.fn(),
    onApplyMode: vi.fn(),
    ...over,
  };
}

function lastOf<T>(items: T[]): T {
  return items[items.length - 1]!;
}

describe("SteamGridDBSection (artwork & controller)", () => {
  beforeEach(() => vi.mocked(showModal).mockClear());

  it("without a key it offers to set one, hints why, and cannot verify", () => {
    const p = props();
    const { getByTestId, container } = render(<SteamGridDBSection {...p} />);
    expect(getByTestId("edit-sgdb-key").textContent).toBe("Set key");
    expect(container.textContent).toContain("Needed for hero, logo and wide artwork");
    fireEvent.click(getByTestId("verify-sgdb-key"));
    expect(p.onVerifyKey).not.toHaveBeenCalled();
  });

  it("editing opens a password modal that submits the key", () => {
    const p = props({ sgdbApiKey: "x" });
    const { getByTestId } = render(<SteamGridDBSection {...p} />);
    expect(getByTestId("edit-sgdb-key").textContent).toBe("•••• Edit");
    fireEvent.click(getByTestId("edit-sgdb-key"));
    const el = vi.mocked(showModal).mock.calls[0]![0] as ReactElement<Record<string, unknown>>;
    expect(el.props).toMatchObject({ label: "SteamGridDB API Key", bIsPassword: true, onSubmit: p.onSubmitKey });
  });

  it("verifies a set key, and shows 'Verifying...' meanwhile", () => {
    const p = props({ sgdbApiKey: "x" });
    const first = render(<SteamGridDBSection {...p} />);
    fireEvent.click(first.getByTestId("verify-sgdb-key"));
    expect(p.onVerifyKey).toHaveBeenCalledTimes(1);
    first.unmount();
    const busy = render(<SteamGridDBSection {...props({ sgdbApiKey: "x", sgdbVerifying: true })} />);
    expect(busy.getByTestId("verify-sgdb-key").textContent).toBe("Verifying...");
  });

  it("shows the verification status line", () => {
    expect(render(<SteamGridDBSection {...props({ sgdbStatus: "Valid" })} />).container.textContent).toContain("Valid");
  });

  it("the Steam Input button cycles Default → Force On → Force Off → Default", () => {
    const p = props();
    const first = render(<SteamGridDBSection {...p} />);
    expect(first.getByTestId("choice-Steam Input").textContent).toBe("Default");
    fireEvent.click(first.getByTestId("choice-Steam Input"));
    expect(p.onModeChange).toHaveBeenLastCalledWith("force_on");
    first.unmount();
    const last = render(<SteamGridDBSection {...props({ steamInputMode: "force_off", onModeChange: p.onModeChange })} />);
    expect(last.getByTestId("choice-Steam Input").textContent).toBe("Force Off");
    fireEvent.click(last.getByTestId("choice-Steam Input"));
    expect(p.onModeChange).toHaveBeenLastCalledWith("default");
  });

  it("Apply writes the mode to every shortcut (blocked while loading) and shows its result", () => {
    const p = props({ steamInputStatus: "Applied to 12 shortcuts" });
    const { getByTestId, container } = render(<SteamGridDBSection {...p} />);
    expect(container.textContent).toContain("Applied to 12 shortcuts");
    fireEvent.click(getByTestId("apply-steam-input"));
    expect(p.onApplyMode).toHaveBeenCalledTimes(1);
    const busy = props({ loading: true });
    const second = render(<SteamGridDBSection {...busy} />);
    fireEvent.click(lastOf(second.getAllByTestId("apply-steam-input")));
    expect(busy.onApplyMode).not.toHaveBeenCalled();
  });
});
