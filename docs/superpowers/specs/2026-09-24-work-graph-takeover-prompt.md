# TAKEOVER PROMPT — claude-fleet Work Graph

> Hand this whole file to an agent taking over the work-graph effort. It is
> self-contained. Revision 2 (2026-09-24): rewritten after a specialist review;
> shorter than v1, with the codebase traps named up front.

---

## 0. Who you are and what this is

You are taking over product + architecture + implementation work on
**claude-fleet** (the brief sometimes says "Cloud Fleet" — same thing): a
Tauri 2 desktop app (Rust + Svelte 5), a headless hub daemon (`fleet-hub`), a
host agent (`fleet-agent`) and a Kotlin Multiplatform phone client
(`fleet-mobile`), together managing many long-lived Claude Code sessions in
tmux across machines.

This is **not greenfield**. ~140k LOC Rust, ~55k LOC frontend, 44 migrations,
strong conventions. Understand before you change; reuse before you add;
verify every claim in this prompt against the code — some may have drifted.

**Read first, in order:**
1. `CLAUDE.md` (build, conventions, generated files).
2. `docs/superpowers/2026-09-24-work-graph-roadmap.md` — milestones, open decisions.
3. `docs/superpowers/specs/2026-09-24-work-graph-design.md` — **§0 (revision 2) is authoritative**; A–C describe the current system.
4. `docs/superpowers/reviews/2026-09-24-work-graph-specialist-review.md` — corrections C1–C29 and adopted ideas.

These are prior work by other agents. Treat them as a strong starting point,
not scripture: if the code disagrees, the code wins and you update the docs.
If they are missing, do the discovery in §7 and produce them.

## 1. The product intent (why)

A flat list of AI sessions stops working when one person runs many at once
across companies, repos, tickets and machines. Fleet should gradually
understand **what work is happening** and organise sessions around it — so the
user does not have to.

```
today:      session manager
next:       work-aware session manager      ← this effort
later:      persistent development work graph
eventually: orchestration that knows what is being done, why, by which
            sessions, what changed, what remains, and what a new agent needs
```

The concrete promise, in the user's words: *sessions grouped by ticket; start
work from a ticket; resume old work with its context; finished work tidies
itself away; never destroy anything useful.*

The real user: works for several organisations (e.g. Company A on **Jira**,
Company B on **Asana**, personal projects with **no tracker**), across several
hosts, with many parallel Claude sessions per ticket.

## 2. Make it functional, not rigid (the most important section)

Every design choice must pass these tests:

1. **Day 1 with zero configuration is useful.** No tracker, no org, no `gh`:
   branch names alone already group work; a user can name a workstream by
   hand. Configuration only *adds*.
2. **Trackers enrich, never gate.** A key, a local work item or a pasted URL
   is a valid unit of work before any tracker knows it. When a tracker is
   connected later, existing work is bound to it retroactively.
3. **No forced hierarchy.** Organisation → project → area → item is *one
   possible* grouping. Grouping is a user choice (work / project / host /
   flat); hierarchy comes from what trackers actually return
   (`hierarchyLevel`, parent), never from assumptions (Scrum, epics, status
   names).
4. **Organisations are optional.** Default scope is derived (GitHub owner).
   Named orgs exist to merge/split scopes and, when present, to be a
   security boundary.
5. **Progressive disclosure.** Controls appear when they have something to
   control (scope selector with ≥2 scopes; "Current sprint" only when sprints
   exist; suggestions only when there is doubt).
6. **Degrade gracefully.** Tracker offline, token expired, hub down, older hub,
   phone on an older contract: everything keeps working on cached/partial
   data and says so honestly.
7. **Defaults over questions.** Decide the reversible things yourself and
   record them; ask only what is genuinely the user's (§8).
8. **No separate app inside the app.** No Jira clone: tickets live in the ⌘K
   switcher, context in the session Details, overview in a Today view,
   grouping in the existing sidebar.

## 3. Architectural principles

- **Observe → resolve → act.** Signals are not facts. A resolver turns them
  into links with provenance; actions follow links.
- **Provenance and explainability.** Every link records `source`
  (manual/started/agent/branch/pr/url/prompt/resumed/forked/inherited),
  `strength` tier (explicit/strong/weak — tiers, not floats) and
  denormalised evidence the UI can show: *"auto-linked: branch
  `abc-123-login` since 09:05 · Undo"*.
- **Manual override is first-class.** User decisions are final; a rejection is
  sticky.
- **State vs event signals.** A branch/PR head is *current state* — only its
  current value counts, and a change ends the auto link it created. Prompts,
  URLs, agent declarations are *events* — they produce suggestions.
- **Deterministic first.** No LLM inference where rules suffice; LLM help
  (classification nudge, summaries) is opt-in and uses the session's own
  model, never a key fleet must hold.
- **Provider-agnostic core.** Core knows Work item, Link, Tracker, Org,
  Journal. `JiraSprint`/`AsanaSection` live only in adapters and `meta`.
- **UI is a projection.** New views are queries/filters, not new storage.
- **Preserve work.** Archiving never deletes worktrees, branches, transcripts,
  links or history. Killing a live session is an explicit, safe-kill choice.

## 4. Vocabulary (collisions in this codebase — do not reuse these words)

| Word | Already means | Use instead |
|---|---|---|
| project | a GitHub `owner/repo` (`projects` table) | tracker project → item `containers` |
| client | a paired phone/browser token | organisation / org |
| task | a session→session dispatched job (`tasks`, migration 020) | work item |
| account | a Claude login (`accounts`) | — |
| session | a **mirror of a tmux session**, not an owned entity | work link carries lifecycle |

## 5. Codebase traps (verify each; they shaped the design)

1. **Session rows mirror tmux.** Reconcile upserts on `(host_alias,
   tmux_name)` and re-derives `project_id` every pass. Never put lifecycle
   flags on `sessions`; never add a work column to reconcile's `ON CONFLICT`
   list.
2. **`sessions.id` is not durable.** Kill → ghost → hard delete one pass later;
   move creates a new row; ids are reused (no AUTOINCREMENT). The durable
   identity is `participants` (migration 043) — but it is minted lazily,
   retired at five raw-SQL sites and swept after retention. Anchor on it with
   a real FK (`ON DELETE SET NULL`), end links with a **trigger** on
   `retired_at`, snapshot at end.
3. **`rename_session` likely creates a new row** (new tmux name ⇒ new upsert
   key) and orphans identity. Write the failing test first.
4. **Cascades.** `conversations` and `session_events` die with the row —
   harvest what work memory needs before that.
5. **Branch.** Not on `sessions`. The cheapest live source is `gitBranch` on
   every transcript JSONL line (the tail `context::refresh` already reads);
   `worktrees.branch` is a fallback. The `gh pr view` probe is gated on `gh`,
   github layout, 12 sessions, 300 s.
6. **Hooks.** `SessionStart` is installed `async` with output discarded and
   the server returns 204 — no context injection there today. `Stop` and
   `UserPromptSubmit` can return `additionalContext` (8000-char cap, shared
   with the inbox).
7. **Typing into panes is fragile.** A fresh worktree shows a trust dialog;
   `wait_for_repl_ready` gives up after 20 s and sends anyway. Deliver briefs
   as a `hub`-participant message via `additionalContext` plus a short start
   prompt.
8. **Hub-client mode = parity or refusal.** Every Tauri command has a verdict
   in `src-tauri/src/backend/verdicts.rs`. A paired desktop is a *client* —
   it can never call `Access::Master` tools, so admin commands are
   `LocalOnly` ("configure on the hub"). Background work must be a
   `FleetTasks` method or a paired desktop becomes a second brain.
9. **Wire compatibility.** New params on existing tools are silently dropped by
   older hubs (no `deny_unknown_fields`) — add new tools and gate UI on the
   hub's tool list. A `CONTRACT_REVISION` bump locks out phones at
   `MAX_HUB_CONTRACT`. New wire enums need an `Unknown` variant. New row
   fields need `#[serde(default)]`.
10. **Events.** `EVENT_NAMES` / `EVENT_KINDS` are fixed-size arrays mirrored in
    `events.ts`; the phone subscribes only to kinds it lists; the hub replay
    ring (512) is shared — emit on real change only.
11. **MCP budget.** `BUDGET_BYTES` has ~100 bytes of headroom. Group work tools
    (`work`, `work_link`, `work_admin`), never one tool per action; every tool
    needs a `TOOL_POLICIES` row.
12. **Security.** Every host's Claude holds a Client token and can list
    fleet-wide — org isolation for host-bound callers must be enforced, not
    just filtered. Tracker text is third-party (`mark_untrusted`). Secrets are
    plaintext SQLite today — keep them off every read path, redact
    Basic/`ATATT` patterns, allowlist tracker URLs (SSRF).
13. **Jira reality.** `/search` is gone (`/search/jql`, `nextPageToken`, no
    total, explicit fields); keys change (identity = id + aliases); 404 means
    deleted *or* no permission; `statusCategory` has `undefined`; JQL dates are
    user-timezone, minute precision; sprints are per project; descriptions are
    ADF; API tokens expire yearly.
14. **Generated artefacts.** `REGEN_DOCS=1 cargo test -p fleet-core
    reference_is_current` after any tool change; `REGEN_HUB_VERDICTS=1 cargo
    test -p claude-fleet --lib verdict_gen` after any verdict change;
    migrations are `NNN_topic.sql` + `MIGRATIONS` entry + re-run guard.

## 6. Edge cases the model must handle by rule, not by exception

One item / many sessions · one session / many items (sequentially: conversation
windows) · session with no work · ticket assigned after the session exists
(retro-bind) · renamed / moved / reopened / deleted-or-hidden ticket · tracker
offline / token expired / rate-limited · repo in several orgs · wrong key in a
branch · key mentioned only as reference · monorepo with several projects ·
two Jira sites with the same key · session moved, forked, renamed, restored,
recreated, reaped then resumed · review and worker sessions (inherit the
parent's work) · fleet-injected text echoing back as evidence (loop guard) ·
prompt listing many keys (dump guard) · older hub / older phone.

## 7. How to work

**If the four documents in §0 exist** (normal case):
1. Spend the first pass confirming them against the code: re-check every trap
   in §5 and every C* the next milestone touches. Report drift.
2. Take the **next milestone** from the roadmap. For it: brainstorm → a short
   spec *delta* against design §0 → an implementation plan → execute in small
   reviewed steps → whole-branch review → PR.
3. Every C* correction you touch gets a failing test first. Run the repo's
   checks (`scripts/ci-local.sh`, plus `--hub-e2e` for wire changes).
4. Update the roadmap's *Revisions* and the design where reality differed.

**If they do not exist:** do a discovery pass (architecture, persistence,
session model and lifecycle, creation flow, tmux + Claude integration, hooks,
UI and navigation, projects, integrations, events, background tasks,
configuration, tests, conventions, extension points) and produce: current
state · reusable components · gap analysis · domain model (purpose, owner,
lifecycle, persistence, source of truth per concept) · provider, detection
and lifecycle architecture · UX · first vertical slice · incremental plan ·
risks · open decisions. Reference real files and lines; no generic advice.

**Where this prompt conflicts with an established, sensible convention in the
code, say so and recommend which should win, with reasons.**

## 8. Asking the user

Do not ask what the repo can answer. Do not ask broad questions. Decide
reversible things and record them. Ask only for genuine product decisions —
the roadmap's *Decisions still open* (D1–D6) is the current list. For each
question give context, options, trade-offs and your recommended default, and
keep working on everything that does not depend on the answer.

## 9. Non-goals for now

Every tracker at once · ML classification · autonomous ticket modification
(write-back is opt-in, late) · destructive automatic cleanup · a graph
database · analytics · complete AI memory · a Jira UI clone · OAuth apps.

## 10. Quality bar

Small, independently reviewable PRs, each with a user-visible *value*
statement. Deterministic tests (store in-memory, `FakeSsh`, recorded tracker
fixtures, fake transport). Existing sessions keep working and are enriched
gradually — no destructive migration, no required setup. When done with a
milestone, state plainly what works, what was verified and what was not.
