import { describe, it, expect, vi } from "vitest";
import { render, fireEvent } from "@testing-library/react";
import { RmButton, RmChoice, RmToggle, RmRow, RmCard, RmValueRow } from ".";

describe("RmButton", () => {
  it("fires once for a press that arrives as A and as a click, and not when disabled", () => {
    const onActivate = vi.fn();
    const { getByText, rerender } = render(<RmButton onActivate={onActivate}>Go</RmButton>);
    fireEvent(getByText("Go"), new CustomEvent("decky-activate"));
    fireEvent.click(getByText("Go"));
    expect(onActivate).toHaveBeenCalledTimes(1);
    rerender(<RmButton disabled onActivate={onActivate}>Go</RmButton>);
    fireEvent.click(getByText("Go"));
    expect(onActivate).toHaveBeenCalledTimes(1);
  });

  it("turns variants into rm- classes", () => {
    const { getByText } = render(<RmButton variant="sm ghost" onActivate={() => undefined}>X</RmButton>);
    expect(getByText("X").className).toBe("rm-btn rm-sm rm-ghost");
  });
});

describe("RmToggle", () => {
  it("reports the opposite value and shows its state", () => {
    const onChange = vi.fn();
    const { getByLabelText, rerender } = render(<RmToggle label="T" checked={false} onChange={onChange} />);
    expect(getByLabelText("T").className).not.toContain("rm-on");
    fireEvent.click(getByLabelText("T"));
    expect(onChange).toHaveBeenCalledWith(true);
    rerender(<RmToggle label="T" checked onChange={onChange} />);
    expect(getByLabelText("T").className).toContain("rm-on");
  });
  it("does nothing when disabled", () => {
    const onChange = vi.fn();
    const { getByLabelText } = render(<RmToggle label="T" checked disabled onChange={onChange} />);
    fireEvent.click(getByLabelText("T"));
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe("RmChoice", () => {
  const options = [{ data: "a", label: "Alpha" }, { data: "b", label: "Beta" }];
  it("shows the current label and advances, wrapping round", () => {
    const onChange = vi.fn();
    const first = render(<RmChoice label="C" value="a" options={options} onChange={onChange} />);
    expect(first.getByTestId("choice-C").textContent).toBe("Alpha");
    fireEvent.click(first.getByTestId("choice-C"));
    expect(onChange).toHaveBeenLastCalledWith("b");
    first.unmount();
    const second = render(<RmChoice label="C" value="b" options={options} onChange={onChange} />);
    fireEvent.click(second.getByTestId("choice-C"));
    expect(onChange).toHaveBeenLastCalledWith("a");
  });
  it("shows an unknown value as is and ignores an empty option list", () => {
    const onChange = vi.fn();
    const { getByTestId } = render(<RmChoice label="C" value="zzz" options={[]} onChange={onChange} />);
    expect(getByTestId("choice-C").textContent).toBe("zzz");
    fireEvent.click(getByTestId("choice-C"));
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe("rows and cards", () => {
  it("RmRow shows label, small sub line and its control; RmValueRow a right-aligned value", () => {
    const { container } = render(
      <RmCard title="Card">
        <RmRow label="Label" sub="small">
          <b>ctl</b>
        </RmRow>
        <RmValueRow label="Version" value="1.0" />
      </RmCard>,
    );
    expect(container.querySelector("h3")?.textContent).toBe("Card");
    expect(container.textContent).toContain("small");
    expect(container.querySelector(".rm-v")?.textContent).toBe("1.0");
  });
});
