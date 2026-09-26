# Work graph: roadmap

**Date:** 2026-09-24
**Read this first** when you pick up the work-graph effort: work items,
trackers, work-aware sessions, resume and work memory. Other documents:

- Design: `specs/2026-09-24-work-graph-design.md`. **§0 (revision 2) is authoritative.**
- Review: `reviews/2026-09-24-work-graph-specialist-review.md`. Its corrections are C1–C29.
- Takeover prompt for a new agent: `specs/2026-09-24-work-graph-takeover-prompt.md`.

## The goal

Fleet should know **what work** each session is doing, and do it without the
user having to organise anything:

1. Sessions group by ticket or workstream.
2. Work can start from a ticket.
3. Old work can be resumed with its context intact.
4. Finished work tidies itself away, and nothing is ever destroyed automatically.

The same model serves one developer with no tracker and a contractor juggling
Jira for Company A and Asana for Company B.

## How to work this roadmap

- **One milestone yields one or more PRs, and each PR ships usable value.**
  Every milestone has a *Value* line. If a PR cannot say what the user gains,
  it is too thin or in the wrong order.
- **Each milestone follows the same cycle:** brainstorm → spec (a delta
  against §0) → plan → subagent-driven execution → whole-branch review → PR.
  This is how Transfer was built (`2026-09-20-transfer-roadmap.md`).
- **Test before you trust.**
  - Every correction in the review (C*) that a milestone touches gets a
    failing test first.
  - Data-model milestones run `cargo test --workspace` and `pnpm test`.
  - Anything touching the wire runs `scripts/ci-local.sh --hub-e2e`.
- **Stay flexible.** Each milestone must degrade gracefully when the thing
  before it is unused. With no tracker, no org and no `gh`, the features still
  work with what is there.
- **Revise this file** when a milestone lands or reality disagrees with it.
  Record the change in *Revisions* at the bottom.

## Milestones

### M0: prerequisites and hygiene (independent small PRs, can start now)

Bugs the review found that hurt today and that the work graph builds on.

| PR | What | Evidence | Test |
|---|---|---|---|
| M0.1 | Add `tags` to `PHONE_SESSION_FIELDS`. The phone currently **wipes tags** when you add one. | `mcp/tools/views.rs:71` | views projection test + phone `SessionRow` decode |
| M0.2 | Keep identity across `rename_session`: `UPDATE sessions SET tmux_name` (or repoint the participant) before the reconcile | C2 | failing test first: rename keeps id, participant, conversations |
| M0.3 | Mint participants eagerly on the reconcile INSERT; reassign rows other than messages in the repoint collision merge (a hook point for `work_links`) | C1, C8 | store tests |
| M0.4 | `move_session` copies `tags` | review §3 | move finalise test |
| M0.5 | `fleet-friendly-name` skill: `whoami` instead of `list_sessions`; fix the hook-spec drift note | review §3 | — |
| M0.6 | Tool-description budget: tighten the existing descriptions to recover headroom **before** the work tools land | C21 | `BUDGET_BYTES` test |

**Status (2026-09-24):**

- M0.1, M0.2, M0.4 and M0.5 are written on `claude/cloud-fleet-work-graph-0x0l5k`.
- M0.3 is written as migration **045** (`045_session_participants.sql`):
  an `AFTER INSERT` trigger plus a backfill.
- The repoint-collision half of M0.3 moves to M1, because there are no
  `work_links` to reassign yet.
- **None of it has been compiled or tested yet.** The session that wrote it
  had no access to `static.crates.io`. Run `cargo test --workspace` and
  `cargo clippy` before a PR.
- M0.6 is still open: it needs `REGEN_DOCS` and a byte measurement.
- Found on the way, **not changed**:
  - `whoami` has `readonly: false` in `TOOL_POLICIES` (`mcp/guard.rs:244`),
    although its own code comment says a readonly token may call it.
  - Decide which one is right. The skill now tolerates both.

**Value:** phone tags stop disappearing; renames stop orphaning mail and
history. **Exit:** everything the graph anchors on (participants) is present
and durable for every session kind.

### M1: work without a tracker (value on day 1, no setup)

- **Storage:** migration 046 (045 went to M0.3) with the full §0.2 schema. The tables not yet in
  use stay empty. The migration includes the end-snapshot trigger, the FKs and
  the re-run guard.
- **Signals:**
  - The current branch key comes from transcript `gitBranch`, read in the
    `context::refresh` tail.
  - The fallback is `worktrees.branch`.
  - Keys are recognised case-insensitively, with word boundaries.
  - Without a tracker, a key only becomes an **unresolved ref**, and only when
    it came from a branch name.
- **Local work items:** "Name this work…" on a row or a group header.
- **Manual link actions:** set, clear, reject, primary.
- **`SessionRow.work`** plus the `work` event kind.
- **Sidebar:**
  - Group by `Project · Work · Host · Flat` in the View ▾ menu. Work mode is
    hybrid: work groups first, sessions without work fall back under their
    project, and there is no "Unclassified" bucket.
  - A key chip on the row.
  - A scope selector derived from `projects.owner`, shown only when there are
    two or more owners.
- **NewSessionDialog:**
  - An optional "Work" field (key, local item or URL).
  - It prefills the branch (`slugifyBranch(key + title)`) and the friendly name.
  - Duplicate guard: "ABC-123 already running on X · Jump / Start another".
- **Group header:** PR/CI roll-up from the existing `pr_url` / `ci_status`.
- **MCP:** `work` (read) and `work_link`. Verdicts: Routed. Regenerate the docs
  and verdicts.

**Value:** sessions group by ticket or workstream, you can start work
by key, and names stop being "yes". **Tests:**
- the trigger fires on all five retire paths;
- a move, restore or recreate keeps links;
- the builder `buildSessionsByWork`;
- regex cases (`abc-123-x`, `XABC-12`, `UTF-8`, `SHA-256`).

**Status (2026-09-24):**

- **M1a (frontend, verified with pnpm check / test / build):**
  - `work_keys.ts` recognises keys in tags, the worktree branch or the
    worktree name.
  - "⧉ by work" hybrid grouping with a PR/CI roll-up, and a key chip on rows.
  - The New session dialog shows a work note, a duplicate guard ("Open it")
    and turns a pasted ticket URL into its key.
  - The frontend recognition stays as the fallback when a row has no
    `SessionRow.work`.
- **M1b.1 (storage):**
  - Migration 046 (`work_items`, `work_links`, and the end-with-snapshot
    trigger on participant retirement) plus `store/work.rs`.
  - Verified with `cargo fmt`, `clippy -D warnings` and `cargo test`: it
    needed no fix.
- **M1b.2 (landed, 2026-09-24):**
  - `SessionRow.work` (`#[serde(default)]`): the primary confirmed live link,
    read by one correlated subselect in `SESSION_COLUMNS`, so listed and
    emitted rows both carry it. Also `SessionRow.work_rejected` (the row's
    sticky rejections): without it the frontend's own recognition showed a
    rejected branch key again on the next render.
  - A link write bumps `row_version` and emits `session_updated`. No separate
    `work` event kind yet: nothing but the row changes in this slice.
  - `work` is in `PHONE_SESSION_FIELDS` (18 columns).
  - MCP `work` (read) and `work_link` (link / reject / unlink; `source`
    manual | agent), `require_host`-gated, one `service::work` entry for
    both transports. `BUDGET_BYTES` raised 64,832 → 65,787 (measured
    65,687): two tools do not fit in 100 B. M0.6 is where it is paid back.
    `declare`, `primary`, `start` and `resume` are not actions yet.
  - Tauri `session_work_links` / `link_session_work` / `reject_session_work`
    / `unlink_session_work`, all `Routed` (137 commands).
  - Frontend: `workKeyFor` prefers the link (source `link`), skips rejected
    keys; `work.ts`; a `#` work menu on each row (set, *Not KEY*, *Clear*).
  - Not done here: "Name this work…" on a group header (local items with a
    title — the store has `create_local_work_item`, no tool or command
    exposes it yet), the New session dialog does not link with
    `source: started` yet, and the phone app does not read `work`.

### M2: resume and work memory

- **Archived work:**
  - Ended links render as ghost rows from their snapshot inside the work group,
    with a collapsed `Done · n` section.
  - **Resume ▾** is a split button: *continue last conversation* (default, when
    the host is reachable), *fresh with brief*, or *fresh*. The tooltip shows
    the host and branch it will land on.
- **Carry rules:**
  - A resumed `claude_session_id` that matches an ended link re-attaches it.
  - A `keep_source` fork copies links (`source=forked`).
  - Reviews and task workers inherit the parent's link (`role`).
  - Purge warns when a conversation is linked.
- **`work_journal` harvest.** It runs at SessionEnd, compaction, kill and the
  ghost reap, **before** the cascade delete, and records:
  - the conversation span and first prompt;
  - the last five `turn_done` entries;
  - Claude Code's own compaction summary;
  - the outcome (branch, head, ahead count, PR, diff stat).
- **Handover:** a deterministic template (§0.6 and review §5) built from the
  item, the snapshots and the journal. It is delivered as a message from the
  `hub` participant through `additionalContext`, followed by a short start
  prompt, and is **never typed into a pane**. A `work` read action returns the
  full context (pull, not push).

**Value:** "pick up ABC-123 where I left off" works even weeks after the
session was killed. **Tests:**
- claude-id carry;
- the journal survives session deletion;
- the handover is under 8000 chars and wrapped with `mark_untrusted`;
- no send while `stuck_kind=trust_prompt`.

**Status (2026-09-24): landed** on `claude/cloud-fleet-work-graph-0x0l5k`,
per `plans/2026-09-24-work-graph-m2-resume-and-memory.md` (its *Revisions*
list every deviation). Verified with `cargo fmt`, `clippy -D warnings`,
`cargo test` (fleet-core, claude-fleet, fleet-hub) and `pnpm check` /
`pnpm test`; the manual acceptance on a real fleet is still to do.

- **M2.1 journal** (1296dfa): migration 047 `work_journal`, keyed by
  `claude_session_id`, capped per conversation; SQL triggers write the one
  `conversation` row on every close and BEFORE every session delete (kill,
  reap, move, host/project removal — all before the cascade); Stop journals
  `turn_done`, PostCompact Claude's own summary (off-lock tail read);
  retention `work.journal_days` (90) never sweeps confirmed work.
- **M2.2 carry** (11d3e2c): a rebind onto a conversation an ended link
  snapshotted re-attaches the work (`resumed`), whatever started it; a
  `keep_source` fork copies links (`forked`); reviews and task workers
  inherit (`role`); C8's collision merge keeps links; purge marks
  `resumable=0`.
- **M2.3 handover** (5845cfc): deterministic brief ≤ 4000 chars plus one
  read-only git probe; third-party text fenced by `mark_untrusted`; the
  brief is a `handover` journal row packed ahead of inbox mail in
  `additionalContext` and stamped once.
- **M2.4 resume** (6da403d): `work { action: context | resume_plan |
  purge_impact }` and `work_link { action: resume }` on the existing tools
  (budget 66,521); Tauri `work_resume_plan` / `resume_work` /
  `work_purge_impact` Routed (140 commands). The start prompt is typed only
  into a ready REPL, never into the trust dialog.
- **M2.5 UI** (d14b9f6, 94c4961): Done sections and past-only groups in group-by-work,
  Resume split button + dialog (reasons, Jump, host override, editable
  brief), the New-session "has previous work · Resume" note, and the purge
  warning.
- **Not done:** the phone does not show past work or resume (M8); the
  resume plan does not probe whether the transcript file still exists on
  the host (purge flag + reachability + held-conversation only); the
  acceptance run on a real fleet.

### M3: tracker foundation and Jira Cloud (read-only)

- **Provider layer:** the `TrackerProvider` trait and `HttpTransport` (§0.4).
  - Transport spike first. `reqwest` with minimal features and ring, versus the
    lifted `remote.rs` client; `cargo deny` decides.
  - Jira adapter per C23–C29:
    - `search/jql` with fields;
    - identity by id, with key aliases;
    - `statusCategory` plus `resolution`;
    - `hierarchyLevel`;
    - sprints per project;
    - favourite filters referenced as `filter = <id>`;
    - ADF → text.
- **Sync as a `FleetTasks` method:**
  - per-view time watermarks with overlap;
  - linked items refreshed by id;
  - 429 handling with `Retry-After`;
  - a `state` field of `ok|auth_failed|rate_limited|unreachable`;
  - frames only on a real change.
- **Credentials:**
  - Stored in `tracker_secrets`, with `env:` / `file:` refs.
  - Redaction patterns for Basic auth and `ATATT` tokens, a diagnostics literal
    list, and `last_error` redacted.
  - An SSRF allowlist.
- **Admin:** `work_admin` (Master) and `fleet-hub tracker add|set-credential|test`.
  Settings → Work is a small section: trackers and orgs only.
- **Connecting and starting work:**
  - Connect by **pasting any ticket URL**: the site and key are inferred, and
    the user is asked only for email and token.
  - ⌘K gains a *ticket* kind with My work and Current sprint (when sprints
    exist). Pasting a URL resolves it. Enter starts work or jumps to the live
    session; ⌘↵ starts with the defaults.
  - `start_work` and `resume_work` actions on `work_link` perform the whole
    compound flow on the hub. Desktop and phone share them.
- **After connecting:**
  - Unresolved refs bind to items.
  - A retro-link reveal shows "14 sessions mention ABC-* · Review".
- **UI:** status appears on chips and group headers.

**Value:** start from a ticket, and see its status beside the sessions.
**Tests:**
- normalisation over recorded fixtures: custom statuses, no sprints,
  team-managed and company-managed projects, a moved issue, ADF;
- HTTP cases 401, 403 on a view, CAPTCHA, 429, a repeating page token,
  offline;
- no secret on any read path;
- a paired desktop runs no sync (`tests_startup.rs`).

**Status (2026-09-24): landed** on `claude/cloud-fleet-work-graph-m3`
(stacked on the M0–M2 branch), per
`plans/2026-09-24-work-graph-m3-trackers-and-jira.md` (its *Revisions* list
every deviation). Verified with `cargo fmt`, `clippy -D warnings`
(workspace), `cargo test` (fleet-core, claude-fleet, fleet-hub),
`cargo deny check`, `pnpm check` / `pnpm test`, and `scripts/hub-e2e.sh`
(102/102); no test talks to a real Jira. The manual acceptance on a real
Jira Cloud site is still to do.

- **M3.0 transport** (7260779): option A — the desktop's TLS/HTTP/1.1
  client lifted into `fleet-core::net` (zero new crates); `HttpTransport`,
  `DirectTransport` (https only, host policy before connect, no redirects,
  body cap, timeout), `FakeTransport`.
- **M3.1 schema, secrets, admin** (b44a5e4, 98debf2): migration 048
  (`trackers`, `tracker_secrets`, `tracker_views`, tracker columns on
  `work_items`); secrets only through `resolve_tracker_credential`,
  `env:`/`file:` refs; the `*.atlassian.net` fence; redaction of Basic auth
  and `ATATT…`, diagnostics literals; `work_admin` (Master, confirm-gated
  remove), `fleet-hub tracker …` (token from stdin/env/ref, never argv),
  five `LocalOnly` desktop commands; events kind `work`, never on a
  host-bound `/events` stream.
- **M3.2 Jira adapter** (33d6f44): the `TrackerProvider` trait and
  `JiraCloud` per C23–C29, over sanitised fixtures.
- **M3.3 sync** (d149570): the `FleetTasks` tick (hub and standalone
  desktop only), views + linked-by-id + keys typed before connecting,
  retro-binding, unavailable-not-gone, events only on real change,
  `status_change` journal rows, `work.sync_interval_secs`.
- **M3.4 API** (6ed000a): `work { tickets | lookup | trackers }`,
  `work_link { start }`, the per-host fence, four Routed commands.
- **M3.5 UI** (d147523): ⌘K tickets and lookup, Enter / ⌘↵, the dialog's
  ticket mode with an editable brief, status on chips and work headers,
  Settings → Work, the retro-link reveal.
- **M3.6 docs**: `docs/hub.md` → *Trackers*, `docs/concepts.md` → *Work*,
  `docs/control-api.md`, the control skill.

### M4: smarter detection and explanations

- **More signals:**
  - URL extraction (Jira, Linear, Asana, GitHub).
  - The PR probe gains `headRefName,title,body,closingIssuesReferences` plus
    commit trailers.
  - Keys in prompts, with the loop and dump guards.
- **Resolver behaviour:**
  - Conversation link windows: `/clear` provisionally ends the window, and the
    latest conversation decides the primary link.
  - The full §0.3 resolver runs as a pure function, tested with table tests.
- **UI:**
  - Suggestion chip with evidence ("branch `abc-123-login` since 09:05 · rule R3").
  - Confirm, *Not this*, or *Pick another*.
  - Batch review from Attention (j/k, y/n).
  - "Trust branch keys in this repo".
- **SessionStart:**
  - Measure a synchronous SessionStart hook.
  - If it is acceptable, inject the linked ticket context on startup, resume and
    compact (not on `clear`).
- **Optional nudge:** an opt-in classification prompt through
  `additionalContext`, using the session's own model. At most once per
  conversation, and only with at most five candidates in scope.

**Status (2026-09-24): landed** on `claude/cloud-fleet-work-graph-m4`
(stacked on M3), per `plans/2026-09-24-work-graph-m4-detection.md` (its
*Revisions* list every deviation). Verified with `cargo fmt`, `clippy -D
warnings` (workspace), `cargo test` (fleet-core, claude-fleet, fleet-hub),
`pnpm check` / `pnpm test` and `scripts/hub-e2e.sh` (102/102); the manual
acceptance on a real fleet is still to do.

- **M4.1** (01a1a90): one recogniser, `service/work/recognize.rs`, and its
  TypeScript twin `extractTicketRefs`, both over one shared fixture.
- **M4.2 + M4.3** (d5f28a9): migration 049; the live branch from the
  transcript, PR fields and commit trailers from the probe, prompt matches
  behind the loop and dump guards; the pure resolver (R1–R9), applied only
  on a real change; `SessionRow.work_suggested`.
- **M4.4** (ea1dc5e): `work_link` confirm / reject by `link_id` /
  `trust_project` (+172 B), two Routed commands; chip states, the evidence
  popover, `y` / `n` / `l`, the batch review sheet, the Undo toast.
- **M4.5** (51fc47b): SessionStart work context behind
  `work.session_start_context`, **off**; measured and recorded (D5 stays
  the user's).
- **M4.6** (landed 2026-09-25, OFF behind `work.classify_nudge`): the
  opt-in classification nudge — one ≤ 400-char note per conversation after
  three link-less turns with 1–5 candidates in the host's scope; Claude's
  answer (`source: agent_inferred`) is a pre-selected suggestion only (R11).
  See the M4 plan's Revisions.
- **Remote-host SessionStart measurement:** the procedure is written
  (`scripts/measure-session-start.sh`, M10.4) and the M4 plan has a
  placeholder table; **to be measured by the user**. D5 stays open.
  (Suggestions on the phone landed with M8.)

**Value:** most sessions are linked correctly without touching anything, and
every link says why. **Tests:** the resolver table (conflicts, a sticky reject,
a branch change, two trackers sharing a key, a late-known key), the guards,
and hook/probe integration via `FakeSsh`.

### M5: organisations and isolation

- Named orgs and `org_rules` (owner, repo, path prefix, host).
- The scope selector can use named orgs.
- The scope never hides needs-you rows: "2 need you in Personal →".
- **Security boundary:** `hosts.org_id`. A host-bound caller sees only its org
  (or unassigned); `work` frames on host-bound `/events` are filtered by org.
  Org and rule mutations are Master-only.
- Filters compose: org × tracker × status category × assignee × has-session ×
  archived.

**Value:** Company A and Company B stay separate in the UI *and* in what each
company's hosts can read. **Tests:** isolation matrix per caller kind
(master, client full, client readonly, host A, host B).

**Status (2026-09-24): landed** on `claude/cloud-fleet-work-graph-m5`
(stacked on M4), per `plans/2026-09-24-work-graph-m5-orgs-and-isolation.md`
(its *Revisions* list every deviation). Verified with `cargo fmt`, `clippy
-D warnings` (workspace), `cargo test` (fleet-core, claude-fleet, fleet-hub;
only the four chmod tests that fail as root on `main` fail), `cargo deny
check`, `pnpm check` / `pnpm test` and `scripts/hub-e2e.sh` (102/102). An
independent security review of M5.3 ran before it was pushed; its findings
are fixed and covered. The manual acceptance on a hub with two orgs is still
to do.

- **M5.1 + M5.2** (bb9e44a): migration 050 (`orgs`, text-keyed `org_rules`,
  `hosts.org_id`, `work_links.snap_org_id` + its retirement trigger);
  `SessionRow.org_id` computed in SQL and held equal to the pure resolver;
  `org_id` on hosts, trackers, links and `work` summaries; `work { scopes |
  orgs | org_suggestions }`; `work_admin`'s org actions (Master-only,
  removals confirm-gated); `fleet-hub org …`; seven LocalOnly and three
  Routed desktop commands (161).
- **M5.3** (5515199): `OrgScope` + `Caller::org_scope`, applied in the work
  service layer; M3's per-host ticket fence kept and composed with the org
  fence; no existence oracle; the cross-org integrity rule with
  `force_cross_org`; readers-scoped briefs and SessionStart context; the
  `call_tool` redaction backstop and per-frame `/events` fencing; D7
  `isolate_sessions`; the isolation matrix.
- **M5.4 + M5.5** (74a29c3): the scope selector (⌘⇧O), needs-you across
  scopes, Settings → Work → Organisations (read-only when paired), colour
  bars, the cross-org "Link anyway"; one `rowMatches` for the sidebar's
  two modes, past work and ⌘K.
- **Filter chrome** (M10.4): tracker / status / mine / has-session /
  archived are chips under the sidebar's "⚑ work" pill, persisted, through
  the same `rowMatches`.
- **Not done:** a Today view to scope (M9 does not exist yet); the phone
  does not show orgs (M8); the manual acceptance.

### M6: more providers

The order serves the user's real setup. Decision D1 below may reorder it.

1. **GitHub Issues through `gh`.** No credentials. Tests repo-relative `#n`,
   multiple assignees and `NOT_PLANNED`.
2. **Asana**, Company B's tracker.
   - It has no human keys, so detection relies on URLs and manual links.
   - A task can sit in several projects, which maps to `containers`.
   - `completed` gives the status; sections optionally map to status categories.
3. **`ViaHost` transport:** curl, `acli` or `gh` on a chosen host. It reaches
   a tracker only visible from a laptop VPN and keeps the token out of fleet.
4. **Linear** (GraphQL; `ENG-123` keys are told apart from Jira keys by the
   probed team keys).
5. **Jira Data Center** (PAT, v2 API, Epic Link, an extra CA).

**Value:** one work model across all of the user's trackers.

**Status (2026-09-24): landed** on `claude/cloud-fleet-work-graph-m6`
(from M4, with M5 merged in), per
`plans/2026-09-24-work-graph-m6-more-providers.md` (its *Revisions* list
every deviation). Verified with `cargo fmt`, `clippy -D warnings`
(workspace), `cargo test` (fleet-core, claude-fleet, fleet-hub), `cargo
deny check`, `scripts/hub-e2e.sh` (102/102) and `pnpm check` / `pnpm test`;
no test reaches a real tracker. The manual acceptance on real accounts is
still to do.

- **M6.0** (008de56): the provider conformance suite (ten scenarios, a
  golden per adapter), `Caps` / `ItemRef` / `RefCtx` / sync-token
  refinements, `TrackerNet`; Jira passes it unchanged.
- **M6.1 + M6.2** (2694653): GitHub Issues through `gh` on a host
  (`via_cli`, no token in fleet) and Asana (PAT, `asana:<gid>` keys, events
  API sync tokens, the section map); `SshExec::run_with_stdin`; per-provider
  site fences and host policies; `tracker_claims`.
- **M6.3** (456f517): `via_host` — curl on a host, the credential on stdin
  into a private temp file, never in argv.
- **M6.4** (5017bab): Linear (team keys settle `ENG-123`, team moves keep
  links).
- **M6.5** (33ff03e): Jira Data Center with an admin-fenced site (exact
  host, resolve-then-refuse loopback / link-local, pinned connect, an extra
  CA) and `jira_common.rs`.
- **M5 merge** (a478a51): migration 050 → 051; every new provider passes
  M5's isolation rules (acceptance 6).
- **M6.6** (ea01b01): Connect any tracker by pasting a URL, provider
  badges, the Asana section map editor; docs.
- **Not done:** `acli`, GitHub Enterprise Server, per-provider metrics, the
  phone (M8), the manual acceptance.

### M7: self-cleaning lifecycle

- **Tidy-up sheet** in Attention, shown only when there are candidates.
  - Reasons:
    - a done item, idle for at least N days;
    - a merged PR with an idle session;
    - duplicate sessions on one worktree;
    - a ghost nearing `lost_ttl`.
  - Rows are preselected. The default action is **safe kill**. Alternatives:
    kill, snooze 7 days, or "never for this work".
  - Reuses `selectedIds` and the bulk bar.
- **GC:** the `gc.rs` planner feeds suggestions and never kills a session
  linked to in-progress work. Automatic archiving is opt-in, per org, and only
  through the safe path.
- **Reopened items:** the badge "reopened · 2 past sessions" plus an Attention
  entry and a toast.

**Value:** the sidebar stays clean, and nothing useful is lost.

**Status (2026-09-24): landed** on `claude/cloud-fleet-work-graph-m7`
(stacked on M4), per `plans/2026-09-24-work-graph-m7-self-cleaning-lifecycle.md`
(its *Revisions* list every deviation). Verified with `cargo fmt`, `clippy -D
warnings` (workspace), `cargo test` (fleet-core, claude-fleet, fleet-hub;
only the four chmod tests that fail as root fail), `pnpm check` / `pnpm
test` and `scripts/hub-e2e.sh` (102/102); the manual acceptance on a real
fleet is still to do.

- **M7.1** (1c9c2e2, the pure planner): `plan_tidy` in `service/gc/tidy.rs` — five
  reasons, ranked secondary reasons, hard-coded protections each with its
  own test, snooze / never, and what auto-tidy may act on.
- **M7.2** (6932b72, storage and API): migration 052 after the M5 and M6 merges (`work_links.archived_at` /
  `tidy_snoozed_until` / `tidy_never`, `sessions.last_touch_at`,
  `work_items.reopened_at`); archive UI-only and undone by the next prompt
  or attach; reopen as an event (`reopened` journal row); `state` in the PR
  probe; four `work.*` settings; `work { tidy | reopened }` and `work_link {
  archive | unarchive | snooze | never | dismiss | tidy_apply }` (+639 B, no
  new tool, `work_link` confirm-gated for tidy_apply's kills); auto-tidy in
  the GC sweep behind `work.auto_tidy` (off); eight Routed commands (159).
- **M7.3** (e7b4be9, UI): "Tidy up · n" and "Reopened · n" in the attention strip,
  the Tidy-up sheet, archived sessions in their group's Done, the reopened
  badge and Resume, Settings → Work → Lifecycle with a dry run.
- **M7.4**: `docs/concepts.md` → *Lifecycle*, `docs/hub.md` → *Tidy-up and
  auto-tidy*, `docs/control-api.md`.
- **After M5 merged:** migration 051 (M5 took 050; 052 once M6 took 051), the per-org override
  `orgs.auto_tidy` (migration 052; 053 after M6), and org-scoped candidates / apply / reopened for a
  per-host token.
- **Not done:** `idle_unlinked`
  (an unlinked session has no link to archive under); the phone (M8); the
  manual acceptance.

### M8: phone

- **Hub side:** `SessionRow.work` in `PHONE_SESSION_FIELDS`.
- **Phone data:** subscribe to `work` in `SNAPSHOT_EVENT_KINDS`, and update the
  allowed-tools test.
- **Screens:**
  - `groupSessions` gains group-by-work.
  - A "My work" filter chip.
  - A ticket chip in `SessionScreen`; a suggested link offers Confirm or
    *Not this* (hidden for `readonly` tokens).
  - A ticket list with *Start here* and *Resume* (host picker only).
- **Gating:** by the hub's tool list; coordinate any `MAX_HUB_CONTRACT` change.

**Value:** triage and start work from the phone.

**Status (2026-09-24): M8.0 (hub side) landed** on
`claude/cloud-fleet-work-graph-m8` (stacked on M4), per
`plans/2026-09-24-work-graph-m8-phone.md`: `work_suggested` in
`PHONE_SESSION_FIELDS`; `work` / `work_link` `action` served as a schema
`enum` from the parsers' own tables (+66 B); a test pinning that a readonly
client token sees `work` but not `work_link`, and no client `work_admin`. No
contract bump. **M8.1–M8.5** (fleet-mobile: the model, `tools/list`
discovery and work transport; group by work, "My work" and the row chip; the
session screen's ticket chip and decisions; the Tickets sheet with Start /
Resume / Jump; docs) landed on fleet-mobile `main` via
martin-janci/fleet-mobile#32 (merge `9b9212e`). A parallel M8.1–M8.2 on
fleet-mobile's `claude/cloud-fleet-work-graph-m8` is superseded by it. Note
the plan's Revisions item 4 (an absent `work` on a row means none), which
#32 follows.

**M8.6** (the phone catches up with M4.6, M5 and M9.1–M9.3): `org_id` joins
`PHONE_SESSION_FIELDS` (hub side, no contract bump); on fleet-mobile the
*Today* sheet with *Copy standup*, a ticket's acceptance criteria and past
work, *Ask for a handover* followed on the timeline, org labels and an org
filter, and the `agent_inferred` wording — on fleet-mobile's
`claude/cloud-fleet-work-graph-m8-6`, per the M8 plan's §M8.6.

### M9: beyond

The ideas the review ranked, for when M1–M8 have settled.

- **Today view** (⌘⇧T, also the empty state of Details):
  - in progress;
  - waiting on you, grouped by work;
  - shipped today;
  - stale;
  - a **Copy standup** button.
- **Ticket context card** in Details: acceptance criteria, the link, and
  "insert into composer".
- **Agent-written handover** through the safe-kill marker pattern.
- **Opt-in summaries of dead sessions:** `claude -p --resume --fork-session`,
  hooks disabled, a small model.
- **Write-back (opt-in, per tracker):**
  - Start work moves the ticket to the transition whose *target category* is
    in-progress.
  - The PR is added as a remote link with an idempotent `globalId`.
  - Worklog from session time.
- **Multi-repo start:** one ticket, several repos, sibling sessions.
- **Operator (AgentFab):** "start ABC-123 on hetzner" and "tidy up done
  tickets", both through the confirm dialog.
- **Webhook nudges** for a hub with a public URL. A webhook only triggers a
  targeted fetch.

**Status (2026-09-25): M9.1 and M9.2 landed** on
`claude/cloud-fleet-work-graph-m9`, per
`plans/2026-09-24-work-graph-m9-beyond.md` (its *Revisions* list every
deviation). Verified with `cargo fmt`, `clippy -D warnings` (workspace),
`cargo test` (fleet-core, claude-fleet, fleet-hub; only the four chmod tests
that fail as root on `main` fail) and `pnpm check` / `pnpm test`; the manual
acceptance is still to do.

- **M9.1 Today view**: `work { action: today, since }` (the digest, scoped
  by `OrgScope`), the Routed `work_today`, the Today view as Details' empty
  state and on ⌘⇧T, the scope-aware plain-text **Copy standup**.
- **M9.2 ticket context card**: `work { action: card, key }` (cache only,
  acceptance criteria parsed in Rust, `composer_text` fenced by
  `fence_untrusted`), the Routed `work_ticket_card`, the card in Details and
  **Insert into composer** (inserts, never sends).
- **M9.7 operator confirmation** (D12): the operator's session starts
  (`new_session`, `new_shell_session`, `work_link` start / resume) and kills
  always need a person's approval, whatever `mcp.confirm_destructive` says;
  a hub (no approver) refuses them. "Tidy up done tickets" waits for M7.
- **M9.3 agent-written handover** (D9, on demand): `work_link { action:
  handover }`, marker-delimited reply read by the Stop hook, kept as an
  agent `note`, shown first (fenced) in briefs; the card's *Ask for a
  handover*.
- **M9.6 multi-repo start** (D11): `work_link start { project_ids }`, one
  sibling per repo on one branch, per-repo duplicate guard; *Also start in*
  in the New-session dialog.
- **Not built, by decision:** write-back (D3 none), dead-session summaries
  (D10 off), webhook nudges (D13 no).
- **Follow-ups on M7** (after #268): Today's **Stale** section opens M7's
  Tidy-up sheet with its stale candidates picked; the agent panel's
  **Tidy up done tickets** command fills the operator's composer, and the
  operator's instructions say how to tidy through `work { tidy }` /
  `work_link { tidy_apply }` without ever turning a safe kill into a kill.

### M10: settle, prove, and reach the phone

Plan: `plans/2026-09-25-work-graph-m10-settle.md`.

- **Review leftovers:** the M9.3 / M9.6 should-fix items, per-caller
  isolation rows.
- **End to end:** `hub-e2e.sh` covers the work graph against a loopback fake
  tracker.
- **Acceptance:** one written manual acceptance run, for the user to execute.
- **Recorded gaps closed or decided:** Today → Tidy-up, filter chrome, the
  remote SessionStart numbers (D5), M4.6 (D14).
- **Phone:** Today and the ticket card, read-only (D15).
- **Replay ring:** pressure measured before anything is changed.

**Value:** the work graph is proven end to end and has no silent "not done".

**Status (2026-09-25):**
- **M10.4** (branch `claude/cloud-fleet-work-graph-m10`): Today's Stale
  opens the Tidy-up sheet narrowed to those sessions (Show all widens it);
  the work filters have chips; the remote SessionStart measurement has a
  script and a placeholder table for the user (D5 open). M4.6 was built
  on its own branch and merged (#273, OFF), which settles D14. Frontend and docs only: no new tool, action, command,
  migration or contract bump.
- M10.0 (the plan) is committed; M10.1–M10.3, M10.5 and M10.6 are not
  started.

### M12: ship and operate

Plan: `plans/2026-09-25-work-graph-m12-ship-and-operate.md`. (M11, the long
tail, has its own plan: `plans/2026-09-25-work-graph-m11-long-tail.md`.)

- **M12.1** upgrade path proven on a generated pre-work-graph database.
- **M12.2** scale budgets and the indexes they needed (migration 058).
- **M12.3** retention: the `work.retention.*` windows and `work_admin
  { status | sweep_now }`.
- **M12.4** trackers in `fleet_health` and the "Reconnect …" Attention item.
- **M12.5** the user guide, `docs/work-graph.md`.
- **M12.6** the decided-against list revisited (docs only).

**Value:** the work graph can be shipped, upgraded into, run for months and
explained.

**Status (2026-09-26):**
- M12.1, M12.2 and M12.3 are landed (see the plan's *Revisions*).
- M12.4 is landed (#303).
- **M12.5 done** on `claude/cloud-fleet-work-graph-m12-guide`:
  `docs/work-graph.md` covers the whole feature and lists every `work.*`
  setting with its default. `service::settings`'s
  `work_settings_are_in_the_user_guide` fails when a registered `work.*`
  setting is missing from that table, carries another default, or the
  table names one that is not registered. Getting started, troubleshooting,
  concepts and both READMEs link to it. Its fleet-health section was
  rewritten from M12.4's code once #303 merged.
- **M12.6 written; decisions wait on the user.**
  `reviews/2026-09-26-work-graph-decisions-revisited.md` revisits D3, D10,
  D13, D15 / D20 and D17 with the repository's evidence (M10.3 has not run,
  so no usage is claimed) and recommends. D16 is corrected below. The
  decisions table is otherwise unchanged: the user updates it, and each
  "yes" becomes its own milestone (M13+).

### M13: the decided-against list, built

Plan: `plans/2026-09-26-work-graph-m13-decided-yes.md`. The user said yes to
D3, D10, D13 and D20 on 2026-09-26; each is built in the smallest safe form
the M12.6 review describes.

- **M13.1** summaries of dead sessions, on demand (D10).
- **M13.2** write-back: a PR remote link on Jira, opt-in per tracker, through
  an outbox (D3, migration 061).
- **M13.3** webhook nudges on a public hub: HMAC-verified, the payload never
  trusted, polling unchanged (D13).
- **M13.4** naming work on the phone, in fleet-mobile (D20).

Status: planned.

## Critical path and parallelism

```
M0 ─┬─> M1 ─> M2 ─┬─> M4 ─> M7
    │             └─> M3 ─┬─> M5
    │                     ├─> M6
    │                     └─> M8 (after M3's start/resume)
    └─ M0 PRs are independent of each other
```

- M3 can start its transport spike and fixtures in parallel with M2.
- M5's security boundary must land **before** a second org's tracker is
  connected on a shared hub.

## Decisions still open (the user's)

| # | Question | Options | Default if no answer |
|---|---|---|---|
| D1 | Provider order after Jira | GitHub → Asana → Linear, or Asana first (Company B is real work) | GitHub first (no credentials, a quick check of the abstraction), then Asana immediately |
| D2 | Can "done" ever kill a live session automatically? | never · opt-in per org via safe kill | Never by default; opt-in per org |
| D3 | Write-back to trackers | none · transition on start · plus a PR remote link · plus worklog | **Decided 2026-09-26: yes, PR remote link only** (Jira Cloud / DC, opt-in per tracker): M13.2. Transition and worklog stay out (D26) |
| D4 | Must org isolation for host tokens exist before the second tracker? | yes · later | Yes, if both companies' hosts share one hub |
| D5 | Can a synchronous SessionStart hook cost up to about 2 s at start-up when the hub is down? | yes · no (keep the brief via UserPromptSubmit only) | Measure in M4, then decide. Local numbers in the M4 plan; the remote ones are the user's to take (`scripts/measure-session-start.sh`, M10.4). Off until the remote numbers are under ~300 ms p95 |
| D6 | Jira Data Center needed? | yes (which companies) · no | No; Cloud only |
| D9 | May fleet spend a turn of a session's model to write its handover (M9.3)? | on demand · also at safe kill · never | **Decided 2026-09-25: on demand only** (a button; never at safe kill) |
| D10 | Summarise dead sessions with `claude -p --fork-session` (M9.4)? Which model? | off · on (small model) | **Decided 2026-09-26: yes, on demand only** (a *Summarise* button; `work.summary_model`, default `haiku`, D24): M13.1 |
| D11 | Multi-repo start (M9.6): one branch name in every repo; which projects are offered? | same `{key}-{slug}` · per repo | **Decided 2026-09-25: the same name; projects the key ran in before** |
| D12 | Must operator-initiated starts / kills always confirm, even with `mcp.confirm_destructive` off (M9.7)? | yes · follow the setting | **Decided 2026-09-25: yes, always** |
| D13 | Expose an inbound webhook endpoint on a public hub (M9.8)? | no (poll) · yes (HMAC, targeted fetch only) | **Decided 2026-09-26: yes, a nudge only** (HMAC per tracker; GitHub, Jira Cloud, Linear, D25; polling stays): M13.3 |
| D14 | Build M4.6, the opt-in classification nudge? | build (off by default) · decided against | **Built, off by default (2026-09-25, #273)**: `work.classify_nudge` |
| D15 | Multi-start on the phone? (Restated 2026-09-26: handover is already on the phone, M8.6.3, for full tokens; Today and the card are read-only, M10.5) | desktop only · also on the phone | Desktop only; decide after M10.3 |
| D16 | Run `hub-e2e` in GitHub CI, not only locally (M10.2)? | local opt-in · CI on `main` pushes | **Done: hub-e2e already runs in CI** (`hub-headless` job, `.github/workflows/ci.yml`, every PR and `main` push). The M10.2 work-graph leg needs an `e2e`-feature hub and is skipped there; `ci-local.sh --hub-e2e` runs it |
| D20 | Name or rename local work on the phone? (M11 plan) | no · yes (full token) | **Decided 2026-09-26: yes**, fleet-mobile only, no hub change: M13.4 |

## Risks to watch

- **The replay ring (512 frames) is shared by every event kind.** A noisy sync
  pushes session frames out, and phones then re-list about 61 KB. Emit only
  real changes, and coalesce.
- **The tool-description budget.** Keep the work tools grouped, and never add a
  tool per action.
- **Third-party text reaching agents.** Always `mark_untrusted`, cap it, and
  preview it.
- **The first outbound HTTP client in fleet-core:** watch TLS roots, licences
  and redirects.
- **Wire enums.** Every new enum needs an `Unknown` (`#[serde(other)]`)
  variant, so later variants never force a contract bump.
- **The reconcile `ON CONFLICT` list.** It must never gain a work column.

## Revisions

- 2026-09-24: first version, from design revision 2 and the round-1 review.
- 2026-09-24: M0 written, not yet compiled (see M0 status). Migration 045 is
  now the participant trigger, so the work graph schema is 046.
- 2026-09-24: M1b.1 verified by the compiler; M1b.2 landed (SessionRow.work
  and work_rejected, `work` / `work_link`, four Routed commands, the row's
  work menu). The tool budget was raised by 955 B for the two tools.
- 2026-09-24: M2 landed (journal, carry rules, handover, resume over MCP /
  Tauri, the sidebar's past work and Resume). The tool budget was raised by
  734 B for eight parameters on `work` / `work_link`; no new tool, no
  contract bump. Deviations are in the M2 plan's *Revisions*.
- 2026-09-24: M3 landed on `claude/cloud-fleet-work-graph-m3` (trackers,
  Jira Cloud read-only, sync, tickets / lookup / start, the UI). The tool
  budget grew by 1,792 B in three steps for `work_admin` (the one new tool)
  and eleven parameters on `work` / `work_link`; no contract bump. Deviations
  (relative JQL windows instead of timezone formatting, locally evaluated
  views, `tracker_views.enabled`, and more) are in the M3 plan's
  *Revisions*.
- 2026-09-24: M4 landed on `claude/cloud-fleet-work-graph-m4` (detection,
  the resolver, explanations and correction; SessionStart context built but
  off). The tool budget grew by 172 B for `work_link`'s confirm / reject by
  id / trust_project; no new tool, no contract bump. M4.6 is not done.
  Deviations are in the M4 plan's *Revisions*.
- 2026-09-24: M7 landed on `claude/cloud-fleet-work-graph-m7` (stacked on
  M4; built in parallel with M5 and M6): tidy-up suggestions, UI-only
  archive, snooze / never, reopened work, auto-tidy off by default. The tool
  budget grew by 639 B (three parameters on `work_link`, two read actions on
  `work`); no new tool, no contract bump. The per-org auto-tidy override
  waits for M5. Deviations are in the M7 plan's *Revisions*.
- 2026-09-24: M5 landed on `claude/cloud-fleet-work-graph-m5` (orgs, the
  org boundary for per-host tokens, D7 `isolate_sessions` per org, default
  off, the scope selector and Organisations settings, one `rowMatches`).
  Migration 050. The tool budget grew by 714 B (627 for `work_admin`'s org
  actions, 87 for `force_cross_org`); no new tool, no contract bump. M3's
  per-host ticket fence was kept and composed with the org fence rather
  than removed. Deviations are in the M5 plan's *Revisions*.
- 2026-09-24: M5 merged into M7 (no rebase). M7's migration became 051, plus 052;
  `orgs.auto_tidy` (on / off / inherit) overrides `work.auto_tidy` per org,
  and tidy candidates, tidy_apply and reopened work respect the org scope
  of a per-host token. Details in the M7 plan's *Revisions*.
- 2026-09-24: M6 landed on `claude/cloud-fleet-work-graph-m6` (GitHub
  through `gh`, Asana, `via_host`, Linear, Jira Data Center — built because
  the M6 brief asked, overriding D6's default — and the Connect / badge /
  section-map UI), with M5 merged in: M6's migration is 051. The tool budget
  grew by 189 B over M5 for `work_admin`'s `transport` / `settings`; no new
  tool, no contract bump. Deviations are in the M6 plan's *Revisions*.
- 2026-09-24: M8.0 (hub side) landed on `claude/cloud-fleet-work-graph-m8`:
  `work_suggested` on the phone view, the `work` / `work_link` action enums
  (+72 B on top of M5; generated from M5's action tables), the client
  tool-list gate test. No new tool, no contract bump.
- 2026-09-25: M6 (main, #266) merged into M7 (no rebase). M6 kept 051; M7's
  migrations were renumbered to 052 (`work_lifecycle`) and 053
  (`org_auto_tidy`), and a database that already ran M6's 051 gets both.
  `work_admin` carries M6's `transport` / `settings` and M7's `auto_tidy`.
- 2026-09-25: M9 planned (`plans/2026-09-24-work-graph-m9-beyond.md`) and
  M9.1 (Today view, Copy standup) and M9.2 (ticket context card) landed on
  `claude/cloud-fleet-work-graph-m9`: two `work` actions (`today`, `card`)
  and one parameter (`since`), two Routed commands (163). The tool budget
  grew by 113 B (94 raised the constant to 69,365; `card`'s 19 fit the
  headroom); no new tool, no contract bump. The other six M9 items wait on
  the new decisions D9–D13 and on D3.
- 2026-09-25: decisions D3 (none) and D9–D13 (defaults) recorded; `main`
  (M6) merged into the M9 branch; M9.7, M9.3 and M9.6 landed. Two
  `work_link` things (the `handover` action, the `project_ids` parameter)
  and `confirm_nonce` on four tools; two more Routed commands (165). The
  tool budget is 70,098 (measured 69,998). No new tool, no migration, no
  contract bump.
- 2026-09-25: M7 (main, #267) merged into the M9 branch. `work_link` is
  `confirm: true` since M7 (its tidy kills); M9.7's start / resume gate
  therefore runs only for the operator, so a person's start is never gated.
  Tool budget 70,865 (measured 70,765).
- 2026-09-25: the two M9 follow-ups on M7 (Today → Tidy up, the operator's
  "Tidy up done tickets"); frontend and the operator's CLAUDE.md only — no
  new action, no budget change.
- 2026-09-25: M10 planned (`plans/2026-09-25-work-graph-m10-settle.md`):
  review leftovers, a work-graph e2e, the written acceptance, the recorded
  gaps, the phone's M9 moments, replay-ring measurement. New decisions
  D14–D16; D5 re-asked with remote numbers.
- 2026-09-25: M10.4 on `claude/cloud-fleet-work-graph-m10`: Today → Tidy-up
  narrowed to the stale sessions, the work-filter chips, the remote
  SessionStart measurement procedure (`scripts/measure-session-start.sh`;
  numbers to be taken by the user, D5 open). D14–D16 added to the decisions table with their defaults. No new tool,
  action, command, migration or contract bump; the tool budget is untouched.
- 2026-09-25: `main` merged into M10.4. M4.6 had landed on `main` (#273,
  OFF behind `work.classify_nudge`), so M10.4's "decided against" is
  withdrawn: D14 reads *built, off by default*.
- 2026-09-26: M12 added to the milestones (it was planned without a
  section here). **M12.5 done**: the user guide `docs/work-graph.md`, with a
  docs check (`work_settings_are_in_the_user_guide`) that fails CI when a
  `work.*` setting is missing from its table. Revise the guide with any
  milestone that changes what a user sees.
- 2026-09-26: M12.4 merged (#303); the guide's *Trackers in fleet health*
  section now describes the code as landed, and its verify marker is gone.
- 2026-09-26: M12.6 written, docs only
  (`reviews/2026-09-26-work-graph-decisions-revisited.md`): per
  decided-against item, the source, the evidence in the repository, the
  smallest safe version, the risks and a recommendation (D3, D13, D17 stay
  decided against; D10, D15 / D20 decide after M10.3). D16 is corrected to
  done: hub-e2e already runs in CI's `hub-headless` job. The note also
  records that D15's row understates the phone (M8.6.3 built *Ask for a
  handover*); the row is left for the user to restate.
- 2026-09-26: D15 restated by the user to cover multi-start only. *Ask for
  a handover* is already on the phone (M8.6.3, full token), and Today and
  the ticket card stay read-only (M10.5). Only multi-start is still open;
  the default stays desktop only until M10.3.
- 2026-09-26: the user said yes to D3, D10, D13 and D20. Their rows record
  the decision, and M13 is added with its plan
  (`plans/2026-09-26-work-graph-m13-decided-yes.md`). D15 (multi-start on
  the phone) and D17 (`acli`) are unchanged.
