import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";
import { RegisteredDevicesSection } from "./RegisteredDevicesSection";
import type { RegisteredDevice } from "../../types";

const device = (o: Partial<RegisteredDevice> = {}): RegisteredDevice =>
  ({ id: "abcdef123456", name: "steamdeck", client: "romm-emudecky-ng", client_version: "0.52.0", platform: "linux", last_seen: new Date().toISOString(), is_current_device: false, ...o }) as RegisteredDevice;

describe("RegisteredDevicesSection", () => {
  it("says it is loading, shows a load error, and an empty list", () => {
    expect(render(<RegisteredDevicesSection devicesLoading devicesError={null} registeredDevices={null} />).container.textContent).toContain("Loading...");
    const err = render(<RegisteredDevicesSection devicesLoading={false} devicesError="boom" registeredDevices={null} />);
    expect(err.container.textContent).toContain("Could not load devices");
    expect(err.container.textContent).toContain("boom");
    const none = render(<RegisteredDevicesSection devicesLoading={false} devicesError={null} registeredDevices={[]} />);
    expect(none.container.textContent).toContain("No devices registered");
  });

  it("lists devices with client, platform and last seen, and tags this device", () => {
    const { container } = render(
      <RegisteredDevicesSection
        devicesLoading={false}
        devicesError={null}
        registeredDevices={[device({ is_current_device: true }), device({ id: "z", name: null as never, client: null as never, client_version: null as never })]}
      />,
    );
    expect(container.textContent).toContain("steamdeck");
    expect(container.textContent).toContain("This device");
    expect(container.textContent).toContain("romm-emudecky-ng v0.52.0 · linux");
    expect(container.textContent).toContain("(unnamed)");
    expect(container.textContent).toContain("unknown client v?");
  });
});
