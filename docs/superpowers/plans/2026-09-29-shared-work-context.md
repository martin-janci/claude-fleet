# Shared work context (roadmap part 1) — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A ticket (or a native task) becomes one place that holds:

- its subtasks (made by a person, accepted from an agent's proposal, or mirrored from a delegated job),
- its sessions,
- its jobs,
- the steps agents took, captured from Claude Code's own task tools.

The Work tab keeps its header and filters and gains a **List | Grouped** toggle. List is a To do / Doing / Done view with **+ New task** and **Start**.

**Architecture:**

- **Data.** Additive columns on `work_items` (migration 086) carry origin, project, notes, job and proposal state. Native items get `TASK-<id>` keys, so the key-driven start path and branch detection work unchanged.
- **Steps.** They are `work_journal` rows of a new `step` kind, written from the `PostToolUse` hook (matcher extended to `TaskCreate|TaskUpdate|TodoWrite`), with a transcript backstop for hosts whose hooks are not re-provisioned yet.
- **Desktop.** One `work_tree` read with the current filters, grouped by status in the client. The task page gets new sections inside the existing `WorkTaskDetail`.

**Tech Stack:** Rust (fleet-core: rusqlite, rmcp, axum hooks), Tauri 2 commands, Svelte 5 runes, vitest + @testing-library/svelte.

**Spec:** `docs/superpowers/specs/2026-09-29-shared-work-context-design.md`. Read it whole, including "Owner's review of the mockup" and "Existing Work functionality: where each piece goes". Mockup: https://claude.ai/artifact/V1HLQr4yBUdNDx1hof6Ums (version 3).

**Supersedes:** `docs/superpowers/plans/2026-09-29-internal-task-list.md`. Do not execute that plan. Code shapes from it are repeated here where reused.

## Global Constraints

- Work in `.claude/worktrees/internal-task-list` (branch `docs/internal-task-list-spec`, tracking `origin`). Paths below are relative to it. Before starting, run `git fetch && git merge --ff-only origin/main` if `main` moved. If it does not fast-forward, stop and ask.
- **Store mutex:** take, work, drop. Never hold `Mutex<Store>` across `.await`.
- **Best-effort writes** (job mirror, step capture) never fail the mutation or hook that caused them. Log with `tracing::debug!` and swallow the error.
- **Wire:** snake_case fields. New Rust `Option<T>` fields are `#[serde(default, skip_serializing_if = "Option::is_none")]`, and their TS mirrors are `field?: T | null`. **No `CONTRACT_REVISION` bump**, because every change is additive and new `work_link` actions are refused clearly by an older hub.
- **MCP descriptions change once:** all `work_link` enum and param additions land in this branch's release.
- **`status_set_by`** values: `NULL | 'person' | 'derived' | 'task'`. `'task'` ranks with `'derived'` (final over the live lift) and below `'person'`.
- **Native keys** are `TASK-<item id>`. **Depth:** a subtask's parent is never itself a subtask.
- **Limits** (constants, exact names):
  - `LOCAL_WORK_TITLE_MAX_CHARS` = 120 (existing);
  - `service::work::handover::BRIEF_MAX_CHARS` = 4000 (existing), used for notes and why, which are cut rather than refused;
  - `PROPOSALS_OPEN_CAP` = 10;
  - `STEP_CAP` = 200;
  - `STEP_TEXT_MAX_CHARS` = 300;
  - `DONE_WINDOW_SECS` = 7 × 86400.
- **Claude Code tool shapes** (verified 2026-09-29 against local transcripts, Claude Code 2.1.284):
  - `TaskCreate` input is `{subject, description, activeForm}`. The id is only in `tool_response`: `{"task": {"id": "1", "subject": …}}`.
  - `TaskUpdate` input is `{taskId | task_id, status?: "pending"|"in_progress"|"completed", subject?, description?}`.
  - `TodoWrite` input is `{todos: [{content, status, activeForm}]}` and has no ids.
  - Subagent (`Agent`) calls are **not** steps in this part.
- Third-party and agent text (tracker text, notes, proposal why, step text, job results) renders as text, never markup. Fence it for `OrgScope::Host` callers the way `view::task` fences descriptions.
- **Local CI mirror** before the PR (GitHub Actions is billing-blocked):
  ```bash
  cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
  pnpm install --frozen-lockfile && pnpm run check && pnpm run test && pnpm run build
  ```
  If the workspace commands do not cover `src-tauri`, also run `(cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test)`. Pre-existing frontend failures (`localStorage is undefined` in `session_ui.test.ts` and `App.test.ts`) must also fail on `origin/main` before they are dismissed.

---

## File structure

| File | Responsibility |
|---|---|
| `crates/fleet-core/migrations/086_shared_work_context.sql` (new) | Seven columns, backfill, three indexes. |
| `crates/fleet-core/src/store/schema.rs` | Register 086 with an `already_applied` guard, plus its test. |
| `crates/fleet-core/src/store/work.rs` | `WorkItemRow` fields, `ITEM_COLUMNS`, `ITEM_COLUMN_COUNT`, `map_item`. |
| `crates/fleet-core/src/service/work/status.rs`, `store/work_status.rs`, `store/rows.rs` | `'task'` in the precedence (Rust, SQL macro, cross-check). |
| `crates/fleet-core/src/store/work_tasks.rs` + `work_tasks/tests.rs` (new) | Native items: create, job mirror, status from a job, job states, proposals. |
| `crates/fleet-core/src/service/work/steps.rs` (new) | `StepEvent`, the Claude Code adapter, the transcript backstop parser. Pure. |
| `crates/fleet-core/src/store/work_journal.rs` | The `step` kind, `STEP_CAP`, `record_steps`, and step reads for keys. |
| `crates/fleet-core/src/service/hooks.rs`, `service/hooks_install.rs` | The step hook route, the matcher, and the Stop-hook backstop spawn. |
| `crates/fleet-core/src/service/work/local.rs` | `create_task`, `propose`, `decide_proposal` (scope gates). |
| `crates/fleet-core/src/service/work/mod.rs` | `WorkLinkArgs` fields, `WORK_LINK_ACTIONS`, `ROUTED_WORK_COMMANDS`. |
| `crates/fleet-core/src/mcp/tools/orchestration.rs` | `work_link` dispatch for the new actions, and `dispatch_task` calling the mirror. |
| `crates/fleet-core/src/service/tasks.rs` | The job mirror and the state follow. |
| `crates/fleet-core/src/service/trackers/tickets.rs` | `with_native_defaults` for Start. |
| `crates/fleet-core/src/service/work/view.rs` + `view_tests.rs` | Tree fields, hiding proposals from the tree, and the task detail's subtasks, proposals, jobs and steps. |
| `crates/fleet-core/src/mcp/tools/tests_isolation.rs` | Matrix rows for `create`, `propose`, `accept`, `reject`. |
| `src-tauri/src/commands/work.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/backend/verdicts.rs`, `tests_routing.rs` | Routed `create_work_task`, `accept_work_proposal`, `reject_work_proposal`. |
| `src/lib/work_view.ts`, `src/lib/work.ts` | TS wire, the command wrappers, and the `workLayout` preference. |
| `src/lib/task_list.ts` + test (new) | Pure grouping into sections and nesting. |
| `src/lib/TaskList.svelte` + test (new) | The List layout: + New task, sections, rows, Start. |
| `src/lib/WorkTree.svelte` | The List \| Grouped toggle, and rendering `TaskList` in List. |
| `src/lib/TaskWorkSections.svelte` + test (new) | Task page sections: Notes, Subtasks, Proposals, Jobs, Agent steps. |
| `src/lib/WorkTaskDetail.svelte` | Mount the sections, and move placement and rules into a *Placement & rules* disclosure. |
| `src/lib/Sidebar.svelte`, `SidebarFilters.svelte`, `WorkViewSwitch.test.ts` | Remove the ☑ popover. |
| `skills/claude-fleet-control/SKILL.md`, `docs/work-graph.md`, `docs/control-api-reference.md` | Docs. |

---

### Task 1: Migration 086 and the `WorkItemRow` fields

**Files:**
- Create: `crates/fleet-core/migrations/086_shared_work_context.sql`
- Modify: `crates/fleet-core/src/store/schema.rs` (MIGRATIONS after the `85` entry; guard next to `work_items_has_status_set_at` ~line 377; test after `migration_081_…` ~line 3806)
- Modify: `crates/fleet-core/src/store/work.rs` (struct :127-192, `ITEM_COLUMNS` :529, `ITEM_COLUMN_COUNT` :538, `map_item` :546)
- Modify: `crates/fleet-core/src/service/work/today.rs` (~line 515, test literal)

**Interfaces:**
- Produces:
  - `WorkItemRow { origin, project_id, notes, task_id, proposal_state, proposed_by, proposal_why }`, all `Option`;
  - `ITEM_COLUMN_COUNT == 32`;
  - `fn work_items_has_origin(&Connection) -> rusqlite::Result<bool>`.

- [ ] **Step 1: Failing test** (`schema.rs` tests):

```rust
    #[test]
    fn migration_086_adds_the_shared_work_columns_backfills_origin_and_is_safe_to_rerun() {
        let s = store_at_version(85);
        s.conn
            .execute_batch(
                "INSERT INTO work_items (source, key, title, created_at, updated_at) VALUES ('local', 'OPS', 'named', 1, 1);
                 INSERT INTO work_items (source, key, title, created_at, updated_at) VALUES ('jira', 'TK-1', 'ticket', 1, 1);",
            )
            .unwrap();
        assert!(!work_items_has_origin(&s.conn).unwrap());
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let got: Vec<(String, String)> = s
            .conn
            .prepare("SELECT title, origin FROM work_items ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(got, vec![("named".into(), "manual".into()), ("ticket".into(), "detected".into())]);
        for col in ["project_id", "notes", "task_id", "proposal_state", "proposed_by", "proposal_why"] {
            let n: i64 = s
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('work_items') WHERE name = ?1",
                    [col],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{col}");
        }
        s.conn.execute_batch("DELETE FROM schema_version WHERE version >= 86;").unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }
```

- [ ] **Step 2: Run** `cargo test -p fleet-core --lib migration_086`. Expected: FAIL (`work_items_has_origin` not found).

- [ ] **Step 3: The migration file:**

```sql
-- Shared work context (design 2026-09-29, AI task system roadmap part 1).
--
-- `origin`         manual | proposed | agent | detected. NULL reads as
--                  `detected` (a row an older hub wrote).
-- `project_id`     where a native item starts.
-- `notes`          a native item's brief (≤ BRIEF_MAX_CHARS, cut by the writer).
-- `task_id`        the dispatched job (`tasks`) an `agent` item mirrors.
-- `proposal_state` proposed | accepted | rejected, for `origin = 'proposed'`.
-- `proposed_by`    who proposed it (a session label).
-- `proposal_why`   the proposer's reason (≤ BRIEF_MAX_CHARS).
--
-- Backfill: every local item existing today was named through "Name this
-- work…" (manual); every other row came from a tracker (detected).
ALTER TABLE work_items ADD COLUMN origin         TEXT;
ALTER TABLE work_items ADD COLUMN project_id     INTEGER REFERENCES projects(id) ON DELETE SET NULL;
ALTER TABLE work_items ADD COLUMN notes          TEXT;
ALTER TABLE work_items ADD COLUMN task_id        INTEGER REFERENCES tasks(id) ON DELETE SET NULL;
ALTER TABLE work_items ADD COLUMN proposal_state TEXT;
ALTER TABLE work_items ADD COLUMN proposed_by    TEXT;
ALTER TABLE work_items ADD COLUMN proposal_why   TEXT;

UPDATE work_items SET origin = CASE WHEN source = 'local' THEN 'manual' ELSE 'detected' END
 WHERE origin IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS ux_work_items_task ON work_items(task_id) WHERE task_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_work_items_parent ON work_items(parent_id) WHERE parent_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_work_items_proposals ON work_items(parent_id) WHERE proposal_state = 'proposed';

INSERT OR IGNORE INTO schema_version (version) VALUES (86);
```

- [ ] **Step 4: Register** after `Migration::plain(85, …)`:

```rust
    // Shared work context (design 2026-09-29): origin, project, notes, job
    // and proposal columns on `work_items`. The ADD COLUMNs are not
    // idempotent, so the same guard 084 uses; the backfill and indexes are.
    Migration {
        version: 86,
        sql: include_str!("../../migrations/086_shared_work_context.sql"),
        already_applied: Some(work_items_has_origin),
    },
```

```rust
/// `already_applied` guard of migration 086 (shared work context).
fn work_items_has_origin(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('work_items') WHERE name = 'origin'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}
```

- [ ] **Step 5: Struct fields** (`store/work.rs`, after `unavailable_reason`):

```rust
    // --- shared work context (migration 086); all default, so an older
    // hub's row still reads.
    /// manual | proposed | agent | detected (`None` reads as detected).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// The dispatched job an `agent` item mirrors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<i64>,
    /// proposed | accepted | rejected (origin `proposed` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_why: Option<String>,
```

- [ ] **Step 6: Columns, count, mapper:**

```rust
pub(super) const ITEM_COLUMNS: &str =
    "id, source, key, title, url, status_category, created_at, updated_at, \
     tracker_id, external_id, aliases, kind, hierarchy_level, status_name, resolution, parent_id, \
     assignees, iteration, updated_ext, status_changed_at, fetched_at, unavailable_at, \
     unavailable_reason, status_set_by, status_set_at, origin, project_id, notes, task_id, \
     proposal_state, proposed_by, proposal_why";

pub(super) const ITEM_COLUMN_COUNT: usize = 32;
```

In `map_item`, after `status_set_at: r.get(24)?,`:

```rust
        origin: r.get(25)?,
        project_id: r.get(26)?,
        notes: r.get(27)?,
        task_id: r.get(28)?,
        proposal_state: r.get(29)?,
        proposed_by: r.get(30)?,
        proposal_why: r.get(31)?,
```

Check every `ITEM_COLUMNS` use (`git grep -n ITEM_COLUMNS crates`) for a JOIN where `project_id` or `task_id` becomes ambiguous (for example `sessions.project_id`). Where one does, qualify the list the way that query already qualifies `id`.

- [ ] **Step 7: Test literal** in `service/work/today.rs` (~515): add `origin: None, project_id: None, notes: None, task_id: None, proposal_state: None, proposed_by: None, proposal_why: None,`.

- [ ] **Step 8: Run** `cargo test -p fleet-core --lib migration_086 && cargo test -p fleet-core --lib store::`. Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/fleet-core/migrations/086_shared_work_context.sql crates/fleet-core/src/store/schema.rs crates/fleet-core/src/store/work.rs crates/fleet-core/src/service/work/today.rs
git commit -m "feat(work): migration 086 — origin, project, notes, job and proposal columns"
```

---

### Task 2: `'task'` joins the status precedence

**Files:**
- Modify: `crates/fleet-core/src/service/work/status.rs:90-104` and tests
- Modify: `crates/fleet-core/src/store/work_status.rs:103-124` (macro)
- Test: `crates/fleet-core/src/store/rows.rs` (`the_sql_macro_and_the_rust_function_agree_arm_by_arm`, ~line 1740)

- [ ] **Step 1: Failing test** in `status.rs` `effective_status_tests`:

```rust
    #[test]
    fn a_jobs_status_is_final_over_the_live_lift() {
        assert_eq!(effective_status("todo", Some("task"), "local", true), Some("todo"));
        assert_eq!(effective_status("done", Some("task"), "local", true), Some("done"));
    }
```

- [ ] **Step 2: Run** `cargo test -p fleet-core --lib a_jobs_status_is_final`. Expected: FAIL.

- [ ] **Step 3: Implement:** `Some("person") | Some("derived") | Some("task") => normalize(status_category),`. The macro becomes `WHEN i.status_set_by IN ('person', 'derived', 'task') THEN \`. Extend both doc comments with one sentence: *a job's status (`'task'`, an agent subtask mirroring its dispatched job) is final like a stamped `done`*.

- [ ] **Step 4: Cross-check block** in `rows.rs`, after the `// derived, with a working session` block. Use the same raw write the `empty` block uses (`s.conn_ref()`):

```rust
            // a job's status ('task'), with a working session: final.
            {
                let s = Store::open_in_memory().unwrap();
                let sid = seed(&s, "task");
                let item = s.create_local_work_item(Some("X-6"), "t").unwrap();
                s.link_session_work(sid, WorkTarget::Item(item.id), "manual").unwrap();
                s.conn_ref()
                    .execute(
                        "UPDATE work_items SET status_category = 'todo', status_set_by = 'task' WHERE id = ?1",
                        rusqlite::params![item.id],
                    )
                    .unwrap();
                mark_working(&s, sid, "c-x-6");
                agree(&s, "task", item.id, true, "job status, working session");
            }
```

- [ ] **Step 5: Run** `cargo test -p fleet-core --lib effective_status && cargo test -p fleet-core --lib agree_arm_by_arm && cargo test -p fleet-core --lib tidy`. Expected: PASS. `gc/tidy.rs:499` reads only `'derived'` and is unaffected.

- [ ] **Step 6: Commit** `git commit -am "feat(work): a job's status ('task') is final over the live lift"`

---

### Task 3: Store — native items and job mirrors

**Files:**
- Create: `crates/fleet-core/src/store/work_tasks.rs`, `crates/fleet-core/src/store/work_tasks/tests.rs`
- Modify: `crates/fleet-core/src/store/mod.rs` (`mod work_tasks;` next to `mod work_local;` line 49; `pub use work_tasks::{job_status, NativeItem, PROPOSALS_OPEN_CAP, TASK_KEY_PREFIX};`)

**Interfaces:**
- Produces:
  - `pub const TASK_KEY_PREFIX: &str = "TASK";`
  - `pub fn job_status(state: &str) -> &'static str`
  - `pub struct NativeItem<'a> { title: &'a str, parent_id: Option<i64>, project_id: Option<i64>, notes: Option<&'a str> }`
  - `Store::create_native_item(&self, n: &NativeItem<'_>) -> Result<WorkItemRow, IpcError>`. It uses origin `manual`, checks depth, and emits `WorkItemUpdated`.
  - `Store::create_agent_task_item(&self, task: &TaskRow, parent_item_id: Option<i64>, project_id: Option<i64>) -> Result<WorkItemRow, IpcError>`
  - `Store::work_item_for_task(&self, task_id: i64) -> Result<Option<WorkItemRow>, IpcError>`
  - `Store::set_item_status_from_task(&self, item_id: i64, status: &str) -> Result<bool, IpcError>`
  - `Store::job_states_by_item(&self) -> Result<HashMap<i64, String>, IpcError>`
  - `Store::native_children(&self, parent_id: i64) -> Result<Vec<WorkItemRow>, IpcError>`, which returns every native child including proposals, oldest first.
  - `pub(super) fn parent_for_new_child(&self, parent_id: i64) -> Result<WorkItemRow, IpcError>`, the depth rule: `E_NOTFOUND` for an unknown parent, `E_INVALID` when the parent is a native subtask.

- [ ] **Step 1: Failing tests** (`store/work_tasks/tests.rs`):

```rust
use super::*;
use crate::ipc_error::codes;
use crate::store::Store;

fn native<'a>(title: &'a str) -> NativeItem<'a> {
    NativeItem { title, parent_id: None, project_id: None, notes: None }
}

#[test]
fn a_native_task_gets_a_task_key_its_project_and_notes() {
    let s = Store::open_in_memory().unwrap();
    let pid = s.upsert_project("acme", "api", "/src/api").unwrap();
    let t = s
        .create_native_item(&NativeItem { title: " Fix login ", parent_id: None, project_id: Some(pid), notes: Some("CI run 12") })
        .unwrap();
    assert_eq!((t.source.as_str(), t.origin.as_deref()), ("local", Some("manual")));
    assert_eq!(t.key.as_deref(), Some(format!("TASK-{}", t.id).as_str()));
    assert_eq!((t.title.as_str(), t.project_id, t.notes.as_deref()), ("Fix login", Some(pid), Some("CI run 12")));
    assert_eq!(t.status_category, "todo");
}

#[test]
fn a_subtask_hangs_under_a_ticket_but_never_under_a_subtask() {
    let s = Store::open_in_memory().unwrap();
    let ticket = s.create_local_work_item(Some("OM-110"), "Qomora").unwrap();
    let sub = s.create_native_item(&NativeItem { parent_id: Some(ticket.id), ..native("Stats") }).unwrap();
    assert_eq!(sub.parent_id, Some(ticket.id));
    let e = s.create_native_item(&NativeItem { parent_id: Some(sub.id), ..native("Deeper") }).unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    let e = s.create_native_item(&NativeItem { parent_id: Some(999_999), ..native("x") }).unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
    assert_eq!(s.native_children(ticket.id).unwrap().len(), 1);
}

#[test]
fn a_native_task_refuses_an_empty_title_and_an_unknown_project() {
    let s = Store::open_in_memory().unwrap();
    assert_eq!(s.create_native_item(&native("  ")).unwrap_err().code, codes::E_INVALID);
    assert_eq!(
        s.create_native_item(&NativeItem { project_id: Some(999_999), ..native("x") }).unwrap_err().code,
        codes::E_NOTFOUND
    );
}

#[test]
fn a_mirrored_job_is_an_agent_subtask_that_follows_the_job() {
    let s = Store::open_in_memory().unwrap();
    let parent = s.create_native_item(&native("Ship v1")).unwrap();
    let job = s.insert_task(None, None, "Write the changelog\nfrom git log", "n1").unwrap();
    let it = s.create_agent_task_item(&job, Some(parent.id), None).unwrap();
    assert_eq!((it.origin.as_deref(), it.task_id, it.parent_id), (Some("agent"), Some(job.id), Some(parent.id)));
    assert_eq!(it.title, "Write the changelog");
    assert_eq!((it.status_category.as_str(), it.status_set_by.as_deref()), ("todo", Some("task")));
    assert_eq!(s.create_agent_task_item(&job, None, None).unwrap().id, it.id, "once per job");
    s.mark_task_running(job.id).unwrap();
    assert_eq!(s.job_states_by_item().unwrap().get(&it.id).map(String::as_str), Some("running"));
}

#[test]
fn a_jobs_status_never_overrides_a_person() {
    let s = Store::open_in_memory().unwrap();
    let t = s.create_native_item(&native("x")).unwrap();
    assert!(s.set_item_status_from_task(t.id, "in_progress").unwrap());
    assert!(!s.set_item_status_from_task(t.id, "in_progress").unwrap(), "no repeat write");
    s.set_item_status(t.id, "done").unwrap();
    assert!(!s.set_item_status_from_task(t.id, "in_progress").unwrap());
    assert_eq!(s.get_work_item(t.id).unwrap().unwrap().status_set_by.as_deref(), Some("person"));
}

#[test]
fn the_job_status_map() {
    assert_eq!(job_status("queued"), "todo");
    assert_eq!(job_status("running"), "in_progress");
    for st in ["done", "failed", "cancelled"] {
        assert_eq!(job_status(st), "done");
    }
}
```

- [ ] **Step 2: Run** `cargo test -p fleet-core --lib work_tasks`. Expected: FAIL (module missing).

- [ ] **Step 3: Implement `store/work_tasks.rs`:**

```rust
//! Native work items (design 2026-09-29, shared work context): tasks and
//! subtasks written in fleet (`origin = 'manual'`), agent proposals
//! (`'proposed'`) and the mirror of a dispatched job (`'agent'`). Every one is
//! a local item with a `TASK-<id>` key, so the key-driven start path and
//! branch detection work on them unchanged. Depth is one: a native subtask
//! is never a parent.
//!
//! The only writers of `origin`, `project_id`, `notes`, `task_id` and the
//! proposal columns, and (with `work_status.rs`) of `status_set_by`:
//! `'task'` is written here only.

use super::work::{map_item, ITEM_COLUMNS};
use super::work_local::{validate_local_work_title, LOCAL_WORK_TITLE_MAX_CHARS};
use super::{now_unix, Store, TaskRow, WorkItemRow};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use std::collections::HashMap;

pub const TASK_KEY_PREFIX: &str = "TASK";
/// Open proposals one parent may hold (counter-review risk 2).
pub const PROPOSALS_OPEN_CAP: usize = 10;

/// A dispatched job's state as a work status.
pub fn job_status(state: &str) -> &'static str {
    match state {
        "queued" => "todo",
        "running" => "in_progress",
        _ => "done",
    }
}

/// What a person (or an agent's proposal) writes.
#[derive(Debug, Clone, Copy, Default)]
pub struct NativeItem<'a> {
    pub title: &'a str,
    pub parent_id: Option<i64>,
    pub project_id: Option<i64>,
    pub notes: Option<&'a str>,
}

pub(super) fn cut_brief(s: &str) -> String {
    s.chars().take(crate::service::work::handover::BRIEF_MAX_CHARS).collect()
}

fn job_title(prompt: &str) -> String {
    let first = prompt.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("Delegated job");
    first
        .chars()
        .filter(|c| !c.is_control())
        .take(LOCAL_WORK_TITLE_MAX_CHARS)
        .collect()
}

/// Everything `insert_native` writes.
pub(super) struct NativeRow<'a> {
    pub origin: &'a str,
    pub title: &'a str,
    pub parent_id: Option<i64>,
    pub project_id: Option<i64>,
    pub notes: Option<&'a str>,
    pub task_id: Option<i64>,
    pub status: &'a str,
    pub status_set_by: Option<&'a str>,
    pub proposal_state: Option<&'a str>,
    pub proposed_by: Option<&'a str>,
    pub proposal_why: Option<&'a str>,
}

impl Store {
    /// The parent a new native child may hang under.
    pub(super) fn parent_for_new_child(&self, parent_id: i64) -> Result<WorkItemRow, IpcError> {
        let p = self
            .get_work_item(parent_id)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("work item {parent_id} not found")))?;
        let native = matches!(p.origin.as_deref(), Some("manual" | "proposed" | "agent"));
        if native && p.parent_id.is_some() {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("{} is itself a subtask; add the subtask to its parent instead", p.key.as_deref().unwrap_or("that item")),
            ));
        }
        Ok(p)
    }

    fn check_project(&self, project_id: Option<i64>) -> Result<(), IpcError> {
        if let Some(pid) = project_id {
            let known: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
                [pid],
                |r| r.get(0),
            )?;
            if !known {
                return Err(IpcError::new(codes::E_NOTFOUND, format!("project {pid} not found")));
            }
        }
        Ok(())
    }

    /// Insert one native row and give it its `TASK-<id>` key, in one
    /// transaction; emits the row.
    pub(super) fn insert_native(&self, r: &NativeRow<'_>) -> Result<WorkItemRow, IpcError> {
        let now = now_unix();
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "INSERT INTO work_items (source, key, title, origin, parent_id, project_id, notes, task_id, \
                                     status_category, status_set_by, status_set_at, proposal_state, \
                                     proposed_by, proposal_why, created_at, updated_at) \
             VALUES ('local', NULL, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, \
                     CASE WHEN ?8 IS NULL THEN NULL ELSE ?12 END, ?9, ?10, ?11, ?12, ?12)",
            rusqlite::params![
                r.title, r.origin, r.parent_id, r.project_id, r.notes, r.task_id, r.status,
                r.status_set_by, r.proposal_state, r.proposed_by, r.proposal_why, now
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.conn.execute(
            "UPDATE work_items SET key = ?1 WHERE id = ?2",
            rusqlite::params![format!("{TASK_KEY_PREFIX}-{id}"), id],
        )?;
        tx.commit()?;
        self.emit_work_item(
            id,
            super::tracker_items::SessionChange { primary: false, suggested: false, rejected: false },
        )?;
        self.get_work_item(id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "work item vanished after insert"))
    }

    /// A task or subtask a person writes: `origin = 'manual'`, `todo`.
    pub fn create_native_item(&self, n: &NativeItem<'_>) -> Result<WorkItemRow, IpcError> {
        let title = validate_local_work_title(n.title)?;
        if let Some(p) = n.parent_id {
            self.parent_for_new_child(p)?;
        }
        self.check_project(n.project_id)?;
        let notes = n.notes.map(str::trim).filter(|x| !x.is_empty()).map(cut_brief);
        self.insert_native(&NativeRow {
            origin: "manual",
            title: &title,
            parent_id: n.parent_id,
            project_id: n.project_id,
            notes: notes.as_deref(),
            task_id: None,
            status: "todo",
            status_set_by: None,
            proposal_state: None,
            proposed_by: None,
            proposal_why: None,
        })
    }

    /// Mirror a dispatched job as an `agent` subtask, once per job.
    pub fn create_agent_task_item(
        &self,
        task: &TaskRow,
        parent_item_id: Option<i64>,
        project_id: Option<i64>,
    ) -> Result<WorkItemRow, IpcError> {
        if let Some(existing) = self.work_item_for_task(task.id)? {
            return Ok(existing);
        }
        let prompt = task.prompt.clone().unwrap_or_default();
        self.insert_native(&NativeRow {
            origin: "agent",
            title: &job_title(&prompt),
            parent_id: parent_item_id,
            project_id,
            notes: Some(&cut_brief(&prompt)),
            task_id: Some(task.id),
            status: job_status(&task.state),
            status_set_by: Some("task"),
            proposal_state: None,
            proposed_by: None,
            proposal_why: None,
        })
    }

    pub fn work_item_for_task(&self, task_id: i64) -> Result<Option<WorkItemRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {ITEM_COLUMNS} FROM work_items WHERE task_id = ?1"),
                [task_id],
                map_item,
            )
            .optional()?)
    }

    /// A job moved: its item follows unless a person set a status.
    pub fn set_item_status_from_task(&self, item_id: i64, status: &str) -> Result<bool, IpcError> {
        let now = now_unix();
        let wrote = self.conn.execute(
            "UPDATE work_items SET status_category = ?1, status_set_by = 'task', \
                    status_set_at = ?2, status_changed_at = ?2, updated_at = ?2 \
              WHERE id = ?3 AND source = 'local' \
                AND COALESCE(status_set_by, '') <> 'person' \
                AND NOT (status_category = ?1 AND status_set_by = 'task')",
            rusqlite::params![status, now, item_id],
        )? == 1;
        if wrote {
            self.emit_work_item(
                item_id,
                super::tracker_items::SessionChange { primary: true, suggested: false, rejected: false },
            )?;
        }
        Ok(wrote)
    }

    pub fn job_states_by_item(&self) -> Result<HashMap<i64, String>, IpcError> {
        let mut stmt = self
            .conn
            .prepare("SELECT w.id, t.state FROM work_items w JOIN tasks t ON t.id = w.task_id")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
    }

    /// Every native child of `parent_id`, proposals included, oldest first.
    pub fn native_children(&self, parent_id: i64) -> Result<Vec<WorkItemRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ITEM_COLUMNS} FROM work_items \
              WHERE parent_id = ?1 AND origin IN ('manual', 'proposed', 'agent') \
              ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map([parent_id], map_item)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests;
```

`TaskRow` (`store/rows.rs:1126`) has `prompt: Option<String>` and `state: String`. `emit_work_item` and `SessionChange` are `pub(super)` in `tracker_items.rs` and reachable from this sibling, as `work_status.rs` shows.

- [ ] **Step 4: Run** `cargo test -p fleet-core --lib work_tasks`. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/store/work_tasks.rs crates/fleet-core/src/store/work_tasks crates/fleet-core/src/store/mod.rs
git commit -m "feat(work): native tasks and subtasks (TASK-<id>), job mirrors, depth one"
```

---

### Task 4: Store — agent proposals

**Files:**
- Modify: `crates/fleet-core/src/store/work_tasks.rs`, `crates/fleet-core/src/store/work_tasks/tests.rs`

**Interfaces:**
- Consumes: `insert_native`, `parent_for_new_child`, `cut_brief` (Task 3).
- Produces:
  - `pub struct Proposal<'a> { parent_id: i64, title: &'a str, notes: Option<&'a str>, why: Option<&'a str>, proposed_by: &'a str }`
  - `Store::propose_subtask(&self, p: &Proposal<'_>) -> Result<WorkItemRow, IpcError>`
  - `Store::decide_proposal(&self, item_id: i64, accept: bool) -> Result<WorkItemRow, IpcError>`

- [ ] **Step 1: Failing tests** (append):

```rust
fn proposal<'a>(parent: i64, title: &'a str) -> Proposal<'a> {
    Proposal { parent_id: parent, title, notes: None, why: Some("because"), proposed_by: "OM-110 · trn" }
}

#[test]
fn a_proposal_waits_for_a_person_then_becomes_a_subtask() {
    let s = Store::open_in_memory().unwrap();
    let ticket = s.create_local_work_item(Some("OM-110"), "Qomora").unwrap();
    let p = s.propose_subtask(&proposal(ticket.id, "Decide the P0 owner")).unwrap();
    assert_eq!((p.origin.as_deref(), p.proposal_state.as_deref()), (Some("proposed"), Some("proposed")));
    assert_eq!((p.proposed_by.as_deref(), p.proposal_why.as_deref()), (Some("OM-110 · trn"), Some("because")));
    let a = s.decide_proposal(p.id, true).unwrap();
    assert_eq!((a.proposal_state.as_deref(), a.status_category.as_str()), (Some("accepted"), "todo"));
    assert_eq!(s.decide_proposal(p.id, false).unwrap_err().code, codes::E_INVALID, "decided once");
}

#[test]
fn a_rejected_title_is_not_proposed_again_under_the_same_parent() {
    let s = Store::open_in_memory().unwrap();
    let t = s.create_local_work_item(Some("OM-110"), "Qomora").unwrap();
    let p = s.propose_subtask(&proposal(t.id, "Create om-catalog module")).unwrap();
    s.decide_proposal(p.id, false).unwrap();
    let e = s.propose_subtask(&proposal(t.id, "create OM-CATALOG module ")).unwrap_err();
    assert_eq!(e.code, codes::E_EXISTS);
}

#[test]
fn open_proposals_are_capped_per_parent() {
    let s = Store::open_in_memory().unwrap();
    let t = s.create_local_work_item(Some("OM-110"), "Qomora").unwrap();
    for i in 0..PROPOSALS_OPEN_CAP {
        s.propose_subtask(&proposal(t.id, &format!("idea {i}"))).unwrap();
    }
    assert_eq!(s.propose_subtask(&proposal(t.id, "one too many")).unwrap_err().code, codes::E_LIMIT);
}

#[test]
fn deciding_something_that_is_not_a_proposal_is_refused() {
    let s = Store::open_in_memory().unwrap();
    let t = s.create_native_item(&native("plain")).unwrap();
    assert_eq!(s.decide_proposal(t.id, true).unwrap_err().code, codes::E_INVALID);
}
```

If `codes::E_LIMIT` does not exist (`git grep -n "pub const E_" crates/fleet-core/src/ipc_error.rs`), add `pub const E_LIMIT: &str = "E_LIMIT";` with a one-line doc ("a per-object limit was reached") next to the other codes.

- [ ] **Step 2: Run** `cargo test -p fleet-core --lib work_tasks`. Expected: FAIL.

- [ ] **Step 3: Implement** (in `work_tasks.rs`, inside `impl Store`, plus the struct above it):

```rust
/// An agent's proposed subtask.
#[derive(Debug, Clone, Copy)]
pub struct Proposal<'a> {
    pub parent_id: i64,
    pub title: &'a str,
    pub notes: Option<&'a str>,
    pub why: Option<&'a str>,
    pub proposed_by: &'a str,
}
```

```rust
    /// A subtask an agent proposes; a person accepts or rejects it.
    pub fn propose_subtask(&self, p: &Proposal<'_>) -> Result<WorkItemRow, IpcError> {
        let title = validate_local_work_title(p.title)?;
        self.parent_for_new_child(p.parent_id)?;
        let norm = |t: &str| t.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        let (open, rejected_same): (i64, bool) = {
            let siblings = self.native_children(p.parent_id)?;
            let open = siblings.iter().filter(|c| c.proposal_state.as_deref() == Some("proposed")).count() as i64;
            let same = siblings
                .iter()
                .any(|c| c.proposal_state.as_deref() == Some("rejected") && norm(&c.title) == norm(&title));
            (open, same)
        };
        if rejected_same {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!("\"{title}\" was already proposed here and rejected"),
            ));
        }
        if open as usize >= PROPOSALS_OPEN_CAP {
            return Err(IpcError::new(
                codes::E_LIMIT,
                format!("{PROPOSALS_OPEN_CAP} proposals already wait for a decision on this task"),
            ));
        }
        let notes = p.notes.map(str::trim).filter(|x| !x.is_empty()).map(cut_brief);
        let why = p.why.map(str::trim).filter(|x| !x.is_empty()).map(cut_brief);
        let by: String = p.proposed_by.chars().filter(|c| !c.is_control()).take(120).collect();
        self.insert_native(&NativeRow {
            origin: "proposed",
            title: &title,
            parent_id: Some(p.parent_id),
            project_id: None,
            notes: notes.as_deref(),
            task_id: None,
            status: "todo",
            status_set_by: None,
            proposal_state: Some("proposed"),
            proposed_by: Some(&by),
            proposal_why: why.as_deref(),
        })
    }

    /// A person decides a proposal, once.
    pub fn decide_proposal(&self, item_id: i64, accept: bool) -> Result<WorkItemRow, IpcError> {
        let now = now_unix();
        let wrote = self.conn.execute(
            "UPDATE work_items SET proposal_state = ?1, updated_at = ?2 \
              WHERE id = ?3 AND origin = 'proposed' AND proposal_state = 'proposed'",
            rusqlite::params![if accept { "accepted" } else { "rejected" }, now, item_id],
        )? == 1;
        if !wrote {
            return match self.get_work_item(item_id)? {
                None => Err(IpcError::new(codes::E_NOTFOUND, format!("work item {item_id} not found"))),
                Some(_) => Err(IpcError::new(codes::E_INVALID, "that item is not a proposal waiting for a decision")),
            };
        }
        self.emit_work_item(
            item_id,
            super::tracker_items::SessionChange { primary: false, suggested: false, rejected: !accept },
        )?;
        self.get_work_item(item_id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "work item vanished after update"))
    }
```

Export `Proposal` from `store/mod.rs` next to `NativeItem`.

- [ ] **Step 4: Run** `cargo test -p fleet-core --lib work_tasks`. Expected: PASS.

- [ ] **Step 5: Commit** `git commit -am "feat(work): agent proposals — capped, decided once, a rejected title stays rejected"`

---

### Task 5: `work_link` actions `create`, `propose`, `accept`, `reject`

**Files:**
- Modify: `crates/fleet-core/src/service/work/mod.rs` (`WorkLinkArgs` ~line 176; `WORK_LINK_ACTIONS` ~line 366)
- Modify: `crates/fleet-core/src/service/work/local.rs` (new fns after `rename_local_item`)
- Modify: `crates/fleet-core/src/mcp/tools/orchestration.rs` (before `if args.action == "name"` ~line 939)
- Test: `crates/fleet-core/src/service/work/local/tests.rs`, `crates/fleet-core/src/mcp/tools/tests_isolation.rs`

**Interfaces:**
- Consumes: `create_native_item`, `propose_subtask`, `decide_proposal` (Tasks 3–4); `local_item_visible` (existing); `tickets::item_visible` (`service/trackers/tickets.rs:118`, `pub(crate)`).
- Produces:
  - `local::create_task(args, store, scope) -> Result<WorkItemRow, IpcError>`
  - `local::propose(args, store, scope, proposer: &str) -> Result<WorkItemRow, IpcError>`
  - `local::decide(args, store, scope, accept: bool) -> Result<WorkItemRow, IpcError>`
  - `WorkLinkArgs.notes: Option<String>`, `WorkLinkArgs.parent: Option<String>` (`item:<id>`), `WorkLinkArgs.why: Option<String>`.

- [ ] **Step 1: Failing service tests** (append to `local/tests.rs`, adapting imports to the file's existing `use` lines):

```rust
fn args(action: &str) -> crate::service::work::WorkLinkArgs {
    crate::service::work::WorkLinkArgs { action: action.into(), ..Default::default() }
}

#[test]
fn an_unscoped_caller_creates_a_task_and_a_subtask() {
    let store = std::sync::Mutex::new(crate::store::Store::open_in_memory().unwrap());
    let all = crate::service::orgs::OrgScope::All;
    let t = super::create_task(&crate::service::work::WorkLinkArgs { title: Some("Release notes".into()), ..args("create") }, &store, &all).unwrap();
    let sub = super::create_task(
        &crate::service::work::WorkLinkArgs { title: Some("Changelog".into()), parent: Some(format!("item:{}", t.id)), ..args("create") },
        &store,
        &all,
    )
    .unwrap();
    assert_eq!(sub.parent_id, Some(t.id));
}

#[test]
fn a_host_token_may_not_create_a_standalone_task() {
    let s = crate::store::Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let scope = crate::service::orgs::OrgScope::for_host(&s, "h").unwrap();
    let store = std::sync::Mutex::new(s);
    let e = super::create_task(&crate::service::work::WorkLinkArgs { title: Some("x".into()), ..args("create") }, &store, &scope).unwrap_err();
    assert_eq!(e.code, crate::ipc_error::codes::E_FORBIDDEN);
}

#[test]
fn a_host_token_proposes_under_work_its_own_session_does_but_cannot_decide() {
    let s = crate::store::Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let sid = s.upsert_session("w", "h", None, None, 1, 1, "running", None).unwrap();
    let (item, _) = s.name_session_work(sid, Some("OPS-1"), "ops").unwrap();
    let scope = crate::service::orgs::OrgScope::for_host(&s, "h").unwrap();
    let store = std::sync::Mutex::new(s);
    let p = super::propose(
        &crate::service::work::WorkLinkArgs { title: Some("Add a test".into()), parent: Some(format!("item:{}", item.id)), why: Some("no coverage".into()), ..args("propose") },
        &store,
        &scope,
        "w · h",
    )
    .unwrap();
    let e = super::decide(&crate::service::work::WorkLinkArgs { item_id: Some(p.id), ..args("accept") }, &store, &scope, true).unwrap_err();
    assert_eq!(e.code, crate::ipc_error::codes::E_FORBIDDEN);
    let ok = super::decide(&crate::service::work::WorkLinkArgs { item_id: Some(p.id), ..args("accept") }, &store, &crate::service::orgs::OrgScope::All, true).unwrap();
    assert_eq!(ok.proposal_state.as_deref(), Some("accepted"));
}
```

`name_session_work` signature: check `store/work_local.rs:126` (`name_session_work_by`) and `status.rs`'s test helper, which calls `s.name_session_work(sid, None, "local work")`. Match that.

- [ ] **Step 2: Run** `cargo test -p fleet-core --lib service::work::local`. Expected: FAIL.

- [ ] **Step 3: `WorkLinkArgs`** (after `title`):

```rust
    /// create / propose: the parent task, `item:<id>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// create / propose: notes (the brief when started).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// propose: the agent's one-paragraph reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
```

`WORK_LINK_ACTIONS`: add `"create", "propose", "accept", "reject",` before `"name",`.

- [ ] **Step 4: Service fns** (`local.rs`):

```rust
/// `item:<id>` → the id.
fn parent_id(args: &WorkLinkArgs) -> Result<Option<i64>, IpcError> {
    match args.parent.as_deref() {
        None => Ok(None),
        Some(raw) => raw
            .strip_prefix("item:")
            .and_then(|n| n.parse::<i64>().ok())
            .map(Some)
            .ok_or_else(|| IpcError::new(codes::E_INVALID, "parent is item:<id>")),
    }
}

/// The parent a scoped caller may add under: an item it can see.
fn visible_parent(s: &Store, scope: &OrgScope, id: i64) -> Result<(), IpcError> {
    let Some(item) = s.get_work_item(id)? else {
        return Err(orgs::not_found("work item", id));
    };
    let visible = if item.source == "local" {
        local_item_visible(s, scope, id)?
    } else {
        crate::service::trackers::tickets::item_visible(scope, s, &item)?
    };
    if visible { Ok(()) } else { Err(orgs::not_found("work item", id)) }
}

/// `work_link { action: create, title, parent?, project_id?, notes? }`.
/// A standalone task needs an unscoped caller (a new item has no links, and
/// a scoped caller sees a local item only through its links); a subtask
/// needs a parent the caller sees.
pub fn create_task(args: &WorkLinkArgs, store: &Mutex<Store>, scope: &OrgScope) -> Result<WorkItemRow, IpcError> {
    let title = args.title.as_deref().ok_or_else(|| IpcError::new(codes::E_INVALID, "create needs title"))?;
    let parent = parent_id(args)?;
    let s = lock(store)?;
    match parent {
        None if !scope.is_all() => {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                "a standalone task needs an unscoped caller (the desktop or the master token); add a subtask under a task you work on instead",
            ))
        }
        Some(p) if !scope.is_all() => visible_parent(&s, scope, p)?,
        _ => {}
    }
    s.create_native_item(&crate::store::NativeItem {
        title,
        parent_id: parent,
        project_id: args.project_id,
        notes: args.notes.as_deref(),
    })
}

/// `work_link { action: propose, parent, title, notes?, why? }`: a subtask
/// for a person to accept or reject.
pub fn propose(args: &WorkLinkArgs, store: &Mutex<Store>, scope: &OrgScope, proposer: &str) -> Result<WorkItemRow, IpcError> {
    let title = args.title.as_deref().ok_or_else(|| IpcError::new(codes::E_INVALID, "propose needs title"))?;
    let parent = parent_id(args)?.ok_or_else(|| IpcError::new(codes::E_INVALID, "propose needs parent"))?;
    let s = lock(store)?;
    if !scope.is_all() {
        visible_parent(&s, scope, parent)?;
    }
    s.propose_subtask(&crate::store::Proposal {
        parent_id: parent,
        title,
        notes: args.notes.as_deref(),
        why: args.why.as_deref(),
        proposed_by: proposer,
    })
}

/// `work_link { action: accept | reject, item_id }`: a person's decision.
/// Refused to per-host tokens and bound clients: an agent never accepts
/// its own (or any) proposal.
pub fn decide(args: &WorkLinkArgs, store: &Mutex<Store>, scope: &OrgScope, accept: bool) -> Result<WorkItemRow, IpcError> {
    if !scope.is_all() {
        return Err(IpcError::new(codes::E_FORBIDDEN, "a person decides proposals, from the desktop or the master token"));
    }
    let id = args.item_id.ok_or_else(|| IpcError::new(codes::E_INVALID, "accept / reject needs item_id"))?;
    lock(store)?.decide_proposal(id, accept)
}
```

Whether a `ClientFull` paired client is `is_all()` is decided by `OrgScope`. The isolation matrix in Step 6 shows the truth; follow it.

- [ ] **Step 5: Dispatch** in `orchestration.rs`, before `if args.action == "name" {`:

```rust
        // Shared work context (design 2026-09-29): native tasks, subtasks
        // and agent proposals. The scope gates are inside.
        if args.action == "create" {
            return ok_json(&crate::service::work::local::create_task(&args, &self.store, &scope).map_err(to_mcp_err)?);
        }
        if args.action == "propose" {
            let proposer = args
                .session_id
                .and_then(|sid| self.store.lock().ok()?.get_session_by_id(sid).ok().flatten())
                .map(|r| format!("{} · {}", r.friendly_name.clone().unwrap_or(r.tmux_name.clone()), r.host_alias))
                .unwrap_or_else(|| caller.label());
            return ok_json(
                &crate::service::work::local::propose(&args, &self.store, &scope, &proposer).map_err(to_mcp_err)?,
            );
        }
        if args.action == "accept" || args.action == "reject" {
            return ok_json(
                &crate::service::work::local::decide(&args, &self.store, &scope, args.action == "accept")
                    .map_err(to_mcp_err)?,
            );
        }
```

`caller.label()` exists (used in `cancel_task`). The store lock in the `propose` branch is released before `propose` locks again, because the temporary guard drops at the end of the expression.

- [ ] **Step 6: Isolation-matrix rows** (next to the `set_status` rows ~line 2320). The fixture has host A's local item `local_a` and org B's ticket `fx.item_b`:

```rust
    m.row(
        "work_link",
        "create",
        |_, _| json!({ "action": "create", "title": "matrix task" }),
        |_, who, a| {
            if readonly_refused(who, a) { return; }
            match who {
                Who::Master | Who::ClientFull => assert!(text(a).contains("\"origin\":\"manual\""), "{who:?}: {a:?}"),
                _ => is_code(who, a, "E_FORBIDDEN", "a standalone task needs an unscoped caller"),
            }
        },
    )
    .await;
    m.row(
        "work_link",
        "propose",
        move |_, _| json!({ "action": "propose", "parent": format!("item:{local_a}"), "title": "matrix idea", "why": "x" }),
        move |_, who, a| {
            if readonly_refused(who, a) { return; }
            match who {
                Who::HostB | Who::HostNone | Who::BoundB => is_code(who, a, "E_NOTFOUND", "another org's parent"),
                _ => assert!(text(a).contains("\"proposal_state\":\"proposed\""), "{who:?}: {a:?}"),
            }
        },
    )
    .await;
    for action in ["accept", "reject"] {
        m.row(
            "work_link",
            action,
            move |_, _| json!({ "action": action, "item_id": 999_999 }),
            |_, who, a| {
                if readonly_refused(who, a) { return; }
                match who {
                    Who::Master | Who::ClientFull => is_code(who, a, "E_NOTFOUND", "unknown proposal"),
                    _ => is_code(who, a, "E_FORBIDDEN", "a person decides"),
                }
            },
        )
        .await;
    }
```

Before editing expectations, read one failing run's output. The matrix decides which `Who` counts as unscoped, and the rows then encode it.

- [ ] **Step 7: Run** `cargo test -p fleet-core --lib service::work::local && cargo test -p fleet-core --lib isolation_matrix`. Expected: PASS, including the coverage check. Then `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` (`mcp/doc_gen.rs:149`), and run it again without the variable to confirm it passes.

- [ ] **Step 8: Commit**

```bash
git add crates/fleet-core/src docs/control-api-reference.md
git commit -m "feat(work): work_link create / propose / accept / reject"
```

---

### Task 6: Start fills in a native item's project and brief

**Files:**
- Modify: `crates/fleet-core/src/service/trackers/tickets.rs` (`start_work` ~line 1256, `resolve_start` :640)
- Test: the file holding the existing `start_work` tests (`git grep -ln "fn .*start_work" crates/fleet-core/src/service/trackers`)

**Interfaces:**
- Produces: `fn with_native_defaults(store: &Mutex<Store>, args: &StartArgs) -> Result<StartArgs, IpcError>` (private). `resolve_start` refuses a proposal that is not accepted.

- [ ] **Step 1: Failing tests:**

```rust
#[test]
fn a_subtask_starts_in_its_project_with_the_ticket_brief_then_its_own() {
    let s = crate::store::Store::open_in_memory().unwrap();
    let pid = s.upsert_project("acme", "api", "/src/api").unwrap();
    let ticket = s.create_local_work_item(Some("OM-110"), "Qomora harmonization").unwrap();
    let sub = s
        .create_native_item(&crate::store::NativeItem { title: "SELECT stats", parent_id: Some(ticket.id), project_id: Some(pid), notes: Some("suppliers, shared EANs") })
        .unwrap();
    let store = std::sync::Mutex::new(s);
    let got = with_native_defaults(&store, &StartArgs { item_id: Some(sub.id), ..Default::default() }).unwrap();
    assert_eq!(got.project_id, Some(pid));
    let brief = got.brief.unwrap();
    assert!(brief.contains("OM-110 Qomora harmonization"), "{brief}");
    assert!(brief.contains("## Subtask"), "{brief}");
    assert!(brief.ends_with("SELECT stats\n\nsuppliers, shared EANs"), "{brief}");
    let mine = with_native_defaults(&store, &StartArgs { item_id: Some(sub.id), project_id: Some(7), brief: Some("mine".into()), ..Default::default() }).unwrap();
    assert_eq!((mine.project_id, mine.brief.as_deref()), (Some(7), Some("mine")));
}

#[tokio::test]
async fn an_unaccepted_proposal_cannot_be_started() {
    let s = crate::store::Store::open_in_memory().unwrap();
    let t = s.create_local_work_item(Some("OM-110"), "Qomora").unwrap();
    let p = s.propose_subtask(&crate::store::Proposal { parent_id: t.id, title: "idea", notes: None, why: None, proposed_by: "x" }).unwrap();
    let store = std::sync::Mutex::new(s);
    let e = resolve_start(&store, &StartArgs { item_id: Some(p.id), ..Default::default() }, &OrgScope::All, &default_net())
        .await
        .unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
}
```

`default_net()` is what `routed::start_work` uses (`service::trackers::default_net`). Import it in the test module the way the file's other async tests do.

- [ ] **Step 2: Run.** Expected: FAIL.

- [ ] **Step 3: Implement** (above `start_work`):

```rust
/// A native item (design 2026-09-29) starts where it belongs and with what
/// was written: an unset project comes from the item, else its parent's;
/// an unset brief is the parent's (a ticket's own brief, fenced as every
/// start brief is) followed by the item's title and notes. Anything else
/// starts exactly as asked.
fn with_native_defaults(store: &Mutex<Store>, args: &StartArgs) -> Result<StartArgs, IpcError> {
    let Some(id) = args.item_id else { return Ok(args.clone()) };
    let s = lock(store)?;
    let Some(item) = s.get_work_item(id)? else { return Ok(args.clone()) };
    if !matches!(item.origin.as_deref(), Some("manual" | "proposed" | "agent")) {
        return Ok(args.clone());
    }
    let parent = item.parent_id.map(|p| s.get_work_item(p)).transpose()?.flatten();
    let mut out = args.clone();
    if out.project_id.is_none() {
        out.project_id = item.project_id.or_else(|| parent.as_ref().and_then(|p| p.project_id));
    }
    if out.brief.is_none() {
        let own = match item.notes.as_deref().filter(|n| !n.trim().is_empty()) {
            Some(n) => format!("{}\n\n{n}", item.title),
            None => item.title.clone(),
        };
        out.brief = Some(match &parent {
            Some(p) => {
                let head = format!("{} {}", p.key.as_deref().unwrap_or_default(), p.title);
                let desc = s
                    .work_item_meta(p.id)?
                    .description
                    .filter(|d| !d.trim().is_empty())
                    .map(|d| crate::mcp::guard::fence_untrusted(&d, "a tracker ticket", crate::service::work::view::DESCRIPTION_MAX_CHARS))
                    .unwrap_or_default();
                let brief = format!("## Task {head}\n\n{desc}\n\n## Subtask {}\n\n{own}", item.key.as_deref().unwrap_or_default());
                brief.chars().take(crate::service::work::handover::BRIEF_MAX_CHARS).collect()
            }
            None => own,
        });
    }
    Ok(out)
}
```

Adjust the test's `ends_with` expectation if the `## Subtask` line format differs. The contract is: parent heading, fenced parent description, `## Subtask <key>`, then title and notes.

In `resolve_start`'s `(Some(id), None)` arm, right after the visibility check:

```rust
            if item.origin.as_deref() == Some("proposed") && item.proposal_state.as_deref() != Some("accepted") {
                return Err(IpcError::new(codes::E_INVALID, "accept the proposal first"));
            }
```

At the top of `start_work`: `let args = &with_native_defaults(store, args)?;`. The lock inside the helper is dropped before `plan_start` locks again.

- [ ] **Step 4: Run** `cargo test -p fleet-core --lib service::trackers`. Expected: PASS.

- [ ] **Step 5: Commit** `git commit -am "feat(work): starting a native item uses its project and the parent ticket's brief"`

---

### Task 7: Mirror dispatched jobs

**Files:**
- Modify: `crates/fleet-core/src/service/tasks.rs` (`start_task` :361, `fail_task` :372, `cancel_task` :385, `complete_task` :403; new fns after `note_finished` :425)
- Modify: `crates/fleet-core/src/mcp/tools/orchestration.rs:446-459`
- Test: `service/tasks.rs` `mod tests` (its `seed(s, host, name)` helper at ~line 822)

**Interfaces:**
- Produces: `pub fn mirror_dispatched(s: &Store, task: &TaskRow, requester: Option<i64>, worker: i64)`, `fn mirror_state(s: &Store, task_id: i64)`. Both are infallible and log only.

- [ ] **Step 1: Failing tests:**

```rust
    #[test]
    fn a_dispatched_job_is_a_subtask_of_the_requesters_task_and_follows_the_job() {
        let s = Store::open_in_memory().unwrap();
        let req = seed(&s, "local", "ctl");
        let w = seed(&s, "local", "w");
        let parent = s.create_native_item(&crate::store::NativeItem { title: "Ship v1", ..Default::default() }).unwrap();
        s.link_session_work(req, crate::store::WorkTarget::Item(parent.id), "manual").unwrap();
        let t = create_task(&s, Some(req), Some(w), "Write the changelog").unwrap();
        mirror_dispatched(&s, &t, Some(req), w);
        let it = s.work_item_for_task(t.id).unwrap().expect("mirrored");
        assert_eq!(it.parent_id, Some(parent.id));
        assert!(s.session_work_links(w).unwrap().iter().any(|l| l.item_id == Some(it.id) && l.state == "confirmed" && !l.is_primary));
        let t = start_task(&s, &t).unwrap();
        assert_eq!(s.get_work_item(it.id).unwrap().unwrap().status_category, "in_progress");
        assert!(complete_task(&s, &t, "done: CHANGELOG.md").unwrap());
        assert_eq!(s.get_work_item(it.id).unwrap().unwrap().status_category, "done");
    }

    #[test]
    fn a_requester_on_a_subtask_parents_the_job_to_that_subtasks_parent() {
        let s = Store::open_in_memory().unwrap();
        let req = seed(&s, "local", "ctl");
        let w = seed(&s, "local", "w");
        let ticket = s.create_local_work_item(Some("OM-110"), "Qomora").unwrap();
        let sub = s.create_native_item(&crate::store::NativeItem { title: "Stats", parent_id: Some(ticket.id), ..Default::default() }).unwrap();
        s.link_session_work(req, crate::store::WorkTarget::Item(sub.id), "manual").unwrap();
        let t = create_task(&s, Some(req), Some(w), "Run the SELECTs").unwrap();
        mirror_dispatched(&s, &t, Some(req), w);
        assert_eq!(s.work_item_for_task(t.id).unwrap().unwrap().parent_id, Some(ticket.id), "depth stays one");
    }

    #[test]
    fn failed_and_cancelled_jobs_are_done_and_an_unmirrored_job_still_moves() {
        let s = Store::open_in_memory().unwrap();
        let w = seed(&s, "local", "w");
        let f = create_task(&s, None, Some(w), "a").unwrap();
        mirror_dispatched(&s, &f, None, w);
        fail_task(&s, f.id, "send failed").unwrap();
        assert_eq!(s.work_item_for_task(f.id).unwrap().unwrap().status_category, "done");
        let plain = create_task(&s, None, Some(w), "b").unwrap();
        let plain = start_task(&s, &plain).unwrap();
        assert!(complete_task(&s, &plain, "ok").unwrap());
        assert!(s.work_item_for_task(plain.id).unwrap().is_none());
    }
```

`NativeItem` derives `Default` (Task 3), so `..Default::default()` works.

- [ ] **Step 2: Run** `cargo test -p fleet-core --lib service::tasks`. Expected: FAIL.

- [ ] **Step 3: Implement** after `note_finished`:

```rust
/// Shared work context (design 2026-09-29): a dispatched job becomes an
/// `agent` subtask of the requester's primary work — or of that work's
/// parent when the requester is on a native subtask (depth one) — with the
/// worker linked SECONDARY, so the primary `inherit_worker_work` gave it
/// stays. Best-effort: the dispatch already happened.
pub fn mirror_dispatched(s: &Store, task: &TaskRow, requester: Option<i64>, worker: i64) {
    let run = || -> Result<(), IpcError> {
        let primary = match requester {
            Some(r) => s
                .session_work_links(r)?
                .into_iter()
                .find(|l| l.is_primary && l.state == "confirmed" && l.ended_at.is_none())
                .and_then(|l| l.item_id),
            None => None,
        };
        let parent = match primary.map(|id| s.get_work_item(id)).transpose()?.flatten() {
            Some(p) if matches!(p.origin.as_deref(), Some("manual" | "proposed" | "agent")) && p.parent_id.is_some() => p.parent_id,
            Some(p) => Some(p.id),
            None => None,
        };
        let project = s.get_session_by_id(worker)?.and_then(|r| r.project_id);
        let item = s.create_agent_task_item(task, parent, project)?;
        s.link_session_work_as(worker, crate::store::WorkTarget::Item(item.id), "agent_started", false, None)?;
        Ok(())
    };
    if let Err(e) = run() {
        tracing::debug!(task = task.id, error = %e.message, "[tasks] mirroring a job into work failed");
    }
}

fn mirror_state(s: &Store, task_id: i64) {
    let run = || -> Result<(), IpcError> {
        let (Some(task), Some(item)) = (s.get_task(task_id)?, s.work_item_for_task(task_id)?) else {
            return Ok(());
        };
        s.set_item_status_from_task(item.id, crate::store::job_status(&task.state))?;
        Ok(())
    };
    if let Err(e) = run() {
        tracing::debug!(task = task_id, error = %e.message, "[tasks] mirroring a job's state failed");
    }
}
```

Wiring:
- `start_task`: after `let row = …?;` add `mirror_state(s, row.id);`.
- `fail_task`: inside `if changed {` add `mirror_state(s, task_id);`.
- `cancel_task`: after `note_finished(…)` add `mirror_state(s, row.id);`.
- `complete_task`: at the top of `if let Some(ref r) = row {` add `mirror_state(s, r.id);`.
- `orchestration.rs`: inside `let task = { … }`, after the `if let Some(req) = p.requester_session_id { … }` block, add `tasks::mirror_dispatched(&s, &task, p.requester_session_id, worker.id);`.

- [ ] **Step 4: Run** `cargo test -p fleet-core --lib service::tasks && cargo test -p fleet-core --lib dispatch`. Expected: PASS.

- [ ] **Step 5: Commit** `git commit -am "feat(work): dispatched jobs appear as agent subtasks and follow the job"`

---

### Task 8: Steps — the adapter, the journal kind, the store writer

**Files:**
- Create: `crates/fleet-core/src/service/work/steps.rs` (register `pub mod steps;` in `service/work/mod.rs`)
- Create: `crates/fleet-core/src/service/work/testdata/steps/task_create.json`, `task_update.json`, `todo_write.json`
- Modify: `crates/fleet-core/src/store/work_journal.rs` (`JOURNAL_KINDS` :21, `cap_kind` :94, new fns)

**Interfaces:**
- Produces:
  - `pub enum StepState { Pending, InProgress, Completed, Cancelled }` with `as_str()` / `parse(&str) -> Option<Self>`;
  - `pub struct StepEvent { native_id: String, text: Option<String>, state: Option<StepState>, agent: &'static str }`. For `TaskUpdate` without `subject`, `text` is `None`, meaning "keep the known text". For an update without `status`, `state` is `None`;
  - `pub fn steps_from_claude_tool(tool: &str, input: &Value, response: Option<&Value>) -> Vec<StepEvent>`;
  - `pub const STEP_CAP: usize = 200; pub const STEP_TEXT_MAX_CHARS: usize = 300;`;
  - `Store::record_steps(&self, claude_session_id: &str, participant_id: Option<i64>, source: &str, events: &[StepEvent]) -> Result<usize, IpcError>`, which returns how many rows it wrote;
  - `pub struct StepView { native_id, text, state, at, claude_session_id }` and `Store::current_steps(&self, conversation_ids: &[String]) -> Result<Vec<StepView>, IpcError>` (the newest state per `(conversation, native_id)`, ordered by first appearance).

- [ ] **Step 1: Fixtures** (copied from real transcripts, shapes verified 2026-09-29).

`task_create.json`:

```json
{ "input": { "subject": "Design mandatory-test-coverage feature", "description": "Decide the approach", "activeForm": "Designing mandatory-test-coverage feature" },
  "response": { "task": { "id": "1", "subject": "Design mandatory-test-coverage feature" } } }
```

`task_update.json`:

```json
{ "input": { "taskId": "1", "status": "in_progress" } }
```

`todo_write.json`:

```json
{ "input": { "todos": [
  { "content": "Read OM-110", "status": "completed", "activeForm": "Reading OM-110" },
  { "content": "Write the synthesis", "status": "in_progress", "activeForm": "Writing the synthesis" },
  { "content": "Prepare SELECTs", "status": "pending", "activeForm": "Preparing SELECTs" } ] } }
```

- [ ] **Step 2: Failing tests** (bottom of `steps.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    fn fx(name: &str) -> serde_json::Value {
        let p = format!("{}/src/service/work/testdata/steps/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    #[test]
    fn task_create_takes_its_id_from_the_response() {
        let f = fx("task_create.json");
        let e = steps_from_claude_tool("TaskCreate", &f["input"], Some(&f["response"]));
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].native_id, "task:1");
        assert_eq!(e[0].text.as_deref(), Some("Design mandatory-test-coverage feature"));
        assert_eq!(e[0].state, Some(StepState::Pending));
    }

    #[test]
    fn task_update_moves_the_state_and_keeps_the_text() {
        let f = fx("task_update.json");
        let e = steps_from_claude_tool("TaskUpdate", &f["input"], None);
        assert_eq!((e[0].native_id.as_str(), e[0].text.as_deref(), e[0].state), ("task:1", None, Some(StepState::InProgress)));
        let alt = steps_from_claude_tool("TaskUpdate", &serde_json::json!({"task_id": "2", "status": "completed"}), None);
        assert_eq!(alt[0].native_id, "task:2");
    }

    #[test]
    fn todo_write_is_one_event_per_item_keyed_by_its_text() {
        let f = fx("todo_write.json");
        let e = steps_from_claude_tool("TodoWrite", &f["input"], None);
        assert_eq!(e.len(), 3);
        assert_eq!(e[1].native_id, "todo:write the synthesis");
        assert_eq!(e[1].state, Some(StepState::InProgress));
    }

    #[test]
    fn a_task_create_without_an_id_and_other_tools_yield_nothing() {
        let f = fx("task_create.json");
        assert!(steps_from_claude_tool("TaskCreate", &f["input"], None).is_empty());
        assert!(steps_from_claude_tool("Bash", &serde_json::json!({"command": "ls"}), None).is_empty());
    }

    #[test]
    fn step_text_is_capped_and_stripped_of_control_characters() {
        let long = format!("a\u{7}{}", "x".repeat(400));
        let e = steps_from_claude_tool("TodoWrite", &serde_json::json!({"todos": [{"content": long, "status": "pending"}]}), None);
        let t = e[0].text.clone().unwrap();
        assert_eq!(t.chars().count(), STEP_TEXT_MAX_CHARS);
        assert!(!t.contains('\u{7}'));
    }
}
```

Store tests (append to the `work_journal.rs` tests module):

```rust
    #[test]
    fn steps_record_only_changes_and_read_back_their_newest_state() {
        use crate::service::work::steps::{StepEvent, StepState};
        let s = Store::open_in_memory().unwrap();
        let ev = |id: &str, text: Option<&str>, st: StepState| StepEvent { native_id: id.into(), text: text.map(str::to_string), state: Some(st), agent: "claude_code" };
        assert_eq!(s.record_steps("c1", None, "hook", &[ev("task:1", Some("Read OM-110"), StepState::Pending)]).unwrap(), 1);
        assert_eq!(s.record_steps("c1", None, "hook", &[ev("task:1", Some("Read OM-110"), StepState::Pending)]).unwrap(), 0, "no change, no row");
        assert_eq!(s.record_steps("c1", None, "hook", &[ev("task:1", None, StepState::Completed)]).unwrap(), 1);
        let cur = s.current_steps(&["c1".to_string()]).unwrap();
        assert_eq!(cur.len(), 1);
        assert_eq!((cur[0].text.as_str(), cur[0].state.as_str()), ("Read OM-110", "completed"));
    }

    #[test]
    fn steps_are_capped_per_conversation() {
        use crate::service::work::steps::{StepEvent, StepState, STEP_CAP};
        let s = Store::open_in_memory().unwrap();
        for i in 0..(STEP_CAP + 5) {
            s.record_steps("c1", None, "hook", &[StepEvent { native_id: format!("todo:{i}"), text: Some(format!("s{i}")), state: Some(StepState::Pending), agent: "claude_code" }]).unwrap();
        }
        let n: i64 = s.conn.query_row("SELECT COUNT(*) FROM work_journal WHERE kind = 'step'", [], |r| r.get(0)).unwrap();
        assert_eq!(n as usize, STEP_CAP);
    }
```

- [ ] **Step 3: Run** `cargo test -p fleet-core --lib steps`. Expected: FAIL.

- [ ] **Step 4: Implement `service/work/steps.rs`:**

```rust
//! Agent steps (design 2026-09-29 §2): the transient level under a task —
//! an agent's own todos, captured, never typed. One adapter per agent turns
//! its native events into [`StepEvent`]s; storage, the view and the UI see
//! only those. Claude Code is the first adapter (`TaskCreate`,
//! `TaskUpdate`, legacy `TodoWrite`; shapes verified 2026-09-29). A step's
//! `completed` is the agent's word, never a task status and never evidence.

use serde_json::Value;

pub const STEP_CAP: usize = 200;
pub const STEP_TEXT_MAX_CHARS: usize = 300;
/// The tools whose PostToolUse carries steps (the hook matcher adds these).
pub const CLAUDE_STEP_TOOLS: &[&str] = &["TaskCreate", "TaskUpdate", "TodoWrite"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState { Pending, InProgress, Completed, Cancelled }

impl StepState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            "cancelled" | "deleted" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepEvent {
    pub native_id: String,
    /// `None`: keep the text already known for this step.
    pub text: Option<String>,
    /// `None`: keep the state already known.
    pub state: Option<StepState>,
    pub agent: &'static str,
}

fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(STEP_TEXT_MAX_CHARS).collect::<String>().trim().to_string()
}

/// Claude Code's adapter.
pub fn steps_from_claude_tool(tool: &str, input: &Value, response: Option<&Value>) -> Vec<StepEvent> {
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    match tool {
        "TaskCreate" => {
            let Some(id) = response.and_then(|r| r.pointer("/task/id")).and_then(|v| {
                v.as_str().map(str::to_string).or_else(|| v.as_i64().map(|n| n.to_string()))
            }) else {
                return Vec::new();
            };
            vec![StepEvent {
                native_id: format!("task:{id}"),
                text: s(input, "subject").map(|t| clean(&t)).filter(|t| !t.is_empty()),
                state: Some(StepState::Pending),
                agent: "claude_code",
            }]
        }
        "TaskUpdate" => {
            let Some(id) = s(input, "taskId").or_else(|| s(input, "task_id")) else { return Vec::new() };
            vec![StepEvent {
                native_id: format!("task:{id}"),
                text: s(input, "subject").map(|t| clean(&t)).filter(|t| !t.is_empty()),
                state: s(input, "status").and_then(|st| StepState::parse(&st)),
                agent: "claude_code",
            }]
        }
        "TodoWrite" => input
            .get("todos")
            .and_then(Value::as_array)
            .map(|todos| {
                todos
                    .iter()
                    .filter_map(|t| {
                        let text = clean(&s(t, "content")?);
                        (!text.is_empty()).then(|| StepEvent {
                            native_id: format!("todo:{}", text.to_lowercase()),
                            text: Some(text),
                            state: s(t, "status").and_then(|st| StepState::parse(&st)),
                            agent: "claude_code",
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}
```

In `work_journal.rs`:
- Add `"step",` to `JOURNAL_KINDS` with the comment `// An agent's own todo/task step (design 2026-09-29 §2); meta {native_id, state, agent}.`
- Add `"step" => Some(crate::service::work::steps::STEP_CAP),` to `cap_kind`.
- Then add:

```rust
/// One step's newest state in a conversation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StepView {
    pub claude_session_id: String,
    pub native_id: String,
    pub text: String,
    pub state: String,
    pub at: i64,
}

impl Store {
    /// Append the events that change a step's known text or state; each
    /// becomes one `step` row (body = text, meta = {native_id, state, agent}).
    pub fn record_steps(
        &self,
        claude_session_id: &str,
        participant_id: Option<i64>,
        source: &str,
        events: &[crate::service::work::steps::StepEvent],
    ) -> Result<usize, IpcError> {
        let known: std::collections::HashMap<String, StepView> = self
            .current_steps(&[claude_session_id.to_string()])?
            .into_iter()
            .map(|v| (v.native_id.clone(), v))
            .collect();
        let mut wrote = 0;
        for e in events {
            let prev = known.get(&e.native_id);
            let text = e.text.clone().or_else(|| prev.map(|p| p.text.clone()));
            let Some(text) = text else { continue };
            let state = e.state.map(|s| s.as_str().to_string()).or_else(|| prev.map(|p| p.state.clone())).unwrap_or_else(|| "pending".into());
            if prev.is_some_and(|p| p.text == text && p.state == state) {
                continue;
            }
            let meta = serde_json::json!({ "native_id": e.native_id, "state": state, "agent": e.agent }).to_string();
            if self.append_journal(Some(claude_session_id), participant_id, "step", source, Some(&text), Some(&meta))?.is_some() {
                wrote += 1;
            }
        }
        Ok(wrote)
    }

    /// The newest state of every step in these conversations, in order of
    /// first appearance.
    pub fn current_steps(&self, conversation_ids: &[String]) -> Result<Vec<StepView>, IpcError> {
        if conversation_ids.is_empty() {
            return Ok(Vec::new());
        }
        let json = serde_json::to_string(conversation_ids).map_err(|e| IpcError::new(codes::E_INTERNAL, e.to_string()))?;
        let mut stmt = self.conn.prepare(
            "SELECT claude_session_id, json_extract(meta, '$.native_id') AS nid, body, \
                    json_extract(meta, '$.state'), at, id \
               FROM work_journal \
              WHERE kind = 'step' AND claude_session_id IN (SELECT value FROM json_each(?1)) \
              ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![json], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, String>(3)?, r.get::<_, i64>(4)?))
        })?;
        let mut order: Vec<(String, String)> = Vec::new();
        let mut last: std::collections::HashMap<(String, String), StepView> = std::collections::HashMap::new();
        for row in rows {
            let (conv, nid, body, state, at) = row?;
            let k = (conv.clone(), nid.clone());
            if !last.contains_key(&k) {
                order.push(k.clone());
            }
            last.insert(k, StepView { claude_session_id: conv, native_id: nid, text: body.unwrap_or_default(), state, at });
        }
        Ok(order.into_iter().filter_map(|k| last.remove(&k)).collect())
    }
}
```

`append_journal`'s "exact repeat of the newest row" skip applies only to `progress` and `compact_summary` (check its body). If it also skips `step` rows, `record_steps`' own change check already guarantees a difference, so it is harmless. Keep the cap enforcement it applies through `cap_kind`.

Export `StepView` from `store/mod.rs`.

- [ ] **Step 5: Run** `cargo test -p fleet-core --lib steps && cargo test -p fleet-core --lib work_journal`. Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/src/service/work/steps.rs crates/fleet-core/src/service/work/testdata/steps crates/fleet-core/src/service/work/mod.rs crates/fleet-core/src/store/work_journal.rs crates/fleet-core/src/store/mod.rs
git commit -m "feat(work): agent steps — Claude Code adapter and the journal's step kind"
```

---

### Task 9: Capture steps from the hook, with a transcript backstop

**Files:**
- Modify: `crates/fleet-core/src/service/hooks_install.rs:82` (matcher) and its tests / rendering pin
- Modify: `crates/fleet-core/src/service/hooks.rs` (`apply_hook` :50-80; new `apply_step_hook`; the Stop hook's background follow-ups ~line 988)
- Modify: `crates/fleet-core/src/service/work/steps.rs` (`steps_from_transcript`), `crates/fleet-core/src/service/work/harvest.rs` (`spawn_harvest_steps`)

**Interfaces:**
- Consumes: `resolve_and_rebind` and `Binding::Current` (existing, see `apply_pre_compact_hook` :926); `record_steps` (Task 8); `transcript::resolve_args_for` and `read_tail_bytes` (as `harvest_compact_summary` uses them).
- Produces:
  - `WORKTREE_TOOL_MATCHER` renamed to `POST_TOOL_MATCHER = "EnterWorktree|ExitWorktree|TaskCreate|TaskUpdate|TodoWrite"`;
  - `pub fn steps_from_transcript(jsonl: &str) -> Vec<StepEvent>`;
  - `pub fn spawn_harvest_steps(store, ssh, row_id, claude_session_id)`.

- [ ] **Step 1: Failing tests.** In `hooks.rs` tests, follow the style of the existing `PostToolUse`/`EnterWorktree` tests (~line 2025), with a session whose `claude_session_id` is `c-steps`:

```rust
    #[test]
    fn a_task_create_post_tool_use_records_a_step_on_the_conversation() {
        let (store, ssh, ctx, _sid) = step_fixture("c-steps"); // build like the EnterWorktree tests' fixture
        let payload = HookPayload {
            session_id: Some("c-steps".into()),
            hook_event_name: Some("PostToolUse".into()),
            tool_name: Some("TaskCreate".into()),
            tool_input: Some(serde_json::json!({"subject": "Read OM-110", "description": "d", "activeForm": "Reading"})),
            tool_response: Some(serde_json::json!({"task": {"id": "1", "subject": "Read OM-110"}})),
            ..Default::default()
        };
        apply_hook(&store, &ssh, &payload, &ctx).unwrap();
        let cur = store.lock().unwrap().current_steps(&["c-steps".to_string()]).unwrap();
        assert_eq!((cur[0].native_id.as_str(), cur[0].text.as_str()), ("task:1", "Read OM-110"));
    }
```

`step_fixture` is a small helper written from the same setup the EnterWorktree tests use: a store with a host and a session bound to `c-steps`, the ssh mock, and a `HookContext`. If `HookPayload` does not derive `Default`, construct it like those tests do.

In `steps.rs` tests:

```rust
    #[test]
    fn the_transcript_backstop_reads_task_tools_and_their_results() {
        let jsonl = [
            serde_json::json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"tu1","name":"TaskCreate","input":{"subject":"Read OM-110","description":"d"}}]}}),
            serde_json::json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":"Task #1 created successfully: Read OM-110"}]},"toolUseResult":{"task":{"id":"1","subject":"Read OM-110"}}}),
            serde_json::json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"tu2","name":"TaskUpdate","input":{"taskId":"1","status":"completed"}}]}}),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let e = steps_from_transcript(&jsonl);
        assert_eq!(e.len(), 2);
        assert_eq!((e[0].native_id.as_str(), e[1].state), ("task:1", Some(StepState::Completed)));
    }
```

- [ ] **Step 2: Run.** Expected: FAIL.

- [ ] **Step 3: Matcher.** In `hooks_install.rs`, rename `WORKTREE_TOOL_MATCHER` to `POST_TOOL_MATCHER`, give it the new value, and update its doc: *`TaskCreate|TaskUpdate|TodoWrite` carry agent steps (design 2026-09-29 §2)*. Update the `FLEET_HOOK_EVENTS` entry and every reference (`git grep -n WORKTREE_TOOL_MATCHER`). The token-free rendering/digest that marks hosts `provision_stale` changes with it, so update any pinned string or golden in its tests. That change is intended: it is what makes hosts re-provision.

- [ ] **Step 4: Route and handler** in `apply_hook`, before the `_ => Ok(())` arm:

```rust
        Some("PostToolUse")
            if payload
                .tool_name
                .as_deref()
                .is_some_and(|t| crate::service::work::steps::CLAUDE_STEP_TOOLS.contains(&t)) =>
        {
            apply_step_hook(store, payload, ctx)
        }
```

```rust
/// PostToolUse on Claude Code's task tools (design 2026-09-29 §2): the
/// agent's own steps, journaled on the conversation. Best-effort — a
/// malformed body records nothing and the hook still succeeds.
fn apply_step_hook(store: &Arc<Mutex<Store>>, payload: &HookPayload, ctx: &HookContext) -> Result<(), IpcError> {
    let (Some(tool), Some(input)) = (payload.tool_name.as_deref(), payload.tool_input.as_ref()) else {
        return Ok(());
    };
    let events = crate::service::work::steps::steps_from_claude_tool(tool, input, payload.tool_response.as_ref());
    if events.is_empty() {
        return Ok(());
    }
    let s = lock(store)?;
    let Some((row, _)) = resolve_and_rebind(&s, payload, ctx, StartSource::Unknown)? else {
        return Ok(());
    };
    let Some(conv) = payload.session_id.as_deref() else { return Ok(()) };
    let participant = s.participant_for_session(row.id)?.map(|p| p.id);
    if let Err(e) = s.record_steps(conv, participant, "hook", &events) {
        tracing::debug!(row = row.id, error = %e.message, "[steps] not recorded");
    }
    Ok(())
}
```

Check `resolve_and_rebind`'s signature. `apply_pre_compact_hook` calls it inside `s.atomically(|s| …)`. Mirror that exactly if it requires the transaction wrapper.

- [ ] **Step 5: Backstop parser** (`steps.rs`):

```rust
/// The transcript backstop: every task-tool call in a transcript tail, as
/// step events in order, pairing each `TaskCreate` with its result's
/// `toolUseResult.task.id`. Used when a host's hooks predate the matcher.
pub fn steps_from_transcript(jsonl: &str) -> Vec<StepEvent> {
    let mut pending: std::collections::HashMap<String, (String, Value)> = std::collections::HashMap::new();
    let mut out = Vec::new();
    for line in jsonl.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let Some(content) = v.pointer("/message/content").and_then(Value::as_array) else { continue };
        for b in content {
            match b.get("type").and_then(Value::as_str) {
                Some("tool_use") => {
                    let (Some(id), Some(name)) = (b.get("id").and_then(Value::as_str), b.get("name").and_then(Value::as_str)) else { continue };
                    if !CLAUDE_STEP_TOOLS.contains(&name) { continue }
                    let input = b.get("input").cloned().unwrap_or(Value::Null);
                    if name == "TaskCreate" {
                        pending.insert(id.to_string(), (name.to_string(), input));
                    } else {
                        out.extend(steps_from_claude_tool(name, &input, None));
                    }
                }
                Some("tool_result") => {
                    let Some(id) = b.get("tool_use_id").and_then(Value::as_str) else { continue };
                    if let Some((name, input)) = pending.remove(id) {
                        out.extend(steps_from_claude_tool(&name, &input, v.get("toolUseResult")));
                    }
                }
                _ => {}
            }
        }
    }
    out
}
```

- [ ] **Step 6: Spawn the backstop from the Stop hook,** only for a host whose hooks are stale. That is the cost rule: no tail read on up-to-date hosts. In `harvest.rs`, add `spawn_harvest_steps` modelled on `spawn_harvest_compact_summary`:
  - read `COMPACT_READ_BYTES` of the tail with the same `resolve_args_for` / `read_tail_bytes` calls;
  - run `steps_from_transcript`;
  - lock, and call `record_steps(conv, participant, "transcript", &events)`.

  In the Stop hook's background follow-ups (next to `spawn_harvest_compact_summary`, ~line 988), call it when `s.get_host_row(&row.host_alias)?.is_some_and(|h| h.provision_stale || !h.provisioned)`. Read that before the lock is dropped, and pass a bool out.

- [ ] **Step 7: Run** `cargo test -p fleet-core --lib hooks && cargo test -p fleet-core --lib steps && cargo test -p fleet-core --lib hooks_install`. Expected: PASS.

- [ ] **Step 8: Commit** `git commit -am "feat(work): capture Claude Code task steps from PostToolUse, transcript backstop for stale hosts"`

---

### Task 10: The view — tree fields, hidden proposals, task-page data

**Files:**
- Modify: `crates/fleet-core/src/service/work/view.rs` (`WorkTask` :179, `TaskDetail` :346, `Graph` :463/486, `build_tasks` :663, `summarize` :1139, `task()` :1848)
- Test: `crates/fleet-core/src/service/work/view_tests.rs` (helpers `world()`, `page()`, `task_of()`)

**Interfaces:**
- Produces (wire):
  - `WorkTask` gains `origin: String`, `project_id`, `project_label`, `parent_task_id`, `job_state`, `title_derived: bool` and `open_proposals: u32`.
  - `TaskDetail` gains `notes`, `job_result`, `subtasks: Vec<SubtaskView>`, `proposals: Vec<ProposalView>`, `rejected_proposals: Vec<ProposalView>`, `jobs: Vec<JobView>` and `steps: Vec<StepGroup>`.
- New structs, all `Serialize, Deserialize, Clone, Debug, PartialEq`:

```rust
pub struct SubtaskView { pub task_id: String, pub item_id: i64, pub key: Option<String>, pub title: String, pub origin: String,
                         pub status: Option<String>, pub project_id: Option<i64>, pub live_sessions: u32, pub job_state: Option<String> }
pub struct ProposalView { pub item_id: i64, pub key: Option<String>, pub title: String, pub why: Option<String>,
                          pub notes: Option<String>, pub proposed_by: Option<String>, pub at: i64 }
pub struct JobView { pub item_id: i64, pub key: Option<String>, pub title: String, pub state: String,
                     pub result: Option<String>, pub worker: Option<String>, pub at: i64 }
pub struct StepGroup { pub label: String, pub claude_session_id: String, pub steps: Vec<StepLine> }
pub struct StepLine { pub text: String, pub state: String, pub at: i64 }
```

Fields that are `Option` or `Vec` carry the usual `#[serde(default, skip_serializing_if = …)]`.

- [ ] **Step 1: Failing tests** (`view_tests.rs`):

```rust
#[test]
fn proposals_are_not_tasks_and_native_children_name_their_parent() {
    let w = world();
    let (parent, sub, prop) = {
        let s = w.st.lock().unwrap();
        let pid = s.upsert_project("acme", "web", "/src/web").unwrap();
        let parent = s.create_native_item(&crate::store::NativeItem { title: "Ship v1", project_id: Some(pid), notes: Some("n"), ..Default::default() }).unwrap();
        let sub = s.create_native_item(&crate::store::NativeItem { title: "Changelog", parent_id: Some(parent.id), ..Default::default() }).unwrap();
        let prop = s.propose_subtask(&crate::store::Proposal { parent_id: parent.id, title: "Idea", notes: None, why: Some("w"), proposed_by: "x" }).unwrap();
        (parent, sub, prop)
    };
    let p = page(&w, &OrgScope::All, WorkTreeFilters { archived: Some(true), ..Default::default() });
    assert!(p.tasks.iter().all(|t| t.item_id != Some(prop.id)), "a proposal is not a task");
    let pt = p.tasks.iter().find(|t| t.item_id == Some(parent.id)).unwrap();
    assert_eq!((pt.origin.as_str(), pt.project_label.as_deref(), pt.open_proposals), ("manual", Some("acme/web"), 1));
    let st = p.tasks.iter().find(|t| t.item_id == Some(sub.id)).unwrap();
    assert_eq!(st.parent_task_id.as_deref(), Some(format!("item:{}", parent.id).as_str()));
    let d = task(&w.st, &OrgScope::All, &format!("item:{}", parent.id)).unwrap();
    assert_eq!(d.notes.as_deref(), Some("n"));
    assert_eq!(d.subtasks.iter().map(|x| x.item_id).collect::<Vec<_>>(), vec![sub.id]);
    assert_eq!(d.proposals.iter().map(|x| x.item_id).collect::<Vec<_>>(), vec![prop.id]);
}

#[test]
fn a_task_page_shows_the_jobs_result_and_its_sessions_steps() {
    let w = world();
    let parent = {
        let s = w.st.lock().unwrap();
        let parent = s.create_native_item(&crate::store::NativeItem { title: "Ship v1", ..Default::default() }).unwrap();
        s.link_session_work(w.s1, WorkTarget::Item(parent.id), "manual").unwrap();
        let conv = s.get_session_by_id(w.s1).unwrap().unwrap().claude_session_id.unwrap_or_else(|| "c-s1".into());
        s.record_steps(&conv, None, "hook", &[crate::service::work::steps::StepEvent { native_id: "task:1".into(), text: Some("Read it".into()), state: Some(crate::service::work::steps::StepState::Completed), agent: "claude_code" }]).unwrap();
        let job = s.insert_task(None, Some(w.s2), "Changelog", "n").unwrap();
        s.create_agent_task_item(&job, Some(parent.id), None).unwrap();
        s.finish_task(job.id, "done", Some("CHANGELOG.md written"), None).unwrap();
        parent
    };
    let d = task(&w.st, &OrgScope::All, &format!("item:{}", parent.id)).unwrap();
    assert_eq!(d.jobs[0].result.as_deref(), Some("CHANGELOG.md written"));
    assert_eq!(d.steps.iter().flat_map(|g| g.steps.iter()).map(|s| s.text.as_str()).collect::<Vec<_>>(), vec!["Read it"]);
}

#[test]
fn an_untitled_task_borrows_its_first_sessions_name() {
    let w = world();
    {
        let s = w.st.lock().unwrap();
        s.link_session_work(w.s1, WorkTarget::Key("#333"), "manual").unwrap();
    }
    let p = page(&w, &OrgScope::All, WorkTreeFilters::default());
    let t = p.tasks.iter().find(|t| t.key.as_deref() == Some("#333")).unwrap();
    assert!(t.title_derived);
    assert_eq!(t.title, "one");
}
```

If `world()`'s sessions have no `claude_session_id`, the second test must give `s1` one first. Use the setter the fixture file already uses for conversations (`git grep -n "claude_session_id" crates/fleet-core/src/service/work/view_tests.rs`), so that the key's conversation lookup (`KEY_CONVERSATIONS`) finds it. Match `WorkTarget::Key` to how other tests there link a bare ref.

- [ ] **Step 2: Run** `cargo test -p fleet-core --lib service::work::view`. Expected: FAIL.

- [ ] **Step 3: Implement.**
  - **`WorkTask` fields** after `archived`: `#[serde(default)] pub origin: String`, then `project_id`, `project_label`, `parent_task_id`, `job_state` (each `Option`), `#[serde(default, skip_serializing_if = "std::ops::Not::not")] pub title_derived: bool` and `#[serde(default)] pub open_proposals: u32`.
  - **`Graph`** gets `pub(crate) job_states: HashMap<i64, String>` (load it with `s.job_states_by_item()?`) and `pub(crate) open_proposals: HashMap<i64, u32>`. Compute the second in `load` from `items`: count items with `proposal_state == Some("proposed")` per `parent_id`.
  - **`build_tasks`**: skip items with `proposal_state` in `('proposed', 'rejected')` when inserting into `by_task`. An accepted proposal is a normal subtask.
  - **`summarize`**, after `let group = group_of(…)`:
    - `origin` is `item.origin` or `"detected"`;
    - `project_label` is `owner/repo`, or `repo` when the owner is empty or `local`;
    - `parent_task_id` is set only for native origins with `parent_id`;
    - `job_state` comes from `g.job_states`, and `open_proposals` from `g.open_proposals`;
    - `title_derived` applies when `title` is empty and `listed.first()` exists: use `link_name(g.row_of(l), l)`.
    - Keep `group_of` fed with the item's real title.
  - **`task()`**, in its second locked block, reads:
    - `children = s.native_children(item_id)?`, split into accepted/manual/agent subtasks, open proposals and rejected proposals;
    - for each `agent` child, its `TaskRow` (`s.get_task(t)`) into `JobView` (`worker` = the worker session's `friendly_name` or `tmux_name`, if it still exists);
    - `job_result` for the item itself when it is an agent item;
    - steps: `keys` = the item's key plus every subtask key, `convs` = `s.work_conversation_ids(k)` for each, deduplicated, then `s.current_steps(&convs)?`, grouped by `claude_session_id`.
  - **Step group labels**: take the task's `sessions` (`TaskLink`) whose link's `snap_claude_ids` contains the conversation (for ended links), or whose live session's `claude_session_id` equals it. Otherwise use `"earlier conversation"`.
  - **`SubtaskView.live_sessions`**: count live confirmed links through `s.local_item_links(Some(child.id))`.
  - **Fencing**: for `OrgScope::Host` callers, fence `notes`, `why`, `result` and step `text` with `crate::mcp::guard::fence_untrusted(&x, "<what>", DESCRIPTION_MAX_CHARS)`, exactly as the description is fenced.

- [ ] **Step 4: Run** `cargo test -p fleet-core --lib service::work`. Expected: PASS, and all existing view tests unchanged.

- [ ] **Step 5: Commit** `git commit -am "feat(work): the view carries origin, project, parent, proposals, jobs and agent steps"`

---

### Task 11: Desktop commands (routed)

**Files:**
- Modify: `src-tauri/src/commands/work.rs`, `src-tauri/src/lib.rs` (after `commands::work::rename_work_item,` :393), `src-tauri/src/backend/verdicts.rs` (after the `rename_work_item` verdict :323), `crates/fleet-core/src/service/work/mod.rs` (`ROUTED_WORK_COMMANDS`, after `rename_work_item`)
- Test: `src-tauri/src/backend/tests_routing.rs` (rows next to `rename_work_item` ~line 779)

**Interfaces:**
- Produces: Tauri `create_work_task { args: { title, parent?, project_id?, notes? } } → WorkItemRow`, `accept_work_proposal { args: { item_id } } → WorkItemRow`, `reject_work_proposal { args: { item_id } } → WorkItemRow`.

- [ ] **Step 1: Routing rows.** Copy the `rename_work_item` row shape three times. The expected JSON bodies are:

```rust
json!({ "action": "create", "title": "Write notes", "parent": "item:5", "project_id": 3, "notes": "v1" })
json!({ "action": "accept", "item_id": 9 })
json!({ "action": "reject", "item_id": 9 })
```

Each returns the `WorkItemRow` payload `r#"{"id":9,"source":"local","key":"TASK-9","title":"Write notes","status_category":"todo","created_at":1,"updated_at":1,"origin":"manual"}"#`. Match the exact null-field convention the neighbouring rows use.

- [ ] **Step 2: Run** `cargo test --manifest-path src-tauri/Cargo.toml routing`. Expected: FAIL.

- [ ] **Step 3: Implement** in `commands/work.rs`:

```rust
/// A task or subtask a person writes (design 2026-09-29).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorkTaskArgs {
    pub title: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub project_id: Option<i64>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkProposalArgs {
    pub item_id: i64,
}

#[tauri::command]
pub async fn create_work_task(args: CreateWorkTaskArgs, backend: State<'_, Arc<FleetBackend>>, store: State<'_, Arc<Mutex<Store>>>) -> Result<WorkItemRow, IpcError> {
    routed::create_work_task(&backend, args, &store).await
}

#[tauri::command]
pub async fn accept_work_proposal(args: WorkProposalArgs, backend: State<'_, Arc<FleetBackend>>, store: State<'_, Arc<Mutex<Store>>>) -> Result<WorkItemRow, IpcError> {
    routed::decide_work_proposal(&backend, args, true, &store).await
}

#[tauri::command]
pub async fn reject_work_proposal(args: WorkProposalArgs, backend: State<'_, Arc<FleetBackend>>, store: State<'_, Arc<Mutex<Store>>>) -> Result<WorkItemRow, IpcError> {
    routed::decide_work_proposal(&backend, args, false, &store).await
}
```

In `mod routed`:

```rust
    pub async fn create_work_task(backend: &FleetBackend, args: CreateWorkTaskArgs, store: &Mutex<Store>) -> Result<WorkItemRow, IpcError> {
        let args = WorkLinkArgs {
            action: "create".into(),
            title: Some(args.title),
            parent: args.parent,
            project_id: args.project_id,
            notes: args.notes,
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("create_work_task", &args).await,
            None => work::local::create_task(&args, store, &fleet_core::service::orgs::OrgScope::All),
        }
    }

    pub async fn decide_work_proposal(backend: &FleetBackend, args: WorkProposalArgs, accept: bool, store: &Mutex<Store>) -> Result<WorkItemRow, IpcError> {
        let cmd = if accept { "accept_work_proposal" } else { "reject_work_proposal" };
        let args = WorkLinkArgs {
            action: if accept { "accept" } else { "reject" }.into(),
            item_id: Some(args.item_id),
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route(cmd, &args).await,
            None => work::local::decide(&args, store, &fleet_core::service::orgs::OrgScope::All, accept),
        }
    }
```

Then register the three commands in `lib.rs`. Add three `Verdict::Routed { tool: "work_link" }` rows and three `ROUTED_WORK_COMMANDS` rows: `("create_work_task", "work_link", "create")`, `("accept_work_proposal", "work_link", "accept")`, `("reject_work_proposal", "work_link", "reject")`.

- [ ] **Step 4: Run** `cargo test --manifest-path src-tauri/Cargo.toml`. Expected: PASS. If the contract golden tests fail, run `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract` and read the diff; only additions are acceptable.

- [ ] **Step 5: Commit** `git commit -am "feat(work): desktop create_work_task and proposal decisions, routed"`

---

### Task 12: Frontend data — wire, wrappers, grouping, layout preference

**Files:**
- Modify: `src/lib/work_view.ts` (`WorkTask` :131, `TaskDetail` :248; add the types and `workLayout`)
- Modify: `src/lib/work.ts` (wrappers after `nameSessionWork` ~line 417)
- Create: `src/lib/task_list.ts`, `src/lib/task_list.test.ts`

**Interfaces:**
- Produces:
  - TS fields mirroring Task 10;
  - `createWorkTask(input: { title: string; parent?: string | null; projectId?: number | null; notes?: string | null }): Promise<Result<WorkItemRow>>`;
  - `decideWorkProposal(itemId: number, accept: boolean): Promise<Result<WorkItemRow>>`;
  - `workLayout: Writable<'list' | 'grouped'>`, persisted in `localStorage` key `fleet.work.layout` with try/catch and default `'list'`;
  - `groupTasksByStatus(tasks: WorkTask[], nowSecs: number): StatusSections`, `displayTitle(t: WorkTask): string`, `DONE_WINDOW_SECS`, `interface TaskNode { task: WorkTask; children: WorkTask[] }`, `interface StatusSections { todo; doing; done }`.

- [ ] **Step 1: Failing tests** (`task_list.test.ts`):

```ts
import { describe, it, expect } from 'vitest';
import { task } from './work_view_fixture';
import { DONE_WINDOW_SECS, displayTitle, groupTasksByStatus } from './task_list';

const NOW = 1_790_700_000;

describe('groupTasksByStatus', () => {
  it('uses the effective status, and a live session means Doing', () => {
    const s = groupTasksByStatus(
      [
        task({ task_id: 'item:1', status_category: 'todo', counts: { active: 0, ended: 0, suggested: 0 }, last_activity_at: NOW - 10 }),
        task({ task_id: 'item:2', status_category: 'todo', counts: { active: 1, ended: 0, suggested: 0 }, last_activity_at: NOW }),
        task({ task_id: 'item:3', status_category: 'done', counts: { active: 1, ended: 0, suggested: 0 }, last_activity_at: NOW }),
        task({ task_id: 'item:4', status_category: 'done', counts: { active: 0, ended: 1, suggested: 0 }, last_activity_at: NOW - 60 }),
      ],
      NOW,
    );
    expect(s.todo.map((n) => n.task.task_id)).toEqual(['item:1']);
    expect(s.doing.map((n) => n.task.task_id).sort()).toEqual(['item:2', 'item:3']);
    expect(s.done.map((n) => n.task.task_id)).toEqual(['item:4']);
  });

  it('Done keeps the last 7 days', () => {
    const s = groupTasksByStatus(
      [
        task({ task_id: 'item:1', status_category: 'done', counts: { active: 0, ended: 1, suggested: 0 }, last_activity_at: NOW - DONE_WINDOW_SECS + 1 }),
        task({ task_id: 'item:2', status_category: 'done', counts: { active: 0, ended: 1, suggested: 0 }, last_activity_at: NOW - DONE_WINDOW_SECS - 1 }),
      ],
      NOW,
    );
    expect(s.done.map((n) => n.task.task_id)).toEqual(['item:1']);
  });

  it('nests native children under a listed parent, else lists them alone', () => {
    const s = groupTasksByStatus(
      [
        task({ task_id: 'item:1', status_category: 'in_progress' }),
        task({ task_id: 'item:2', origin: 'agent', parent_task_id: 'item:1', status_category: 'done' }),
        task({ task_id: 'item:3', origin: 'manual', parent_task_id: 'item:99', status_category: 'todo', counts: { active: 0, ended: 0, suggested: 0 } }),
      ],
      NOW,
    );
    expect(s.doing[0].children.map((c) => c.task_id)).toEqual(['item:2']);
    expect(s.todo.map((n) => n.task.task_id)).toEqual(['item:3']);
  });
});

describe('displayTitle', () => {
  it('title, then key, then id', () => {
    expect(displayTitle(task({ title: 'Login', key: 'ABC-1' }))).toBe('Login');
    expect(displayTitle(task({ title: '', key: 'ABC-1' }))).toBe('ABC-1');
    expect(displayTitle(task({ title: '', key: null, task_id: 'ref:x' }))).toBe('ref:x');
  });
});
```

- [ ] **Step 2: Run** `pnpm vitest run src/lib/task_list.test.ts`. Expected: FAIL.

- [ ] **Step 3: `task_list.ts`:**

```ts
// The Work tab's List layout (design 2026-09-29): one `work_tree` read
// grouped by status in the client — To do, Doing, Done (last 7 days) — with
// native subtasks and agent jobs nested under a listed parent.
import type { WorkTask } from './work_view';

export const DONE_WINDOW_SECS = 7 * 86_400;

export interface TaskNode {
  task: WorkTask;
  children: WorkTask[];
}
export interface StatusSections {
  todo: TaskNode[];
  doing: TaskNode[];
  done: TaskNode[];
}

function sectionOf(t: WorkTask): keyof StatusSections {
  const live = (t.counts?.active ?? 0) > 0;
  if (live || t.status_category === 'in_progress') return 'doing';
  if (t.status_category === 'done') return 'done';
  return 'todo';
}

const newestFirst = (a: WorkTask, b: WorkTask) => (b.last_activity_at ?? 0) - (a.last_activity_at ?? 0);

export function groupTasksByStatus(tasks: WorkTask[], nowSecs: number): StatusSections {
  const byId = new Map(tasks.map((t) => [t.task_id, t]));
  const kids = new Map<string, WorkTask[]>();
  const roots: WorkTask[] = [];
  for (const t of tasks) {
    const p = t.parent_task_id ? byId.get(t.parent_task_id) : undefined;
    if (p) kids.set(p.task_id, [...(kids.get(p.task_id) ?? []), t]);
    else roots.push(t);
  }
  const out: StatusSections = { todo: [], doing: [], done: [] };
  for (const t of [...roots].sort(newestFirst)) {
    const s = sectionOf(t);
    if (s === 'done' && (t.last_activity_at ?? 0) < nowSecs - DONE_WINDOW_SECS) continue;
    out[s].push({ task: t, children: [...(kids.get(t.task_id) ?? [])].sort(newestFirst) });
  }
  return out;
}

export function displayTitle(t: WorkTask): string {
  return t.title || t.key || t.task_id;
}
```

- [ ] **Step 4: Wire types** in `work_view.ts`:
  - `WorkTask` gains `origin?`, `project_id?`, `project_label?`, `parent_task_id?`, `job_state?`, `title_derived?: boolean` and `open_proposals?: number`, each with a one-line doc as in Task 10.
  - Add the interfaces `SubtaskView`, `ProposalView`, `JobView`, `StepGroup` and `StepLine`, mirroring the Rust.
  - `TaskDetail` gains `notes?`, `job_result?`, `subtasks?: SubtaskView[]`, `proposals?: ProposalView[]`, `rejected_proposals?: ProposalView[]`, `jobs?: JobView[]` and `steps?: StepGroup[]`.
  - Add the layout preference:

```ts
const LAYOUT_KEY = 'fleet.work.layout';
function readLayout(): 'list' | 'grouped' {
  try {
    return localStorage.getItem(LAYOUT_KEY) === 'grouped' ? 'grouped' : 'list';
  } catch {
    return 'list';
  }
}
/** The Work tab's layout (design 2026-09-29): List (by status) or Grouped (org → group). */
export const workLayout = writable<'list' | 'grouped'>(readLayout());
workLayout.subscribe((v) => {
  try {
    localStorage.setItem(LAYOUT_KEY, v);
  } catch {
    /* private window: the default stands */
  }
});
```

- [ ] **Step 5: Wrappers** in `work.ts`:

```ts
/** A task, or a subtask under `parent` (`item:<id>`), written in Fleet. */
export async function createWorkTask(input: {
  title: string;
  parent?: string | null;
  projectId?: number | null;
  notes?: string | null;
}): Promise<Result<WorkItemRow>> {
  const notes = input.notes?.trim();
  const r = await invokeCmd<WorkItemRow>('create_work_task', {
    args: {
      title: input.title.trim(),
      ...(input.parent ? { parent: input.parent } : {}),
      ...(input.projectId != null ? { project_id: input.projectId } : {}),
      ...(notes ? { notes } : {}),
    },
  });
  if (r.ok) bumpWorkChanged();
  return r;
}

/** A person accepts or rejects an agent's proposed subtask. */
export async function decideWorkProposal(itemId: number, accept: boolean): Promise<Result<WorkItemRow>> {
  const r = await invokeCmd<WorkItemRow>(accept ? 'accept_work_proposal' : 'reject_work_proposal', { args: { item_id: itemId } });
  if (r.ok) bumpWorkChanged();
  return r;
}
```

Use the file's existing `WorkItemRow` TS type if there is one (`git grep -n "WorkItemRow" src/lib/*.ts`). Otherwise declare `export interface WorkItemRow { id: number; key?: string | null; title: string; source: string; origin?: string | null }` in `work_view.ts`.

- [ ] **Step 6: Run** `pnpm vitest run src/lib/task_list.test.ts src/lib/work.test.ts src/lib/work_view.test.ts && pnpm run check`. Expected: PASS with 0 errors.

- [ ] **Step 7: Commit** `git commit -am "feat(work): list data — wire fields, create/decide wrappers, status grouping, layout pref"`

---

### Task 13: The List layout inside the existing Work header

**Files:**
- Create: `src/lib/TaskList.svelte`, `src/lib/TaskList.test.ts`
- Modify: `src/lib/WorkTree.svelte` (header `.row` :497-526; `load()` :175; the scroller :532-692)

**Interfaces:**
- Consumes: `workTree`, `workViewFilters`, `openTask`, `selectedTaskId`, `readErrorText`, `workLayout` (`work_view.ts`); `createWorkTask`, `onWorkChangedDebounced` (`work.ts`); `startWork` (`trackers.ts`); `projects`, `loadProjects` (`projects.ts`); `groupTasksByStatus`, `displayTitle` (`task_list.ts`); `providerInfo` (`trackers.ts`).
- Produces: `<TaskList />` with test ids `task-list`, `task-add-input`, `task-add-more`, `task-add-project`, `task-add-notes`, `task-section-todo|doing|done`, `task-row`, `task-start`, `task-child`, `task-proposals-badge`. In `WorkTree`: `work-layout-list`, `work-layout-grouped`.

- [ ] **Step 1: Failing tests** (`TaskList.test.ts`):

```ts
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import TaskList from './TaskList.svelte';
import { task } from './work_view_fixture';
import { workViewFilters, type WorkTreePage } from './work_view';

const NOW = Math.floor(Date.now() / 1000);
const page: WorkTreePage = {
  tasks: [
    task({ task_id: 'item:1', item_id: 1, key: 'TASK-1', title: 'Write notes', kind: 'local', origin: 'manual', status_category: 'todo',
      counts: { active: 0, ended: 0, suggested: 0 }, sessions: [], last_activity_at: NOW, project_id: 3, project_label: 'acme/api' }),
    task({ task_id: 'item:110', item_id: 110, key: 'OM-110', title: 'Qomora', status_category: 'todo', status_name: 'Backlog',
      counts: { active: 1, ended: 2, suggested: 0 }, open_proposals: 2, last_activity_at: NOW }),
    task({ task_id: 'item:42', item_id: 42, key: 'TASK-42', title: 'Neutral review', kind: 'local', origin: 'agent',
      parent_task_id: 'item:110', job_state: 'done', status_category: 'done', last_activity_at: NOW }),
  ],
  groups: [], orgs: [], trackers: [], total: 3, archived_hidden: 0, next_cursor: null, generated_at: NOW,
};
const flush = async () => {
  for (let i = 0; i < 6; i++) {
    await Promise.resolve();
    await tick();
  }
};
const calls = (cmd: string) => (invoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === cmd);

beforeEach(() => {
  workViewFilters.set({ tracker: 1 });
  (invoke as ReturnType<typeof vi.fn>).mockReset();
  (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
    if (cmd === 'work_tree') return page;
    if (cmd === 'list_projects') return [];
    if (cmd === 'create_work_task') return { id: 9, key: 'TASK-9', title: 'New', source: 'local' };
    if (cmd === 'start_work') return { id: 50, host_alias: 'mac', tmux_name: 'x' };
    return null;
  });
});

describe('TaskList', () => {
  it('reads once with the current filters and archived on', async () => {
    render(TaskList);
    await flush();
    expect(calls('work_tree')).toHaveLength(1);
    const args = (calls('work_tree')[0][1] as { args: { filters: Record<string, unknown> } }).args.filters;
    expect(args.tracker).toBe(1);
    expect(args.archived).toBe(true);
  });

  it('shows sections, the tracker status, the proposal badge and nested agent jobs', async () => {
    render(TaskList);
    await flush();
    expect(screen.getByTestId('task-section-todo').textContent).toContain('Write notes');
    const doing = screen.getByTestId('task-section-doing');
    expect(doing.textContent).toContain('Qomora');
    expect(doing.textContent).toContain('Backlog');
    expect(screen.getByTestId('task-proposals-badge').textContent).toContain('2');
    expect(screen.getByTestId('task-child').textContent).toContain('Neutral review');
  });

  it('Done is collapsed by default', async () => {
    render(TaskList);
    await flush();
    expect(screen.getByTestId('task-section-done').querySelector('ul')).toBeNull();
  });

  it('quick add creates a task; Start starts one by item id', async () => {
    render(TaskList);
    await flush();
    const input = screen.getByTestId('task-add-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'New' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await flush();
    expect(calls('create_work_task')[0][1]).toEqual({ args: { title: 'New' } });
    await fireEvent.click(screen.getByTestId('task-start'));
    await flush();
    expect(calls('start_work')[0][1]).toEqual({ args: { item_id: 1, project_id: 3 } });
  });

  it('renders tracker text as text', async () => {
    (invoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) =>
      cmd === 'work_tree' ? { ...page, tasks: [task({ task_id: 'item:5', title: '<b>x</b>', status_category: 'todo', counts: { active: 0, ended: 0, suggested: 0 } })] } : [],
    );
    render(TaskList);
    await flush();
    expect(screen.getByTestId('task-section-todo').querySelector('b')).toBeNull();
  });
});
```

- [ ] **Step 2: Run** `pnpm vitest run src/lib/TaskList.test.ts`. Expected: FAIL.

- [ ] **Step 3: `TaskList.svelte`:**

```svelte
<script lang="ts">
  // The Work tab's List layout (design 2026-09-29): every task the current
  // filters match — tickets, native tasks, bare keys — in To do / Doing /
  // Done, from ONE `work_tree` read (archived on, so Done has its rows).
  // The header (Tasks | Review, List | Grouped, saved views, filters) is
  // WorkTree's; this is only the body. Tracker text renders as text.
  import { onDestroy, onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { createWorkTask, onWorkChangedDebounced } from './work';
  import { openTask, readErrorText, selectedTaskId, workTree, workViewFilters, type WorkTask } from './work_view';
  import { providerInfo, startWork } from './trackers';
  import { projects, loadProjects } from './projects';
  import { displayTitle, groupTasksByStatus, type StatusSections } from './task_list';
  import type { IpcError } from './result';

  let { debounceMs = 500, maxWaitMs = 3000 }: { debounceMs?: number; maxWaitMs?: number } = $props();

  let tasks = $state.raw<WorkTask[]>([]);
  let loaded = $state(false);
  let error = $state<IpcError | null>(null);
  let doneOpen = $state(false);
  let addTitle = $state('');
  let addMore = $state(false);
  let addProject = $state<number | null>(null);
  let addNotes = $state('');
  let busy = $state(false);
  let actionError = $state<string | null>(null);

  const sections: StatusSections = $derived(groupTasksByStatus(tasks, Math.floor(Date.now() / 1000)));
  const pickable = $derived($projects.filter((p) => !p.project.system));

  let seq = 0;
  async function load() {
    const mine = ++seq;
    const r = await workTree({ filters: { ...get(workViewFilters), archived: true }, limit: 200, per_task: 3 });
    if (mine !== seq) return;
    loaded = true;
    if (!r.ok) {
      error = r.error;
      return;
    }
    error = null;
    tasks = Array.isArray(r.value?.tasks) ? r.value.tasks : [];
  }

  let lastFilters = JSON.stringify(get(workViewFilters));
  const offFilters = workViewFilters.subscribe((f) => {
    const k = JSON.stringify(f);
    if (k === lastFilters) return;
    lastFilters = k;
    void load();
  });
  const offChanged = onWorkChangedDebounced(() => void load(), () => debounceMs, () => maxWaitMs);
  onMount(() => {
    void load();
    if (get(projects).length === 0) void loadProjects();
  });
  onDestroy(() => {
    offFilters();
    offChanged();
  });

  async function add() {
    const title = addTitle.trim();
    if (!title || busy) return;
    busy = true;
    const r = await createWorkTask({ title, projectId: addProject, notes: addNotes });
    busy = false;
    if (!r.ok) {
      actionError = readErrorText(r.error);
      return;
    }
    actionError = null;
    addTitle = '';
    addNotes = '';
    addMore = false;
    void load();
  }

  async function start(t: WorkTask) {
    if (t.item_id == null || busy) return;
    busy = true;
    const r = await startWork({ item_id: t.item_id, ...(t.project_id != null ? { project_id: t.project_id } : {}) });
    busy = false;
    actionError = r.ok ? null : readErrorText(r.error);
  }

  const canStart = (t: WorkTask) => (t.counts?.active ?? 0) === 0 && t.status_category !== 'done' && t.item_id != null;
  const badge = (t: WorkTask) => (t.kind === 'local' ? 'F' : t.kind === 'ref' ? '#' : (providerInfo(t.provider)?.icon ?? 'T'));
</script>

<div class="task-list" data-testid="task-list">
  <div class="add">
    <input placeholder="+ New task" aria-label="New task" data-testid="task-add-input" bind:value={addTitle}
      onkeydown={(e) => { if (e.key === 'Enter') void add(); }} />
    <button class="btn btn--quiet btn--icon" type="button" title="Project and notes" aria-label="Project and notes"
      data-testid="task-add-more" aria-expanded={addMore} onclick={() => (addMore = !addMore)}>▾</button>
  </div>
  {#if addMore}
    <div class="add-more">
      <select aria-label="Project" data-testid="task-add-project" bind:value={addProject}>
        <option value={null}>No project</option>
        {#each pickable as p (p.project.id)}
          <option value={p.project.id}>{p.project.owner && p.project.owner !== 'local' ? `${p.project.owner}/${p.project.repo}` : p.project.repo}</option>
        {/each}
      </select>
      <textarea rows="3" placeholder="Notes: the first prompt when you press Start" aria-label="Notes" data-testid="task-add-notes" bind:value={addNotes}></textarea>
    </div>
  {/if}
  {#if actionError}<p class="err" role="alert">{actionError}</p>{/if}

  {#if error}
    <p class="err" role="alert">{readErrorText(error)} <button class="btn btn--quiet" type="button" onclick={() => void load()}>Retry</button></p>
  {:else if !loaded}
    <p class="muted">Loading tasks…</p>
  {:else if tasks.length === 0}
    <p class="muted">No tasks match. Type one above and press Enter.</p>
  {:else}
    {@render section('todo', 'To do', sections.todo, true)}
    {@render section('doing', 'Doing', sections.doing, true)}
    {@render section('done', 'Done · last 7 days', sections.done, doneOpen)}
  {/if}
</div>

{#snippet section(id: 'todo' | 'doing' | 'done', label: string, nodes: StatusSections['todo'], open: boolean)}
  <section data-testid="task-section-{id}">
    <h3>
      {#if id === 'done'}
        <button class="sec" type="button" aria-expanded={open} onclick={() => (doneOpen = !doneOpen)}>{open ? '▾' : '▸'} {label}</button>
      {:else}{label}{/if}
      <span class="count">{nodes.length}</span>
    </h3>
    {#if open}
      <ul>
        {#each nodes as n (n.task.task_id)}
          {@const t = n.task}
          <li class:selected={$selectedTaskId === t.task_id}>
            <div class="row" data-testid="task-row">
              <span class="tb" title={t.tracker_name ?? t.kind}>{badge(t)}</span>
              <button class="main" type="button" onclick={() => openTask(t.task_id)}>
                <span class="title">
                  {#if t.needs_you}<span class="needs" aria-label="needs you">●</span>{/if}
                  {#if t.key}<span class="key">{t.key}</span>{/if}
                  <span class="txt" class:derived={t.title_derived}>{displayTitle(t)}</span>
                </span>
                <span class="meta">
                  {#if t.status_name}<span>{t.status_name}</span>{:else if t.project_label}<span>{t.project_label}</span>{/if}
                  <span>{t.counts?.active ?? 0} active · {t.counts?.ended ?? 0} past</span>
                </span>
              </button>
              <span class="right">
                {#if (t.open_proposals ?? 0) > 0}<span class="chip prop" data-testid="task-proposals-badge">{t.open_proposals} to review</span>{/if}
                {#if canStart(t)}<button class="btn" type="button" data-testid="task-start" disabled={busy} onclick={() => void start(t)}>Start</button>{/if}
              </span>
            </div>
            {#if n.children.length > 0}
              <ul class="children">
                {#each n.children as c (c.task_id)}
                  <li class="child" data-testid="task-child">
                    <span class="dot dot--{c.status_category ?? 'todo'}"></span>
                    <button class="txt" type="button" onclick={() => openTask(c.task_id)}>{displayTitle(c)}</button>
                    {#if c.origin === 'agent'}<span class="chip agent">agent</span>{:else if c.key}<span class="key">{c.key}</span>{/if}
                  </li>
                {/each}
              </ul>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </section>
{/snippet}

<style>
  .task-list { display: flex; flex-direction: column; gap: 2px; font-size: 0.85rem; }
  .add { display: flex; gap: 6px; align-items: center; padding: 4px 0; }
  .add input { flex: 1; min-width: 0; height: 26px; padding: 0 8px; border: 1px dashed var(--control-border); border-radius: var(--radius-md); background: var(--bg); color: var(--fg); font: inherit; }
  .add-more { display: grid; gap: 6px; padding-bottom: 6px; }
  h3 { display: flex; gap: 6px; align-items: center; margin: 10px 4px 4px; font-size: 0.7rem; text-transform: uppercase; letter-spacing: 0.06em; color: var(--fg-muted); font-weight: 600; }
  .sec { background: none; border: 0; color: inherit; font: inherit; text-transform: inherit; letter-spacing: inherit; cursor: pointer; padding: 0; }
  .count { margin-left: auto; font-variant-numeric: tabular-nums; }
  ul { list-style: none; margin: 0; padding: 0; }
  li.selected > .row { background: color-mix(in srgb, var(--accent) 14%, transparent); }
  .row { display: grid; grid-template-columns: 16px minmax(0, 1fr) auto; gap: 8px; align-items: start; padding: 4px; border-radius: 4px; }
  .row:hover { background: color-mix(in srgb, var(--accent) 8%, transparent); }
  .tb { font-size: 0.62rem; border: 1px solid var(--border); border-radius: 3px; text-align: center; color: var(--fg-muted); margin-top: 2px; }
  .main { min-width: 0; display: grid; gap: 1px; background: none; border: 0; padding: 0; text-align: left; color: var(--fg); font: inherit; cursor: pointer; }
  .title { display: flex; gap: 5px; align-items: baseline; min-width: 0; }
  .txt { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .derived { font-style: italic; color: var(--fg-muted); }
  .key { font-family: var(--mono); flex: none; }
  .needs { color: var(--usage-crit, #c62828); font-size: 0.55rem; }
  .meta { display: flex; flex-wrap: wrap; gap: 0 8px; color: var(--fg-muted); font-size: 0.72rem; }
  .right { display: flex; gap: 4px; align-items: center; }
  .chip { font-size: 0.7rem; border-radius: 999px; padding: 0 6px; white-space: nowrap; }
  .prop { background: color-mix(in srgb, var(--usage-warn, #b45309) 14%, transparent); color: var(--usage-warn, #b45309); }
  .agent { background: color-mix(in srgb, #7c3aed 12%, transparent); color: #7c3aed; }
  .children { margin: 0 0 4px 24px; border-left: 1px solid var(--border); padding-left: 8px; }
  .child { display: grid; grid-template-columns: 10px minmax(0, 1fr) auto; gap: 6px; align-items: center; font-size: 0.8rem; }
  .child .txt { background: none; border: 0; padding: 0; text-align: left; color: var(--fg); font: inherit; cursor: pointer; }
  .dot { width: 7px; height: 7px; border-radius: 50%; border: 1.5px solid var(--fg-muted); }
  .dot--in_progress { background: var(--accent); border-color: var(--accent); }
  .dot--done { background: var(--usage-ok, #2e7d32); border-color: var(--usage-ok, #2e7d32); }
  .muted { color: var(--fg-muted); padding: 6px 4px; }
  .err { color: var(--usage-crit, #c62828); padding: 4px; }
</style>
```

- [ ] **Step 4: Wire it into `WorkTree.svelte`:**
  - Import `TaskList` and `workLayout`.
  - In the header `.row`, after the `tabs` div, add the toggle:

```svelte
      <div class="tabs" role="group" aria-label="Layout">
        <button class="btn btn--chip btn--toggle" type="button" aria-pressed={$workLayout === 'list'} class:is-active={$workLayout === 'list'}
          data-testid="work-layout-list" title="By status: To do, Doing, Done" onclick={() => workLayout.set('list')}>List</button>
        <button class="btn btn--chip btn--toggle" type="button" aria-pressed={$workLayout === 'grouped'} class:is-active={$workLayout === 'grouped'}
          data-testid="work-layout-grouped" title="Organisation → group" onclick={() => workLayout.set('grouped')}>Grouped</button>
      </div>
```

  - In the scroller, change `{#if tab === 'review'}` … to render `<TaskList />` when `tab === 'tasks' && $workLayout === 'list'`, before the existing error/loading/tree branches. The tree branch stays for Grouped.
  - In `load()`, return early when `get(workLayout) === 'list' && tab === 'tasks'`, so List mode does not also pay for the tree read. Subscribe to `workLayout` so switching to Grouped calls `void load()`.
  - `revealTaskRequest` ("Show in Work view") must still work. In `flushReveal`, set `workLayout.set('grouped')` before `showTasks()`.
  - `WorkFiltersBar` stays in the header for both layouts. Its Archived toggle is disabled in List with the title *In List view, Done shows them*: pass a `listLayout` prop if `WorkFiltersBar` renders the toggle, or hide it with a class. Choose the smallest change.

- [ ] **Step 5: Tests for the toggle** (append to `WorkTree.test.ts`, using its existing setup):

```ts
  it('defaults to List and shows the grouped tree on Grouped', async () => {
    workLayout.set('list');
    render(WorkTree);
    await flush();
    expect(screen.getByTestId('task-list')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('work-layout-grouped'));
    await flush();
    expect(screen.queryByTestId('task-list')).toBeNull();
    expect(screen.getAllByTestId('work-org').length).toBeGreaterThan(0);
  });
```

Existing `WorkTree.test.ts` cases assume the tree. Add `workLayout.set('grouped')` to that file's `beforeEach`, so they keep testing Grouped unchanged.

- [ ] **Step 6: Run** `pnpm vitest run src/lib/TaskList.test.ts src/lib/WorkTree.test.ts src/lib/WorkFiltersBar.test.ts && pnpm run check`. Expected: PASS.

- [ ] **Step 7: Commit** `git commit -am "feat(ui): Work tab List layout — To do / Doing / Done under the existing header and filters"`

---

### Task 14: Task page sections

**Files:**
- Create: `src/lib/TaskWorkSections.svelte`, `src/lib/TaskWorkSections.test.ts`
- Modify: `src/lib/WorkTaskDetail.svelte` (mount after the actions block ~line 353; wrap the org/group `<dl>` (~line 303-325) and the Place / Assign org / Make rule buttons (~line 363-385) in a disclosure)

**Interfaces:**
- Consumes: `TaskDetail` (Task 12), `createWorkTask`, `decideWorkProposal` (`work.ts`), `startWork` (`trackers.ts`), `openTask` (`work_view.ts`).
- Produces: `<TaskWorkSections detail={detail} />` with test ids `task-notes`, `task-subtasks`, `task-subtask-start`, `task-add-subtask`, `task-proposal`, `task-proposal-accept`, `task-proposal-reject`, `task-rejected-toggle`, `task-jobs`, `task-job-result`, `task-steps`, `task-step`.

- [ ] **Step 1: Failing tests** (`TaskWorkSections.test.ts`):

```ts
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import TaskWorkSections from './TaskWorkSections.svelte';
import { task } from './work_view_fixture';
import type { TaskDetail } from './work_view';

const detail: TaskDetail = {
  task: task({ task_id: 'item:110', item_id: 110, key: 'OM-110' }),
  notes: 'Rebase <b>now</b>',
  subtasks: [{ task_id: 'item:41', item_id: 41, key: 'TASK-41', title: 'SELECT stats', origin: 'manual', status: 'todo', project_id: 3, live_sessions: 0, job_state: null }],
  proposals: [{ item_id: 44, key: 'TASK-44', title: 'Decide the P0 owner', why: 'Both reviews block on it.', notes: null, proposed_by: 'OM-110 · trn', at: 1 }],
  rejected_proposals: [{ item_id: 40, key: 'TASK-40', title: 'Create om-catalog', why: null, notes: null, proposed_by: 'x', at: 1 }],
  jobs: [{ item_id: 42, key: 'TASK-42', title: 'Neutral review', state: 'done', result: 'Qomora has offers, not products.', worker: 'review-neutral', at: 1 }],
  steps: [{ label: 'OM-110 Qomora', claude_session_id: 'c1', steps: [{ text: 'Read OM-110', state: 'completed', at: 1 }, { text: 'Prepare SELECTs', state: 'in_progress', at: 2 }] }],
};
const flush = async () => {
  for (let i = 0; i < 5; i++) {
    await Promise.resolve();
    await tick();
  }
};
const calls = (cmd: string) => (invoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === cmd);

beforeEach(() => {
  (invoke as ReturnType<typeof vi.fn>).mockReset();
  (invoke as ReturnType<typeof vi.fn>).mockResolvedValue({ id: 1, title: 'x', source: 'local' });
});

describe('TaskWorkSections', () => {
  it('renders every section, text as text', () => {
    render(TaskWorkSections, { detail });
    expect(screen.getByTestId('task-notes').textContent).toBe('Rebase <b>now</b>');
    expect(screen.getByTestId('task-subtasks').textContent).toContain('SELECT stats');
    expect(screen.getByTestId('task-proposal').textContent).toContain('Both reviews block on it.');
    expect(screen.getByTestId('task-job-result').textContent).toContain('offers, not products');
    expect(screen.getAllByTestId('task-step')).toHaveLength(2);
    expect(screen.getByTestId('task-steps').textContent).toContain('per the agent');
  });

  it('accepts and rejects a proposal', async () => {
    render(TaskWorkSections, { detail });
    await fireEvent.click(screen.getByTestId('task-proposal-accept'));
    await flush();
    expect(calls('accept_work_proposal')[0][1]).toEqual({ args: { item_id: 44 } });
    await fireEvent.click(screen.getByTestId('task-proposal-reject'));
    await flush();
    expect(calls('reject_work_proposal')[0][1]).toEqual({ args: { item_id: 44 } });
  });

  it('starts a subtask and adds one under this task', async () => {
    render(TaskWorkSections, { detail });
    await fireEvent.click(screen.getByTestId('task-subtask-start'));
    await flush();
    expect(calls('start_work')[0][1]).toEqual({ args: { item_id: 41, project_id: 3 } });
    await fireEvent.click(screen.getByTestId('task-add-subtask'));
    const input = screen.getByLabelText('Subtask title') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'Follow-up' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await flush();
    expect(calls('create_work_task')[0][1]).toEqual({ args: { title: 'Follow-up', parent: 'item:110' } });
  });

  it('keeps rejected proposals behind a toggle', async () => {
    render(TaskWorkSections, { detail });
    expect(screen.queryByText('Create om-catalog')).toBeNull();
    await fireEvent.click(screen.getByTestId('task-rejected-toggle'));
    expect(screen.getByText('Create om-catalog')).toBeTruthy();
  });
});
```

- [ ] **Step 2: Run** `pnpm vitest run src/lib/TaskWorkSections.test.ts`. Expected: FAIL.

- [ ] **Step 3: `TaskWorkSections.svelte`:**

```svelte
<script lang="ts">
  // The task page's shared-work sections (design 2026-09-29 §4): notes,
  // subtasks (+ add, Start), agent proposals (accept / reject; rejected
  // behind a toggle), delegated jobs with their result, and agent steps per
  // session — "per the agent", never a status. All text renders as text.
  import { createWorkTask, decideWorkProposal } from './work';
  import { startWork } from './trackers';
  import { openTask, readErrorText, type TaskDetail } from './work_view';

  let { detail }: { detail: TaskDetail } = $props();

  let adding = $state(false);
  let newTitle = $state('');
  let busy = $state(false);
  let showRejected = $state(false);
  let err = $state<string | null>(null);

  async function run<T>(p: Promise<{ ok: boolean; error?: unknown } & T>) {
    busy = true;
    const r = await p;
    busy = false;
    err = r.ok ? null : readErrorText(r.error as never);
  }
  function addSubtask() {
    const title = newTitle.trim();
    if (!title) return;
    void run(createWorkTask({ title, parent: detail.task.task_id })).then(() => {
      newTitle = '';
      adding = false;
    });
  }
</script>

<div class="tws">
  {#if err}<p class="err" role="alert">{err}</p>{/if}

  {#if detail.notes}
    <section><h4>Notes</h4><p class="text" data-testid="task-notes">{detail.notes}</p></section>
  {/if}

  <section data-testid="task-subtasks">
    <h4>Subtasks <span class="n">{detail.subtasks?.length ?? 0}</span>
      <button class="btn btn--quiet" type="button" data-testid="task-add-subtask" onclick={() => (adding = true)}>+ Add subtask</button></h4>
    {#if adding}
      <input class="add" aria-label="Subtask title" placeholder="Subtask title" bind:value={newTitle}
        onkeydown={(e) => { if (e.key === 'Enter') addSubtask(); if (e.key === 'Escape') adding = false; }} />
    {/if}
    {#each detail.subtasks ?? [] as s (s.item_id)}
      <div class="row">
        <span class="dot dot--{s.status ?? 'todo'}"></span>
        <button class="link" type="button" onclick={() => openTask(s.task_id)}>{s.title}</button>
        {#if s.key}<span class="key">{s.key}</span>{/if}
        {#if s.origin === 'agent'}<span class="chip agent">delegated job</span>{:else if s.origin === 'proposed'}<span class="chip prop">from a proposal</span>{/if}
        <span class="spacer"></span>
        {#if s.status === 'todo' && s.live_sessions === 0}
          <button class="btn" type="button" data-testid="task-subtask-start" disabled={busy}
            onclick={() => void run(startWork({ item_id: s.item_id, ...(s.project_id != null ? { project_id: s.project_id } : {}) }))}>Start</button>
        {:else}<span class="muted">{s.live_sessions > 0 ? `${s.live_sessions} live` : s.status}</span>{/if}
      </div>
    {:else}
      {#if !adding}<p class="muted">No subtasks yet.</p>{/if}
    {/each}
  </section>

  {#if (detail.proposals?.length ?? 0) > 0 || (detail.rejected_proposals?.length ?? 0) > 0}
    <section>
      <h4>Proposals <span class="n">{detail.proposals?.length ?? 0}</span><span class="hint">agents propose, you decide</span></h4>
      {#each detail.proposals ?? [] as p (p.item_id)}
        <div class="prop-card" data-testid="task-proposal">
          <div><strong>{p.title}</strong> {#if p.key}<span class="key">{p.key}</span>{/if}</div>
          {#if p.why}<p class="text">{p.why}</p>{/if}
          {#if p.proposed_by}<p class="muted">Proposed by {p.proposed_by}</p>{/if}
          <div class="acts">
            <button class="btn btn--primary" type="button" data-testid="task-proposal-accept" disabled={busy} onclick={() => void run(decideWorkProposal(p.item_id, true))}>Accept</button>
            <button class="btn" type="button" data-testid="task-proposal-reject" disabled={busy} onclick={() => void run(decideWorkProposal(p.item_id, false))}>Reject</button>
          </div>
        </div>
      {/each}
      {#if (detail.rejected_proposals?.length ?? 0) > 0}
        <button class="btn btn--quiet" type="button" data-testid="task-rejected-toggle" aria-expanded={showRejected} onclick={() => (showRejected = !showRejected)}>
          {showRejected ? 'Hide' : 'Show'} rejected ({detail.rejected_proposals?.length})</button>
        {#if showRejected}
          <ul class="rejected">{#each detail.rejected_proposals ?? [] as r (r.item_id)}<li>{r.title}</li>{/each}</ul>
        {/if}
      {/if}
    </section>
  {/if}

  {#if (detail.jobs?.length ?? 0) > 0}
    <section data-testid="task-jobs">
      <h4>Delegated jobs <span class="n">{detail.jobs?.length}</span></h4>
      {#each detail.jobs ?? [] as j (j.item_id)}
        <div class="job">
          <div><strong>{j.title}</strong> <span class="state state--{j.state}">{j.state}</span>{#if j.worker}<span class="muted"> · {j.worker}</span>{/if}</div>
          {#if j.result}<p class="text result" data-testid="task-job-result">{j.result}</p>{/if}
        </div>
      {/each}
    </section>
  {/if}

  <section data-testid="task-steps">
    <h4>Agent steps <span class="hint">from Claude Code tasks · per the agent, not proof of done</span></h4>
    {#each detail.steps ?? [] as g, i (g.claude_session_id)}
      <details open={i === 0}>
        <summary>{g.label} <span class="muted">{g.steps.filter((s) => s.state === 'completed').length}/{g.steps.length}</span></summary>
        <ul>
          {#each g.steps as s, k (k)}
            <li class="step step--{s.state}" data-testid="task-step">
              <span class="m" aria-hidden="true">{s.state === 'completed' ? '✓' : s.state === 'in_progress' ? '◐' : s.state === 'cancelled' ? '×' : '○'}</span>{s.text}
            </li>
          {/each}
        </ul>
      </details>
    {:else}
      <p class="muted">No agent steps recorded.</p>
    {/each}
  </section>
</div>

<style>
  .tws { display: grid; gap: 12px; }
  h4 { display: flex; gap: 6px; align-items: center; margin: 0 0 6px; font-size: 0.72rem; text-transform: uppercase; letter-spacing: 0.06em; color: var(--fg-muted); }
  h4 .btn { margin-left: auto; text-transform: none; letter-spacing: 0; }
  .n { font-variant-numeric: tabular-nums; font-weight: 500; }
  .hint { margin-left: auto; text-transform: none; letter-spacing: 0; font-weight: 400; }
  .text { margin: 0; white-space: pre-wrap; max-width: 72ch; }
  .row { display: flex; gap: 8px; align-items: center; padding: 3px 0; }
  .spacer { flex: 1; }
  .link { background: none; border: 0; padding: 0; color: var(--fg); font: inherit; cursor: pointer; text-align: left; }
  .key { font-family: var(--mono); font-size: 0.75rem; color: var(--fg-muted); }
  .chip { font-size: 0.7rem; border-radius: 999px; padding: 0 6px; }
  .prop { background: color-mix(in srgb, var(--usage-warn, #b45309) 14%, transparent); color: var(--usage-warn, #b45309); }
  .agent { background: color-mix(in srgb, #7c3aed 12%, transparent); color: #7c3aed; }
  .prop-card { border: 1px dashed var(--border); border-radius: 6px; padding: 8px 10px; display: grid; gap: 4px; margin-bottom: 6px; }
  .acts { display: flex; gap: 6px; }
  .job { padding: 4px 0; }
  .result { border-left: 2px solid #7c3aed; padding-left: 8px; }
  .state--done { color: var(--usage-ok, #2e7d32); }
  .state--failed, .state--cancelled { color: var(--usage-crit, #c62828); }
  .dot { width: 7px; height: 7px; border-radius: 50%; border: 1.5px solid var(--fg-muted); flex: none; }
  .dot--in_progress { background: var(--accent); border-color: var(--accent); }
  .dot--done { background: var(--usage-ok, #2e7d32); border-color: var(--usage-ok, #2e7d32); }
  details { border: 1px solid var(--border); border-radius: 6px; padding: 4px 8px; margin-bottom: 4px; }
  ul { list-style: none; margin: 4px 0; padding: 0 0 0 12px; }
  .step { display: flex; gap: 6px; }
  .m { font-family: var(--mono); color: var(--fg-muted); width: 1em; }
  .step--completed .m { color: var(--usage-ok, #2e7d32); }
  .step--in_progress .m { color: var(--accent); }
  .muted { color: var(--fg-muted); margin: 0; }
  .err { color: var(--usage-crit, #c62828); }
  .add { width: 100%; margin-bottom: 4px; }
</style>
```

If `readErrorText` does not accept `unknown`, narrow the type the way `WorkTaskDetail` does. Check that `btn--primary` is the house class name (`git grep -n "btn--primary\|btn--accent" src/app.css`) and use whichever exists.

- [ ] **Step 4: Mount in `WorkTaskDetail.svelte`:**
  - Import `TaskWorkSections` and render `{#if detail}<TaskWorkSections {detail} />{/if}` right after the action buttons and action-error block. The existing Sessions list stays below it, unchanged.
  - Wrap the org/group `<dl>` and the Place / Assign org / Make rule buttons in `<details data-testid="work-task-more"><summary>Placement & rules</summary> … </details>`, keeping every inner element and test id.
  - Update any `WorkTaskDetail.test.ts` assertion that clicked those buttons: open the `<details>` first with `await fireEvent.click(screen.getByText('Placement & rules'))`.

- [ ] **Step 5: Run** `pnpm vitest run src/lib/TaskWorkSections.test.ts src/lib/WorkTaskDetail.test.ts && pnpm run check`. Expected: PASS.

- [ ] **Step 6: Commit** `git commit -am "feat(ui): task page — notes, subtasks, proposals, jobs and agent steps"`

---

### Task 15: Remove the ☑ popover

**Files:**
- Modify: `src/lib/Sidebar.svelte` (import :98, `showTasks` :123, props :1165-1167, modal :1593-1597)
- Modify: `src/lib/SidebarFilters.svelte` (props ~70-72 and ~94-96, button :259-267)
- Modify: `src/lib/WorkViewSwitch.test.ts:42`

- [ ] **Step 1: Test first.** In `WorkViewSwitch.test.ts`, drop `'tasks-open'` from the global-chrome list, and after the loop add:

```ts
    // The ☑ Tasks popover is gone: delegated jobs live in the Work list.
    expect(screen.queryByTestId('tasks-open')).toBeNull();
```

Where it asserts `work-tree`, it still holds, because `WorkTree` stays mounted in the Work tab.

- [ ] **Step 2: Run** `pnpm vitest run src/lib/WorkViewSwitch.test.ts`. Expected: FAIL (the button is still there).

- [ ] **Step 3: Remove** the button and its two props from `SidebarFilters.svelte`, and the `showTasks` state, the props and the `{#if showTasks}<Modal …><TasksPanel /></Modal>{/if}` block from `Sidebar.svelte`. Remove the `TasksPanel` import there only if nothing else in the file uses it. `SessionDetails.svelte:627` keeps its `TasksPanel`.

- [ ] **Step 4: Run** `pnpm vitest run src/lib/WorkViewSwitch.test.ts src/lib/TasksPanel.test.ts && pnpm run check`. Expected: PASS.

- [ ] **Step 5: Commit** `git commit -am "feat(ui): remove the global Tasks popover; jobs are in the Work list"`

---

### Task 16: Docs, skill, full CI mirror, PR

**Files:**
- Modify: `skills/claude-fleet-control/SKILL.md` (the work section ~lines 225-269)
- Modify: `docs/work-graph.md`
- Modify: `docs/superpowers/specs/2026-09-29-shared-work-context-design.md` (Status line)

- [ ] **Step 1: Skill** — add under the work section:

```markdown
### Tasks, subtasks, proposals (shared work context)

- `work_link { action: "create", title, parent?: "item:<id>", project_id?, notes? }` — a native task `TASK-<id>`, or a subtask under `parent`. A standalone task needs the desktop or the master token; a per-host token may add subtasks under a task its own sessions work on.
- `work_link { action: "propose", parent, title, notes?, why? }` — propose a subtask. A person accepts or rejects it; you cannot. At most 10 open proposals per task; a rejected title is not accepted again.
- `work_link { action: "start", item_id }` on a subtask starts it in its project with the parent ticket's brief followed by the subtask's title and notes.
- Every `dispatch_task` job shows as an agent subtask under the requester's task and follows the job's state.
- Your `TaskCreate` / `TaskUpdate` steps are captured on the task page as "per the agent". They never mark anything done — the task's status and evidence do.
```

Then copy it to the local install (`cp skills/claude-fleet-control/SKILL.md ~/.claude/skills/claude-fleet-control/SKILL.md`). On this Mac `~/.claude` is a symlink into the dotfiles checkout: say so in the PR body and do not commit there.

- [ ] **Step 2: `docs/work-graph.md`** — add a section *The task list and the task page (2026-09-29)* covering:
  - the List | Grouped toggle, with List as the default and the header and filters unchanged;
  - To do / Doing / Done, with Done collapsed and covering 7 days;
  - + New task, ▾ project and notes, and Start;
  - subtasks, proposals (accept/reject), jobs and agent steps on the task page;
  - that tracker items stay read-only;
  - that hosts re-provision to pick up the new `PostToolUse` matcher (the Hosts view shows them as needing provisioning), and that until then the Stop hook's transcript backstop fills steps in.

- [ ] **Step 3: Spec status** → `**Status:** implemented on branch docs/internal-task-list-spec (PR link).`

- [ ] **Step 4: Full local CI mirror** (Global Constraints). Every command must pass, apart from the documented pre-existing failures, which must reproduce on `origin/main`.

- [ ] **Step 5: Commit, push, PR** (merge-commit style; ask the owner before merging):

```bash
git add skills/claude-fleet-control/SKILL.md docs/work-graph.md docs/superpowers/specs/2026-09-29-shared-work-context-design.md
git commit -m "docs(work): shared work context — skill, user guide, spec status"
git push
gh pr create --base main --head docs/internal-task-list-spec \
  --title "Shared work context: task list, subtasks, agent proposals, jobs and steps" \
  --body "Implements docs/superpowers/specs/2026-09-29-shared-work-context-design.md (roadmap part 1). Plan: docs/superpowers/plans/2026-09-29-shared-work-context.md. Mockup: https://claude.ai/artifact/V1HLQr4yBUdNDx1hof6Ums. Local CI mirror green (Actions billing-blocked). Hosts need re-provisioning for the new PostToolUse matcher."
```
