# One internal task list — design

**Date:** 2026-09-29
**Status:** design approved by the owner, nothing implemented.
**Revised:** 2026-09-29, after reading the code — see "Revisions after reading the code" below; they win where they differ from §1–§4.
**Builds on:** `2026-09-24-work-graph-design.md` (§0), `2026-09-27-work-view-design.md`,
`2026-09-28-sprints-releases-epics-design.md` (decision E1 "native owns").

## Why

The owner does not use the Work tab. On a live install on 2026-09-29 it held no
trackers, no local items, no dispatched tasks and five auto-detected refs
(`#2`, `#3`, `#333`, …) with **empty titles**. The Work view today is
tracker-shaped: its main concepts are orgs, placements, rules, review and link
suggestions, and your own work only enters it through "Name this work…" on a
session that already exists.

Separately, the ☑ **Tasks** popover shows `dispatch_task` jobs. Those jobs are a
different system, and nothing ties them to the Work view.

The goal is **one list of tasks the owner and agents both put work into**:

- tasks the owner writes and starts, and
- tasks one session dispatches to another,

each with its sessions and a status that follows the evidence.

## Decisions (owner, 2026-09-29)

| # | Question | Answer |
|---|---|---|
| T1 | What "internal task system" means | Own tasks **and** agent-dispatched tasks, merged into one list. |
| T2 | Trackers and auto-detection | **Unchanged and on.** Detected items join the same list. |
| T3 | Creating an own task | **Quick add + Start.** Title, optional project and notes. Start opens a session in that project with title + notes as its first prompt, and links it. |
| T4 | Where dispatched jobs appear | **Under the parent task.** If the requester session has a primary task, the job is its subtask; otherwise it is a top-level row. It carries an agent badge. |
| T5 | Work tab layout | **By status:** To do / Doing / Done (Done collapsed, last 7 days). The advanced machinery moves behind one menu. |
| T6 | How dispatched jobs join | **Mirror as work items (approach A).** `dispatch_task` also writes a work item. The `tasks` table and its orchestration semantics are untouched. |

Rejected alternatives for T6:

- **B, merge only when displaying.** The Work list query would pull rows from `tasks` at read time. Every consumer (mobile, MCP `work {tree}`, filters) would then have to handle two kinds of row.
- **C, replace `tasks`.** Rewrites `dispatch_task` / `wait_for_task` and the `FLEET_TASK_DONE_<nonce>` scan, which is the riskiest code for this feature's size.

## 1. Data model — migration `086_internal_tasks.sql`

This migration only adds columns. No row is deleted and no column changes meaning.

```sql
ALTER TABLE work_items ADD COLUMN origin     TEXT;     -- manual | agent | detected
ALTER TABLE work_items ADD COLUMN project_id INTEGER REFERENCES projects(id) ON DELETE SET NULL;
ALTER TABLE work_items ADD COLUMN notes      TEXT;     -- brief for Start; ≤ BRIEF_MAX_CHARS
ALTER TABLE work_items ADD COLUMN task_id    INTEGER REFERENCES tasks(id) ON DELETE SET NULL;

-- Fill in origin for existing rows. Tracker rows and local rows that carry a
-- key were detected or named from a session; only keyless local rows were
-- created by a person.
UPDATE work_items SET origin = CASE
  WHEN source = 'local' AND key IS NULL THEN 'manual'
  ELSE 'detected' END
WHERE origin IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS ux_work_items_task
  ON work_items(task_id) WHERE task_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_work_items_origin_status
  ON work_items(origin, status_category);

INSERT OR IGNORE INTO schema_version (version) VALUES (86);
```

- `origin` is nullable at the SQL level, and Rust reads `NULL` as `detected`. That way a row written by an older binary during a rolling hub upgrade still classifies.
- `parent_id` already exists (migration 048). Until now only tracker sync wrote it. Agent subtasks now write it too.
- **Status.** Own items use the native status from migration 084 (`status_set_by`). Agent items get their status from the job state (§2.3), stamped `status_set_by = 'task'`. A person's explicit `set_status` still wins (E6). `'task'` becomes a third `status_set_by` value next to 084's `'person'` / `'derived'`, and it ranks with `'derived'`, below `'person'`. The precedence code in `store/work.rs` and its SQL CASE mirror (`a51199c1`) must both learn it.

## 2. Backend

### 2.1 Create an own task

- New store function `create_manual_task(title, project_id, notes)`. It inserts `source='local', origin='manual', key=NULL`.
  - It is a sibling of `create_local_work_item` (`store/work.rs:598`), which stays as it is for "Name this work…".
  - Checks: title 1..=`WORK_TITLE_MAX_CHARS`; notes are cut to `BRIEF_MAX_CHARS`; `project_id` must exist.
- Two ways to call it:
  - MCP: `work_link { action: "create", title, project_id?, notes? }`. `"create"` is added to `WORK_LINK_ACTIONS`, and the call runs under the caller's org scope like other actions.
  - Tauri: the `work_create_task` command in `src-tauri/src/commands/work_view.rs`.
- It emits `work:changed`.

### 2.2 Start an own task

`work_link { action: "start", task_id: "item:<id>" }` and the Tauri `start_work` path already exist. For a `manual` item:

- The session's project defaults to the item's `project_id`. If there is none, the caller must pass one; otherwise the call fails with `E_INVALID` "pick a project".
- The brief is `title + "\n\n" + notes`, queued through the existing handover (`enqueue_handover` + `spawn_start_prompt`).
- The link source stays `started` / `agent_started` (D34), as today. Nothing new is needed for the link itself.

### 2.3 Mirror dispatched jobs (`service/tasks.rs`)

Every job state change already goes through `create_task`, `start_task`, `fail_task`, `cancel_task` and `complete_task`. The mirror calls each of them **best-effort**: it runs `let _ = mirror_*(…)`, logs any error and swallows it. A failed mirror never fails a dispatch.

| Job event | Work item effect |
|---|---|
| `create_task` | Insert `source='local', origin='agent', task_id, title` = the first line of the prompt (≤ 120 chars, no `FLEET_TASK_DONE` instruction), `notes` = the prompt (cut). `parent_id` = the requester session's **primary live** item, if any. `project_id` = the worker's project. Link the worker session (source `agent_started`). Status `todo`. |
| `start_task` | Status `in_progress`. |
| `complete_task` | Status `done`. |
| `fail_task` / `cancel_task` | Status `done` plus a failed/cancelled marker, read from `tasks.state` at view time. There is no new column. |

- Each status write sets `status_set_by='task'`, but only when the current `status_set_by` is not `'person'`. A person's override wins.
- The mirror takes the store lock only for its own writes. It is never held across `.await` (repo rule).

### 2.4 Titles for detected items

When a `detected` item has an empty title (no tracker, or a bare `#N`), the view shows the **name of its first linked session**, marked as derived. Nothing is written; the fallback happens in the tree builder.

### 2.5 View shape (`service/work/view.rs`)

- `work {tree}` gets a new grouping, `group_by: "status"`, which becomes the desktop default. The groups are:
  - `todo`
  - `in_progress` (Doing)
  - `done`, only items whose status changed in the last 7 days, and collapsed
- Each row already carries a `task_id`. It gains these fields:
  - `origin`
  - `project` (id + label)
  - `parent_task_id`
  - `job_state` (for agent rows)
  - `child_count`
- Agent children are nested under their parent row; they do not appear as top-level rows.
- The existing groupings (repo/org/placement) stay available through `filters.group_by`.

## 3. Desktop UI

- **`WorkTree.svelte`**
  - A **"+ New task"** field at the top. Enter creates the task from the title. An expander shows a project picker (the existing project list) and a notes textarea.
  - Three sections: **To do**, **Doing** and **Done ▸** (collapsed).
  - Each row shows: status dot, title (derived titles in italics), project label, 🤖 on agent rows, the live-session count, and a **Start** button on To do rows that have no live session.
  - Agent children are indented under their parent and can be collapsed. A child shows its job state (queued/running/done/failed) and, in the detail pane, the worker's `result`.
- **One ⋯ Advanced menu** replaces the always-visible `WorkFiltersBar`, org select, `WorkReview` and `WorkRules` entry points. It opens: Filters, Group by (repo/org/placement), Review link suggestions, Rules, Orgs.
  - The attention strip entries (`LinkReview`, `TidyReview`, …) stay as they are.
- **The ☑ Tasks button and its modal are removed** (`SidebarFilters.svelte` ~261, `Sidebar.svelte` ~1594). `TasksPanel` stays in session details (`SessionDetails.svelte:627`).
- Today view, the session-row work chip, "Name this work…" and tracker settings are unchanged.

## 4. Compatibility

- The wire changes only add fields. `CONTRACT_REVISION` goes 6 → 7 (`crates/fleet-core/src/wire_contract.rs`), and `docs/control-api-reference.md` is regenerated.
- fleet-mobile keeps working at contract 6, because nothing it reads is removed or renamed. The default `group_by` changes **only** when the caller sends `group_by: "status"`, or when the desktop sends it.
- Mobile support for the new list comes later, as separate work.
- The MCP tool descriptions for `work` and `work_link` change. This invalidates the clients' tool-definition cache, so both edits ship in one release.
- `skills/claude-fleet-control/SKILL.md` documents `work_link {create}` and states that dispatched jobs appear in Work.

## 5. Testing

- **Store**
  - Migration 086 on a DB at 085: origin is filled in for tracker, keyed-local and keyless-local rows.
  - `create_manual_task` checks (empty title, overlong title, unknown project).
- **Mirror**
  - Each job event maps to its status, including a parent found and not found.
  - A person's `set_status` is not overwritten by a later job event.
  - A mirror failure (e.g. a forced constraint error) still returns a successful `dispatch_task`.
  - `wait_for_task` behaviour is unchanged (the existing tests pass untouched).
- **Start**: a manual item with a project, and without a project (`E_INVALID`). The brief is queued.
- **View**
  - `group_by: "status"` builds the sections, nests agent children, applies the Done 7-day window and the derived-title fallback.
  - Every existing `work {tree}` test passes without `group_by`.
- **Frontend (vitest)**
  - Quick add creates a task and it shows under To do.
  - Start calls the start command.
  - Children are nested.
  - The ☑ button is gone.
  - The ⋯ menu opens the moved panels.
- **CI**: the full local CI mirror (the GitHub Actions billing block still applies).

## 6. Not in scope

- Sprints, releases, epics and the board (sprints spec, phases 2–5).
- The Work view reload cost: 2+K full graph loads per refresh, and the store lock held through the build (`reviews/2026-09-28-two-day-review.md` :220, :224). This should be the next PR; the status grouping will not make it worse.
- fleet-mobile UI changes.
- Any change to tracker sync, detection rules or org isolation.

## Revisions after reading the code (2026-09-29)

The implementation plan (`docs/superpowers/plans/2026-09-29-internal-task-list.md`) is written against these changes:

1. **Native tasks get a key, `TASK-<id>`.** The start path (`service::trackers::tickets::resolve_start`) refuses an item with no key ("that work item has no key to start from"), and its duplicate guard, session name and branch name are all derived from the key. Giving manual and agent items a key lets Start reuse the existing path unchanged, and lets a branch named `task-12-…` link itself.
2. **Backfill:** a local item → `manual`, anything else → `detected`. Every local item that exists today was named through "Name this work…".
3. **No server-side `group_by: status`.** The tree is org/group/section-paged (sections, cursors, `TreeGroup`). The desktop's new `TaskList.svelte` reads one `work_tree` page (`archived: true`, limit 200) and groups it by status in the client. The old `WorkTree.svelte` stays, reachable as ⋯ → Grouped view, so nothing it does is lost. The backend view only gains fields. This is also one graph load per refresh, fewer than the tree's own reads.
4. **No `CONTRACT_REVISION` bump.** The wire changes only add fields. `create` is a new action on an existing tool, and an older hub answers it with `E_INVALID "unknown work_link action"`, which is a clear refusal. None of the changed types are in `hub_contract.golden.json`.
5. **`create` is for unscoped callers only** (the desktop, the master token, an unbound client). A new task has no links, and a scoped caller sees a local item only through its links, so it could not read back what it created.
6. **Starting a manual task:** `start_work` fills an unset `project_id` from the item and an unset brief from `title + notes` (`with_manual_defaults`). An agent's `work_link { start, item_id }` therefore behaves like the desktop's Start.
7. **The dispatch mirror runs in `dispatch_task` after `inherit_worker_work`.** It links the worker to the agent item as a **secondary** link, so the worker's inherited primary (work graph M2.2) is unchanged. State changes are mirrored from `start_task` / `fail_task` / `cancel_task` / `complete_task`.
8. **`'task'` joins `'person'` / `'derived'` as a final `status_set_by`**, in both `effective_status` and `effective_status_sql!`. The tidy planner reads only `'derived'`, so it is unaffected.
