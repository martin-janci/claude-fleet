---
name: claude-fleet-repo
description: Use when developing the claude-fleet app itself — adding MCP tools, service functions, store helpers, migrations, frontend stores; building, testing, or shipping a PR. Triggers when working inside the `claude-fleet` repo (Tauri 2 + Svelte 5 + Rust), or when the user asks to "add to fleet", "fix fleet", "ship a fleet PR", or similar. Sister skill: `claude-fleet-control` (for *operating* sessions, not for hacking on the app).
---

# Working on claude-fleet

claude-fleet is a Tauri 2 desktop app (Rust backend + Svelte 5 frontend) for
driving long-lived Claude Code sessions in tmux across machines over SSH. Read
`CLAUDE.md` at the repo root once — this skill is the workflow that ties it
together. Use the sister skill `claude-fleet-control` to *operate* sessions;
this skill is for *changing* the app.

## Build & verify locally

```bash
# Frontend — builds anywhere
pnpm install
pnpm run check                          # svelte-check / TS
pnpm run test                           # vitest
pnpm run build                          # production bundle

# Backend — needs Tauri system libs (dbus, gtk, atk, pkg-config) on Linux.
# On a headless box the cargo build script will fail; that's an environment
# gap, not a code error.
cd src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test                              # full suite
```

Mirror this exact sequence to reproduce CI locally — and you'll have to,
because the GitHub Actions account currently can't start jobs (billing block).
See "Shipping a PR" below.

## Where things go

| Adding… | File(s) | Pattern |
|---|---|---|
| A new MCP tool | `src-tauri/src/mcp/tools.rs` — params struct + `#[tool]` method calling into `service::*` | Audit non-secret args; pass bodies / prompts but never log them. Return `ok_json(&result)` or `text_content`. |
| A new service function | `src-tauri/src/service/<area>.rs` | Take `&Mutex<Store>` + `&Arc<SshClient>`, never `tauri::State`. Same code path runs from both Tauri IPC and MCP. |
| A new store helper | `src-tauri/src/store.rs` | Hold the `Mutex<Store>` guard *briefly*; never across `.await`. Use `unchecked_transaction` for multi-step writes. |
| A schema change | `src-tauri/migrations/NNN_<topic>.sql` + a `tx.execute_batch(include_str!(…))` arm in `migrate()` + bump `assert_eq!(…schema_version, NNN)` in the relevant tests (currently `15`). | One `.sql` per change. Wrap in a transaction in the migrate arm so an interrupted run rolls back cleanly. End each file with `INSERT OR IGNORE INTO schema_version (version) VALUES (NNN);`. |
| A new Tauri IPC command | `src-tauri/src/commands/<area>.rs` thin wrapper → `service::*` | Validate frontend inputs (`crate::validate::*`); never trust paths. Use `IpcError` with an `E_*` code. |
| Frontend state | `src/lib/<store>.ts` as Svelte 5 runes; patch via `mergeOne`/`removeOne` from row events, plus the optimistic merge from the mutation's return value. | Don't re-fetch on every event; the event bus + optimistic merge is the contract. |
| A wire type | Mirror Rust struct (`#[derive(Serialize)]`) ↔ TS interface in `src/lib/*.ts`. Field names are **snake_case** on the wire (no serde rename). | Add the TS field as `value | null` for Rust `Option<T>`. |
| A new skill | `skills/<name>/SKILL.md` | If it should ship to every host, add it to the provisioner's `include_str!` list (`service/provision.rs`). |

## Critical conventions (easy to miss)

- **Shell-quoting** has *one* canonical impl: `crate::shell::quote` (alias `shq`). Every value interpolated into an SSH/bash command string MUST be quoted with it. The four duplicate copies (`shell_quote` / `shell_quote_str` / `shell_escape`) were consolidated — don't reintroduce them.
- **`IpcError`** is the wire shape: `{ code: "E_*", message, details? }`. Pick a stable `E_*` code; the frontend's `Result` type unwraps it.
- **`Store` mutex**: take, work, drop — never `await` while holding it. The runtime is single-threaded for the DB; holding across `.await` will deadlock under reconcile.
- **Best-effort writes** (timeline events, intel) should never block the mutation that produced them. Pattern: `let _ = s.insert_session_event(…);` and log/swallow errors.
- **Terminal is hand-rolled**: `src/lib/ansi.ts` + `TerminalView.svelte`. xterm.js was tried and abandoned (WKWebView repaint bug). Only one PTY is attached at a time — see `pty.rs`.
- **Test caveats**: a few frontend tests (`session_ui.test.ts`, `App.test.ts`) fail pre-existingly with `localStorage is undefined`; verify against `main` before blaming your change. The `Sidebar` "without quadratic blow-up" perf test is timing-sensitive and occasionally flakes on a loaded box.

## Shipping a PR (current CI billing block)

GitHub Actions on `martin-janci/claude-fleet` currently can't start jobs —
billing/spending limit on the repo owner's account. Until that's resolved,
**run CI locally and merge with `--admin`**:

1. Branch off latest `origin/main`:
   ```bash
   git fetch origin && git checkout -b <kind>/<slug> origin/main
   ```
2. Work, commit, then mirror `.github/workflows/ci.yml`:
   ```bash
   # rust job
   (cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test)
   # frontend job
   pnpm install --frozen-lockfile && pnpm run check && pnpm run test && pnpm run build
   ```
3. Push + PR + merge:
   ```bash
   git push -u origin HEAD
   gh pr create --base main --head $(git branch --show-current) --title "…" --body "…"
   gh pr merge <num> --merge --delete-branch --admin
   ```
4. `git checkout main && git pull --ff-only && git branch -d <branch>`

The repo lands work via merge commits (see history: `Merge pull request #N from …`). Squash is *not* the project style.

## Skill / provisioner reminder

The `claude-fleet-control` and `claude-fleet-repo` skills under `skills/` are
**baked into the binary** at compile time via `include_str!` in
`src-tauri/src/service/provision.rs`. A `provision_hosts` call pushes whatever
was compiled — *not* the latest repo content. Until the user rebuilds the
app:

- For a quick skill update, copy directly: `cp skills/<name>/SKILL.md ~/.claude/skills/<name>/SKILL.md` (locally), or `scp` + `mkdir -p` (remote).
- For the long-term fix, land the skill change in `main` and rebuild the desktop app — then `provision_hosts` will push the current version.

## Common mistakes

- **Adding a wire field on the Rust struct but not the TS interface** → silent `undefined` at runtime.
- **Holding the store mutex across `.await`** → reconcile blocks, app feels frozen.
- **Forgetting to gate a migration arm** (`if v < N { … }`) → re-runs and fails on the second launch with "table already exists".
- **Building a new command without quoting an interpolated path with `shq`** → shell injection or a broken script on names with spaces/quotes.
- **Skipping the PR's local-CI mirror** because "it's a docs-only change" — `pnpm run build` still has to pass; do all six steps.
