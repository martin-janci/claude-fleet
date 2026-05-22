# claude-fleet

A native cross-platform desktop app for managing long-lived [Claude Code](https://claude.com/claude-code) sessions running in tmux across multiple machines (mac, mefistos, hetzner). Built with Rust + Tauri 2 + Svelte.

> Status: iteration 4a complete — multi-host SSH, account model, cross-host
> session memory, prompt transfer, and the async/event-driven responsiveness
> rework are all landed. Handoff and Freeze (original spec §8.3–8.4) remain
> unimplemented. See [docs/specs](docs/specs/) for the per-iteration designs and
> [docs/plans](docs/plans/) for the implementation plans.

## Features

- **Multi-host** — attach to tmux sessions on any host in `~/.ssh/config`, plus
  `local`. SSH connections are multiplexed via per-host ControlMaster.
- **Project tree** — scans `~/projects/github.com/<owner>/<repo>` (and git
  worktrees) on each host; sessions are grouped under their project.
- **Account model** — each host's logged-in Claude account (email / org / tier)
  is auto-detected by probing the remote `~/.claude.json` `oauthAccount` object.
  No credentials are ever read or stored.
- **Terminal pane** — a custom ANSI screen-buffer renderer (not xterm.js — see
  `src/lib/ansi.ts` for why) shows the attached session live.
- **Prompt transfer** — send a prompt to one or many sessions at once.
- **Files, history & branches** — per-session worktree browser: changed files
  with inline diffs, a full file tree, an interactive commit graph (branch
  tree, all branches), and a branch list. Git actions run in the session's
  worktree: stage & commit, checkout branch/commit, create/delete branch, and
  fetch/pull/push. Checkout is guarded against a dirty worktree so it can't
  clobber the agent's in-progress work.
- **Event-driven UI** — backend mutations emit row events; the frontend patches
  its stores in place rather than re-fetching.

## Requirements

- macOS 13+ (primary) or Linux (mefistos / hetzner)
- Rust 1.83+ (`rustup install stable`)
- pnpm 9+ (`npm i -g pnpm`)
- Tauri 2 prerequisites: https://v2.tauri.app/start/prerequisites/

## Build & run

```bash
pnpm install
pnpm tauri dev      # dev mode (hot-reload frontend, debug Rust)
pnpm tauri build    # release bundle in src-tauri/target/release/bundle/
```

## Test

```bash
pnpm test                      # frontend (Vitest)
pnpm check                     # frontend Svelte/TS type-check
cd src-tauri && cargo test     # backend (rusqlite + commands)
cd src-tauri && cargo clippy --all-targets -- -D warnings
cd src-tauri && cargo fmt --check
```

## Project layout

```
src/lib/            # Svelte 5 components + TS stores (hosts, sessions, projects, accounts, events)
src-tauri/src/      # Rust backend: Tauri commands, ssh/tmux/pty, SQLite store, event bus
src-tauri/src/commands/  # IPC command handlers (hosts, sessions, projects, health)
src-tauri/migrations/    # SQLite migrations (001–004)
docs/specs/         # per-iteration design specs
docs/plans/         # per-iteration implementation plans
CLAUDE.md           # orientation for Claude Code working in this repo
```

## Known gaps

A hardening review (2026-05-21, see
[docs/specs/2026-05-21-hardening-review.md](docs/specs/2026-05-21-hardening-review.md))
catalogues open issues. Highest priority: SSH host-alias validation, migration
atomicity, and the single-global-PTY races in `TerminalView`.

## License

Personal project. No license declared yet.
