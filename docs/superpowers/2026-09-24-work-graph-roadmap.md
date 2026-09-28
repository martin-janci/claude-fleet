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

**Status (2026-09-26, M13.0):** M0.1–M0.5 reached `main` with #260 and were
compiled and tested there. **M0.6 done (M11.5, #286):** the master tool
surface went from 71,590 B to 54,646 B. The `whoami` question is unchanged:
`TOOL_POLICIES` still says `readonly: false`, and no decision records it.

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
  - Left over from M1b.2, all since done: "Name this work…" on a group
    header and a row (done, M11.1, #288); the New session dialog links
    `source: started` through `start_work` (done, M3.4 / M3.5, #262); the
    phone reads `work` (done, M8.1–M8.5, fleet-mobile #32).

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
- **Follow-ups:** resume on the phone (done, M8.5, fleet-mobile #32) and
  past work on the phone (done, M8.6, fleet-mobile #35); the resume plan
  probes that the transcript is still on the host (done, M11.2, #290); the
  acceptance run on a real fleet waits on the user (M10.3,
  `docs/work-graph-acceptance.md`).

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
- **Follow-ups:** reached `main` with #262. The acceptance on a real Jira
  Cloud site waits on the user (M10.3).

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
  on a real change; `SessionRow.work_suggested`. (Label hygiene, D34, later
  added R9u: a person's *Clear work* holds against the unchanged branch /
  PR that named the target — migration 070 `work_unlinks`, applied when
  `detect` builds the resolver's input; see the design's rule table.)
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
  See the M4 plan's Revisions. It was opened as #273, which was closed
  unmerged; the same commit (c688bc7) reached `main` with #270.
- **Remote-host SessionStart measurement:** the procedure is written
  (`scripts/measure-session-start.sh`, M10.4) and the M4 plan has a
  placeholder table; **waits on the user (D5)**. M4.1–M4.5 reached `main`
  with #263. (Suggestions on the phone landed with M8.)

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
- **Follow-ups:** reached `main` with #264. The Today view respects the
  scope (done, M9.1, #268: `work { today }` is filtered by `OrgScope`); the
  phone shows org labels and an org filter (done, M8.6, fleet-mobile #35,
  on `org_id` from #275); the manual acceptance on two orgs waits on the
  user (M10.3).

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
- **Follow-ups:** reached `main` with #266. `acli`: decided against (D17);
  GitHub Enterprise Server and per-tracker sync metrics: done (M11.4, #292);
  the phone: done (M8, fleet-mobile #32 — it shows every provider's keys and
  status through the hub); the manual acceptance on real accounts waits on
  the user (M10.3).

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
- **Follow-ups:** reached `main` with #267. `idle_unlinked`: done (M11.3,
  #291; auto-tidy never acts on it, D19). The phone reads `archived_at` and
  can hide archived sessions (M8.6, fleet-mobile #35); Tidy-up and
  *Reopened* stay desktop-only: no plan or decision puts them on the phone
  (the phone is a pager, M8 plan). The manual acceptance waits on the user
  (M10.3).

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

**Status (2026-09-26, M13.0): M8 done.** The hub side reached `main` with
#265 (M8.0) and #275 (`org_id`). M8.6 reached fleet-mobile `main` with
martin-janci/fleet-mobile#35 (merge `6ed86ed`, first in fleet-mobile
v0.2.41): *Ask for a handover* (M8.6.3) is in `WorkSheet.kt`, gated in
`SessionWorkViewModel.kt` (`canHandover`: a primary key, the session can be
asked, the hub lists the action) and sent through `WorkActions.handover`.
M10.5 (Today, *Share standup*, the ticket card) followed with
fleet-mobile #36. Multi-start (D15) and naming local work (D20) stay
desktop-only.

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
- **Not built in M9, by decision then:** write-back (D3), dead-session
  summaries (D10) and webhook nudges (D13). The owner later said yes to D3
  (the PR remote link only) and D10 (on demand only); both were built in
  M13.4e / M13.4c. D13 stays no (M13.4f was built and removed, #330).
- **On `main`:** M9.1 / M9.2 and M9.3 / M9.6 / M9.7 with #268, the M7
  follow-ups with #271, the handover fix (idle REPL only) with #274.
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

**Status (2026-09-26, M13.0): M10.1–M10.6 landed.**
- **M10.1** review leftovers (multi-start, handover bound to its turn, the
  per-caller isolation rows): #279.
- **M10.2** the work graph end to end in `scripts/hub-e2e.sh` against a
  loopback fake tracker: #285, plus `get_settings` / `set_setting` for its
  settings scenarios (#287). Since #314 the leg runs in CI on every PR
  (D16).
- **M10.3** the written acceptance: #313 (below).
- **M10.4** as above: #278.
- **M10.5** Today, *Share standup* and the ticket card on the phone:
  martin-janci/fleet-mobile#36; the card on the session's work sheet with
  *Copy* (never *Send*), and the org colour bar on rows:
  martin-janci/fleet-mobile#48.
- **M10.6** replay-ring pressure measured
  (`reviews/2026-09-25-replay-ring-pressure.md`), `session:updated` for a
  tracker item only when the row shows the change: #280. The first-sync
  flood is accepted (D18).
- **M10.3: script written (#313); the run waits on the user.**
  `docs/work-graph-acceptance.md` (2026-09-26) is the one manual
  acceptance run, 59 steps, with the evidence M12.6 asked for and the D5
  measurement. The filled-in copy is committed as the record and updates
  the decisions table.

### M11: the long tail

Plan: `plans/2026-09-25-work-graph-m11-long-tail.md`.

- **M11.1** "Name this work…": local work items named and renamed from a
  row's `#` menu or a group header.
- **M11.2** resume probes that the transcript is still on the host.
- **M11.3** tidy reason `idle_unlinked`, with a per-session *keep*.
- **M11.4** GitHub Enterprise Server through `gh --hostname`, and
  per-tracker sync metrics.
- **M11.5** pay back the tool-description budget (M0.6).
- **M11.6** every "Not done" line done or decided (D17–D20).

**Value:** every "not done" of M0–M10 is either done or decided; local work
has a name; resume says before it runs that a transcript is gone.

**Status (2026-09-26, M13.0): landed.**
- **M11.1** (#288): `work_link { action: name }` (new item, or rename by
  `item_id`), `work { action: local_items }`; Routed
  `list_local_work_items` / `name_session_work` / `rename_work_item`; no
  migration.
- **M11.2** (#290): `resume_plan` / `resume` (mode `last`) probe the
  recorded transcript path, then `~/.claude/projects/*/<uuid>.jsonl`;
  absent falls back to fresh with the brief, unknown adds
  `ResumePlan.warnings`.
- **M11.3** (#291): `idle_unlinked` in `service/gc/tidy.rs`, behind
  `work.tidy_idle_unlinked_days` (7); *keep* is a `tidy_kept` timeline
  event; never preselected; never auto-tidied (D19, enforced in the
  planner, `auto_selection` and the executor); no migration.
- **M11.4** (#292): `trackers.settings.hostname` for GHES (admin-fenced),
  keys `host/owner/repo#n`; in-memory `SyncMetrics` in
  `work_admin { status }`, Settings → Work and `fleet-hub tracker status`;
  LocalOnly `tracker_sync_metrics`.
- **M11.5** (#286): 71,590 B → 54,646 B (16,944 B paid back); no tool,
  action or parameter renamed.
- **M11.6**: never ran as its own pass; M13.0 did it. D17 decided against,
  D18 accept, D19 never, D20 waits on the user (after M10.3).

### M12: ship and operate

Plan: `plans/2026-09-25-work-graph-m12-ship-and-operate.md`.

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
- **On `main` (verified 2026-09-26, M13.0):** M12.1 #297, M12.2 #298
  (migration 058), M12.3 #299 and the index-backed `tidy_kept` lookup #300,
  M12.4 #303, M12.5 #304, M12.6 #307 (D15 restated #309; the `acli` promise
  removed from `net/https.rs` #310). **M12 is done.**

### M13: live use

Plan: `plans/2026-09-26-work-graph-m13-live-use.md`.

- **M13.0** this roadmap and CLAUDE.md say what is true on `main` (docs).
- **M13.1** a tracker that skips items reads `degraded`, then `failing`
  ("Sync skipping items"), from #318's per-item failures (D25).
- **M13.2** `work_admin { usage }`: master-only counts over existing rows,
  nothing leaves the machine (D24).
- **M13.3** triage of the owner's M10.3 run (waits on the run).
- **M13.4** the decision-gated builds, each only on a "yes": D20, D5, D10,
  D15, D3, and D13 (M13.4f). The user said yes to D3, D10 and D20 on
  2026-09-27 (D13 was built, then removed: it stays no); build notes in `plans/2026-09-26-work-graph-m13-decided-yes.md`.
- **M13.5** close-out: the work graph is *operating* (D26).

**Value:** the roadmap matches `main`, partial sync failures are visible,
and every open decision gets an answer backed by usage.

**Status (2026-09-27): M13 is closed; the work graph is operating (D26).**
- **M13.0** the truth pass: #319.
- **M13.1** partial sync failures visible ("Sync skipping items", D25):
  #320 / #324.
- **M13.2** `work_admin { usage }` (D24): #323 / #324.
- **M13.4** on the owner's yes (2026-09-27):
  - **M13.4a** (D20): naming and renaming local work on the phone,
    martin-janci/fleet-mobile#51;
  - **M13.4c** (D10): summaries of dead sessions, on demand, #327, with
    the no-hooks fix #331;
  - **M13.4d** (D15): multi-start on the phone, martin-janci/fleet-mobile#50;
  - **M13.4e** (D3): the PR remote link to Jira, #327, with #332 and #334;
  - **M13.4f** (D13): built with #327 and removed by #330 (D13 stays no;
    migration 064 drops its table).
- **M13.5** close-out: this revision, and the next release's
  `[Unreleased]` notes in `CHANGELOG.md`.

### Operating (after M13)

The work graph has no more milestones (D26), except M14, the Work view,
by the owner's choice (D36). New work arrives as an issue,
or as a small plan in `plans/` against `docs/work-graph.md`, and every change
a user sees updates the guide in the same PR. Two items carry over from M13
as open work, not as a milestone:

- **The owner's acceptance run** (`docs/work-graph-acceptance.md`, M10.3).
  Its triage is what M13.3 described: a blocker gets a PR with a regression
  test, a major is fixed or ticketed, minor and docs findings go into one
  PR, and the decisions table takes the run's evidence.
- **D5**, the SessionStart context: it stays off until the remote numbers
  from `scripts/measure-session-start.sh` are in; a yes is then M13.4b's
  one-line default flip plus the guide's settings table.

### M14: the Work view

Plan: `plans/2026-09-27-work-graph-m14-work-view.md`. Design (binding):
`specs/2026-09-27-work-view-design.md`.

The other direction through the same data: org → project / group → task →
*every* session of that task (primary, secondary, suggested and past), and
tasks with no session at all; one contract (`work { tree | task | … }`,
`work_link { set_primary | … }`, no new tool, `CONTRACT_REVISION` 4) for
the desktop and the phone, and a phone paired **to one org** that sees only
that org (a new boundary, proved like M5's). M14 lands the backend already
built on `claude/fleet-dynamic-work-view-kwc3r9`, cut into reviewable PRs
and rebased on `main`; it does not redesign it.

- **M14.0** the plan, the spec on `main`, the M13 truth pass (docs).
- **M14.1a–d** the backend: the `start` race fix; migration `0NN_work_view`
  (numbered at merge time) + reads + org-bound clients (D31); mutations with
  compare-and-set; the desktop commands and `work:changed`.
- **M14.2 / M14.3** the desktop Work view, read, then edits and Review.
- **M14.4** the phone's *My work* tab (fleet-mobile), read, then edits.
- **M14.5** acceptance (Part R), the user guide, close-out.

**Status (2026-09-27): M14.0–M14.4 built.** M14.1a–d are on `main`
(#341, #342, #345, #347); M14.2 / M14.3 (the desktop Work view, its edits
and Review) and M14.4 (fleet-mobile's *My work*) came from
`claude/fleet-dynamic-work-view-kwc3r9`, merged over that backend, with
`scripts/hub-e2e.sh` hub W section 10 running the contract on a real hub
(tree, session tasks, the primary's compare-and-set, placement,
`work:changed`, Acme- and Beta-bound and readonly clients) (#349,
fleet-mobile#54). D31's per-org switch is in Settings → Work →
Organisations and `fleet-hub org set --bound-sees-unassigned`. Open:
M14.5, the acceptance run (Part R), waits on the owner. D31–D36 are
answered (the table below). M14 is the one milestone after M13's close-out, by the
owner's choice (D36); D26's "operating" applies again once it is done. The
two open items carried from M13 (the acceptance run, D5) stay where
*Operating* puts them; M14 neither absorbs nor blocks them.

## Critical path and parallelism

```
M0 ─┬─> M1 ─> M2 ─┬─> M4 ─> M7
    │             └─> M3 ─┬─> M5
    │                     ├─> M6
    │                     └─> M8 (after M3's start/resume)
    └─ M0 PRs are independent of each other

M4, M5, M6, M7, M8 ─> M9 ─> M10 ─┬─> M11 ─┐
                                 └─> M12 ─┴─> M13 (live use) ─> M14 (the Work view)
                                        M10.3 (the owner's run) ─> M13.3 ─> M13.4
```

- M0–M12 are on `main`. M11 and M12 ran in parallel; M12.4 reads M11.4's
  sync metrics.
- In M13, M13.0–M13.2 do not wait on anything; M13.3 waits on the owner's
  M10.3 run, and each M13.4 item on its decision's "yes".
- M14 follows M13's close-out; the two items M13 left open (the acceptance
  run, D5) do not block it. Its order is in the M14 plan (M14.1a first).

- M3 can start its transport spike and fixtures in parallel with M2.
- M5's security boundary must land **before** a second org's tracker is
  connected on a shared hub.

## Decisions still open (the user's)

The last column is the current answer (verified against `main` on
2026-09-26, M13.0). *Waits on the user* means the default holds until the
user writes an answer here; each "yes" becomes an M13.4 item.

| # | Question | Options | Default if no answer / current answer |
|---|---|---|---|
| D1 | Provider order after Jira | GitHub → Asana → Linear, or Asana first (Company B is real work) | GitHub first (no credentials, a quick check of the abstraction), then Asana immediately. **Done** in that order (M6, #266) |
| D2 | Can "done" ever kill a live session automatically? | never · opt-in per org via safe kill | Never by default; opt-in per org. **Built** (M7, #267): `work.auto_tidy` off, `orgs.auto_tidy` per org, safe kill only |
| D3 | Write-back to trackers | none · transition on start · plus a PR remote link · plus worklog | **Decided 2026-09-26: yes, the PR remote link only** (Jira Cloud / DC, opt-in per tracker, through an outbox): M13.4e. Transition on start and worklog stay out (D29) |
| D4 | Must org isolation for host tokens exist before the second tracker? | yes · later | Yes, if both companies' hosts share one hub. **Done** (M5.3, #264) |
| D5 | Can a synchronous SessionStart hook cost up to about 2 s at start-up when the hub is down? | yes · no (keep the brief via UserPromptSubmit only) | Measure in M4, then decide. Local numbers in the M4 plan; the remote ones are the user's to take (`scripts/measure-session-start.sh`, M10.4). Off until the remote numbers are under ~300 ms p95. **2026-09-27: not yet** — the owner keeps it off until the remote numbers are in (M13.4b not started) |
| D6 | Jira Data Center needed? | yes (which companies) · no | No; Cloud only. **Built anyway** (M6.5, #266): the M6 brief asked for it |
| D7 | Isolate sessions (list, message, dispatch) across orgs? (M5 plan) | off per org · on per org | Off per org; the user turns it on per org. **Built** (M5.3, #264): `orgs.isolate_sessions`, off |
| D8 | — | — | **Unused**: no decision was ever numbered D8 in the work graph |
| D9 | May fleet spend a turn of a session's model to write its handover (M9.3)? | on demand · also at safe kill · never | **Decided 2026-09-25: on demand only** (a button; never at safe kill). Built (M9.3, #268) |
| D10 | Summarise dead sessions with `claude -p --fork-session` (M9.4)? Which model? | off · on (small model) | **Decided 2026-09-26: yes, on demand only** (*Summarise* on a past-work row; `work.summary_model`, default `haiku`, D27; never automatic, D30): M13.4c, **built** |
| D11 | Multi-repo start (M9.6): one branch name in every repo; which projects are offered? | same `{key}-{slug}` · per repo | **Decided 2026-09-25: the same name; projects the key ran in before.** Built (M9.6, #268) |
| D12 | Must operator-initiated starts / kills always confirm, even with `mcp.confirm_destructive` off (M9.7)? | yes · follow the setting | **Decided 2026-09-25: yes, always.** Built (M9.7, #268) |
| D13 | Expose an inbound webhook endpoint on a public hub (M9.8)? | no (poll) · yes (HMAC, targeted fetch only) | **No.** Recorded yes on 2026-09-26 and built as M13.4f (reached `main` with #327); **the owner reversed it on 2026-09-27 → no** (the M12.6 recommendation), and M13.4f was removed. Migration 062 stays in the chain, 064 drops its table (063 is another change, #333) |
| D14 | Build M4.6, the opt-in classification nudge? | build (off by default) · decided against | **Built, off by default (2026-09-25)**: `work.classify_nudge`. Opened as #273 (closed unmerged); the commit reached `main` with #270 |
| D15 | Multi-start on the phone? (Restated 2026-09-26: handover is already on the phone, M8.6.3, for full tokens — on fleet-mobile `main` since martin-janci/fleet-mobile#35, first in v0.2.41; Today and the card are read-only, M10.5, fleet-mobile #36) | desktop only · also on the phone | **Decided 2026-09-27 by the owner, before the M10.3 run: also on the phone** (M13.4d: multi-select with a confirm sheet, cross-org refused in words, full token only), **built** (martin-janci/fleet-mobile#50) |
| D16 | Run `hub-e2e` in GitHub CI, not only locally (M10.2)? | local opt-in · CI on `main` pushes | **Done.** hub-e2e runs in CI's `hub-headless` job (`.github/workflows/ci.yml`, every PR and `main` push). Since #314 the job also builds an `e2e`-feature hub into `target/e2e` and passes it as `WBIN`, so the M10.2 work-graph leg runs on every PR; with `CI=true` a missing `WBIN` fails instead of skipping (`scripts/hub-e2e.sh`) |
| D17 | Support `acli` (Atlassian CLI) as a Jira transport? (M11 plan) | yes · decided against | **Decided against:** REST and `via_host` (`gh`, `curl`) cover it. M12.6 keeps it; M13 does not reopen it |
| D18 | First sync of a new tracker: suppress per-item frames? (M11 plan) | accept the flood · suppress (design change) | **Accept** (the M10.6 report, `reviews/2026-09-25-replay-ring-pressure.md`); nothing suppresses them on `main` |
| D19 | May auto-tidy ever act on `idle_unlinked`? (M11 plan) | never · per org | **Never.** Enforced (M11.3, #291) in `TidyReason::auto_allowed`, `auto_selection` and the executor |
| D20 | Local work items on the phone (name / rename)? (M11 plan) | no (read-only phone) · yes | **Decided 2026-09-26: yes**, fleet-mobile only, no hub change: M13.4a, **built** (martin-janci/fleet-mobile#51) |
| D21 | Retention defaults: journal / done items / work timeline events (M12 plan) | 365 / 180 / 180 days · keep forever | **Built** (M12.3, #299): `work.retention.journal_days` 365, `…tracker_items_days` 180, `…timeline_work_events_days` 180; `0` = forever |
| D22 | Should a failing tracker raise an Attention item, not only a health row? (M12 plan) | yes · health only | **Yes, built** (M12.4, #303): one "Reconnect …" item per tracker, deduplicated |
| D23 | Per-org retention override (needs a migration)? (M12 plan) | now · later | **Later:** settings only (M12.3, #299); no migration |
| D24 | Build the usage summary, `work_admin { usage }`? (M13 plan) | yes (master-only, counts only, local) · no | Yes (default); M13.2, **built** (#323 / #324) |
| D25 | May a partial sync failure alone make a tracker `failing` (after N passes)? (M13 plan) | yes · degraded only | Yes (default), with its own Attention wording ("Sync skipping items", not "Reconnect"); M13.1, **built** (#320) |
| D26 | After M13, is the work graph "operating" (issues, not milestones)? (M13 plan) | yes · keep milestones | **Yes: operating since 2026-09-27** (M13.5). Issues and small plans, no milestones, except M14 (D36) |
| D27 | Which model summarises a dead session, on whose quota? (M13.4c) | a small model · the session's configured model | `haiku` (`work.summary_model`: haiku / sonnet / opus), on the session's own host and account |
| D28 | Which providers get webhook nudges (M13.4f)? | GitHub, Jira Cloud, Linear · also Asana, Jira DC | **Moot**: D13 is no, and M13.4f was removed |
| D29 | Which write-back operations (M13.4e)? | PR remote link · also transition on start · also worklog | The PR remote link only |
| D30 | Summarise automatically at session end (M13.4c)? | off · on | Off: on demand only |
| D31 | May an org-bound client see *unassigned* work and sessions? (M14) | yes (as a host does) · by setting | **Answered 2026-09-27: by setting.** A per-org flag `orgs.bound_sees_unassigned` (M14.1b's migration; `work_admin` org edit; Settings → Work → Organisations), **default on**. Off: the org's bound clients see only rows assigned to their org. An isolation row covers both values |
| D32 | Does a forced cross-org link raise a review item until it is acknowledged? (M14) | yes · no | **Answered 2026-09-27: the default, yes** (`cross_org` review kind, cleared by `ack`) |
| D33 | May a full, unbound phone change a local task's org? (M14) | yes, with the impact preview · no | **Answered 2026-09-27: the default, yes**, with the impact preview. Bound clients and hosts may not |
| D34 | Placement rules only, or also link rules? (M14) | placement only · also link rules | **Answered 2026-09-27: the default, placement only.** Link rules would bypass detection's evidence and R9 |
| D35 | Saved views: shared on the hub, or per device? (M14) | shared on the hub · per device | **Answered 2026-09-27: the default, shared on the hub.** A bound client's views are its org's |
| D36 | M14 as a milestone, despite D26? Who drives it? (M14 plan) | yes · issues only | **Answered 2026-09-27: yes, M14 is the last work-graph milestone** (D26 applies after it); one driver session. The backend branch's session does not continue it |
| D31–D47 (Jev) | The Jev (decision model) evaluation, fleet-wide rather than work graph only | — | Its own numbering in `specs/2026-09-27-jev-language-census-design.md`; it collides with M14's D31–D36 above (both were numbered after D30 on the same day), so a bare `D3x` in code or docs must say which table it means |

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
- 2026-09-26: M10.3 script written (`docs/work-graph-acceptance.md`, docs
  only; linked from `docs/work-graph.md`): 59 steps from the upgrade to the
  phone, the M11 / M12 manual items, the evidence for D3, D10, D13,
  D15 / D20, D17 and tracker health, and the D5 measurement. It waits on the
  user's run; no result is claimed.
- 2026-09-26: **M13.0, the truth pass** (docs only; facts from `main`
  a599ffa and its merge history, and fleet-mobile `main` d3af329). M11 and
  M13 sections added; M11 is landed (#286, #288, #290, #291, #292) and M11.6
  is done by this pass. Every "Not done" line of M1, M2, M5, M6 and M7 (and
  the leftovers of M0, M3, M4) now reads done (with its PR), decided
  against or waits on the user. M0.6 done (M11.5). M8 marked done: M8.6 is
  on fleet-mobile `main` (#35, v0.2.41), which answers the M12.6 question
  about M8.6.3. M10's status corrected: M10.1–M10.6 landed (#279, #285 /
  #287, #313, #278, fleet-mobile #36, #280). M12's PRs recorded. The
  decisions table gained D7, D8 (unused), D17–D26 and a current answer on
  every row; D16 corrected (the work-graph leg runs in CI since #314); D14
  corrected (#273 was closed; M4.6 reached `main` with #270). The critical
  path covers M9–M13.
- 2026-09-27: the user said **yes** to D3, D10, D13 and D20 (in a session
  that planned them as a separate M13 before this plan's M13 reached
  `main`; that plan is folded in here). Their rows record it, and D27–D30
  hold the defaults for their details. M13.4c (D10) is built; M13.4e (D3)
  is next. D13 had no slot in M13.4 ("M13 does not reopen it"), so it gets
  **M13.4f**. D5 and D15 are unchanged.
- 2026-09-27: M10.5 finished in fleet-mobile (martin-janci/fleet-mobile#48):
  the ticket card on the session's work sheet (`work card`, read-only, any
  token), *Copy* on every card, and the org colour bar on session rows
  when two or more orgs show. No hub change, no contract bump.
- 2026-09-27: **D13 back to no.** The owner reversed the yes; M13.4f
  (webhook nudges), which reached `main` with #327, is removed. Migration 062
  stays in the chain so a database that ran it is not refused as newer, and
  064 drops its `tracker_webhooks` table. D28 is moot.
- 2026-09-27: The owner confirmed D3, D10 and D20 **yes** in this
  session, and answered **D15 yes** (multi-start on the phone, M13.4d,
  martin-janci/fleet-mobile#50). D5 stays off until the remote
  SessionStart numbers are in. M13.3 still waits on the run. D13 is set
  back to no with M13.4f's removal (#330).
- 2026-09-27: **M13.5, close-out.** M13's status lists what landed and
  where; the work graph is *operating* (D26), with the owner's acceptance
  run and D5 carried as open work. `CHANGELOG.md` has an `[Unreleased]`
  section naming what M13 changed for users, and `scripts/release.sh` now
  turns that section into the release's own instead of stacking the
  release above it.
- 2026-09-27: **M14.2–M14.4.** The desktop Work view (tree, task detail,
  Review, placement and rules, saved views, the org dialog) and the phone's
  *My work* merged over M14.1's backend from
  `claude/fleet-dynamic-work-view-kwc3r9`; the desktop follows
  `work:changed` through `onWorkChanged` (a `resync` re-reads too).
- 2026-09-27: **M14.0** (docs only; facts from `main` `f10d0b92` and its
  merge history). M14 (the Work view) added, with its plan and the design
  spec brought to `main`; D31–D36 added with the owner's answers. The M13
  status block corrected: M13.1 (#320) and M13.2 (#323 / #324) are on
  `main`, M13.4c / M13.4e with their fixes (#331, #332) and tests (#334),
  M13.4a / M13.4d on fleet-mobile (#51, #50), M13.4f removed (#330). (Merged
  with M13.5's close-out, #337, whose M13 status block is kept: M13 is
  closed, and the acceptance run and D5 are open work.) The M9 "not built" line no longer says D3 none
  and D10 off. D15, D20, D24 and D25 name their PRs.
