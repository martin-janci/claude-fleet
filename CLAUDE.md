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
clean `main`. See `docs/RELEASING.md`. Once the release is pushed,
`scripts/release-mobile.sh <same-version>` tags fleet-mobile's `main` so its
own workflow builds the signed APK under the same version.

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
- **Settings metadata** (`service/settings.rs`): every `SPECS` row carries
  label, help, unit, what `0` means, tags, danger, restart and AI policy
  next to its kind and default (`every_spec_has_consistent_metadata`).
  `describe()` serves it to `get_settings { describe: true }` and
  `describe_fleet_settings`; `docs/settings-reference.md` and the settings
  tables in `docs/work-graph.md` / `docs/decisions.md` are generated from
  it — after editing a spec run
  `REGEN_SETTINGS_DOCS=1 cargo test -p fleet-core settings_docs_are_current`.
  This is P1 of the declarative pages framework
  (`docs/superpowers/specs/2026-09-28-declarative-pages-design.md`).
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
  verdict is written down once, in `backend/verdicts.rs`, for all 205
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
(pairing the desktop itself to a hub) are landed; see `docs/hub.md`. Since contract revision 5 a hub client adds
projects through the hub (`add_project` / `list_github_repos` tools), per
`docs/superpowers/specs/2026-09-27-hub-add-project-design.md`. Host
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
The user guide is `docs/work-graph.md`: update it with any change a user
sees. Its `work.*` settings table is generated (see *Settings metadata*
below).
Tracker secrets are read ONLY by `Store::resolve_tracker_credential`.
Work graph M4 (detection) is landed: one recogniser in Rust and TS over a
shared fixture (`service/work/recognize.rs`, `src/lib/work_keys.ts`), the
pure resolver (`service/work/resolve.rs`, rules R1–R9 and R3u; state signals are
current, rejections are final), `detect.rs` wiring the prompt / Stop / PR
probe / sync triggers, migration 049, `SessionRow.work_suggested` (a guess
never groups a session), and the chip / popover / batch review UI. The
SessionStart context (M4.5) is built but OFF behind
`work.session_start_context` (decision D5; the remote-host numbers are the
user's to take with `scripts/measure-session-start.sh`). M4.6, the opt-in classification
nudge, is landed OFF behind `work.classify_nudge`: after three turns with no
link and 1–5 candidates in the host's scope, one UserPromptSubmit per
conversation carries a ≤400-char note after the mail
(`service/work/nudge.rs`, migration 056 `conversations.classify_nudged_at`; migration 057 re-issues the read-cursor delete trigger that a rewrite of 044 left out of some databases);
Claude's answer, `work_link { source: agent_inferred }`, is only ever a
pre-selected suggestion (rule R11, strength `inferred`).
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
Work graph M6 (more providers) is landed: GitHub Issues (through `gh` on a
host, `transport = via_cli:<host>`, no token in fleet), Asana (`asana:<gid>`
keys, events-API sync tokens, the section map), Linear, and Jira Data Center
(admin-fenced site: exact host, resolve-then-refuse loopback / link-local,
optional `extra_ca`), all behind the `TrackerProvider` trait and the
`HttpTransport` seam (`net/via_host.rs` has `gh` and host-side `curl`, the
credential only ever on stdin), migration 051 (`tracker_views.sync_mark`,
`trackers.settings`). A new provider must pass the conformance suite
(`service/trackers/conformance.rs`, `conformance_suite!`) and
`tests_isolation_providers.rs`.
Work graph M7 (self-cleaning lifecycle) is landed: the pure planner
`service/gc/tidy.rs` (reasons, hard-coded protections), migrations 052
(UI-only archive, snooze / never per link, `sessions.last_touch_at`,
`work_items.reopened_at`) and 053 (`orgs.auto_tidy`), `work { tidy | reopened }` and
`work_link { archive | unarchive | snooze | never | dismiss | tidy_apply }`,
and auto-tidy in the GC sweep behind `work.auto_tidy` (OFF; safe kill only),
overridable per org. Tidy-up suggests; it never kills a dirty tree except
through safe kill, and a per-host token sees only its host's and org's
candidates.
Work graph M9.1 / M9.2 are landed: the Today view (`work { action: today }`,
Details' empty state and ⌘⇧T, a plain-text Copy standup) and the ticket
context card (`work { action: card }`, acceptance criteria from the cache,
Insert into composer with the hub-fenced `composer_text` — never sent).
M9.7 (the operator's starts and kills always confirmed; refused on a hub),
M9.3 (agent-written handover on demand, `work_link { action: handover }`)
and M9.6 (multi-repo start, `work_link start { project_ids }`) are landed
too. Write-back (D3), dead-session summaries (D10) and webhooks (D13) were
decided against; the user said yes to D3, D10 and D20 on 2026-09-27, as
M13.4e, M13.4c and M13.4a of M13. D13 stays no: its build (M13.4f) reached
`main` and was removed again (migration 062 stays, 064 drops its table;
build notes:
`docs/superpowers/plans/2026-09-26-work-graph-m13-decided-yes.md`).
M13.4c (D10) is landed (#327, fixed in #331):
`work_link { action: summarize }`, on demand only, one tool-less
print-mode fork on the session's own host (`service/work/summary.rs`,
`work.summary_model`), stored as a journal `summary` and fenced in the
brief.
M13.4e (D3) is landed too (#327, fixed in #332; tests #334): the PR
remote link on Jira only, per tracker `settings.write_back.pr_remote_link` (off), queued by the PR probe
for `manual` / `started` confirmed links in the tracker's own org, through
the outbox `tracker_writes` (migration 061) drained by the sync pass
(`service/trackers/write_back.rs`, `TrackerProvider::write`).
Work graph M10 (`docs/superpowers/plans/2026-09-25-work-graph-m10-settle.md`):
M10.4 is landed — Today's Stale opens Tidy-up narrowed to those sessions,
and the M5.5 filters (tracker / status / mine / has-session / archived)
have chips under the sidebar's "⚑ work" pill (`work_filters.ts`, through
`rowMatches`). The rest of M10 is landed too: the M9 review leftovers, the
work graph end to end in `scripts/hub-e2e.sh` against a loopback fake
tracker (the `e2e` feature; CI builds it as `WBIN` and fails without it),
the replay-ring numbers, and the phone's Today / ticket card; the written
acceptance run, `docs/work-graph-acceptance.md`, waits on the owner.
Work graph M11 (the long tail) is landed: "Name this work…" (`work_link
{ action: name }`, `work { local_items }`), resume probes the transcript,
tidy reason `idle_unlinked` (`work.tidy_idle_unlinked_days`, never
auto-tidied), GitHub Enterprise and per-tracker `SyncMetrics`
(`work_admin { status }`), and the tool budget paid back.
Work graph M12 (ship and operate) is landed: the upgrade test and
downgrade guard (`store::testgen`), the scale fixture and budget tests
(`service/work/scale_tests.rs`, migration 058), the `work.retention.*`
windows (`store/work_retention.rs`), trackers in `fleet_health` with a
Reconnect Attention item, and the review of the decided-against list
(`reviews/2026-09-26-work-graph-decisions-revisited.md`).
M13 (live use, `docs/superpowers/plans/2026-09-26-work-graph-m13-live-use.md`)
is closed (M13.5, #337): M13.1 (partial sync failures, #320), M13.2
(`work_admin { usage }`, #323 / #324), M13.4c and M13.4e above are on
`main`; M13.4a (D20) and M13.4d (D15, multi-start) are on fleet-mobile
(#51, #50). The work graph is *operating* (D26): new work is issues and
small plans. Two items stay open, waiting on the owner: the acceptance run
and its triage (M13.3), and D5 (M13.4b). Open decisions are the roadmap's table, and a decision-gated
feature starts only on the user's "yes".
Work graph M14 (the Work view: org → group → task → every session, and a
phone paired to one org) is the one milestone after it (D36): plan
`docs/superpowers/plans/2026-09-27-work-graph-m14-work-view.md`, design
`docs/superpowers/specs/2026-09-27-work-view-design.md`. M14.1a–d (the
backend: `work { tree | task | session_tasks | review | rules | … }` in
`service/work/view.rs`, `work_link { set_primary | place | assign_org | … }`
in `service/work/structure.rs`, migrations 066–067, org-bound clients as
`OrgScope::Org`, compare-and-set with `E_CONFLICT`, the desktop commands
and `work:changed`) and M14.2–M14.4 (the desktop Work view — `WorkTree`,
`WorkTaskDetail`, `WorkReview`, the rules / place / org dialogs, state in
`src/lib/work_view.ts` — and fleet-mobile's *My work*) are landed; the
desktop re-reads on `onWorkChanged` and the `workChanged` tick in
`work.ts`. `scripts/hub-e2e.sh` hub W section 10 runs the contract on a
real hub. M14.5, the acceptance run (Part R), waits on the owner.

The Jev evaluation (TypeSafe's decision model as an optional reader for
closed-set decisions) has started with a local language census: `fleet-hub
census languages` over `service::nl` (cargo feature `nl-detect`, lingua, ON
only in fleet-hub — the models add ~45 MB, kept there by D47). The decision
envelope is built and OFF (D35–D37): `service::decide` (`gate` / `decide`,
`DecisionBackend`, `jev.rs` fenced to api.typesafe.ai), `decide.*` settings,
per-org consent `orgs.jev_allowed` (migration 068), the record
`decision_runs` + key `decision_secrets` (069; the key is read ONLY by
`Store::resolve_decision_credential`, never raw text in a run), `fleet-hub
decide`; guide `docs/decisions.md` (its `decide.*` settings table is generated). The first use case, J3 `status_map`, is built (shadow / assist
only, off): the Asana probe keeps `config.unmapped_sections` /
`project_sections`, `service/decide/status_map.rs` asks one Choice per
unclassified section after a clean sync (`StatusMapTrigger`, daily), and
`fleet-hub decide proposals` lists what a person applies with `fleet-hub
tracker section-map`; follow-ups are recorded in `work_admin update`.
Assist is usable one proposal at a time (`status_map::decide_proposal`,
by run id: apply / apply_as through `work_admin update`, reject → the
follow-up only, hidden until a new answer): Settings → Work on a
standalone desktop (`status_map_proposals` / `decide_status_map_proposal`,
`LocalOnly` when paired) and `fleet-hub decide proposals apply|reject`.
Phase 0 (offline) is built for J1 `work_link` and J3: `fleet-hub decide bench
work-link | status-map` (`service/decide/bench/`: BM25, leakage guard, time
split, calibration, the test map's acceptance lines, D39 `--export-unlinked`
/ `--labels`) with the `claude -p haiku` baseline (D33,
`service/decide/haiku.rs`: a named host of the SAME org only, prompt on
stdin). J1 has no live adapter: it waits on its acceptance lines. Label
hygiene (D34) is built: an agent never overturns a person's rejection,
`store::Decider` records `agent` / `agent_started` vs `manual` / `started`
(`PERSON_SOURCES` gate write-back, auto-trust and person counts), and a
person's Clear work holds against the unchanged branch / PR (R9u, migration
070 `work_unlinks`). The test map is
`docs/superpowers/specs/2026-09-27-jev-test-map.md`.
Decisions D31–D47 and what is still open
are in `docs/superpowers/specs/2026-09-27-jev-language-census-design.md`.

Conversation event tracking is landed end to end (migration 037
`conversations` table; `SessionStart`/`PreCompact`/`PostCompact` hooks;
`/clear`, `/resume` and compaction tracked as conversation switches;
`session_conversations` API; the Conversations UI panel), per
`docs/superpowers/specs/2026-09-18-conversation-events-design.md`.

The desktop builds for Windows as a **client** (plan
`docs/superpowers/plans/2026-09-27-windows-desktop.md`, user guide
`docs/windows.md`): no `local` host (`retire_local_host`, as on a hub with
`hub.local_host=false`), no ssh multiplexing (`ssh::mux_supported`, off
there), the home/cache dirs through `fleet_core::home` only, and the hub
token in Credential Manager. Unix-only code and tests stay `#[cfg(unix)]`
(for a test module: a `#[cfg(unix)]` line above a bare `#[cfg(test)]`, the
form `no_eprintln_tests` recognises); `rust-windows` in CI keeps clippy and
the tests green there. `fleet-agent` and `fleet-hub` stay Unix-only.

Hub↔hub federation (cycle 3) is landed: two `fleet-hub` daemons link with
`fleet-hub pair --mode peer` / `peer add|list|remove`, a dialer supervisor
and a `peer_exchange` listener carry messages both ways by fleet address,
and `fleet_health.peer_links_down` reports a link in trouble, per
`docs/superpowers/specs/2026-09-24-hub-federation-design.md`.
