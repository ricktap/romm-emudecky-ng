import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, fireEvent } from "@testing-library/react";
import type { ReactElement } from "react";
import { ConnectionSection } from "./ConnectionSection";
import { showModal } from "@decky/ui";

function lastModal(): ReactElement<Record<string, unknown>> {
  const calls = vi.mocked(showModal).mock.calls;
  return calls[calls.length - 1]![0] as ReactElement<Record<string, unknown>>;
}

function defaultProps(overrides: Partial<React.ComponentProps<typeof ConnectionSection>> = {}) {
  return {
    url: "",
    externalUrl: "",
    hasToken: false,
    allowInsecureSsl: false,
    status: "",
    loading: false,
    onUrlChange: vi.fn(),
    onExternalUrlChange: vi.fn(),
    onConnect: vi.fn(),
    onConnectToken: vi.fn(),
    onConnectPairing: vi.fn(),
    onAllowInsecureSslChange: vi.fn(),
    onTestConnection: vi.fn(),
    onSignOut: vi.fn(),
    ...overrides,
  };
}

describe("ConnectionSection", () => {
  beforeEach(() => {
    vi.mocked(showModal).mockClear();
  });

  it("shows the server and external URL on their buttons, with a hint when unset", () => {
    const empty = render(<ConnectionSection {...defaultProps()} />);
    expect(empty.getByTestId("edit-url").textContent).toBe("Set URL");
    expect(empty.getByTestId("edit-external-url").textContent).toBe("Not set");
    empty.unmount();
    const set = render(<ConnectionSection {...defaultProps({ url: "http://romm.local", externalUrl: "https://romm.example.com" })} />);
    expect(set.getByTestId("edit-url").textContent).toBe("http://romm.local");
    expect(set.getByTestId("edit-external-url").textContent).toBe("https://romm.example.com");
  });

  it("the URL button opens the URL modal, the external button its own, each wired to its handler", () => {
    const props = defaultProps({ url: "http://romm.local", externalUrl: "https://x.example.com" });
    const { getByTestId } = render(<ConnectionSection {...props} />);
    fireEvent.click(getByTestId("edit-url"));
    expect(lastModal().props).toMatchObject({ label: "RomM URL", value: "http://romm.local", field: "url", onSubmit: props.onUrlChange });
    fireEvent.click(getByTestId("edit-external-url"));
    expect(lastModal().props).toMatchObject({ value: "https://x.example.com", onSubmit: props.onExternalUrlChange });
  });

  it("the account row says whether you are signed in and opens the sign-in modal", () => {
    const props = defaultProps();
    const out = render(<ConnectionSection {...props} />);
    expect(out.container.textContent).toContain("Not signed in");
    expect(out.queryByTestId("sign-out")).toBeNull();
    fireEvent.click(out.getByTestId("sign-in"));
    expect(lastModal().props).toMatchObject({ onConnect: props.onConnect, onConnectToken: props.onConnectToken, onConnectPairing: props.onConnectPairing });
    out.unmount();
    const signed = render(<ConnectionSection {...defaultProps({ hasToken: true })} />);
    expect(signed.container.textContent).toContain("Signed in");
    expect(signed.getByTestId("sign-in").textContent).toBe("Sign in again");
  });

  it("sign out asks first and only then calls onSignOut", () => {
    const onSignOut = vi.fn();
    const { getByTestId } = render(<ConnectionSection {...defaultProps({ hasToken: true, onSignOut })} />);
    fireEvent.click(getByTestId("sign-out"));
    const modal = lastModal();
    expect(modal.props.strTitle).toBe("Sign out of RomM?");
    expect(String(modal.props.strDescription)).toContain("only forgets the RomM token on this device");
    expect(onSignOut).not.toHaveBeenCalled();
    (modal.props.onOK as () => void)();
    expect(onSignOut).toHaveBeenCalledTimes(1);
  });

  it("the self-signed toggle exists only for https URLs and reports its new value", () => {
    const http = render(<ConnectionSection {...defaultProps({ url: "http://romm.local" })} />);
    expect(http.queryByLabelText("Allow self-signed certificates")).toBeNull();
    http.unmount();
    const onAllowInsecureSslChange = vi.fn();
    const https = render(<ConnectionSection {...defaultProps({ url: "https://romm.local", onAllowInsecureSslChange })} />);
    fireEvent.click(https.getByLabelText("Allow self-signed certificates"));
    expect(onAllowInsecureSslChange).toHaveBeenCalledWith(true);
  });

  it("Test connection needs a token and is blocked while loading", () => {
    const onTestConnection = vi.fn();
    const off = render(<ConnectionSection {...defaultProps({ onTestConnection })} />);
    fireEvent.click(off.getByTestId("test-connection"));
    expect(onTestConnection).not.toHaveBeenCalled();
    expect(off.container.textContent).toContain("Sign in to RomM first to test the connection.");
    off.unmount();
    const on = render(<ConnectionSection {...defaultProps({ hasToken: true, onTestConnection })} />);
    fireEvent.click(on.getByTestId("test-connection"));
    expect(onTestConnection).toHaveBeenCalledTimes(1);
  });

  it("shows the status line only when there is one", () => {
    expect(render(<ConnectionSection {...defaultProps()} />).queryByTestId("connection-status")).toBeNull();
    const { getByTestId } = render(<ConnectionSection {...defaultProps({ status: "Connected ✓" })} />);
    expect(getByTestId("connection-status").textContent).toContain("Connected ✓");
  });
});
