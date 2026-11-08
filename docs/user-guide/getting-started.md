# Getting Started

## What is EmuDecky RomM Manager?

EmuDecky RomM Manager is a [Decky Loader](https://decky.xyz/) plugin that connects your self-hosted
[RomM](https://github.com/rommapp/romm) ROM library to Steam. Every game in your RomM library appears as a Non-Steam
shortcut in the Steam Library, complete with cover art, metadata, and collections. Games launch through
[EmuDeck](https://emudeck.com/).

## Prerequisites

Before installing the plugin, you need:

1. **A RomM server** — a running RomM instance with your ROM library. You'll need the server URL plus a username and
   password to connect the first time. The plugin exchanges those credentials for a RomM Client API Token and stores
   only the token — your password is never saved. Each user should have their own RomM account (see
   [Save Sync](save-sync.md) for why this matters).

2. **EmuDeck** — installed on your Steam Deck or Linux PC. EmuDeck handles the actual emulation. The plugin creates
   shortcuts that launch games through EmuDeck.

3. **Decky Loader** — the plugin framework. Install it from [decky.xyz](https://decky.xyz/) if you haven't already.
   Decky renders inside Steam's gamepad UI, and the plugin works wherever that UI runs: Gaming Mode on the Steam Deck,
   or Big Picture Mode on a Linux PC/HTPC — a dedicated Game Mode session is not required.

4. **A personal RomM account** — save sync ties saves to the authenticated user. Use your own account, not a shared one.

## Installation

### From Decky's "Install Plugin From URL"

1. Open the Quick Access Menu (QAM) in Gaming Mode by pressing the **...** button
2. Go to the Decky Loader tab (the plug icon) and open settings (gear icon)
3. Under **General → Other**, enable **Developer Mode** — a new **Developer** tab appears in the sidebar
4. Open the **Developer** tab and select **Install Plugin From URL**
5. Enter the direct URL to the release zip

   The URL for the latest release follows this pattern:

   ```text
   https://github.com/ricktap/romm-emudecky-ng/releases/download/v{VERSION}/romm-emudecky-ng.zip
   ```

   For example, for v0.26.1:

   ```text
   https://github.com/ricktap/romm-emudecky-ng/releases/download/v0.26.1/romm-emudecky-ng.zip
   ```

6. Decky downloads and installs the plugin automatically — no restart needed

**Tip:** You can also open the [releases page](https://github.com/ricktap/romm-emudecky-ng/releases) in Steam's built-in
browser (Gaming Mode → long-press the Steam button → Web Browser), long-press the zip download link, and copy the URL
from there.

Any direct URL to the zip file works (GitHub releases, a self-hosted mirror, etc.) as long as it points to a valid
`.zip` containing the plugin.

### Manual installation (alternative)

1. Download `romm-emudecky-ng.zip` from the [releases page](https://github.com/ricktap/romm-emudecky-ng/releases)
2. Extract the zip to `~/homebrew/plugins/` on your device (via SSH, file manager, or USB)
3. Restart Decky Loader — either reboot, or run `sudo systemctl restart plugin_loader` via SSH
4. The plugin appears in your QAM under the Decky tab

### Updating

You only install by URL once. After that the plugin checks GitHub for a newer release by itself:

- **When:** shortly after the plugin starts, then at most every 6 hours (sooner after a failed check). Opening the QAM
  reuses the last result, so it never spams GitHub.
- **Where it shows:** an **Update available: vX.Y.Z** card at the top of the plugin's QAM page, with the release notes
  (long notes behind **Show release notes**) and an **Update** button.
- **Installing:** **Update** hands the release to Decky Loader's own installer. Decky shows its usual install
  confirmation — nothing changes until you confirm. Decky then downloads the zip, checks its SHA-256 checksum, replaces
  the plugin and reloads it. Your settings, library database and downloads are kept (they live outside the plugin
  folder).
- **Manual check:** **Settings → Updates** shows the installed version and the result of the last check. **Check for
  updates** checks right away.
- **If Decky can't start the update** (for example after a Decky Loader change), the card says _Couldn't start the
  update — download vX.Y.Z_ and offers a **Download** button that opens the release page; install that zip by URL as
  above.

Only full releases are offered (no pre-releases), and only a `romm-emudecky-ng.zip` from this project's GitHub
releases with a published checksum is accepted. The plugin never installs anything without your confirmation.

## First-Time Setup

After installation, you need to connect the plugin to your RomM server:

1. Open the QAM and find **EmuDecky RomM Manager**
2. Tap **Connection Settings**
3. Enter your RomM server URL (e.g. `http://192.168.1.100:8080`) — this saves automatically
4. Tap **Sign in**, enter your RomM username and password once, and confirm
5. The **RomM Account** row shows **Signed in** on success. Once signed in, tap **Test Connection** to re-verify at any
   time

The plugin mints a RomM Client API Token from the credentials you enter and discards the password — it is never stored.
The same applies if the plugin auto-migrates an older install that still had a saved password: the password is discarded
as soon as a token is minted. If your RomM account is not allowed to create API tokens, the sign-in step reports that
and you'll need an account with token permissions.

<!-- Screenshot: Connection Settings page with URL field, Sign in button, and token status -->

Once connected, you're ready to sync your library. See [Configuration](configuration.md) for additional settings, or
jump straight to [Syncing Your Library](syncing-your-library.md).

---

**Next:** [Configuration](configuration.md)
