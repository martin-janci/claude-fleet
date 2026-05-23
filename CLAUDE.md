# CLAUDE.md

Orientation for Claude Code working in this repository.

## What this is

`claude-fleet` — a Tauri 2 desktop app (Rust backend + Svelte 5 frontend) for
managing long-lived Claude Code sessions running in tmux across multiple
machines over SSH. ~16,400 LOC Rust, ~12,900 LOC frontend.

## Build & test

```bash
pnpm install
pnpm test                       # frontend (Vitest)
pnpm check                      # Svelte/TS type-check
cd src-tauri && cargo test      # backend
cd src-tauri && cargo clippy --all-targets -- -D warnings
cd src-tauri && cargo fmt --check
```

**Caveat:** `cargo` builds need the Tauri system libraries (dbus, gtk/atk,
pkg-config). On a headless box without them, `cargo build`/`cargo test` fail in
a build script — that is an environment gap, not a code error. Frontend
(`pnpm test`) builds anywhere.

A handful of frontend tests (`session_ui.test.ts`, `App.test.ts`, …) currently
fail with `localStorage is undefined` — a pre-existing test-environment issue,
not caused by app code. Verify against `main` before attributing a failure to
your change.

## Releasing

Versions and `CHANGELOG.md` are automated by release-please from Conventional
Commits — never bump versions by hand. See `docs/RELEASING.md`.

`docs/control-api-reference.md` is generated from the MCP tool router. After
editing any `#[tool(...)]` description or the `generate_handler!` list,
regenerate it or CI fails:

```bash
REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current
```

## Architecture

- **Frontend stores** (`src/lib/*.ts`) hold app state as Svelte 5 runes. Backend
  mutations emit row events (`events.rs` → `events.ts` `subscribeToRowEvents`);
  the frontend patches stores in place (`mergeOne`/`removeOne`) instead of
  re-fetching. Mutation wrappers also do an optimistic patch from the command's
  return value.
- **Backend** (`src-tauri/src/`): thin Tauri command handlers in `commands/`
  wrap the transport-agnostic logic in `service/`; SSH multiplexing in `ssh.rs`
  (per-host `ControlMaster`, async `tokio::process`); tmux command construction
  in `tmux.rs`; the single global PTY in `pty.rs`; SQLite in `store.rs`
  (migrations `001`–`015`); the event bus in `events.rs`; cancellation registry
  in `cancel.rs`.
- **Control API** (`mcp/`): an embedded MCP server (off by default, localhost +
  bearer token) lets an AI assistant drive the fleet. Its tools call the same
  `service/` layer as the Tauri commands. See `docs/control-api.md`.
- **Terminal** is a hand-rolled ANSI screen buffer (`src/lib/ansi.ts` +
  `TerminalView.svelte`), *not* xterm.js — xterm's renderer failed to repaint in
  the WKWebView setup. Only one PTY is attached at a time.

## Conventions

- Backend errors flow as `IpcError` (`ipc_error.rs`) with `E_*` codes; the
  frontend unwraps a `Result` type (`src/lib/result.ts`).
- Shell-quoting for remote commands currently lives in **four** duplicated
  copies (`shell_quote`/`shq`/`shell_quote_str`/`shell_escape`) — consolidating
  them is a long-overdue cleanup. Any value interpolated into an SSH command
  string MUST be quoted.
- SQLite access goes through `Store` behind a `std::sync::Mutex`. Never hold the
  guard across an `.await`.

## Status & known issues

Iterations 1–4a are landed (multi-host, accounts, cross-host sessions, prompt
transfer, async/events rework), plus the MCP control API, background sessions,
the background reconcile tick, fleet_health roll-up, and the persistent session
event timeline (session_history). Handoff and Freeze from the original spec are
not implemented. A full hardening review is in
`docs/specs/2026-05-21-hardening-review.md` — consult it before touching SSH
command construction, the PTY, migrations, or the optimistic-merge / event-bus
paths.
