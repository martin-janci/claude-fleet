# CLAUDE.md

Orientation for Claude Code working in this repository.

## What this is

`claude-fleet` — a Tauri 2 desktop app (Rust backend + Svelte 5 frontend) for
managing long-lived Claude Code sessions running in tmux across multiple
machines over SSH. ~143,000 LOC Rust, ~55,000 LOC frontend.

The Rust side is a workspace: `crates/fleet-core` (Tauri-free service/store/SSH/MCP),
`crates/fleet-hub` (headless daemon, see `docs/hub.md`), `crates/fleet-proto` (the
hub/agent frame types, shared by both ends), `crates/fleet-agent` (the agent binary
for hosts the hub cannot reach — depends on `fleet-proto` only, never `fleet-core`),
`src-tauri` (the desktop app).

## Build & test

```bash
pnpm install
pnpm test                       # frontend (Vitest)
pnpm check                      # Svelte/TS type-check
cargo test --workspace          # backend (all crates)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo deny check                # licenses + advisories (cargo install cargo-deny --locked)
cargo build -p fleet-hub --locked   # headless hub (no Tauri libs needed)
scripts/hub-e2e.sh               # real fleet-hub/-agent e2e; needs tmux, opt-in via ci-local.sh --hub-e2e
scripts/ci-local.sh             # all of the above in CI order; --rust-only / --frontend-only / --hub-e2e
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
the six version files (+ `Cargo.lock`), prefills a `CHANGELOG.md` section
from the Conventional Commits since the last tag, commits, and creates the
`vX.Y.Z` tag. Never edit the version fields by hand; run the script from a
clean `main`. See `docs/RELEASING.md`.

`docs/control-api-reference.md` is generated from the MCP tool router. After
editing any `#[tool(...)]` description or the `generate_handler!` list,
regenerate it or CI fails:

```bash
REGEN_DOCS=1 cargo test -p fleet-core reference_is_current
```

`src/lib/hub_verdicts.generated.json` and the refusal table in `docs/hub.md`
are generated from `src-tauri/src/backend/verdicts.rs`. After editing any row,
regenerate them or CI fails:

```bash
REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen
```

## Architecture

- **Frontend stores** (`src/lib/*.ts`) hold app state as Svelte 5 runes. Backend
  mutations emit row events (`events.rs` → `events.ts` `subscribeToRowEvents`);
  the frontend patches stores in place (`mergeOne`/`removeOne`) instead of
  re-fetching. Mutation wrappers also do an optimistic patch from the command's
  return value.
- **Backend** (`crates/fleet-core/src/`, thin Tauri handlers in
  `src-tauri/src/commands/`): the handlers wrap the transport-agnostic logic in
  `service/`; SSH multiplexing in `ssh.rs` (per-host `ControlMaster`, async
  `tokio::process`); tmux command construction in `tmux.rs`; SQLite in `store/`
  (migrations are registered in the `MIGRATIONS` table there — add a new
  `NNN_<topic>.sql` plus an entry); the event bus in `events.rs`; cancellation
  registry in `cancel.rs`. The single global PTY (`pty.rs`) stays in
  `src-tauri`, since it is desktop-only.
- **Session listing** is cache-first: `service::sessions::list_sessions` serves
  stored rows and only runs a reconcile pass when the last one is stale;
  `refresh_sessions` is the forced path for an explicit user refresh.
- **Status vocabulary** (`claude_status`, `stuck_kind`) lives in the enums in
  `service/pane_intel.rs`; the MCP tool descriptions and the generated
  reference derive from them, so add values there, not in prose.
- **Control API** (`mcp/`): an embedded MCP server (off by default, localhost +
  bearer token) lets an AI assistant drive the fleet. Its tools call the same
  `service/` layer as the Tauri commands. See `docs/control-api.md`.
- **Client access** (`mcp/pairing.rs`, `mcp/events_route.rs`,
  `store/clients.rs`): a phone or browser pairs through a single-use code
  (`pair_client` → `POST /pair`) for a named, revocable client token
  (`full`/`readonly`) that is never the master and never reaches fleet admin,
  and follows `GET /events` instead of polling. Hub-only; `fleet-hub
  pair|client` is the operator's side. See `docs/hub.md` → *Pair a phone*.
- **Terminal** is a hand-rolled ANSI screen buffer (`src/lib/ansi.ts` +
  `TerminalView.svelte`), *not* xterm.js — xterm's renderer failed to repaint in
  the WKWebView setup. Only one PTY is attached at a time.
- **Hub daemon** (`crates/fleet-hub`): the same core headless; `hub.*`
  settings, `HubBase` in `service/hub.rs`.
- **Hub client mode** (`src-tauri/src/backend/`): a desktop paired with a hub
  (Settings → Hub) resolves once at startup to a window onto that hub; every
  command routes to a hub tool, refuses with `E_LOCAL_ONLY`, or is the same in
  both modes, under the rule *parity or refusal* in `docs/hub.md`. That
  verdict is written down once, in `backend/verdicts.rs`, for all 163
  commands; `backend/tests_routing.rs` holds the handler list, each command's
  body, and every routed call and refusal to it, and `backend/verdict_gen.rs`
  publishes it to `src/lib/hub_verdicts.generated.json` and the refusal table
  in `docs/hub.md`. Adding a command means: a row, then `route`/
  `refuse_local_only` **by command name** (never a second tool literal or a
  pasted sentence), then `REGEN_HUB_VERDICTS=1`, then — for a `LocalOnly`
  command the UI can reach — a `REASONS` entry or an allowlist line in
  `src/lib/hub_verdicts.test.ts`.

## Conventions

- Backend errors flow as `IpcError` (`ipc_error.rs`) with `E_*` codes; the
  frontend unwraps a `Result` type (`src/lib/result.ts`).
- Shell-quoting has **one** canonical implementation: `crate::shell::quote`
  (alias `shq`) in `crates/fleet-core/src/shell.rs`. Every value interpolated into an
  SSH/bash command string MUST be quoted with it. The former duplicate copies
  (`shell_quote`/`shell_quote_str`/`shell_escape`) were consolidated — do not
  reintroduce them.
- SQLite access goes through `Store` behind a `std::sync::Mutex`. Never hold the
  guard across an `.await`.
- No blocking I/O under `Mutex<PtyState>` and none on a sync Tauri command (a
  sync command runs on the macOS main thread). PTY input goes to the writer
  thread through its bounded channel — `E_PTY_BUSY` when it is full,
  `E_PTY_CLOSED` when the thread is gone; kill / reap / fd teardown runs on the
  `PtyParts` taken out under the lock, after the guard is released.

## Status & known issues

Iterations 1–4a are landed (multi-host, accounts, cross-host sessions, prompt
transfer, async/events rework), plus the MCP control API, background sessions,
the background reconcile tick, fleet_health roll-up, and the persistent session
event timeline (session_history). Handoff from the original spec is replaced by
`move_session` (Move to host…) and Freeze is descoped, per
`docs/adr/0001-descope-freeze-ship-move.md`. `move_session` now CARRIES
uncommitted and unpushed work plus small git-ignored files to the target
instead of refusing a dirty or unpushed source (`strict: true` restores the
ADR 0001 refusals), and the session's Claude directory and project memory
(slice 2 spec `docs/superpowers/specs/2026-09-20-move-carry-claude-state-design.md`),
per `docs/adr/0002-move-carries-work-as-is.md` and
`docs/superpowers/specs/2026-09-19-move-carry-engine-design.md`. The move's UI
is the Transfer sheet (terminal-header chip + `moves.ts`, live steps from the
`move:progress` event), per
`docs/superpowers/specs/2026-09-20-transfer-sheet-design.md`. A full
hardening review is in
`docs/specs/2026-05-21-hardening-review.md` — consult it before touching SSH
command construction, the PTY, migrations, or the optimistic-merge / event-bus
paths.

The headless `fleet-hub` daemon, `fleet-agent` for hosts the hub cannot reach
over SSH, paired-client access for phones/browsers, and hub-client mode
(pairing the desktop itself to a hub) are landed; see `docs/hub.md`. Host
reboot handling is landed in both halves, per
`docs/superpowers/specs/2026-09-17-host-reboot-session-survival-design.md` and
`docs/superpowers/plans/2026-09-19-host-reboot-recovery.md`: **survival**
(sessions are recovered by boot identity rather than declared lost on a
restart, migration 036 + `lost_reason`) and **recovery** —
`restore_host_sessions` batch-resumes a host's lost sessions over
`recreate_session`, `discover_lost_sessions` scans a host's Claude transcripts
for conversations fleet has no row for, and `new_session` takes a
`resume_claude_session_id`; the UI for both is in `HostDetail`. Recovery
reached `main` only on 2026-09-22: PR #161 was merged into the stacked branch
`feat/host-reboot-survival`, which was never re-merged after PR 1/2 (#135)
landed on its own, so for three days this paragraph described half a feature.
When a stacked PR says MERGED, check what it was merged INTO.

The work graph's M0–M3 are landed (work links anchored on participants,
group-by-work, and M2's work journal, carry rules, handover brief and resume
— `work` / `work_link` actions, `service/work/`; M3's trackers: Jira Cloud
read-only over `fleet-core::net`, the sync tick, `work_admin`, tickets /
lookup / start — `service/trackers/`, `store/trackers.rs`,
`store/tracker_items.rs`); read
`docs/superpowers/2026-09-24-work-graph-roadmap.md` before touching them.
Tracker secrets are read ONLY by `Store::resolve_tracker_credential`.
Work graph M4 (detection) is landed: one recogniser in Rust and TS over a
shared fixture (`service/work/recognize.rs`, `src/lib/work_keys.ts`), the
pure resolver (`service/work/resolve.rs`, rules R1–R9 and R3u; state signals are
current, rejections are final), `detect.rs` wiring the prompt / Stop / PR
probe / sync triggers, migration 049, `SessionRow.work_suggested` (a guess
never groups a session), and the chip / popover / batch review UI. The
SessionStart context (M4.5) is built but OFF behind
`work.session_start_context` (decision D5); M4.6 is not done.
Work graph M5 (organisations) is landed: migration 050 (`orgs`, text-keyed
`org_rules`, `hosts.org_id`, `work_links.snap_org_id`), `SessionRow.org_id`
(SQL, `session_org_sql!`, held equal to `store::org_of_session`), and the
org BOUNDARY for per-host tokens — `service::orgs::OrgScope`, made only by
`Caller::org_scope`, filters every work read in the service layer, and
`call_tool` redacts session rows' work in everything a host receives. M3's
per-host ticket fence is kept (composed with the org). Cross-org links need
`force_cross_org` for every caller; `isolate_sessions` (D7) is per org, off.
Any new `work` / `work_link` / `work_admin` action needs a row in the
isolation matrix (`mcp/tools/tests_isolation.rs`), which fails otherwise.
Work graph M9.1 / M9.2 are landed: the Today view (`work { action: today }`,
Details' empty state and ⌘⇧T, a plain-text Copy standup) and the ticket
context card (`work { action: card }`, acceptance criteria from the cache,
Insert into composer with the hub-fenced `composer_text` — never sent). The
rest of M9 (write-back D3, handover, summaries, multi-repo, operator,
webhooks) is planned in `docs/superpowers/plans/2026-09-24-work-graph-m9-beyond.md`
and waits on the user's decisions.

Conversation event tracking is landed end to end (migration 037
`conversations` table; `SessionStart`/`PreCompact`/`PostCompact` hooks;
`/clear`, `/resume` and compaction tracked as conversation switches;
`session_conversations` API; the Conversations UI panel), per
`docs/superpowers/specs/2026-09-18-conversation-events-design.md`.
