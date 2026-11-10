/**
 * RomM server connection settings — URLs, account sign-in/token, SSL toggle and
 * the "Test Connection" affordance, as one settings card. Pure renderer: the
 * parent owns the field values, the has-token flag, the status string, and the
 * save/sign-in/test logic.
 */

import { FC } from "react";
import { ConfirmModal, showModal } from "@decky/ui";
import { TextInputModal } from "./TextInputModal";
import { ConnectModal } from "./ConnectModal";
import { isHttpsUrl } from "../../utils/serverUrl";
import { RmButton, RmCard, RmRow, RmToggle, RmValueRow } from "../rm";

// Sign-out only forgets the token on this device; it never revokes it in RomM.
const SIGN_OUT_CONFIRM_DESCRIPTION =
  "This only forgets the RomM token on this device. The token itself stays valid in RomM — " +
  "revoke it there (Settings → API Tokens) if you no longer want it.";

interface ConnectionSectionProps {
  url: string;
  externalUrl: string;
  hasToken: boolean;
  allowInsecureSsl: boolean;
  status: string;
  loading: boolean;
  onUrlChange: (value: string) => void;
  onExternalUrlChange: (value: string) => void;
  onConnect: (username: string, password: string) => void;
  onConnectToken: (token: string) => void;
  onConnectPairing: (code: string) => void;
  onAllowInsecureSslChange: (value: boolean) => void;
  onTestConnection: () => void;
  onSignOut: () => void;
}

export const ConnectionSection: FC<ConnectionSectionProps> = ({
  url,
  externalUrl,
  hasToken,
  allowInsecureSsl,
  status,
  loading,
  onUrlChange,
  onExternalUrlChange,
  onConnect,
  onConnectToken,
  onConnectPairing,
  onAllowInsecureSslChange,
  onTestConnection,
  onSignOut,
}) => {
  return (
    <RmCard title="Connection">
      <RmRow label="RomM server" sub={url ? undefined : "(not set)"}>
        <RmButton
          variant="sm ghost"
          testId="edit-url"
          onActivate={() => showModal(<TextInputModal label="RomM URL" value={url} field="url" onSubmit={onUrlChange} />)}
        >
          {url || "Set URL"}
        </RmButton>
      </RmRow>
      <RmRow label="External URL" sub="Used when the URL above is unreachable">
        <RmButton
          variant="sm ghost"
          testId="edit-external-url"
          onActivate={() =>
            showModal(
              <TextInputModal label="External RomM URL (empty to clear)" value={externalUrl} onSubmit={onExternalUrlChange} />,
            )
          }
        >
          {externalUrl || "Not set"}
        </RmButton>
      </RmRow>
      <RmRow label="Account" sub={hasToken ? "Signed in" : "Not signed in"}>
        <span style={{ display: "flex", gap: 8 }}>
          <RmButton
            variant="sm ghost"
            testId="sign-in"
            onActivate={() =>
              showModal(
                <ConnectModal onConnect={onConnect} onConnectToken={onConnectToken} onConnectPairing={onConnectPairing} />,
              )
            }
          >
            {hasToken ? "Sign in again" : "Sign in"}
          </RmButton>
          {hasToken && (
            <RmButton
              variant="sm ghost"
              testId="sign-out"
              onActivate={() =>
                showModal(
                  <ConfirmModal
                    strTitle="Sign out of RomM?"
                    strDescription={SIGN_OUT_CONFIRM_DESCRIPTION}
                    strOKButtonText="Sign out"
                    strCancelButtonText="Cancel"
                    onOK={onSignOut}
                  />,
                )
              }
            >
              Sign out
            </RmButton>
          )}
        </span>
      </RmRow>
      {isHttpsUrl(url) && (
        <RmRow label="Allow self-signed certificates" sub="LAN only">
          <RmToggle label="Allow self-signed certificates" checked={allowInsecureSsl} onChange={onAllowInsecureSslChange} />
        </RmRow>
      )}
      <RmRow
        label="Test connection"
        sub={hasToken ? undefined : "Sign in to RomM first to test the connection."}
      >
        <RmButton variant="sm ghost" testId="test-connection" disabled={loading || !hasToken} onActivate={onTestConnection}>
          Test
        </RmButton>
      </RmRow>
      {status && <RmValueRow label={status} testId="connection-status" />}
    </RmCard>
  );
};
