# Shared work context (roadmap part 1) — design

**Date:** 2026-09-29
**Status:** implemented on branch `docs/internal-task-list-spec` (plan `2026-09-29-shared-work-context.md`).
**Roadmap:** part 1 of `2026-09-29-ai-task-system-brainstorming.md` ("Spoločný kontext práce"). Parts 2–5 get their own specs.
**Supersedes as a product:** `2026-09-29-internal-task-list-design.md` and its plan. They stay on this branch as history, and several of their mechanisms are reused here (§7).
**Builds on:** the work graph (`2026-09-24-work-graph-design.md` §0), native item status (migration 084), the work journal (migration 047), `dispatch_task` (migration 020).

## Goal

A person sees and drives all work on one ticket from one place:

- the ticket itself,
- internal subtasks,
- the sessions that worked on it,
- jobs delegated to other sessions,
- the steps agents took.

A task with no ticket is equally first-class. Agents can propose subtasks, and a person decides. This is the foundation that part 2 (task memory and evidence-backed summaries), part 3 (workflows), part 4 (AI preparation) and part 5 (autonomy) build on.

## Decisions (owner, 2026-09-29)

| # | Question | Answer |
|---|---|---|
| C1 | What is a "task" when a ticket exists | **The ticket is the task.** The tracker item is the task. There is no wrapper layer. A task without a ticket is a native item. |
| C2 | Durable vs transient | **Two levels.** A *subtask* is durable: a person made it, a person accepted an agent's proposal, or it mirrors a delegated job. A *step* is transient: an agent's native todo, captured automatically. Steps show in the task's history and never fill the list. |
| C3 | Data ownership | **Tracker read-only.** The tracker owns the brief, the ticket's status and the assignee. Fleet owns subtasks, steps, sessions, jobs and history. Nothing new is written back. The existing opt-in PR link on Jira stays. Write-backs come later as drafts a person confirms. |
| C4 | Agents | **Claude Code now, behind a generic step interface.** Codex, Gemini and others attach later without a rebuild. Until then they report results over MCP. |
| C5 | Done when | All four: **task page**, **subtask + Start**, **agent proposals**, **my-work list**. |
| C6 | Where steps live | **The work journal**, as a new `step` kind. They are keyed by conversation, so they follow a task through relinks and moves. They are also the raw material for part 2. |

## 1. Model

```
Task (work_items row)
├── tracker item  — owned by its tracker (read-only here), or
└── native item   — source 'local', key TASK-<id>, origin manual

Subtask (work_items row, parent_id → task)
  origin: manual   — a person made it
          proposed — an agent proposed it; proposal_state proposed|accepted|rejected
          agent    — mirrors a dispatch_task job (task_id → tasks.id)

Session  — work_links, as today (a session may work the task or one of its subtasks)
Job      — tasks row; visible through its mirrored subtask
Step     — work_journal row, kind 'step', on the conversation that produced it
```

- **Parent depth is one level** in this part. A subtask's parent is a task, never another subtask. `create` / `propose` refuse a parent whose own `parent_id` is set.
- A tracker's own hierarchy (epic → story, via `parent_id` from sync) is left as it is. This part never writes `parent_id` on a tracker item.
- **Keys.** Every native item (a task or a subtask of any origin) gets the key `TASK-<id>`. The start path is key-driven: its duplicate guard, session name and branch name all come from the key. With a key, Start and branch detection (`task-12-…`) work unchanged.
- **Status.**
  - A native item follows the migration-084 precedence. A person's setting is final.
  - An `agent` subtask follows its job, stamped `status_set_by = 'task'`, which ranks with `'derived'`.
  - A `proposed` subtask that has not been accepted has no status in any list. It shows only as a proposal.

### Migration `086_shared_work_context.sql` (additive)

```sql
ALTER TABLE work_items ADD COLUMN origin         TEXT;    -- manual | proposed | agent | detected
ALTER TABLE work_items ADD COLUMN project_id     INTEGER REFERENCES projects(id) ON DELETE SET NULL;
ALTER TABLE work_items ADD COLUMN notes          TEXT;    -- the brief for Start (≤ BRIEF_MAX_CHARS)
ALTER TABLE work_items ADD COLUMN task_id        INTEGER REFERENCES tasks(id) ON DELETE SET NULL;
ALTER TABLE work_items ADD COLUMN proposal_state TEXT;    -- proposed | accepted | rejected (origin 'proposed' only)
ALTER TABLE work_items ADD COLUMN proposed_by    TEXT;    -- who proposed: a session label / agent id
ALTER TABLE work_items ADD COLUMN proposal_why   TEXT;    -- the agent's one-paragraph reason

UPDATE work_items SET origin = CASE WHEN source = 'local' THEN 'manual' ELSE 'detected' END
 WHERE origin IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS ux_work_items_task ON work_items(task_id) WHERE task_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_work_items_parent ON work_items(parent_id) WHERE parent_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_work_items_proposals ON work_items(parent_id) WHERE proposal_state = 'proposed';
```

- The migration uses the same `already_applied` column guard as 084.
- Rust reads `origin = NULL` as `detected`, so a row that an older hub wrote still classifies.

## 2. Steps: capture behind a generic interface

**Capture.**

- Fleet's `PostToolUse` hook already runs with the matcher `EnterWorktree|ExitWorktree` (`hooks_install.rs:82`). The matcher is extended to Claude Code's todo/task tools.
- The exact tool names and input shapes (`TodoWrite` and any newer task tools, and subagent `Task` calls) are **verified against the Claude Code version in use** before implementation. The spec does not assume them.
- The transcript tail is the backstop when a hook was missed. Fleet already reads it (`context::refresh_context`, `work::harvest`).

**The interface.** One Rust trait-free function per agent kind:

```rust
/// One agent-native event → the steps it says something about.
fn steps_from_claude_tool(tool: &str, input: &serde_json::Value) -> Vec<StepEvent>;

pub struct StepEvent {
    pub native_id: String,      // the agent's id, else a stable hash of (list position, text)
    pub text: String,           // capped, control chars stripped
    pub state: StepState,       // pending | in_progress | completed | cancelled
    pub agent: &'static str,    // "claude_code"
}
```

A later agent adds its own `steps_from_<agent>(…)` that produces the same `StepEvent`s. Storage, the view and the UI never see the agent's own format.

**Storage.**

- Each changed step is appended as a `work_journal` row: `kind = 'step'`, `source = 'hook' | 'transcript'`, `body = text`, and `meta = {"native_id", "state", "agent"}`. It goes on the conversation (`claude_session_id`) the event came from.
- An event that changes nothing is dropped, so repeated `TodoWrite` snapshots do not multiply rows.
- A per-conversation cap, `STEP_CAP` (initially 200 rows, oldest pruned), follows `PROGRESS_CAP`'s pattern.
- **Current state** of a step = the newest row for its `native_id` in that conversation.

**Attribution.**

- A step belongs to the task the same way the handover brief finds a key's journal: through the conversation's links (`snap_claude_ids` / the live session's conversations).
- A session working a subtask attributes its steps to that subtask. They also show, rolled up, on the parent task's page.

**What a step is not** (counter-review risk 3): an agent's `completed` is shown as *"done per the agent"*. It never changes a task's or subtask's status and never counts as evidence. Part 2 decides what evidence is.

## 3. Subtasks, proposals, Start

**Create** (`work_link { action: create, title, parent?, project_id?, notes? }`):

- It makes a native item. With `parent` (`item:<id>`) the item is a subtask; without it, a standalone task.
- Allowed only for callers that can read it back. A standalone task (no `parent`) needs an unscoped caller. A subtask needs a caller whose scope sees the parent task, so a per-host token may add subtasks under a task its own sessions work on.
- The desktop gets a routed `create_work_task` command.

**Propose** (`work_link { action: propose, parent, title, notes?, why? }`):

- For agents, through the MCP tool and a skill section in `claude-fleet-control`.
- It makes a subtask with `origin = 'proposed'`, `proposal_state = 'proposed'`, `proposed_by` (the caller's session label) and `proposal_why`.
- It needs a parent the caller's scope sees.
- A cap of 10 open proposals per parent stops runaway proposing (counter-review risk 2). Past the cap the call answers `E_LIMIT`.

**Decide** (`work_link { action: accept | reject, item_id }`):

- A person's decision, made from the desktop or by the master.
- `accept` flips the proposal to a normal subtask with status `todo`. `origin` stays `proposed`, so where it came from is kept.
- `reject` keeps the row as `rejected`. It is hidden from lists but visible under "rejected proposals" on the task page, so the same idea is not re-proposed blindly: `propose` refuses a title identical to a rejected one under the same parent.
- An agent cannot accept its own proposal. Accept and reject are refused to per-host tokens (`E_FORBIDDEN`).

**Start** (the existing `work_link { action: start, item_id }` / desktop `start_work`). For a native item, empty arguments are filled in (`with_manual_defaults`):

- `project_id` comes from the item, else from its parent task's last project.
- The brief is the parent ticket's brief (the existing `ticket_brief`, fenced as today) followed by `## Subtask` and the item's title and notes.

A proposed-but-not-accepted item cannot be started (`E_INVALID "accept the proposal first"`).

**Jobs.**

- `dispatch_task` mirrors each job as an `agent` subtask: a best-effort write after `inherit_worker_work`.
  - The parent is the requester's primary item. If that item is itself a subtask, the parent is that subtask's parent, which keeps the depth at one.
  - The worker is linked to the subtask as a **secondary** link, so its inherited primary is unchanged.
- The job's status is followed from `start_task` / `fail_task` / `cancel_task` / `complete_task`.
- The job's result paragraph shows on the task page.
- A mirror failure never fails the dispatch.

## 4. Desktop UI

**My work (the Work tab's default).** It is built from one `work_tree` read with the current filters (the filter bar and saved views, below). `archived` is forced on, so the Done section can show finished tasks. The result is grouped in the client.

- The list is split into **To do / Doing / Done**. Done is collapsed and limited to the last 7 days.
- Roots are:
  - tracker items assigned to me (`mine`),
  - tracker items with live or recent sessions,
  - native tasks.
- Each root shows:
  - its subtasks indented, with 🤖 on agent subtasks,
  - a badge with its number of open proposals,
  - the live-session count,
  - **Start**.
- The Work tab's existing header stays as it is: the **Tasks | Review** tabs, ⚙ placement rules, the saved-view select with *Save as…*, search, *▾ Filters*, *Assigned to me* and *To review*.
- One control is added to that header: a **List | Grouped** toggle. List is the new status view and the default. Grouped is today's org → group tree, unchanged.
- A **+ New task** field sits at the top of the List. ▾ adds a project and notes.
- Each row keeps today's facts: tracker badge, key and title, the tracker's own status name (e.g. *Backlog*, *In Development*), org, `N active · N past`, the ★ session occurrences, and the needs-you dot.
- A ticket's section comes from Fleet's effective status plus live sessions, not from the tracker's status name. A Jira *Done* ticket with a live session sits in Doing and still shows *Done* in its row.
- The ☑ Tasks popover is removed, because jobs are in the list. `TasksPanel` stays in the session details.

**Task page (the details pane, `WorkTaskDetail`).** Sections, in this order:

1. **Brief.** The tracker's description (as today), or the native task's notes.
2. **Subtasks.** Each has a status, a Start button and its live sessions. *+ Add subtask* adds one.
3. **Proposals.** Each has its title, its reason, and who proposed it, with **Accept** / **Reject**. Rejected proposals are behind a toggle.
4. **Jobs.** Delegated jobs with their state and result.
5. **Sessions.** Live and ended, as today.
6. **Agent steps.** Grouped by session, collapsed, each marked *per the agent*, with the newest state per step.

Tracker text, proposal text, notes, steps and results are rendered as text, never as markup.

**Owner's review of the mockup (2026-09-29):**

- Proposals show in the list only as the "N to review" badge. They are reviewed on the task page.
- Agent steps stay on the task page, grouped per session and collapsed after the first group.
- Done is collapsed by default.

### Existing Work functionality: where each piece goes

Nothing the Work tab does today is dropped. The new list is a new default view over the same data and the same filters.

| Existing | In the new design |
|---|---|
| **Filters bar** (`WorkFiltersBar`): search, the Filters panel (org, tracker, status, sessions), the *Mine* and *Review* toggles | Stays on top of My work, as the same component and the same `WorkTreeFilters` object, and filters the list. The status filter narrows which sections show. |
| *Archived* toggle | Replaced in My work by the Done section, which always reads archived tasks. It stays in Grouped view. |
| **Saved views** (`work_views`, shared with the phone and other desktops) | Stay in the filter bar and apply to My work. A saved view is the same filters object, so existing views keep working. |
| Org → group tree, placements, placement rules | The **List \| Grouped** toggle's Grouped side (today's `WorkTree`, unchanged). Placement rules stay on the header's ⚙. |
| Review tab (link suggestions) | Unchanged: the header's **Review** tab. |
| Attention strip (`ScopeAttention`, `TrackerAttention`, `LinkReview`, `TidyReview`) | Unchanged. |
| **Today view** (the details empty state, ⌘⇧T) | Unchanged. Part 2 may feed it task summaries. |
| ⌘⇧W (Sessions ↔ Work), ⌘⇧O (cycle org), ⌘K *My work / Current sprint / Recent* | Unchanged. ⌘⇧O cycles the same org filter, so it now filters My work. |
| Session-row work chip, **Name this work…**, the session details' `SessionTasks` and `TicketCard` | Unchanged. Naming work creates a local item, which then appears in My work as a task. |
| Start from a ticket (New session dialog, multi-repo start) | Unchanged. The list's **Start** calls the same `start_work`. |
| Resume past work, handover, summarize past work, tidy-up, reopened tickets | Unchanged. The task page's Sessions section uses the existing Resume dialog. |
| Task detail's existing sections (description, last outcome, placement, matching rules) | Kept. Description becomes *Brief*, last outcome sits under Sessions, and placement and rules move under the task page's ⋯. |
| `TasksPanel` in session details | Stays. Only the sidebar's global ☑ popover is removed. |

## 5. Counter-review countermeasures in this part

| Risk | What this part does |
|---|---|
| Another system to maintain by hand | The tracker is read-only (C3). Subtasks never create external tickets. Steps are captured, never typed. |
| AI creates more process than value | Proposals need a person. There is a cap per parent, and a rejected title is not re-proposed. Steps never enter the list. |
| A convincing summary without a verified result | An agent's step state is labelled "per the agent" and never moves a status. Evidence and summaries are part 2's. |

## 6. Wire and compatibility

- Every wire change is additive:
  - new `WorkItemRow` / `WorkTask` / `TaskDetail` fields,
  - new `work_link` actions,
  - one new routed desktop command.
- There is **no `CONTRACT_REVISION` bump**, following `wire_contract.rs`'s rules. An older hub answers the new actions with `E_INVALID "unknown work_link action"`.
- **Mobile app:** fleet-mobile keeps working unchanged. Showing subtasks, proposals and steps there is later work.
- **MCP:** the `work_link` action enum and its parameters change once, in one release.
- The isolation matrix gets rows for `create`, `propose`, `accept` and `reject`.

## 7. Reused from the paused internal-task-list plan

These carry over, adapted to this model:

- migration shape and guard, now with the proposal columns;
- `TASK-<id>` keys;
- the `'task'` status precedence;
- the job mirror, now with the depth-one parent rule;
- `with_manual_defaults`, now with the parent-brief composition;
- `TaskList.svelte` and the client grouping, now with subtasks and proposal badges;
- removing the ☑ popover.

## 8. Testing

- **Store:**
  - migration 086 on a v85 DB, including the backfill and a re-run;
  - create, propose, accept and reject, and their refusals (depth, cap, duplicate rejected title, self-accept, unaccepted Start);
  - step append, dedup, cap and the current state per `native_id`.
- **Capture:**
  - `steps_from_claude_tool` against recorded tool inputs, kept as fixtures under `testdata/steps/`;
  - the hook path end to end with a fake session;
  - the transcript backstop.
- **Attribution:** a subtask session's steps show on the subtask and roll up to the task. They survive a relink (the journal's conversation keying).
- **Jobs:** each job transition, the depth-one parent rule, the secondary link, and a failed mirror not failing a dispatch.
- **View:** the new fields, proposals excluded from status sections, and rejected proposals hidden.
- **Isolation matrix:** the four new actions.
- **Frontend (vitest):**
  - my-work grouping and nesting;
  - quick add;
  - Start;
  - accept and reject;
  - the task-page sections;
  - text-not-markup for every third-party string;
  - the ☑ button gone.
- **CI:** the full local CI mirror.

## 9. Prerequisite and open items

- **Trackers:** the hub (`fleet.rlt.sk`) already has *Papaya POS Jira* and *SalesTwins Asana* connected, with orgs 32bit, Papaya POS and SalesTwins. An earlier "0 trackers" reading came from a local instance, not the hub. "My work" needs no setup.
- **To verify before the plan:**
  - Claude Code's current todo/task tool names and input shapes, and whether subagent (`Task`) calls should become steps;
  - whether `PostToolUse` fires for them with the input in the payload.
- **Left to later parts:**
  - summaries and evidence (part 2);
  - workflow steps as a first-class, person-editable plan (part 3), including who owns a detailed plan when an agent also has one;
  - what an agent may decide alone (part 5).
