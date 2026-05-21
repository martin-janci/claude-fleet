# Changelog

All notable changes to `claude-fleet` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Versioning

While the app is pre-1.0, the **minor** version tracks shipped product
milestones (one per development iteration) and the **patch** version is for
fixes and internal changes that don't add user-facing features. The `0.x`
series will culminate in `1.0.0` once Handoff and Freeze (original spec
§8.3–8.4) land and the open items in
[`docs/specs/2026-05-21-hardening-review.md`](docs/specs/2026-05-21-hardening-review.md)
are resolved.

The version is kept in sync across three manifests — `package.json`,
`src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json` — which must always
agree. Bump all three together.

> **Note:** versions `0.1.0`–`0.4.0` below are reconstructed from git history.
> Development moved faster than the version field was bumped (it sat at `0.2.0`
> from phase 2 through iteration 4b), so only `v0.2.0-phase2` exists as a git
> tag. Dates are approximate, derived from the per-iteration design specs in
> `docs/specs/`.

## [Unreleased]

### Added
- `docs/ARCHITECTURE.md` — backend/frontend internals, data model, IPC surface,
  event flow, and operation lifecycles.
- `docs/USE_CASES.md` — product overview and step-by-step workflow walkthroughs.
- `CHANGELOG.md` — this file.

### Changed
- `README.md` and `CLAUDE.md` refreshed: corrected the migration range
  (`001`–`007`), status bumped to iteration 4b, and cross-links to the new docs.
- Version bumped to `0.5.0` across all three manifests to reflect shipped
  milestones (it had been stale at `0.2.0`).

## [0.5.0] — 2026-05-21

Iteration 4b — Reviews — plus a full performance pass.

### Added
- **Reviews** — an on-demand action that spawns a `kind=review` tmux session in
  a source session's worktree, seeded with an editable multi-pass review
  prompt. The review is linked to its source and surfaced in the UI with a 🔍
  badge (`spawn_review` command; migration `005`).
- `ReviewDialog` with an editable review template; `SessionDetails` shows
  Reviewing / Reviews links between paired sessions.
- Worktree-aware related-session matching: sessions are grouped by a portable
  `worktree_key` derived from their cwd (migration `006`), so related sessions
  match across hosts regardless of path differences.
- Composite index `idx_sessions_project_wtkey` for the related-sessions query
  (migration `007`).

### Changed
- **Performance pass:** `list_sessions` reduced from O(hosts × sessions) to
  indexed queries; SSH round-trips cut in `new_session` and `send_prompt`;
  reconcile/probe/parse cleanups; the PTY now drains off-lock with a capped
  buffer; the frontend got dirty-row terminal rendering, batched scroll, an
  adaptive self-rescheduling PTY drain loop, and parallel store bootstrap.
- ANSI renderer gained `DECSTBM` scroll-region and `SU`/`SD` scroll support,
  fixing terminal overlap on update.
- `spawn_review` polls the pane for `cl` readiness instead of a fixed delay,
  and soft-fails prompt seeding (the live review session is kept either way).

### Fixed
- Terminal rows keyed by content to fix duplicated lines on update.
- Migrations `005` and `006` wrapped in transactions for consistency.

## [0.4.0] — 2026-05-20

Iterations 1–4a — multi-host foundations, the account model, cross-host
session memory, prompt transfer, and the async/event-driven responsiveness
rework.

### Added
- **Multi-host** — register hosts from `~/.ssh/config`, probe reachability,
  attach to remote tmux sessions over SSH. Per-host `ControlMaster` SSH
  multiplexing (`SshClient`). `RemoteTmux` / `LocalTmux` via a `TmuxExec` trait.
- **Account model** — each host's logged-in Claude account (email / org / seat
  tier) is auto-detected by probing the remote `~/.claude.json` `oauthAccount`.
  Normalized `accounts` table (migrations `003`, `004`). No credentials read or
  stored.
- **Cross-host session memory** — a session caches the account it was created
  under and preserves it across re-probe.
- **Prompt transfer** — `send_prompt` pushes text into a session via
  `tmux send-keys`; the `PromptComposer` can fan a prompt out to many sessions.
- **Related sessions** — `related_sessions` surfaces other sessions sharing a
  project, with a count badge per sidebar row.
- **Event-driven UI** — backend mutations emit row events (`EventBus` →
  `subscribeToRowEvents`); stores patch in place via `mergeOne` / `removeOne`
  instead of re-fetching, with optimistic merge from command return values.
- **Cancellation** — a `CancellationRegistry` + `cancel_command` lets the UI
  abort in-flight long operations (probe, `git clone`).
- Remote session creation auto-clones the repo and creates worktrees in the
  correct cwd.

### Changed
- All SSH-touching command handlers converted to `async fn` (`tokio`).
- Reconcile fans out across hosts in parallel (`JoinSet`) and writes each
  host's burst in a single transaction with emit-after-commit.

## [0.3.0] — 2026-05-19

Phase 3 — the embedded terminal.

### Added
- Embedded terminal attached to the selected tmux session, with input
  forwarding and resize handling.
- Remote attach via `ssh -tt` for sessions on other hosts.

### Changed
- Replaced xterm.js with a hand-rolled ANSI screen-buffer renderer
  (`src/lib/ansi.ts` + `TerminalView.svelte`) — xterm's renderer failed to
  repaint reliably in the WKWebView. Added DEC graphics + UTF-8 locale handling.

### Fixed
- Backfill `PATH` and locale at startup so a Finder-launched app finds `tmux`,
  `git`, and user wrappers.

## [0.2.0] — 2026-05-19

Phase 2 — local discovery and the project tree. Tagged `v0.2.0-phase2`.

### Added
- Filesystem scan of `~/projects/github.com/<owner>/<repo>` and a
  `git worktree` parser populating `projects` / `worktrees`.
- Local tmux discovery; `list_sessions` / `new_session` / `kill_session`.
- Sidebar project/worktree/session tree, recency filter pills, search.
- `NewSessionDialog` with a worktree picker; refresh on window focus.

## [0.1.0] — 2026-05-19

Phase 1 — bootstrap and UI shell.

### Added
- Tauri 2 + Svelte 5 project scaffold.
- Three-pane resizable layout with design tokens; theme store
  (auto / light / dark) with persistence.
- SQLite `Store` with a migrations module (migration `001`); `schema_version`
  tracking; `health_check` command.
- `Result<T, IpcError>` typed-failure contract for commands.
- CI: Rust + frontend checks on push and PR.

[Unreleased]: https://github.com/martin-janci/claude-fleet/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/martin-janci/claude-fleet/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/martin-janci/claude-fleet/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/martin-janci/claude-fleet/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.2.0-phase2
[0.1.0]: https://github.com/martin-janci/claude-fleet/releases/tag/v0.1.0
