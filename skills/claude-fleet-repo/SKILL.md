---
name: claude-fleet-repo
description: Use when developing claude-fleet itself — adding MCP tools, services, migrations, frontend stores; building, testing, or shipping a PR. Triggers inside the `claude-fleet` repo (Tauri 2 + Svelte 5 + Rust) or on asks like "add to fleet", "fix fleet", "ship a fleet PR". Sister: `claude-fleet-control` for *operating* sessions.
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

# Backend — a cargo workspace: crates/fleet-core (Tauri-free core),
# crates/fleet-hub (headless daemon), crates/fleet-proto (hub/agent wire
# types, shared by both ends), crates/fleet-agent (the agent binary,
# depends on fleet-proto only, never fleet-core), src-tauri (desktop app).
# Building src-tauri needs Tauri system libs (dbus, gtk, atk, pkg-config) on
# Linux; on a headless box its build script fails — an environment gap, not
# a code error (`-p fleet-core` / `-p fleet-hub` / `-p fleet-agent` build
# without them).
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                  # full suite
cargo test -p fleet-core <filter>       # iterate on the core quickly
```

Mirror this exact sequence to reproduce CI locally before pushing — CI
(`.github/workflows/ci.yml`) runs the same commands on every PR and gates the
merge. See "Shipping a PR" below.

## Where things go

| Adding… | File(s) | Pattern |
|---|---|---|
| A new MCP tool | `crates/fleet-core/src/mcp/tools/` — params struct + `#[tool]` method calling into `service::*` | Audit non-secret args; pass bodies / prompts but never log them. Return `ok_json(&result)` or `text_content`. Add the tool's name to exactly one of `guard::ADMIN_TOOLS` / `guard::CLIENT_TOOLS` (`crates/fleet-core/src/mcp/guard.rs`) — an exhaustiveness test in `mcp/tools/tests.rs` fails otherwise. After adding: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` to refresh `docs/control-api-reference.md`. |
| A new service function | `crates/fleet-core/src/service/<area>.rs` | Take `&Mutex<Store>` + `&Arc<SshClient>`, never `tauri::State`. Same code path runs from both Tauri IPC and MCP. |
| A new store helper | `crates/fleet-core/src/store/` | Hold the `Mutex<Store>` guard *briefly*; never across `.await`. Use `unchecked_transaction` for multi-step writes. |
| A schema change | `crates/fleet-core/migrations/NNN_<topic>.sql` + an entry in the `MIGRATIONS` list in `crates/fleet-core/src/store/schema.rs` (latest is 37) | `Migration::plain(NNN, include_str!(…))` when the SQL is safe to re-run as written (`CREATE TABLE`/`INDEX IF NOT EXISTS`); otherwise the `Migration { version, sql, already_applied }` form with an `already_applied(&Connection) -> rusqlite::Result<bool>` guard (see `sessions_has_tmux_pane_id` etc. above `MIGRATIONS`) so a re-run of e.g. `ALTER TABLE … ADD COLUMN` or a table rebuild doesn't fail. End the `.sql` file with `INSERT OR IGNORE INTO schema_version (version) VALUES (NNN);`. |
| A new Tauri IPC command | `src-tauri/src/commands/<area>.rs` thin wrapper → `service::*` | Validate frontend inputs (`crate::validate::*`); never trust paths. Use `IpcError` with an `E_*` code. Add it to `generate_handler!` in `lib.rs` and give it a row (routed / local-only / same-in-both-modes) in `src-tauri/src/backend/verdicts.rs` — `every_command_has_a_verdict` fails on anything left unclassified. |
| Frontend state | `src/lib/<store>.ts` as Svelte 5 runes; patch via `mergeOne`/`removeOne` from row events, plus the optimistic merge from the mutation's return value. | Don't re-fetch on every event; the event bus + optimistic merge is the contract. |
| A wire type | Mirror Rust struct (`#[derive(Serialize)]`) ↔ TS interface in `src/lib/*.ts`. Field names are **snake_case** on the wire (no serde rename). | Add the TS field as `value | null` for Rust `Option<T>`. |
| A new skill | `skills/<name>/SKILL.md` | If it should ship to every fleet host, add it to the provisioner's `include_str!` list (`service/provision.rs`) — today only `claude-fleet-control` and `fleet-friendly-name` are on it. `claude-fleet-repo` (this skill) is **not** provisioned to hosts; it lives only in the repo for whoever develops claude-fleet itself. |

## Critical conventions (easy to miss)

- **Shell-quoting** has *one* canonical impl: `crate::shell::quote` (alias `shq`). Every value interpolated into an SSH/bash command string MUST be quoted with it. The former duplicate copies (`shell_quote` / `shell_quote_str` / `shell_escape`) were consolidated — don't reintroduce them.
- **`IpcError`** is the wire shape: `{ code: "E_*", message, details? }`. Pick a stable `E_*` code; the frontend's `Result` type unwraps it.
- **`Store` mutex**: take, work, drop — never `await` while holding it. The runtime is single-threaded for the DB; holding across `.await` will deadlock under reconcile.
- **Best-effort writes** (timeline events, intel) should never block the mutation that produced them. Pattern: `let _ = s.insert_session_event(…);` and log/swallow errors.
- **Terminal is hand-rolled**: `src/lib/ansi.ts` + `TerminalView.svelte`. xterm.js was tried and abandoned (WKWebView repaint bug). Only one PTY is attached at a time — see `pty.rs`.
- **Test caveats**: after pulling, run `pnpm install --frozen-lockfile` — stale `node_modules` fail `App.test.ts` and `clipboard_native.test.ts` with `Failed to resolve import "@tauri-apps/plugin-clipboard-manager"`. `localStorage` is polyfilled in `vitest.setup.ts` (no known pre-existing failures). The `Sidebar` "without quadratic blow-up" perf test is timing-sensitive and occasionally flakes on a loaded box.
- **MCP prefix stability**: every `#[tool(...)]` addition/rename/description edit in `crates/fleet-core/src/mcp/tools/` invalidates the Claude API tool-definition cache for every connected client. Add tools sparingly; when you must rename or rewrite a description, batch with sibling edits in one release rather than churning across many.

## Shipping a PR

CI runs normally on this repo (`.github/workflows/ci.yml`, both the rust job
on `macos-latest`/`ubuntu-24.04` and the frontend job) — no `--admin` merges
needed.

1. Branch off latest `origin/main`:
   ```bash
   git fetch origin && git checkout -b <kind>/<slug> origin/main
   ```
2. Work, commit, and run the same checks locally before pushing (see "Build &
   verify locally" above) — `scripts/ci-local.sh` mirrors CI in one command.
3. Push + PR:
   ```bash
   git push -u origin HEAD
   gh pr create --base main --head $(git branch --show-current) --title "…" --body "…"
   ```
4. Wait for CI to go green (`gh pr checks <num>`, or `gh run list --limit 3`),
   then merge normally: `gh pr merge <num> --merge --delete-branch`.
5. `git checkout main && git pull --ff-only && git branch -d <branch>`

The repo lands work via merge commits (see history: `Merge pull request #N from …`). Squash is *not* the project style.

## Skill / provisioner reminder

**Only some skills ship to fleet hosts.** `provision_hosts` pushes whatever
was `include_str!`-compiled into the binary in
`crates/fleet-core/src/service/provision.rs` — today that is
`claude-fleet-control` and `fleet-friendly-name` only, *not* the latest repo
content, and *not* this skill. `claude-fleet-repo` itself is never provisioned
to a managed host — it is the workflow for developing claude-fleet, relevant
only on whoever's machine is doing that development; install it locally
(`cp skills/claude-fleet-repo/SKILL.md ~/.claude/skills/claude-fleet-repo/SKILL.md`)
if you want it there.

For a skill that IS provisioned (`claude-fleet-control`, `fleet-friendly-name`):
a `provision_hosts` call after editing it still pushes the OLD compiled text
until the app is rebuilt. Until then:

- For a quick update, copy directly: `cp skills/<name>/SKILL.md ~/.claude/skills/<name>/SKILL.md` (locally), or `scp` + `mkdir -p` (remote).
- For the long-term fix, land the change in `main` and rebuild the desktop app (or `fleet-hub`) — then `provision_hosts` pushes the current version.

## Common mistakes

- **Adding a wire field on the Rust struct but not the TS interface** → silent `undefined` at runtime.
- **Holding the store mutex across `.await`** → reconcile blocks, app feels frozen.
- **Forgetting an `already_applied` guard on a non-idempotent migration** (`ALTER TABLE … ADD COLUMN`, a table rebuild) → re-runs and fails on the second launch with "duplicate column name" / "table already exists".
- **Building a new command without quoting an interpolated path with `shq`** → shell injection or a broken script on names with spaces/quotes.
- **Adding an MCP tool without putting it on `guard::ADMIN_TOOLS` or `guard::CLIENT_TOOLS`** → the exhaustiveness test in `mcp/tools/tests.rs` fails; left off both by accident, it used to mean any paired `full` client could call it.
- **Adding a Tauri command without a row in `src-tauri/src/backend/verdicts.rs`** → `every_command_has_a_verdict` fails; unclassified in remote mode it would silently run local instead of routing to the hub.
- **Skipping the PR's local-CI mirror** because "it's a docs-only change" — `pnpm run build` still has to pass; run `scripts/ci-local.sh` (or its `--rust-only`/`--frontend-only` flags).
