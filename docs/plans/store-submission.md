# Decky store submission: what is known and what is open (#59, #60, #61)

Sources: the plugin-addition PR template of `SteamDeckHomebrew/decky-plugin-database` (read 2026-11-11) and the Decky
CLI / template conventions this repo already follows. The wiki pages ("Submitting plugins", "Review and testing") did not
render for me; read them before submitting.

## What the store asks (from the PR template)

- Submission is a pull request that adds the plugin as a **git submodule**; later updates are PRs that move the submodule.
- **Version:** the version in `package.json` must be bumped at the submitted commit.
- Developer checklist: you are the author or an authorised maintainer; library licences are followed (a `LICENSE` exists
  here); **"Generative AI was not used to write a majority of the submitted code."** (see *Decisions* below).
- Plugin checklist: works on SteamOS **Stable and Beta**; unique or better than a plugin already in the store (explain the
  difference to RomM/ROM-manager plugins such as the upstream `decky-romm-sync`).
- Backend questions (answer Yes/No): custom backend other than Python → **Yes** (Rust); third-party FOSS tool without
  static linking → No; custom binary with all dependencies statically linked → **No**: the binary is built against
  SteamOS's glibc (holo toolchain), TLS is rustls (no OpenSSL), SQLite is bundled. A dynamically linked binary means the
  template wants testing on the **Preview** channel by a third party.
- Community: test two other plugins and comment the reports on the PR (optional, but unprioritised without it); testing
  boxes are ticked only after a third party commented a report.

## State of this repo against that (#60)

| Item | State |
| --- | --- |
| `plugin.json` (name, author, flags, `publish` with tags, description, image) | present |
| `LICENSE` | present |
| `pnpm-lock.yaml` lockfileVersion 9.0 | yes |
| Backend built by the Decky CLI from `backend/` in Docker (`backend/Dockerfile`, `entrypoint.sh`, output in `out/`) | the same path our release workflow uses and smoke-tests (ELF x86-64, `--list-methods` count) |
| Toolchain pinned, deps `--locked` | `rust-toolchain.toml` 1.99.1, `cargo ... --locked` |
| Docker-safe plugin name (image tag derives from it) | "EmuDecky RomM NG" (letters, spaces) |
| **Version in `package.json` / `plugin.json` at a commit** | **stale (0.33.2): only the release workflow stamps it from the tag.** The store builds the submodule commit as it is, so the submitted commit must carry the real version. |

Not verifiable from here: that the store's own CI builds `backend/` identically (it should: same CLI and Dockerfile
convention) and that its time or memory limits fit a cold `cargo build --release` in the holo image. The honest test is a
**draft** submission PR; nothing is sent without Patrick's go.

## Decisions for Patrick

1. **The generative-AI line.** This code base is largely written with Claude. The template says a majority of AI-written
   code is not allowed and PRs may be denied for suspected AI use. Options: submit and answer truthfully (likely refused),
   stay on GitHub releases (what works today), or ask the Decky team how they treat a maintainer-reviewed, tested code
   base. Everything else in this file is moot until this is decided.
2. **Version at the submitted commit.** If we submit: tag the commit, and have a commit on that tag whose `package.json`
   and `plugin.json` carry the version (CI could push the stamped files to a `release` branch), instead of stamping only
   inside the workflow.
3. **Self-update (#61).** The in-plugin update check reads GitHub releases (never pre-releases) and hands the install to
   Decky. In the store, updates come from the store, so a second channel would offer the same plugin twice. Proposed:
   keep the check for the GitHub channel and make it opt-out (`update_check_enabled`, default on; the store build turns it
   off through a flag file in the plugin directory that the store build never ships). Needs an on-device decision about
   how a store install is recognised; until then it stays as is.

## If the answer to 1 is "submit"

Order: stamp-on-tag change (2) → draft PR → third-party Preview-channel report → fix findings → update-check decision (3) →
changelog and docs for 1.0.0 (#62).
