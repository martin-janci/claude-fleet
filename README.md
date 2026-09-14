# claude-fleet

[![CI](https://github.com/martin-janci/claude-fleet/actions/workflows/ci.yml/badge.svg)](https://github.com/martin-janci/claude-fleet/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/martin-janci/claude-fleet?display_name=tag&sort=semver)](https://github.com/martin-janci/claude-fleet/releases)

A native cross-platform desktop app for managing long-lived [Claude Code](https://claude.com/claude-code) sessions running in tmux across multiple machines. Built with Rust + Tauri 2 + Svelte 5.

## Quickstart

```bash
pnpm install
pnpm tauri dev
```

On first launch the app walks you through setup — see the **[Getting Started guide](docs/getting-started.md)**.

## Features

- **Multi-host** — attach to tmux sessions on any host in `~/.ssh/config`, plus
  `local`. SSH connections are multiplexed via per-host ControlMaster.
- **Project tree** — finds repos (and git worktrees) under a per-host projects
  base set in Settings → Projects, laid out as `<base>/<owner>/<repo>` (default
  `~/projects/github.com`) or flat `<base>/<repo>`; sessions are grouped under
  their project.
- **Account model** — each host's logged-in Claude account (email / org / tier)
  is auto-detected by probing the remote `~/.claude.json`. No credentials are
  ever read or stored.
- **Terminal pane** — a custom ANSI screen-buffer renderer shows the attached
  session live.
- **Prompt transfer** — send a prompt to one or many sessions at once.
- **Files, history & branches** — per-session worktree browser: changed files
  with inline diffs, a full file tree, an interactive commit graph, and a
  branch list. Git actions (stage & commit, checkout, create/delete branch,
  fetch/pull/push) run directly in the session's worktree.
- **Event-driven UI** — backend mutations emit row events; the frontend patches
  its stores in place rather than re-fetching.

## Documentation

- [Getting Started](docs/getting-started.md)
- [Concepts](docs/concepts.md)
- [Troubleshooting](docs/troubleshooting.md)
- [Control API](docs/control-api.md)
- [Docs index](docs/README.md)

## Development

### Requirements

- macOS 13+ (primary) or Linux — CI runs on both (`macos-latest`,
  `ubuntu-24.04`) and tagged releases ship unsigned macOS `.dmg` (arm64 and
  x86_64) plus Linux `.AppImage`/`.deb` bundles (see `docs/RELEASING.md`)
- Rust 1.83+ (`rustup install stable`)
- Node 20 (`.node-version`) and pnpm 10 via `corepack enable` (or
  `npm i -g pnpm@10`). The workspace file uses the pnpm 10 `allowBuilds` key;
  if a local pnpm 9 prints `packages field missing or empty`, run
  `corepack pnpm@10 <cmd>` or `npx -y pnpm@10 <cmd>` instead.
- `cargo install cargo-deny --locked` for the local license/advisory audit
  step (`cargo deny check`, run by `scripts/ci-local.sh` and CI).
- Tauri 2 prerequisites: https://v2.tauri.app/start/prerequisites/

### Build & run

```bash
pnpm install
pnpm tauri dev      # dev mode (hot-reload frontend, debug Rust)
pnpm tauri build    # release bundle in src-tauri/target/release/bundle/
```

### Test

```bash
pnpm test                      # frontend (Vitest)
pnpm check                     # frontend Svelte/TS type-check
cd src-tauri && cargo test     # backend (rusqlite + commands)
cd src-tauri && cargo clippy --all-targets -- -D warnings
cd src-tauri && cargo fmt --check
cargo deny --manifest-path src-tauri/Cargo.toml check   # licenses + advisories
```

Run `scripts/ci-local.sh` (or `--rust-only` / `--frontend-only`) before
pushing; it mirrors CI. Opt in to the fast pre-commit hook with
`git config core.hooksPath .githooks`.

### Project layout

```
src/lib/            # Svelte 5 components + TS stores (hosts, sessions, projects, accounts, events)
src-tauri/src/      # Rust backend: Tauri commands, ssh/tmux/pty, SQLite store, event bus
src-tauri/src/commands/  # IPC command handlers (hosts, sessions, projects, health)
src-tauri/migrations/    # SQLite migrations (registered in the MIGRATIONS table in src-tauri/src/store.rs)
docs/specs/         # per-iteration design specs
docs/plans/         # per-iteration implementation plans
CLAUDE.md           # orientation for Claude Code working in this repo
```

## Known gaps

A hardening review (2026-05-21, see
[docs/specs/2026-05-21-hardening-review.md](docs/specs/2026-05-21-hardening-review.md))
catalogues open issues. Highest priority: SSH host-alias validation, migration
atomicity, and the single-global-PTY races in `TerminalView`. Handoff
(original spec §8.3) is replaced by Move to host… / `move_session`, and Freeze
(§8.4) is descoped; see
[ADR 0001](docs/adr/0001-descope-freeze-ship-move.md).

## Releasing & documentation

- Versioning and changelog are cut manually with `scripts/release.sh` — see
  [docs/RELEASING.md](docs/RELEASING.md).
- **Control API reference:** [docs/control-api-reference.md](docs/control-api-reference.md)
  (generated from source) and [docs/control-api.md](docs/control-api.md) (guide).
- **Rust API docs (rustdoc):** published to GitHub Pages on each release —
  https://martin-janci.github.io/claude-fleet/

## License

Personal project. `package.json` and `src-tauri/Cargo.toml` declare MIT; a
`LICENSE` file has not been added to the repository yet.
