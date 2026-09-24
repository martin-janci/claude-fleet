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
- **Not done:** M4.6 (the classification nudge), suggestions on the phone
  (M8), the remote-host SessionStart measurement.

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
contract bump. **M8.1** (fleet-mobile's model, `tools/list` discovery,
work transport and the `work` event kind) landed on fleet-mobile's
`claude/cloud-fleet-work-graph-m8`; M8.2–M8.5 (the screens and docs) are not
started. Note the plan's Revisions item 4 (an absent `work` on a row means
none).

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
| D3 | Write-back to trackers | none · transition on start · plus a PR remote link · plus worklog | None until M9 |
| D4 | Must org isolation for host tokens exist before the second tracker? | yes · later | Yes, if both companies' hosts share one hub |
| D5 | Can a synchronous SessionStart hook cost up to about 2 s at start-up when the hub is down? | yes · no (keep the brief via UserPromptSubmit only) | Measure in M4, then decide |
| D6 | Jira Data Center needed? | yes (which companies) · no | No; Cloud only |

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
- 2026-09-24: M8.0 (hub side) landed on `claude/cloud-fleet-work-graph-m8`:
  `work_suggested` on the phone view, the `work` / `work_link` action enums
  (+66 B), the client tool-list gate test. No new tool, no contract bump.
