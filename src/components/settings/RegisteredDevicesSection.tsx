/**
 * Read-only list of devices currently registered with the RomM save-sync
 * backend, as a settings card. Visible only when save-sync is enabled; the
 * parent owns the device list, loading flag and error message.
 */

import { FC } from "react";
import type { RegisteredDevice } from "../../types";
import { formatRelativeTime } from "./helpers";
import { RmCard, RmValueRow } from "../rm";

interface RegisteredDevicesSectionProps {
  devicesLoading: boolean;
  devicesError: string | null;
  registeredDevices: RegisteredDevice[] | null;
}

export const RegisteredDevicesSection: FC<RegisteredDevicesSectionProps> = ({
  devicesLoading,
  devicesError,
  registeredDevices,
}) => (
  <RmCard title="Devices">
    {devicesLoading && <RmValueRow label="Loading..." />}
    {!devicesLoading && devicesError && <RmValueRow label="Could not load devices" sub={devicesError} />}
    {!devicesLoading && !devicesError && registeredDevices !== null && registeredDevices.length === 0 && (
      <RmValueRow label="No devices registered" />
    )}
    {!devicesLoading &&
      !devicesError &&
      registeredDevices?.map((device, i) => (
        <RmValueRow
          key={device.id || `idx-${i}`}
          label={
            <span>
              {device.name ?? "(unnamed)"}
              {device.is_current_device && <span className="rm-tag rm-blue" style={{ marginLeft: 8 }}>This device</span>}
            </span>
          }
          sub={`${device.client ?? "unknown client"} v${device.client_version ?? "?"}${device.platform ? ` · ${device.platform}` : ""}`}
          value={formatRelativeTime(device.last_seen)}
        />
      ))}
  </RmCard>
);
