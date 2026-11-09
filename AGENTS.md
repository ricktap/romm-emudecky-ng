# Agent instructions (Codex and other agents)

This repo is **EmuDecky RomM Manager**, a Decky Loader plugin. The detailed instructions live in three files; read them
in this order before changing anything:

1. [`HANDOFF.md`](HANDOFF.md): current state, the Rust-rewrite workflow, remaining steps, working agreements, gotchas.
2. [`CLAUDE.md`](CLAUDE.md): hard technical constraints and architecture rules. It is named for Claude Code, but it
   applies to every agent working here.
3. [`EMUDECK_PORT_PLAN.md`](EMUDECK_PORT_PLAN.md): roadmap, decisions, phases.

## The rules that matter most

- **Switch only until v0.75.** Don't extend other systems. `REMOVED_TILL_0_90_0.md` lists what was dropped on purpose.
- **Rewrite in progress:** `main` is the shipping Python backend (v0.33.x). The Rust rewrite lives on the `rust` branch and
  is merged in one PR per step; the Python code and its tests are the specification for each port. Keep Rust line
  coverage at or above 80% (`cd backend && cargo llvm-cov --locked --summary-only --fail-under-lines 80`).
- **Tests:** Python `.venv/bin/python -m pytest tests/ -q -p no:cacheprovider`; frontend `node_modules/.bin/vitest run` and
  `npx -y pnpm@10.29.3 build`; Rust `cd backend && cargo llvm-cov --locked --summary-only`. Run them yourself before
  reporting anything as done.
- **No linters, formatters, type-check gates or extra CI.** CI is `test.yml` (tests) and `release.yml` (on a `v*` tag).
- **Never change the plugin name** `EmuDecky RomM NG` in `plugin.json` / `src/index.tsx` (Decky's updater finds the plugin
  by it; it must stay Docker-safe).
- **Don't hardcode `/home/deck`**; derive paths from `$HOME` and `~/.config/EmuDeck/settings.json`.
- **Never read credentials** (`~/homebrew/settings/`), never log secrets.
- **Git:** use `GIT_SSH_COMMAND="ssh -o BatchMode=yes -o ConnectTimeout=15"`. Never commit `backend/target` or
  `backend/out` (check `git status` before `git add -A`).
- **Ask Patrick before** merging into `main`, cutting a major/milestone release (0.40.0), or moving/deleting tags or
  releases. Minor and patch releases of the `main` line are pre-authorized: bump `version` in `package.json` and
  `plugin.json`, merge, tag `vX.Y.Z`, then verify the release asset and its sha256 digest.
