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
cargo deny --manifest-path src-tauri/Cargo.toml check   # licenses + advisories (cargo install cargo-deny --locked)
scripts/ci-local.sh             # all of the above in CI order; --rust-only / --frontend-only
```

`devtools` is an off-by-default cargo feature: `cargo tauri build --features
devtools` enables the Web Inspector in a release bundle; dev builds have it
automatically.

**Caveat:** `cargo` builds need the Tauri system libraries (dbus, gtk/atk,
pkg-config). On a headless box without them, `cargo build`/`cargo test` fail in
a build script — that is an environment gap, not a code error. Frontend
(`pnpm test`) builds anywhere.

After pulling, run `pnpm install --frozen-lockfile` before testing. Stale
`node_modules` cause `Failed to resolve import "@tauri-apps/plugin-clipboard-manager"`
in `App.test.ts` and `clipboard_native.test.ts` — that is a dependency gap, not a
code error. (`localStorage` is polyfilled in `vitest.setup.ts`; there are no
known pre-existing frontend test failures.)

## Releasing

Releases are cut manually with `scripts/release.sh <new-version>` — it bumps
the three version files (+ `Cargo.lock`), prefills a `CHANGELOG.md` section
from the Conventional Commits since the last tag, commits, and creates the
`vX.Y.Z` tag. Never edit the version fields by hand; run the script from a
clean `main`. See `docs/RELEASING.md`.

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
  (migrations are registered in the `MIGRATIONS` table there — add a new
  `NNN_<topic>.sql` plus an entry); the event bus in `events.rs`; cancellation
  registry in `cancel.rs`.
- **Session listing** is cache-first: `service::sessions::list_sessions` serves
  stored rows and only runs a reconcile pass when the last one is stale;
  `refresh_sessions` is the forced path for an explicit user refresh.
- **Status vocabulary** (`claude_status`, `stuck_kind`) lives in the enums in
  `service/pane_intel.rs`; the MCP tool descriptions and the generated
  reference derive from them, so add values there, not in prose.
- **Control API** (`mcp/`): an embedded MCP server (off by default, localhost +
  bearer token) lets an AI assistant drive the fleet. Its tools call the same
  `service/` layer as the Tauri commands. See `docs/control-api.md`.
- **Terminal** is a hand-rolled ANSI screen buffer (`src/lib/ansi.ts` +
  `TerminalView.svelte`), *not* xterm.js — xterm's renderer failed to repaint in
  the WKWebView setup. Only one PTY is attached at a time.

## Conventions

- Backend errors flow as `IpcError` (`ipc_error.rs`) with `E_*` codes; the
  frontend unwraps a `Result` type (`src/lib/result.ts`).
- Shell-quoting has **one** canonical implementation: `crate::shell::quote`
  (alias `shq`) in `src-tauri/src/shell.rs`. Every value interpolated into an
  SSH/bash command string MUST be quoted with it. The former duplicate copies
  (`shell_quote`/`shell_quote_str`/`shell_escape`) were consolidated — do not
  reintroduce them.
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
