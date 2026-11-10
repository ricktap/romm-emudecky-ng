import { describe, it, expect, vi } from "vitest";
import { render, fireEvent } from "@testing-library/react";
import { BackOnCancel } from "./BackOnCancel";

describe("BackOnCancel", () => {
  it("steps back on the controller's B button and does not pass the cancel on to the panel", () => {
    const onBack = vi.fn();
    const outer = vi.fn();
    const { getByTestId, getByText } = render(
      <div onClick={undefined}>
        <BackOnCancel onBack={onBack}>
          <span>page content</span>
        </BackOnCancel>
      </div>,
    );
    const root = getByTestId("focusable");
    root.parentElement?.addEventListener("decky-cancel", outer);
    expect(getByText("page content")).toBeInTheDocument();
    fireEvent(root, new CustomEvent("decky-cancel", { bubbles: true, cancelable: true }));
    expect(onBack).toHaveBeenCalledTimes(1);
    expect(outer).not.toHaveBeenCalled();
  });

  it("does nothing until B is pressed", () => {
    const onBack = vi.fn();
    render(
      <BackOnCancel onBack={onBack}>
        <span>x</span>
      </BackOnCancel>,
    );
    expect(onBack).not.toHaveBeenCalled();
  });
});
