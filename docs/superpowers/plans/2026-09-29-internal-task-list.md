# One internal task list — implementation plan

> **Superseded — do not execute.** Replaced by `2026-09-29-shared-work-context.md` (roadmap part 1). Kept as history.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Work tab becomes one list of tasks. It holds tasks a person writes, tasks one session dispatches to another (`dispatch_task`), and detected tickets. The list is split into To do / Doing / Done, each task has a Start button, and the ☑ Tasks popover goes away.

**Architecture:**

- **Data.** Four additive columns on `work_items`: `origin`, `project_id`, `notes`, `task_id`.
  - A person creates a task through a new `work_link { action: create }`.
  - A dispatched job is mirrored best-effort into a work item. Its status follows the job, stamped `status_set_by = 'task'`, which ranks with `'derived'`.
- **Start.** It reuses the existing `start_work` path. The hub fills in a manual task's project and brief.
- **Desktop.** A new `TaskList.svelte` groups one `work_tree` read by status in the client. The old `WorkTree.svelte` stays reachable as "Grouped view" behind a ⋯ menu.

**Tech Stack:** Rust (fleet-core, rusqlite, rmcp), Tauri 2 commands, Svelte 5 runes, vitest + @testing-library/svelte.

**Spec:** `docs/superpowers/specs/2026-09-29-internal-task-list-design.md` — read its "Revisions after reading the code" section first; this plan follows the revisions where they differ from §1–§4.

## Global Constraints

- Repo: `martin-janci/claude-fleet`. Work in the worktree `.claude/worktrees/internal-task-list` (branch `docs/internal-task-list-spec`, off `origin/main`). All paths below are relative to it.
- **Store mutex:** take, work, drop — never hold `Mutex<Store>` across `.await`.
- **Best-effort writes** (the dispatch mirror) never fail the mutation that caused them: `let _ = …` / log and swallow.
- **Wire field names are snake_case**; every new Rust `Option<T>` field is `#[serde(default, skip_serializing_if = "Option::is_none")]` and the TS mirror is `field?: T | null`.
- **No `CONTRACT_REVISION` bump**: every wire change is additive, and `create` is a new action on an existing tool (an older hub answers `E_INVALID "unknown work_link action"`).
- **MCP tool descriptions/params change once**: the `work_link` params gain `notes` and the action enum gains `create` in the same release (tool-definition cache churn).
- `status_set_by` values after this change: `NULL` | `'person'` | `'derived'` | `'task'`; `'task'` ranks with `'derived'` (final over the live lift), below `'person'`.
- Task keys for native tasks: `TASK-<item id>` (upper case, e.g. `TASK-12`).
- Local title limit: `LOCAL_WORK_TITLE_MAX_CHARS` (120) via `validate_local_work_title`; notes limit: `service::work::handover::BRIEF_MAX_CHARS` (4000), cut, never refused.
- Done section window: 7 days (`DONE_WINDOW_SECS = 7 * 86_400`) on `last_activity_at`.
- Local CI mirror before the PR (GitHub Actions is billing-blocked):
  ```bash
  (cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test)
  (cargo fmt --check -p fleet-core && cargo clippy -p fleet-core --all-targets -- -D warnings && cargo test -p fleet-core)
  pnpm install --frozen-lockfile && pnpm run check && pnpm run test && pnpm run build
  ```
  Known pre-existing frontend failures (`localStorage is undefined` in `session_ui.test.ts`, `App.test.ts`) — verify against `main` before blaming a change.

---

## File structure

| File | Responsibility |
|---|---|
| `crates/fleet-core/migrations/086_internal_tasks.sql` (new) | The four columns, the backfill, two indexes. |
| `crates/fleet-core/src/store/schema.rs` | Register 086 with an `already_applied` guard; migration test. |
| `crates/fleet-core/src/store/work.rs` | `WorkItemRow` + `ITEM_COLUMNS` / `map_item` / `ITEM_COLUMN_COUNT`. |
| `crates/fleet-core/src/store/work_status.rs` + `service/work/status.rs` | `'task'` in the precedence (SQL macro and Rust). |
| `crates/fleet-core/src/store/work_tasks.rs` (new) + `work_tasks/tests.rs` | Native task writers: create a manual task, mirror a job, set status from a job, job states for the view. |
| `crates/fleet-core/src/service/tasks.rs` | The best-effort mirror calls at each job transition. |
| `crates/fleet-core/src/mcp/tools/orchestration.rs` | `dispatch_task` calls the mirror after `inherit_worker_work`; `work_link { create }` dispatch. |
| `crates/fleet-core/src/service/work/mod.rs` | `WorkLinkArgs.notes`, `"create"` in `WORK_LINK_ACTIONS`, `("create_work_task", "work_link", "create")` in `ROUTED_WORK_COMMANDS`. |
| `crates/fleet-core/src/service/work/local.rs` | `create_task` (the scope gate + store call). |
| `crates/fleet-core/src/service/trackers/tickets.rs` | `with_manual_defaults`: a manual item's project and brief on start. |
| `crates/fleet-core/src/service/work/view.rs` | `WorkTask` / `TaskDetail` new fields; job states in `Graph`. |
| `crates/fleet-core/src/mcp/tools/tests_isolation.rs` | A matrix row for `work_link create`. |
| `src-tauri/src/commands/work.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/backend/verdicts.rs`, `src-tauri/src/backend/tests_routing.rs` | The routed `create_work_task` command. |
| `src/lib/work_view.ts`, `src/lib/work.ts` | TS wire fields; `createWorkTask`. |
| `src/lib/task_list.ts` (new) + `task_list.test.ts` | Pure grouping: status sections, agent nesting, Done window, display title. |
| `src/lib/TaskList.svelte` (new) + `TaskList.test.ts` | The list, quick add, Start, ⋯ menu, Grouped view. |
| `src/lib/Sidebar.svelte`, `src/lib/SidebarFilters.svelte`, `src/lib/WorkViewSwitch.test.ts` | Mount `TaskList`; remove ☑ and its modal. |
| `skills/claude-fleet-control/SKILL.md`, `docs/work-graph.md`, `docs/control-api-reference.md` | Docs. |

---

### Task 1: Migration 086 and the new `WorkItemRow` columns

**Files:**
- Create: `crates/fleet-core/migrations/086_internal_tasks.sql`
- Modify: `crates/fleet-core/src/store/schema.rs` (MIGRATIONS array after the `85` entry ~line 921; guard fn near `work_items_has_status_set_at` ~line 377; test near `migration_081_…` ~line 3806)
- Modify: `crates/fleet-core/src/store/work.rs:127-192` (struct), `:529-574` (columns, count, map)
- Modify: `crates/fleet-core/src/service/work/today.rs:515` (test literal)

**Interfaces:**
- Produces: `WorkItemRow { origin: Option<String>, project_id: Option<i64>, notes: Option<String>, task_id: Option<i64>, .. }`; `ITEM_COLUMN_COUNT == 29`; guard `work_items_has_origin(&Connection) -> rusqlite::Result<bool>`.

- [ ] **Step 1: Write the failing migration test** (in `schema.rs` `mod tests`, after `migration_081_adds_the_pane_working_stamp_and_is_safe_to_rerun`)

```rust
    #[test]
    fn migration_086_adds_native_task_columns_backfills_origin_and_is_safe_to_rerun() {
        let s = store_at_version(85);
        s.conn
            .execute_batch(
                "INSERT INTO work_items (source, key, title, created_at, updated_at) \
                   VALUES ('local', 'OPS', 'named work', 1, 1);
                 INSERT INTO work_items (source, key, title, created_at, updated_at) \
                   VALUES ('local', NULL, 'keyless', 1, 1);
                 INSERT INTO work_items (source, key, title, created_at, updated_at) \
                   VALUES ('jira', 'TK-1', 'a ticket', 1, 1);",
            )
            .unwrap();
        assert!(!work_items_has_origin(&s.conn).unwrap());
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        let origins: Vec<(String, String)> = s
            .conn
            .prepare("SELECT title, origin FROM work_items ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            origins,
            vec![
                ("named work".into(), "manual".into()),
                ("keyless".into(), "manual".into()),
                ("a ticket".into(), "detected".into()),
            ]
        );
        s.conn
            .execute_batch("DELETE FROM schema_version WHERE version >= 86;")
            .unwrap();
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p fleet-core --lib migration_086`
Expected: FAIL — `cannot find function work_items_has_origin`.

- [ ] **Step 3: Write the migration**

`crates/fleet-core/migrations/086_internal_tasks.sql`:

```sql
-- One internal task list (design 2026-09-29): a work item says where it came
-- from, and a native task carries what starting it needs.
--
-- `origin`     manual | agent | detected. NULL reads as `detected` (a row an
--              older binary wrote during a rolling hub upgrade).
-- `project_id` the project a native task starts in.
-- `notes`      a native task's brief (≤ BRIEF_MAX_CHARS, cut by the writer).
-- `task_id`    the dispatched job (`tasks`) an `agent` item mirrors.
--
-- Backfill: every local item existing today was named by a person or an
-- agent through "Name this work…" (manual); every tracker item was synced
-- (detected). A bare ref has no row at all.
ALTER TABLE work_items ADD COLUMN origin     TEXT;
ALTER TABLE work_items ADD COLUMN project_id INTEGER REFERENCES projects(id) ON DELETE SET NULL;
ALTER TABLE work_items ADD COLUMN notes      TEXT;
ALTER TABLE work_items ADD COLUMN task_id    INTEGER REFERENCES tasks(id) ON DELETE SET NULL;

UPDATE work_items SET origin = CASE WHEN source = 'local' THEN 'manual' ELSE 'detected' END
 WHERE origin IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS ux_work_items_task
  ON work_items(task_id) WHERE task_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_work_items_origin_status
  ON work_items(origin, status_category);

INSERT OR IGNORE INTO schema_version (version) VALUES (86);
```

- [ ] **Step 4: Register it in `schema.rs`**

After the `Migration::plain(85, …)` entry:

```rust
    // One internal task list (design 2026-09-29): `work_items.origin`,
    // `project_id`, `notes`, `task_id`. The ADD COLUMNs are not idempotent,
    // so the same guard 084 uses; the backfill and indexes are.
    Migration {
        version: 86,
        sql: include_str!("../../migrations/086_internal_tasks.sql"),
        already_applied: Some(work_items_has_origin),
    },
```

Next to `work_items_has_status_set_at`:

```rust
/// `already_applied` guard of migration 086 (one internal task list).
fn work_items_has_origin(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('work_items') WHERE name = 'origin'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}
```

- [ ] **Step 5: Add the fields to `WorkItemRow`** (`store/work.rs`, after `unavailable_reason`)

```rust
    // --- native tasks (migration 086, design 2026-09-29); all default, so an
    // older hub's row still reads.
    /// manual | agent | detected. `None` from an older hub or row: read as
    /// `detected`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// The project a native task starts in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
    /// A native task's brief.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// The dispatched job an `agent` item mirrors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<i64>,
```

- [ ] **Step 6: Extend the column list, the count and the mapper**

```rust
pub(super) const ITEM_COLUMNS: &str =
    "id, source, key, title, url, status_category, created_at, updated_at, \
     tracker_id, external_id, aliases, kind, hierarchy_level, status_name, resolution, parent_id, \
     assignees, iteration, updated_ext, status_changed_at, fetched_at, unavailable_at, \
     unavailable_reason, status_set_by, status_set_at, origin, project_id, notes, task_id";

pub(super) const ITEM_COLUMN_COUNT: usize = 29;
```

In `map_item`, after `status_set_at: r.get(24)?,`:

```rust
        origin: r.get(25)?,
        project_id: r.get(26)?,
        notes: r.get(27)?,
        task_id: r.get(28)?,
```

`work_view_items` aliases the table `w` and selects `{ITEM_COLUMNS}` unqualified; the new names are unambiguous there. Check every other `ITEM_COLUMNS` use (`git grep -n ITEM_COLUMNS crates`, 18 hits) for a JOIN that would make `origin`/`project_id`/`notes`/`task_id` ambiguous. If a query joins a table with one of those column names (`sessions` has `project_id`), it must already qualify `ITEM_COLUMNS`; change it to select `{ITEM_COLUMNS}` through a subquery or a prefixed variant the same way it handles `id`. Fix any it finds.

- [ ] **Step 7: Fix the test literal** in `service/work/today.rs` (~line 515, `WorkItemRow { … }`): add

```rust
                origin: None,
                project_id: None,
                notes: None,
                task_id: None,
```

- [ ] **Step 8: Run the tests**

Run: `cargo test -p fleet-core --lib migration_086 && cargo test -p fleet-core --lib store::`
Expected: PASS. `LATEST_SCHEMA_VERSION` is now 86 and the `MIGRATIONS.len()` assertion holds.

- [ ] **Step 9: Commit**

```bash
git add crates/fleet-core/migrations/086_internal_tasks.sql crates/fleet-core/src/store/schema.rs crates/fleet-core/src/store/work.rs crates/fleet-core/src/service/work/today.rs
git commit -m "feat(work): migration 086 — origin, project, notes and job for native tasks"
```

---

### Task 2: `'task'` joins the status precedence

**Files:**
- Modify: `crates/fleet-core/src/service/work/status.rs:90-104` (+ tests in `effective_status_tests`)
- Modify: `crates/fleet-core/src/store/work_status.rs:103-124` (macro)
- Test: `crates/fleet-core/src/store/rows.rs` (the `the_sql_macro_and_the_rust_function_agree_arm_by_arm` block ~line 1740)

**Interfaces:**
- Produces: `effective_status(cat, Some("task"), src, working)` == `normalize(cat)`; SQL `effective_status_sql!()` agrees.

- [ ] **Step 1: Failing unit test** in `status.rs` `effective_status_tests`:

```rust
    #[test]
    fn a_jobs_status_is_final_over_the_live_lift() {
        // An agent task's status follows its dispatched job (design
        // 2026-09-29): a queued job whose worker is already working stays
        // `todo` until the job itself says `running`.
        assert_eq!(
            effective_status("todo", Some("task"), "local", true),
            Some("todo")
        );
        assert_eq!(
            effective_status("done", Some("task"), "local", true),
            Some("done")
        );
    }
```

- [ ] **Step 2: Run it** — `cargo test -p fleet-core --lib a_jobs_status_is_final` → FAIL (reads `in_progress`).

- [ ] **Step 3: Implement.** In `effective_status`:

```rust
        Some("person") | Some("derived") | Some("task") => normalize(status_category),
```

In the doc comment of `effective_status`, extend the precedence paragraph: "a job's status (`"task"`, an agent task mirroring its dispatched job) is final like a stamped `done`". In `effective_status_sql!`:

```rust
              WHEN i.status_set_by IN ('person', 'derived', 'task') THEN \
```

- [ ] **Step 4: Cross-check test.** In `store/rows.rs`, inside `the_sql_macro_and_the_rust_function_agree_arm_by_arm`, after the `// derived, with a working session` block, add a block that writes the stamp with a raw `UPDATE` (the store writer arrives in Task 3). That is the same kind of narrow raw write the file's `empty` block already uses:

```rust
            // a job's status (`'task'`), with a working session: final.
            {
                let s = Store::open_in_memory().unwrap();
                let sid = seed(&s, "task");
                let item = s.create_local_work_item(Some("X-6"), "t").unwrap();
                s.link_session_work(sid, WorkTarget::Item(item.id), "manual")
                    .unwrap();
                s.conn_ref()
                    .execute(
                        "UPDATE work_items SET status_category = 'todo', status_set_by = 'task' \
                         WHERE id = ?1",
                        [item.id],
                    )
                    .unwrap();
                mark_working(&s, sid, "c-x-6");
                agree(&s, "task", item.id, true, "job status, working session");
            }
```

(`conn_ref()` is what the `empty` block in the same test uses for its raw write.)

- [ ] **Step 5: Run** — `cargo test -p fleet-core --lib effective_status && cargo test -p fleet-core --lib agree_arm_by_arm` → PASS.

- [ ] **Step 6: Tidy is unaffected.** `service/gc/tidy.rs:499` reads only `'derived'`, so there is nothing to change. Run `cargo test -p fleet-core --lib tidy` → PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/fleet-core/src/service/work/status.rs crates/fleet-core/src/store/work_status.rs crates/fleet-core/src/store/rows.rs
git commit -m "feat(work): a job's status ('task') is final over the live lift"
```

---

### Task 3: Store writers for native tasks

**Files:**
- Create: `crates/fleet-core/src/store/work_tasks.rs`, `crates/fleet-core/src/store/work_tasks/tests.rs`
- Modify: `crates/fleet-core/src/store/mod.rs` (add `mod work_tasks;` next to `mod work_local;`, and `pub use work_tasks::{job_status, TASK_KEY_PREFIX};`)

**Interfaces:**
- Consumes: `WorkItemRow` fields from Task 1; `validate_local_work_title` (`store/work_local.rs`); `TaskRow` (`store`); `crate::service::work::handover::BRIEF_MAX_CHARS`.
- Produces:
  - `pub const TASK_KEY_PREFIX: &str = "TASK";`
  - `pub fn job_status(state: &str) -> &'static str` (queued→todo, running→in_progress, done|failed|cancelled→done)
  - `Store::create_manual_task(&self, title: &str, project_id: Option<i64>, notes: Option<&str>) -> Result<WorkItemRow, IpcError>`
  - `Store::create_agent_task_item(&self, task: &TaskRow, parent_item_id: Option<i64>, project_id: Option<i64>) -> Result<WorkItemRow, IpcError>`
  - `Store::work_item_for_task(&self, task_id: i64) -> Result<Option<WorkItemRow>, IpcError>`
  - `Store::set_item_status_from_task(&self, item_id: i64, status: &str) -> Result<bool, IpcError>`
  - `Store::job_states_by_item(&self) -> Result<HashMap<i64, String>, IpcError>`

- [ ] **Step 1: Write the failing tests** — `store/work_tasks/tests.rs`:

```rust
use super::*;
use crate::store::Store;

fn project(s: &Store) -> i64 {
    s.upsert_project("acme", "api", "/src/api").unwrap()
}

#[test]
fn a_manual_task_gets_a_task_key_its_project_and_notes() {
    let s = Store::open_in_memory().unwrap();
    let pid = project(&s);
    let t = s
        .create_manual_task("  Fix the login flake ", Some(pid), Some("see CI run 12"))
        .unwrap();
    assert_eq!(t.source, "local");
    assert_eq!(t.origin.as_deref(), Some("manual"));
    assert_eq!(t.key.as_deref(), Some(format!("TASK-{}", t.id).as_str()));
    assert_eq!(t.title, "Fix the login flake");
    assert_eq!(t.project_id, Some(pid));
    assert_eq!(t.notes.as_deref(), Some("see CI run 12"));
    assert_eq!(t.status_category, "todo");
}

#[test]
fn a_manual_task_refuses_an_empty_title_and_an_unknown_project() {
    let s = Store::open_in_memory().unwrap();
    assert_eq!(
        s.create_manual_task("  ", None, None).unwrap_err().code,
        crate::ipc_error::codes::E_INVALID
    );
    assert_eq!(
        s.create_manual_task("x", Some(999_999), None).unwrap_err().code,
        crate::ipc_error::codes::E_NOTFOUND
    );
}

#[test]
fn notes_are_cut_not_refused() {
    let s = Store::open_in_memory().unwrap();
    let long = "n".repeat(crate::service::work::handover::BRIEF_MAX_CHARS + 50);
    let t = s.create_manual_task("x", None, Some(&long)).unwrap();
    assert_eq!(
        t.notes.unwrap().chars().count(),
        crate::service::work::handover::BRIEF_MAX_CHARS
    );
}

#[test]
fn a_mirrored_job_is_an_agent_item_under_its_parent() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let w = s.upsert_session("w", "h", None, None, 0, 0, "running", None).unwrap();
    let parent = s.create_manual_task("parent", None, None).unwrap();
    let job = s
        .insert_task(None, Some(w), "Rebase onto main\nand fix conflicts", "n1")
        .unwrap();
    let it = s.create_agent_task_item(&job, Some(parent.id), None).unwrap();
    assert_eq!(it.origin.as_deref(), Some("agent"));
    assert_eq!(it.task_id, Some(job.id));
    assert_eq!(it.parent_id, Some(parent.id));
    assert_eq!(it.title, "Rebase onto main");
    assert_eq!(it.notes.as_deref(), Some("Rebase onto main\nand fix conflicts"));
    assert_eq!(it.status_category, "todo");
    assert_eq!(it.status_set_by.as_deref(), Some("task"));
    assert_eq!(s.work_item_for_task(job.id).unwrap().unwrap().id, it.id);
    // Mirroring the same job twice is one item.
    assert_eq!(s.create_agent_task_item(&job, None, None).unwrap().id, it.id);
}

#[test]
fn a_jobs_status_never_overrides_a_person() {
    let s = Store::open_in_memory().unwrap();
    let t = s.create_manual_task("x", None, None).unwrap();
    assert!(s.set_item_status_from_task(t.id, "in_progress").unwrap());
    assert_eq!(s.get_work_item(t.id).unwrap().unwrap().status_category, "in_progress");
    s.set_item_status(t.id, "done").unwrap();
    assert!(!s.set_item_status_from_task(t.id, "in_progress").unwrap());
    let row = s.get_work_item(t.id).unwrap().unwrap();
    assert_eq!((row.status_category.as_str(), row.status_set_by.as_deref()), ("done", Some("person")));
}

#[test]
fn job_states_map_by_item() {
    let s = Store::open_in_memory().unwrap();
    let job = s.insert_task(None, None, "p", "n").unwrap();
    let it = s.create_agent_task_item(&job, None, None).unwrap();
    s.mark_task_running(job.id).unwrap();
    assert_eq!(s.job_states_by_item().unwrap().get(&it.id).map(String::as_str), Some("running"));
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

- [ ] **Step 2: Run** — `cargo test -p fleet-core --lib work_tasks` → FAIL (module missing).

- [ ] **Step 3: Implement** — `store/work_tasks.rs`:

```rust
//! Native tasks (design 2026-09-29): work a person writes in fleet itself
//! (`origin = 'manual'`) and the mirror of a dispatched job (`'agent'`).
//! Both are local items with a `TASK-<id>` key, so the key-driven start path
//! (`service::trackers::tickets::start_work`) and branch detection work on
//! them unchanged.
//!
//! The only writers of `origin`, `project_id`, `notes` and `task_id`, and
//! (with `work_status.rs`) of `status_set_by`: `'task'` is written here only.

use super::work::{map_item, ITEM_COLUMNS};
use super::work_local::validate_local_work_title;
use super::{now_unix, Store, TaskRow, WorkItemRow};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use std::collections::HashMap;

/// The key prefix of a native task: `TASK-12`.
pub const TASK_KEY_PREFIX: &str = "TASK";

/// A dispatched job's state as a work status.
pub fn job_status(state: &str) -> &'static str {
    match state {
        "queued" => "todo",
        "running" => "in_progress",
        _ => "done",
    }
}

fn cut_notes(notes: &str) -> String {
    notes
        .chars()
        .take(crate::service::work::handover::BRIEF_MAX_CHARS)
        .collect()
}

/// A job's title: its prompt's first non-empty line, as a local title allows.
fn job_title(prompt: &str) -> String {
    let first = prompt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("Dispatched task");
    let clean: String = first.chars().filter(|c| !c.is_control()).collect();
    clean
        .chars()
        .take(super::work_local::LOCAL_WORK_TITLE_MAX_CHARS)
        .collect()
}

impl Store {
    /// Insert a native item and give it its `TASK-<id>` key, in one
    /// transaction. The caller has validated everything.
    fn insert_native_task(
        &self,
        origin: &str,
        title: &str,
        project_id: Option<i64>,
        notes: Option<&str>,
        task_id: Option<i64>,
        parent_id: Option<i64>,
        status_set_by: Option<&str>,
    ) -> Result<WorkItemRow, IpcError> {
        let now = now_unix();
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "INSERT INTO work_items (source, key, title, origin, project_id, notes, task_id, \
                                     parent_id, status_set_by, status_set_at, created_at, updated_at) \
             VALUES ('local', NULL, ?1, ?2, ?3, ?4, ?5, ?6, ?7, \
                     CASE WHEN ?7 IS NULL THEN NULL ELSE ?8 END, ?8, ?8)",
            rusqlite::params![title, origin, project_id, notes, task_id, parent_id, status_set_by, now],
        )?;
        let id = self.conn.last_insert_rowid();
        self.conn.execute(
            "UPDATE work_items SET key = ?1 WHERE id = ?2",
            rusqlite::params![format!("{TASK_KEY_PREFIX}-{id}"), id],
        )?;
        tx.commit()?;
        self.emit_work_item(
            id,
            super::tracker_items::SessionChange {
                primary: false,
                suggested: false,
                rejected: false,
            },
        )?;
        self.get_work_item(id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "work item vanished after insert"))
    }

    /// A task a person writes: `origin = 'manual'`, status `todo`.
    pub fn create_manual_task(
        &self,
        title: &str,
        project_id: Option<i64>,
        notes: Option<&str>,
    ) -> Result<WorkItemRow, IpcError> {
        let title = validate_local_work_title(title)?;
        if let Some(pid) = project_id {
            let known: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
                [pid],
                |r| r.get(0),
            )?;
            if !known {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("project {pid} not found"),
                ));
            }
        }
        let notes = notes.map(str::trim).filter(|n| !n.is_empty()).map(cut_notes);
        self.insert_native_task("manual", &title, project_id, notes.as_deref(), None, None, None)
    }

    /// Mirror a dispatched job as an `agent` item (once per job): its title
    /// is the prompt's first line, its notes the prompt, its status the
    /// job's (`status_set_by = 'task'`).
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
        let item = self.insert_native_task(
            "agent",
            &job_title(&prompt),
            project_id,
            Some(&cut_notes(&prompt)),
            Some(task.id),
            parent_item_id,
            Some("task"),
        )?;
        self.conn.execute(
            "UPDATE work_items SET status_category = ?1 WHERE id = ?2",
            rusqlite::params![job_status(&task.state), item.id],
        )?;
        self.get_work_item(item.id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "work item vanished after update"))
    }

    /// The item mirroring job `task_id`, if any.
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

    /// A job moved: its item's status follows, unless a person set one.
    /// Returns whether it wrote.
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
                super::tracker_items::SessionChange {
                    primary: true,
                    suggested: false,
                    rejected: false,
                },
            )?;
        }
        Ok(wrote)
    }

    /// `item id → tasks.state` for every mirrored job: one query for a view.
    pub fn job_states_by_item(&self) -> Result<HashMap<i64, String>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT w.id, t.state FROM work_items w JOIN tasks t ON t.id = w.task_id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
    }
}

#[cfg(test)]
mod tests;
```

Notes for the implementer:
- `TaskRow` lives in `store/rows.rs:1126`; `prompt: Option<String>`, `state: String`, `result: Option<String>`.
- `emit_work_item` / `SessionChange` are `pub(super)` in `tracker_items.rs`. `work_status.rs` already calls them as `super::tracker_items::SessionChange` from a sibling module, so the path works.
- If `work_local.rs`'s `LOCAL_WORK_TITLE_MAX_CHARS` is not `pub`, it is `pub const` today. Keep it that way.

- [ ] **Step 4: Run** — `cargo test -p fleet-core --lib work_tasks` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/store/work_tasks.rs crates/fleet-core/src/store/work_tasks crates/fleet-core/src/store/mod.rs
git commit -m "feat(work): store writers for native tasks (TASK-<id>) and job mirrors"
```

---

### Task 4: `work_link { action: create }` over MCP

**Files:**
- Modify: `crates/fleet-core/src/service/work/mod.rs` (`WorkLinkArgs` ~line 176: add `notes`; `WORK_LINK_ACTIONS` ~line 366: add `"create"`)
- Modify: `crates/fleet-core/src/service/work/local.rs` (add `create_task`)
- Modify: `crates/fleet-core/src/mcp/tools/orchestration.rs` (dispatch, before `if args.action == "name"` ~line 939)
- Test: `crates/fleet-core/src/service/work/local/tests.rs`, `crates/fleet-core/src/mcp/tools/tests_isolation.rs`

**Interfaces:**
- Consumes: `Store::create_manual_task` (Task 3).
- Produces: `service::work::local::create_task(args: &WorkLinkArgs, store: &Mutex<Store>, scope: &OrgScope) -> Result<WorkItemRow, IpcError>`; MCP `work_link { action: "create", title, project_id?, notes? }` → `WorkItemRow` JSON.

- [ ] **Step 1: Failing service tests** — append to `service/work/local/tests.rs`:

```rust
#[test]
fn create_makes_a_manual_task_for_an_unscoped_caller() {
    let store = std::sync::Mutex::new(crate::store::Store::open_in_memory().unwrap());
    let args = crate::service::work::WorkLinkArgs {
        action: "create".into(),
        title: Some("Write the release notes".into()),
        notes: Some("v0.4.3".into()),
        ..Default::default()
    };
    let t = super::create_task(&args, &store, &crate::service::orgs::OrgScope::All).unwrap();
    assert_eq!(t.origin.as_deref(), Some("manual"));
    assert_eq!(t.notes.as_deref(), Some("v0.4.3"));
}

#[test]
fn create_is_refused_to_a_scoped_caller() {
    let s = crate::store::Store::open_in_memory().unwrap();
    s.upsert_host("h").unwrap();
    let scope = crate::service::orgs::OrgScope::for_host(&s, "h").unwrap();
    let store = std::sync::Mutex::new(s);
    let args = crate::service::work::WorkLinkArgs {
        action: "create".into(),
        title: Some("x".into()),
        ..Default::default()
    };
    assert_eq!(
        super::create_task(&args, &store, &scope).unwrap_err().code,
        crate::ipc_error::codes::E_FORBIDDEN
    );
}
```

(If `local/tests.rs` imports differ, e.g. `use super::*;`, adapt the paths. The test bodies stay the same.)

- [ ] **Step 2: Run** — `cargo test -p fleet-core --lib create_makes_a_manual_task` → FAIL.

- [ ] **Step 3: Add `notes` to `WorkLinkArgs`** (after `title`):

```rust
    /// Create: the task's notes (its brief when started).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
```

Add `"create",` to `WORK_LINK_ACTIONS` right before `"name",`.

- [ ] **Step 4: Implement `create_task`** in `service/work/local.rs` (after `rename_local_item`):

```rust
/// `work_link { action: create, title, project_id?, notes? }`: a task a
/// person writes in fleet itself (design 2026-09-29), `TASK-<id>`, `todo`.
///
/// Only an unscoped caller (the desktop, the master, an unbound client) may
/// create one: a new task has no links, and a scoped caller sees a local
/// item only through its links — it could not read back what it made.
pub fn create_task(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &OrgScope,
) -> Result<WorkItemRow, IpcError> {
    if !scope.is_all() {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            "creating a task needs an unscoped caller (the desktop or the master token)",
        ));
    }
    let title = args
        .title
        .as_deref()
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "create needs title"))?;
    lock(store)?.create_manual_task(title, args.project_id, args.notes.as_deref())
}
```

- [ ] **Step 5: Dispatch it** in `orchestration.rs`, right before `if args.action == "name" {`:

```rust
        if args.action == "create" {
            // One internal task list (design 2026-09-29): a task a person
            // writes. The scope gate is inside (unscoped callers only).
            return ok_json(
                &crate::service::work::local::create_task(&args, &self.store, &scope)
                    .map_err(to_mcp_err)?,
            );
        }
```

- [ ] **Step 6: Isolation-matrix row** — in `tests_isolation.rs`, next to the `set_status` rows (~line 2320):

```rust
    // One internal task list: only an unscoped caller creates a task.
    m.row(
        "work_link",
        "create",
        |_, _| json!({ "action": "create", "title": "matrix task" }),
        |_, who, a| {
            if readonly_refused(who, a) {
                return;
            }
            match who {
                Who::Master | Who::ClientFull => assert!(
                    text(a).contains("\"origin\":\"manual\""),
                    "{who:?}: {a:?}"
                ),
                _ => is_code(who, a, "E_FORBIDDEN", "create needs an unscoped caller"),
            }
        },
    )
    .await;
```

If `Who::ClientFull` is bound to an org in this fixture (check `Who`'s doc and how its scope is built), it is not `is_all()`. In that case move it to the `E_FORBIDDEN` arm. The matrix answer is the truth, so read one failing run's output before editing.

- [ ] **Step 7: Run** — `cargo test -p fleet-core --lib create_ && cargo test -p fleet-core --lib isolation_matrix` → PASS (both matrix variants, including `every action has a row`).

- [ ] **Step 8: Refresh the generated reference** — `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` (if the test lives in `src-tauri`, run it there. Find it with `git grep -n reference_is_current`). Then run the test again without `REGEN_DOCS` → PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/fleet-core/src/service/work crates/fleet-core/src/mcp/tools docs/control-api-reference.md
git commit -m "feat(work): work_link { action: create } — a task a person writes"
```

---

### Task 5: Start a manual task with its own project and notes

**Files:**
- Modify: `crates/fleet-core/src/service/trackers/tickets.rs` (`start_work` ~line 1256; new helper above it)
- Test: the `tickets.rs` test module (find it with `git grep -n "fn start_work" crates/fleet-core/src/service/trackers/*tests*`; use the file where the existing `start_work` tests live)

**Interfaces:**
- Consumes: `WorkItemRow.origin/project_id/notes` (Task 1).
- Produces: `fn with_manual_defaults(store: &Mutex<Store>, args: &StartArgs) -> Result<StartArgs, IpcError>` (private).

- [ ] **Step 1: Failing pure test** (in the same file's test module):

```rust
#[test]
fn a_manual_task_starts_in_its_project_with_its_notes_as_the_brief() {
    let s = crate::store::Store::open_in_memory().unwrap();
    let pid = s.upsert_project("acme", "api", "/src/api").unwrap();
    let t = s.create_manual_task("Fix login", Some(pid), Some("CI run 12")).unwrap();
    let store = std::sync::Mutex::new(s);
    let args = StartArgs { item_id: Some(t.id), ..Default::default() };
    let got = with_manual_defaults(&store, &args).unwrap();
    assert_eq!(got.project_id, Some(pid));
    assert_eq!(got.brief.as_deref(), Some("Fix login\n\nCI run 12"));
    // A person's own choice wins.
    let args = StartArgs {
        item_id: Some(t.id),
        project_id: Some(77),
        brief: Some("mine".into()),
        ..Default::default()
    };
    let got = with_manual_defaults(&store, &args).unwrap();
    assert_eq!((got.project_id, got.brief.as_deref()), (Some(77), Some("mine")));
}
```

- [ ] **Step 2: Run** — FAIL (`with_manual_defaults` missing).

- [ ] **Step 3: Implement** (above `pub async fn start_work`):

```rust
/// A native task (design 2026-09-29) starts where its author said and with
/// what they wrote: its `project_id` and `title + notes` fill whatever the
/// start leaves empty. Anything else starts exactly as asked.
fn with_manual_defaults(store: &Mutex<Store>, args: &StartArgs) -> Result<StartArgs, IpcError> {
    let Some(id) = args.item_id else {
        return Ok(args.clone());
    };
    let item = match lock(store)?.get_work_item(id)? {
        Some(i) if i.origin.as_deref() == Some("manual") => i,
        _ => return Ok(args.clone()),
    };
    let mut out = args.clone();
    if out.project_id.is_none() {
        out.project_id = item.project_id;
    }
    if out.brief.is_none() {
        out.brief = Some(match item.notes.as_deref().filter(|n| !n.trim().is_empty()) {
            Some(n) => format!("{}\n\n{n}", item.title),
            None => item.title.clone(),
        });
    }
    Ok(out)
}
```

At the top of `start_work`, replace `let plan = plan_start(store, args, scope, net).await?;` with:

```rust
    let args = &with_manual_defaults(store, args)?;
    let plan = plan_start(store, args, scope, net).await?;
```

(The visibility check stays in `resolve_start`. The helper only reads, and it skips items that are not manual.)

- [ ] **Step 4: Run** — `cargo test -p fleet-core --lib a_manual_task_starts && cargo test -p fleet-core --lib start_work` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core/src/service/trackers/tickets.rs
git commit -m "feat(work): starting a native task uses its project and notes"
```

---

### Task 6: Mirror dispatched jobs into the list

**Files:**
- Modify: `crates/fleet-core/src/service/tasks.rs` (`start_task` :361, `fail_task` :372, `cancel_task` :385, `complete_task` :403; new `mirror_dispatched`, `mirror_state`)
- Modify: `crates/fleet-core/src/mcp/tools/orchestration.rs:446-459` (dispatch_task's create block)
- Test: `crates/fleet-core/src/service/tasks.rs` tests module

**Interfaces:**
- Consumes: `create_agent_task_item`, `work_item_for_task`, `set_item_status_from_task`, `job_status` (Task 3); `Store::session_work_links`, `Store::link_session_work_as`, `WorkTarget`.
- Produces: `pub fn mirror_dispatched(s: &Store, task: &TaskRow, requester: Option<i64>, worker: i64)`, `fn mirror_state(s: &Store, task_id: i64)` — both infallible (they log and swallow).

- [ ] **Step 1: Failing tests** (in `service/tasks.rs` `mod tests`, using its `seed` helper):

```rust
    #[test]
    fn a_dispatched_job_shows_under_its_requesters_task_and_follows_the_job() {
        let s = Store::open_in_memory().unwrap();
        let req = seed(&s, "local", "ctl");
        let w = seed(&s, "local", "w");
        let parent = s.create_manual_task("Ship v1", None, None).unwrap();
        s.link_session_work(req, crate::store::WorkTarget::Item(parent.id), "manual")
            .unwrap();
        let t = create_task(&s, Some(req), Some(w), "Write the changelog").unwrap();
        mirror_dispatched(&s, &t, Some(req), w);
        let it = s.work_item_for_task(t.id).unwrap().expect("mirrored");
        assert_eq!(it.parent_id, Some(parent.id));
        assert_eq!(it.status_category, "todo");
        // The worker is linked, as a secondary: its inherited primary stays.
        let links = s.session_work_links(w).unwrap();
        assert!(links.iter().any(|l| l.item_id == Some(it.id) && l.state == "confirmed"));

        let t = start_task(&s, &t).unwrap();
        assert_eq!(s.get_work_item(it.id).unwrap().unwrap().status_category, "in_progress");
        assert!(complete_task(&s, &t, "done: see CHANGELOG.md").unwrap());
        assert_eq!(s.get_work_item(it.id).unwrap().unwrap().status_category, "done");
    }

    #[test]
    fn a_failed_or_cancelled_job_is_done() {
        let s = Store::open_in_memory().unwrap();
        let w = seed(&s, "local", "w");
        let f = create_task(&s, None, Some(w), "a").unwrap();
        mirror_dispatched(&s, &f, None, w);
        fail_task(&s, f.id, "send failed").unwrap();
        let c = create_task(&s, None, Some(w), "b").unwrap();
        mirror_dispatched(&s, &c, None, w);
        cancel_task(&s, c.id, "by hand").unwrap();
        for id in [f.id, c.id] {
            let it = s.work_item_for_task(id).unwrap().unwrap();
            assert_eq!(it.status_category, "done");
            assert_eq!(it.parent_id, None, "no requester, no parent");
        }
    }

    #[test]
    fn a_job_with_no_mirror_still_moves() {
        // The mirror is best-effort: a job whose item was never made (an
        // older row, a failed insert) transitions exactly as before.
        let s = Store::open_in_memory().unwrap();
        let w = seed(&s, "local", "w");
        let t = create_task(&s, None, Some(w), "x").unwrap();
        let t = start_task(&s, &t).unwrap();
        assert!(complete_task(&s, &t, "ok").unwrap());
        assert!(s.work_item_for_task(t.id).unwrap().is_none());
    }
```

- [ ] **Step 2: Run** — `cargo test -p fleet-core --lib service::tasks` → FAIL (`mirror_dispatched` missing).

- [ ] **Step 3: Implement** (in `service/tasks.rs`, after `note_finished`):

```rust
/// One internal task list (design 2026-09-29): a dispatched job becomes an
/// `agent` work item — under the requester's primary work when it has one —
/// with the worker linked as a SECONDARY link, so the worker's inherited
/// primary (work graph M2.2) stays where `inherit_worker_work` put it.
///
/// Best-effort: the dispatch has already happened; a failure here is logged
/// and never reaches the caller.
pub fn mirror_dispatched(s: &Store, task: &TaskRow, requester: Option<i64>, worker: i64) {
    let run = || -> Result<(), IpcError> {
        let parent = match requester {
            Some(r) => s
                .session_work_links(r)?
                .into_iter()
                .find(|l| l.is_primary && l.state == "confirmed" && l.ended_at.is_none())
                .and_then(|l| l.item_id),
            None => None,
        };
        let project = s.get_session_by_id(worker)?.and_then(|r| r.project_id);
        let item = s.create_agent_task_item(task, parent, project)?;
        s.link_session_work_as(
            worker,
            crate::store::WorkTarget::Item(item.id),
            "agent_started",
            false,
            None,
        )?;
        Ok(())
    };
    if let Err(e) = run() {
        tracing::debug!(task = task.id, error = %e.message, "[tasks] mirroring a job into work failed");
    }
}

/// The job moved: its item's status follows (best-effort, never a person's
/// setting — `Store::set_item_status_from_task`).
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

Wire `mirror_state` into each transition, after the state write succeeded:
- `start_task`: after `let row = …?;` add `mirror_state(s, row.id);`
- `fail_task`: inside `if changed { … }` add `mirror_state(s, task_id);`
- `cancel_task`: after `note_finished(s, &row, "task_cancelled", reason);` add `mirror_state(s, row.id);`
- `complete_task`: inside `if let Some(ref r) = row { … }` at its top add `mirror_state(s, r.id);`

- [ ] **Step 4: Call the mirror from `dispatch_task`** (`orchestration.rs`, inside the `let task = { … }` block, after the `if let Some(req) = p.requester_session_id { … }` block, before `if let Some(cid) …`):

```rust
            // One internal task list: the job shows in the Work list, under
            // the requester's task (after inheritance, so the worker keeps
            // the requester's work as its primary).
            tasks::mirror_dispatched(&s, &task, p.requester_session_id, worker.id);
```

- [ ] **Step 5: Run** — `cargo test -p fleet-core --lib service::tasks && cargo test -p fleet-core --lib dispatch` → PASS (existing `wait_for_task` / dispatch tests unchanged).

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/src/service/tasks.rs crates/fleet-core/src/mcp/tools/orchestration.rs
git commit -m "feat(work): a dispatched job shows as an agent task under its requester's work"
```

---

### Task 7: The view carries origin, project, parent and job state

**Files:**
- Modify: `crates/fleet-core/src/service/work/view.rs` — `WorkTask` (:179), `TaskDetail` (:346), `Graph` (:463/486), `summarize` (:1139), `task()` (:1848)
- Test: `crates/fleet-core/src/service/work/view_tests.rs`

**Interfaces:**
- Consumes: `WorkItemRow.origin/project_id/notes/task_id` (Task 1), `Store::job_states_by_item` (Task 3).
- Produces (wire): `WorkTask.origin: String` (`manual|agent|detected`), `project_id: Option<i64>`, `project_label: Option<String>`, `parent_task_id: Option<String>`, `job_state: Option<String>`, `title_derived: bool`; `TaskDetail.notes: Option<String>`, `TaskDetail.job_result: Option<String>`.

- [ ] **Step 1: Failing tests** — append to `view_tests.rs`:

```rust
#[test]
fn native_tasks_carry_origin_project_parent_and_job_state() {
    let w = world();
    let (parent, child, job) = {
        let s = w.st.lock().unwrap();
        let pid = s.upsert_project("acme", "web", "/src/web").unwrap();
        let parent = s.create_manual_task("Ship v1", Some(pid), Some("notes")).unwrap();
        let job = s.insert_task(None, Some(w.s1), "Changelog", "n").unwrap();
        let child = s.create_agent_task_item(&job, Some(parent.id), Some(pid)).unwrap();
        (parent, child, job)
    };
    let p = page(&w, &OrgScope::All, WorkTreeFilters { archived: Some(true), ..Default::default() });
    let pt = p.tasks.iter().find(|t| t.item_id == Some(parent.id)).unwrap();
    assert_eq!(pt.origin, "manual");
    assert_eq!(pt.project_label.as_deref(), Some("acme/web"));
    let ct = p.tasks.iter().find(|t| t.item_id == Some(child.id)).unwrap();
    assert_eq!(ct.origin, "agent");
    assert_eq!(ct.parent_task_id.as_deref(), Some(format!("item:{}", parent.id).as_str()));
    assert_eq!(ct.job_state.as_deref(), Some("queued"));
    let tk = task_of(&p, "TK-1");
    assert_eq!(tk.origin, "detected");
    let d = task(&w.st, &OrgScope::All, &format!("item:{}", parent.id)).unwrap();
    assert_eq!(d.notes.as_deref(), Some("notes"));
    let _ = job;
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

(Adjust `WorkTarget::Key("#333")` to however the fixture's other tests link a bare ref. Find the existing pattern with `grep -n "WorkTarget::" view_tests.rs`. `"one"` is session `s1`'s tmux name in `world()`. If `link_name` prefers `friendly_name`, the tmux name is the fallback it uses.)

- [ ] **Step 2: Run** — `cargo test -p fleet-core --lib native_tasks_carry` → FAIL (no field `origin`).

- [ ] **Step 3: Wire fields.** In `WorkTask`, after `archived`:

```rust
    /// manual | agent | detected (design 2026-09-29). An older hub sends
    /// none: a reader treats empty as `detected`.
    #[serde(default)]
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
    /// `owner/repo` (or the repo alone for a local project).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_label: Option<String>,
    /// `item:<id>` of the task an agent task was dispatched under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task_id: Option<String>,
    /// The mirrored job's state (queued | running | done | failed | cancelled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_state: Option<String>,
    /// `title` is the first session's name, the item having none of its own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub title_derived: bool,
```

In `TaskDetail`, after `rules`:

```rust
    /// A native task's notes (its start brief).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// A finished job's result paragraph (agent tasks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_result: Option<String>,
```

- [ ] **Step 4: Load job states once.** Add to `Graph` (after `working_session_items`):

```rust
    /// `item id → tasks.state` for mirrored jobs: one query per read.
    pub(crate) job_states: HashMap<i64, String>,
```

and in `Graph::load`'s struct literal: `job_states: s.job_states_by_item()?,`.

- [ ] **Step 5: Fill them in `summarize`.** Before `let task = WorkTask {`:

```rust
    let origin = match item {
        Some(i) => i.item.origin.clone().unwrap_or_else(|| "detected".into()),
        None => "detected".into(),
    };
    let project_id = item.and_then(|i| i.item.project_id);
    let project_label = project_id.and_then(|p| g.projects.get(&p)).map(|p| {
        if p.owner.is_empty() || p.owner == "local" {
            p.repo.clone()
        } else {
            format!("{}/{}", p.owner, p.repo)
        }
    });
    let parent_task_id = item
        .filter(|i| i.item.origin.as_deref() == Some("agent"))
        .and_then(|i| i.item.parent_id)
        .map(|p| format!("item:{p}"));
    let job_state = item.and_then(|i| g.job_states.get(&i.item.id).cloned());
    let (title, title_derived) = if title.is_empty() {
        match listed.first() {
            Some((l, _, _)) => (link_name(g.row_of(l), l), true),
            None => (title, false),
        }
    } else {
        (title, false)
    };
```

and in the literal, after `archived,`:

```rust
        origin,
        project_id,
        project_label,
        parent_task_id,
        job_state,
        title_derived,
```

`title` is used by `group_of(…, &title, …)` before this point. Keep that call on the item's real title: move the fallback block to after `let group = …`, and keep `title` bound as it was for `group_of`. `listed` is the sorted visible-link list built above.

- [ ] **Step 6: Detail fields.** In `task()`, inside the second lock block, also read:

```rust
        let job_result = item
            .and_then(|i| i.item.task_id)
            .map(|t| s.get_task(t))
            .transpose()?
            .flatten()
            .and_then(|t| t.result);
```

return it from the block as a third tuple element, and set `notes: item.and_then(|i| i.item.notes.clone())`, `job_result` in `TaskDetail { … }`. For an `OrgScope::Host` caller, fence both as the description is fenced (`crate::mcp::guard::fence_untrusted(&n, "a task's notes", DESCRIPTION_MAX_CHARS)`), because notes and results are text an agent reads.

- [ ] **Step 7: Run** — `cargo test -p fleet-core --lib service::work::view` → PASS, and every existing view test unchanged.

- [ ] **Step 8: Commit**

```bash
git add crates/fleet-core/src/service/work/view.rs crates/fleet-core/src/service/work/view_tests.rs
git commit -m "feat(work): Work view tasks carry origin, project, parent and job state"
```

---

### Task 8: The desktop's `create_work_task` command

**Files:**
- Modify: `src-tauri/src/commands/work.rs` (args struct next to `RenameWorkItemArgs` :184; command next to `rename_work_item` :209; routed fn next to `routed::rename_work_item` :626)
- Modify: `src-tauri/src/lib.rs:393` (register), `src-tauri/src/backend/verdicts.rs:323` (verdict), `crates/fleet-core/src/service/work/mod.rs` (`ROUTED_WORK_COMMANDS`)
- Test: `src-tauri/src/backend/tests_routing.rs` (row next to `rename_work_item` ~line 779)

**Interfaces:**
- Consumes: `work::local::create_task` (Task 4).
- Produces: Tauri `create_work_task { args: { title, project_id?, notes? } } → WorkItemRow`.

- [ ] **Step 1: Routing test row** (copy the `rename_work_item` row's shape):

```rust
        (
            "create_work_task",
            "work_link",
            json!({ "action": "create", "title": "Write notes", "project_id": 3, "notes": "v1" }),
            r#"{"id":9,"source":"local","key":"TASK-9","title":"Write notes","status_category":"todo","created_at":1,"updated_at":1,"origin":"manual"}"#,
            Box::new(|b, s, _| {
                block_on(commands::work::routed::create_work_task(
                    b,
                    commands::work::CreateWorkTaskArgs {
                        title: "Write notes".into(),
                        project_id: Some(3),
                        notes: Some("v1".into()),
                    },
                    s,
                ))
                .map(|_| ())
            }),
        ),
```

The expected-args JSON must match exactly what the routed fn serialises. Mirror how neighbouring rows list `null` fields, since `WorkLinkArgs` skips `None`.

- [ ] **Step 2: Run** — `cargo test --manifest-path src-tauri/Cargo.toml routing` → FAIL (no `create_work_task`).

- [ ] **Step 3: Implement.** In `commands/work.rs`:

```rust
/// A task a person writes in the Work list (design 2026-09-29).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorkTaskArgs {
    pub title: String,
    #[serde(default)]
    pub project_id: Option<i64>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[tauri::command]
pub async fn create_work_task(
    args: CreateWorkTaskArgs,
    backend: State<'_, Arc<FleetBackend>>,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<WorkItemRow, IpcError> {
    routed::create_work_task(&backend, args, &store).await
}
```

In `mod routed`:

```rust
    pub async fn create_work_task(
        backend: &FleetBackend,
        args: CreateWorkTaskArgs,
        store: &Mutex<Store>,
    ) -> Result<WorkItemRow, IpcError> {
        let args = WorkLinkArgs {
            action: "create".into(),
            title: Some(args.title),
            project_id: args.project_id,
            notes: args.notes,
            ..Default::default()
        };
        match backend.hub() {
            Some(hub) => hub.route("create_work_task", &args).await,
            None => work::local::create_task(&args, store, &fleet_core::service::orgs::OrgScope::All),
        }
    }
```

Register `commands::work::create_work_task,` after `commands::work::rename_work_item,` in `lib.rs`. Add `("create_work_task", Verdict::Routed { tool: "work_link" }),` after the `rename_work_item` verdict. Add `("create_work_task", "work_link", "create"),` after `("rename_work_item", "work_link", "name"),` in `ROUTED_WORK_COMMANDS`.

- [ ] **Step 4: Run** — `cargo test --manifest-path src-tauri/Cargo.toml` → PASS. That covers routing, verdicts, and the command/verdict table agreement tests. If `local_only.golden.json` / `hub_contract.golden.json` tests fail, regenerate them as their module docs say (`REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract`) and read the diff: only additions are acceptable.

- [ ] **Step 5: Commit**

```bash
git add src-tauri crates/fleet-core/src/service/work/mod.rs
git commit -m "feat(work): desktop create_work_task, routed to work_link { create }"
```

---

### Task 9: Frontend data — wire types, `createWorkTask`, pure grouping

**Files:**
- Modify: `src/lib/work_view.ts:131-171` (`WorkTask`), the `TaskDetail` TS interface in the same file
- Modify: `src/lib/work.ts` (add `createWorkTask` after `nameSessionWork` ~line 417)
- Create: `src/lib/task_list.ts`, `src/lib/task_list.test.ts`

**Interfaces:**
- Produces:
  - `WorkTask.origin?: 'manual' | 'agent' | 'detected' | string | null`, `project_id?`, `project_label?`, `parent_task_id?`, `job_state?`, `title_derived?: boolean`
  - `createWorkTask(title: string, projectId?: number | null, notes?: string | null): Promise<Result<WorkItemRow>>`
  - `DONE_WINDOW_SECS = 7 * 86400`
  - `interface TaskNode { task: WorkTask; children: WorkTask[] }`
  - `interface StatusSections { todo: TaskNode[]; doing: TaskNode[]; done: TaskNode[] }`
  - `groupTasksByStatus(tasks: WorkTask[], nowSecs: number): StatusSections`
  - `displayTitle(t: WorkTask): string`

- [ ] **Step 1: Failing tests** — `src/lib/task_list.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { task } from './work_view_fixture';
import { DONE_WINDOW_SECS, displayTitle, groupTasksByStatus } from './task_list';

const NOW = 1_790_700_000;

describe('groupTasksByStatus', () => {
  it('sorts tasks into To do / Doing / Done by their effective status', () => {
    const s = groupTasksByStatus(
      [
        task({ task_id: 'item:1', status_category: 'todo', last_activity_at: NOW - 10 }),
        task({ task_id: 'item:2', status_category: 'in_progress', last_activity_at: NOW - 5 }),
        task({ task_id: 'item:3', status_category: 'done', last_activity_at: NOW - 60 }),
        task({ task_id: 'ref:#9', item_id: null, status_category: null, last_activity_at: NOW }),
      ],
      NOW,
    );
    expect(s.todo.map((n) => n.task.task_id)).toEqual(['ref:#9', 'item:1']);
    expect(s.doing.map((n) => n.task.task_id)).toEqual(['item:2']);
    expect(s.done.map((n) => n.task.task_id)).toEqual(['item:3']);
  });

  it('a task with a live session is Doing even when its stored status says to do', () => {
    const s = groupTasksByStatus(
      [task({ task_id: 'item:1', status_category: 'todo', counts: { active: 1, ended: 0, suggested: 0 } })],
      NOW,
    );
    expect(s.doing).toHaveLength(1);
  });

  it('Done keeps only the last 7 days', () => {
    const s = groupTasksByStatus(
      [
        task({ task_id: 'item:1', status_category: 'done', last_activity_at: NOW - DONE_WINDOW_SECS + 1 }),
        task({ task_id: 'item:2', status_category: 'done', last_activity_at: NOW - DONE_WINDOW_SECS - 1 }),
      ],
      NOW,
    );
    expect(s.done.map((n) => n.task.task_id)).toEqual(['item:1']);
  });

  it('nests an agent task under its parent when the parent is listed, else shows it on its own', () => {
    const s = groupTasksByStatus(
      [
        task({ task_id: 'item:1', origin: 'manual', status_category: 'in_progress' }),
        task({ task_id: 'item:2', origin: 'agent', parent_task_id: 'item:1', status_category: 'done' }),
        task({ task_id: 'item:3', origin: 'agent', parent_task_id: 'item:99', status_category: 'todo' }),
      ],
      NOW,
    );
    expect(s.doing[0].children.map((c) => c.task_id)).toEqual(['item:2']);
    expect(s.done).toHaveLength(0);
    expect(s.todo.map((n) => n.task.task_id)).toEqual(['item:3']);
  });
});

describe('displayTitle', () => {
  it('prefers the title, then the key, then the task id', () => {
    expect(displayTitle(task({ title: 'Login', key: 'ABC-1' }))).toBe('Login');
    expect(displayTitle(task({ title: '', key: 'ABC-1' }))).toBe('ABC-1');
    expect(displayTitle(task({ title: '', key: null, task_id: 'ref:x' }))).toBe('ref:x');
  });
});
```

- [ ] **Step 2: Run** — `pnpm vitest run src/lib/task_list.test.ts` → FAIL (module missing).

- [ ] **Step 3: Extend `WorkTask`** in `work_view.ts` (after `sessions_more?: number;`):

```ts
  /** manual | agent | detected (design 2026-09-29); absent from an older hub = detected. */
  origin?: 'manual' | 'agent' | 'detected' | string | null;
  project_id?: number | null;
  /** `owner/repo`, or the repo alone. */
  project_label?: string | null;
  /** `item:<id>` an agent task was dispatched under. */
  parent_task_id?: string | null;
  /** The mirrored job's state: queued | running | done | failed | cancelled. */
  job_state?: string | null;
  /** The title is the first session's name, the task having none of its own. */
  title_derived?: boolean;
```

and to the TS `TaskDetail` interface: `notes?: string | null; job_result?: string | null;`.

- [ ] **Step 4: `task_list.ts`:**

```ts
// The Work list (design 2026-09-29): one `work_tree` read grouped by status
// in the client — To do, Doing, Done (last 7 days) — with agent tasks nested
// under the task they were dispatched from. Pure, so it is tested alone.
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

type Section = keyof StatusSections;

function sectionOf(t: WorkTask): Section {
  // A live session means someone is on it, whatever the stored status says
  // (a tracker item's status belongs to its tracker, but the list is about
  // what is happening now).
  if (t.status_category === 'done' && (t.counts?.active ?? 0) === 0) return 'done';
  if (t.status_category === 'in_progress' || (t.counts?.active ?? 0) > 0) return 'doing';
  return 'todo';
}

const newestFirst = (a: WorkTask, b: WorkTask) => (b.last_activity_at ?? 0) - (a.last_activity_at ?? 0);

export function groupTasksByStatus(tasks: WorkTask[], nowSecs: number): StatusSections {
  const byId = new Map(tasks.map((t) => [t.task_id, t]));
  const children = new Map<string, WorkTask[]>();
  const roots: WorkTask[] = [];
  for (const t of tasks) {
    const parent = t.parent_task_id ? byId.get(t.parent_task_id) : undefined;
    if (parent) {
      const list = children.get(parent.task_id) ?? [];
      list.push(t);
      children.set(parent.task_id, list);
    } else {
      roots.push(t);
    }
  }
  const out: StatusSections = { todo: [], doing: [], done: [] };
  for (const t of roots.sort(newestFirst)) {
    const s = sectionOf(t);
    if (s === 'done' && (t.last_activity_at ?? 0) < nowSecs - DONE_WINDOW_SECS) continue;
    out[s].push({ task: t, children: (children.get(t.task_id) ?? []).sort(newestFirst) });
  }
  return out;
}

export function displayTitle(t: WorkTask): string {
  return t.title || t.key || t.task_id;
}
```

- [ ] **Step 5: `createWorkTask`** in `work.ts`, after `nameSessionWork`:

```ts
/** A task a person writes in the Work list (design 2026-09-29). */
export async function createWorkTask(
  title: string,
  projectId?: number | null,
  notes?: string | null,
): Promise<Result<WorkItemRow>> {
  const n = notes?.trim();
  const r = await invokeCmd<WorkItemRow>('create_work_task', {
    args: { title: title.trim(), ...(projectId != null ? { project_id: projectId } : {}), ...(n ? { notes: n } : {}) },
  });
  if (r.ok) bumpWorkChanged();
  return r;
}
```

(Import `WorkItemRow` from where `work.ts` already imports row types. If there is no TS `WorkItemRow`, use `{ id: number; key?: string | null; title: string }` inline as the result type.)

- [ ] **Step 6: Run** — `pnpm vitest run src/lib/task_list.test.ts src/lib/work.test.ts src/lib/work_view.test.ts` → PASS; `pnpm run check` → 0 errors.

- [ ] **Step 7: Commit**

```bash
git add src/lib/work_view.ts src/lib/work.ts src/lib/task_list.ts src/lib/task_list.test.ts
git commit -m "feat(work): Work list data — wire fields, createWorkTask, status grouping"
```

---

### Task 10: `TaskList.svelte` and the sidebar swap

**Files:**
- Create: `src/lib/TaskList.svelte`, `src/lib/TaskList.test.ts`
- Modify: `src/lib/Sidebar.svelte:1178-1179` (mount), `:98`, `:123`, `:1165-1167`, `:1593-1597` (remove the ☑ modal)
- Modify: `src/lib/SidebarFilters.svelte:70-96, 259-267` (remove the ☑ button and its props)
- Modify: `src/lib/WorkViewSwitch.test.ts:36-43`

**Interfaces:**
- Consumes: `workTree`, `openTask`, `selectedTaskId`, `revealTaskRequest`, `workViewFilters` (`work_view.ts`); `onWorkChangedDebounced`, `createWorkTask` (`work.ts`); `startWork` (`trackers.ts`); `projects`, `loadProjects` (`projects.ts`); `groupTasksByStatus`, `displayTitle` (`task_list.ts`); `WorkTree.svelte`, `WorkReview.svelte`, `WorkRules.svelte`.
- Produces: `<TaskList />` with test ids `task-list`, `task-add-input`, `task-add-more`, `task-add-project`, `task-add-notes`, `task-section-todo|doing|done`, `task-row`, `task-start`, `task-child`, `task-more-menu`, `task-view-grouped`, `task-view-review`, `task-view-rules`, `task-back-to-list`.

- [ ] **Step 1: Failing component tests** — `src/lib/TaskList.test.ts`:

```ts
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import TaskList from './TaskList.svelte';
import { task } from './work_view_fixture';
import type { WorkTreePage } from './work_view';

const NOW = Math.floor(Date.now() / 1000);
const page: WorkTreePage = {
  tasks: [
    task({ task_id: 'item:1', item_id: 1, key: 'TASK-1', title: 'Write notes', kind: 'local', origin: 'manual',
      status_category: 'todo', counts: { active: 0, ended: 0, suggested: 0 }, sessions: [], last_activity_at: NOW,
      project_id: 3, project_label: 'acme/api' }),
    task({ task_id: 'item:2', item_id: 2, title: 'Ship v1', kind: 'local', origin: 'manual',
      status_category: 'in_progress', last_activity_at: NOW }),
    task({ task_id: 'item:3', item_id: 3, title: 'Changelog', kind: 'local', origin: 'agent',
      parent_task_id: 'item:2', job_state: 'running', status_category: 'in_progress', last_activity_at: NOW }),
  ],
  groups: [], orgs: [], trackers: [], total: 3, archived_hidden: 0, next_cursor: null, generated_at: NOW,
};

const flush = async () => { for (let i = 0; i < 5; i++) { await Promise.resolve(); await tick(); } };

beforeEach(() => {
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
  it('shows To do, Doing and Done with agent tasks nested', async () => {
    render(TaskList);
    await flush();
    expect(screen.getByTestId('task-list')).toBeTruthy();
    expect(screen.getByTestId('task-section-todo').textContent).toContain('Write notes');
    const doing = screen.getByTestId('task-section-doing');
    expect(doing.textContent).toContain('Ship v1');
    expect(screen.getByTestId('task-child').textContent).toContain('Changelog');
  });

  it('reads the tree once, with archived tasks included', async () => {
    render(TaskList);
    await flush();
    const calls = (invoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'work_tree');
    expect(calls).toHaveLength(1);
    expect((calls[0][1] as { args: { filters: { archived: boolean } } }).args.filters.archived).toBe(true);
  });

  it('quick add creates a task from the title', async () => {
    render(TaskList);
    await flush();
    const input = screen.getByTestId('task-add-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'New' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await flush();
    const call = (invoke as ReturnType<typeof vi.fn>).mock.calls.find((c) => c[0] === 'create_work_task');
    expect(call?.[1]).toEqual({ args: { title: 'New' } });
  });

  it('Start starts the task by item id', async () => {
    render(TaskList);
    await flush();
    await fireEvent.click(screen.getByTestId('task-start'));
    await flush();
    const call = (invoke as ReturnType<typeof vi.fn>).mock.calls.find((c) => c[0] === 'start_work');
    expect(call?.[1]).toEqual({ args: { item_id: 1, project_id: 3 } });
  });

  it('the ⋯ menu opens the grouped view and comes back', async () => {
    render(TaskList);
    await flush();
    await fireEvent.click(screen.getByTestId('task-more-menu'));
    await fireEvent.click(screen.getByTestId('task-view-grouped'));
    await flush();
    expect(screen.getByTestId('work-tree')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('task-back-to-list'));
    await flush();
    expect(screen.getByTestId('task-list')).toBeTruthy();
  });
});
```

`startWork`'s `StartWorkArgs` TS type lives in `trackers.ts`. Check that it has `item_id` and `project_id`, and add them if missing. The Tauri `StartWorkArgs` struct already has both.

- [ ] **Step 2: Run** — `pnpm vitest run src/lib/TaskList.test.ts` → FAIL (component missing).

- [ ] **Step 3: Implement `TaskList.svelte`:**

```svelte
<script lang="ts">
  // The Work list (design 2026-09-29): every task — written here, dispatched
  // by an agent, or detected from a ticket key — in To do / Doing / Done,
  // from ONE `work_tree` read per refresh. The org → group tree it replaces
  // stays one click away (⋯ → Grouped view), with Review and Rules.
  import { onDestroy, onMount } from 'svelte';
  import { get } from 'svelte/store';
  import WorkTree from './WorkTree.svelte';
  import WorkReview from './WorkReview.svelte';
  import WorkRules from './WorkRules.svelte';
  import { createWorkTask, onWorkChangedDebounced } from './work';
  import { openTask, readErrorText, revealTaskRequest, selectedTaskId, workTree, type WorkTask } from './work_view';
  import { startWork } from './trackers';
  import { projects, loadProjects } from './projects';
  import { displayTitle, groupTasksByStatus, type StatusSections } from './task_list';
  import type { IpcError } from './result';

  let { debounceMs = 500, maxWaitMs = 3000 }: { debounceMs?: number; maxWaitMs?: number } = $props();

  type Mode = 'list' | 'grouped' | 'review';
  let mode = $state<Mode>('list');
  let menuOpen = $state(false);
  let rulesOpen = $state(false);
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
    const r = await workTree({ filters: { archived: true }, limit: 200, per_task: 3 });
    if (mine !== seq) return;
    loaded = true;
    if (!r.ok) {
      error = r.error;
      return;
    }
    error = null;
    tasks = Array.isArray(r.value?.tasks) ? r.value.tasks : [];
  }

  const offChanged = onWorkChangedDebounced(
    () => {
      if (mode === 'list') void load();
    },
    () => debounceMs,
    () => maxWaitMs,
  );
  // "Show in Work view" is the grouped tree's to answer.
  const offReveal = revealTaskRequest.subscribe((req) => {
    if (req) mode = 'grouped';
  });

  onMount(() => {
    void load();
    if (get(projects).length === 0) void loadProjects();
  });
  onDestroy(() => {
    offChanged();
    offReveal();
  });

  async function add() {
    const title = addTitle.trim();
    if (!title || busy) return;
    busy = true;
    const r = await createWorkTask(title, addProject, addNotes);
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

  function show(m: Mode) {
    menuOpen = false;
    mode = m;
    if (m === 'list') void load();
  }

  function canStart(t: WorkTask): boolean {
    return t.origin === 'manual' && (t.counts?.active ?? 0) === 0 && t.status_category !== 'done';
  }
</script>

{#if mode === 'grouped'}
  <div class="back"><button class="btn btn--quiet" type="button" data-testid="task-back-to-list" onclick={() => show('list')}>← Task list</button></div>
  <WorkTree />
{:else if mode === 'review'}
  <div class="back"><button class="btn btn--quiet" type="button" data-testid="task-back-to-list" onclick={() => show('list')}>← Task list</button></div>
  <WorkReview onchanged={() => {}} />
{:else}
  <div class="task-list" data-testid="task-list">
    <header class="head">
      <input
        class="add"
        placeholder="+ New task"
        aria-label="New task"
        data-testid="task-add-input"
        bind:value={addTitle}
        onkeydown={(e) => {
          if (e.key === 'Enter') void add();
        }}
      />
      <button class="btn btn--quiet btn--icon" type="button" title="Project and notes" aria-label="Project and notes"
        data-testid="task-add-more" aria-expanded={addMore} onclick={() => (addMore = !addMore)}>▾</button>
      <div class="menu-wrap">
        <button class="btn btn--quiet btn--icon" type="button" aria-label="More" aria-expanded={menuOpen}
          data-testid="task-more-menu" onclick={() => (menuOpen = !menuOpen)}>⋯</button>
        {#if menuOpen}
          <div class="menu" role="menu">
            <button role="menuitem" type="button" data-testid="task-view-grouped" onclick={() => show('grouped')}>Grouped view (orgs, groups, filters)</button>
            <button role="menuitem" type="button" data-testid="task-view-review" onclick={() => show('review')}>Review link suggestions</button>
            <button role="menuitem" type="button" data-testid="task-view-rules" onclick={() => { menuOpen = false; rulesOpen = true; }}>Placement rules</button>
          </div>
        {/if}
      </div>
    </header>
    {#if addMore}
      <div class="add-more">
        <select aria-label="Project" data-testid="task-add-project" bind:value={addProject}>
          <option value={null}>No project</option>
          {#each pickable as p (p.project.id)}
            <option value={p.project.id}>{p.project.owner && p.project.owner !== 'local' ? `${p.project.owner}/${p.project.repo}` : p.project.repo}</option>
          {/each}
        </select>
        <textarea rows="3" placeholder="Notes (the first prompt when started)" aria-label="Notes" data-testid="task-add-notes" bind:value={addNotes}></textarea>
      </div>
    {/if}
    {#if actionError}<p class="err" role="alert">{actionError}</p>{/if}

    <div class="scroller">
      {#if error}
        <p class="err" role="alert">{readErrorText(error)} <button class="btn btn--quiet" type="button" onclick={() => void load()}>Retry</button></p>
      {:else if !loaded}
        <p class="muted">Loading tasks…</p>
      {:else if tasks.length === 0}
        <p class="muted">No tasks yet. Type one above and press Enter.</p>
      {:else}
        {@render section('todo', 'To do', sections.todo, true)}
        {@render section('doing', 'Doing', sections.doing, true)}
        {@render section('done', 'Done · last 7 days', sections.done, doneOpen)}
      {/if}
    </div>
  </div>
{/if}

{#if rulesOpen}
  <WorkRules onclose={() => (rulesOpen = false)} />
{/if}

{#snippet section(id: 'todo' | 'doing' | 'done', label: string, nodes: StatusSections['todo'], open: boolean)}
  <section data-testid="task-section-{id}">
    <h3>
      {#if id === 'done'}
        <button class="sec-toggle" type="button" aria-expanded={open} onclick={() => (doneOpen = !doneOpen)}>{open ? '▾' : '▸'} {label}</button>
      {:else}
        {label}
      {/if}
      <span class="count">{nodes.length}</span>
    </h3>
    {#if open}
      <ul>
        {#each nodes as n (n.task.task_id)}
          <li class:selected={$selectedTaskId === n.task.task_id}>
            {@render row(n.task, false)}
            {#if n.children.length > 0}
              <ul class="children">
                {#each n.children as c (c.task_id)}
                  <li data-testid="task-child">{@render row(c, true)}</li>
                {/each}
              </ul>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </section>
{/snippet}

{#snippet row(t: WorkTask, child: boolean)}
  <div class="row" data-testid={child ? undefined : 'task-row'}>
    <button class="title" type="button" onclick={() => openTask(t.task_id)} title={t.key ?? t.task_id}>
      {#if t.origin === 'agent'}<span class="badge" title="dispatched by an agent">🤖</span>{/if}
      <span class:derived={t.title_derived}>{displayTitle(t)}</span>
    </button>
    {#if t.job_state && t.origin === 'agent'}<span class="meta">{t.job_state}</span>{/if}
    {#if t.project_label}<span class="meta">{t.project_label}</span>{/if}
    {#if (t.counts?.active ?? 0) > 0}<span class="meta">{t.counts?.active} live</span>{/if}
    {#if !child && canStart(t)}
      <button class="btn btn--chip" type="button" data-testid="task-start" disabled={busy} onclick={() => void start(t)}>Start</button>
    {/if}
  </div>
{/snippet}

<style>
  .task-list { display: flex; flex-direction: column; flex: 1 1 auto; min-height: 0; font-size: 0.85rem; }
  .head { display: flex; gap: 0.3rem; align-items: center; padding: 0.4rem 0.6rem; border-bottom: 1px solid var(--border); }
  .add { flex: 1 1 auto; min-width: 0; }
  .add-more { display: flex; flex-direction: column; gap: 0.3rem; padding: 0.4rem 0.6rem; border-bottom: 1px solid var(--border); }
  .menu-wrap { position: relative; }
  .menu { position: absolute; right: 0; top: 100%; z-index: 10; display: flex; flex-direction: column; min-width: 14rem;
    background: var(--bg-pane); border: 1px solid var(--border); border-radius: 6px; padding: 0.2rem; }
  .menu button { text-align: left; background: transparent; border: 0; color: var(--fg); font: inherit; padding: 0.3rem 0.5rem; border-radius: 4px; cursor: pointer; }
  .menu button:hover { background: color-mix(in srgb, var(--accent) 10%, transparent); }
  .scroller { flex: 1 1 auto; overflow: auto; min-height: 0; padding: 0.4rem 0.6rem; }
  h3 { display: flex; align-items: center; gap: 0.4rem; margin: 0.6rem 0 0.2rem; font-size: 0.75rem; text-transform: uppercase; color: var(--fg-muted); }
  .count { margin-left: auto; }
  .sec-toggle { background: transparent; border: 0; color: inherit; font: inherit; text-transform: inherit; cursor: pointer; padding: 0; }
  ul { list-style: none; margin: 0; padding: 0; }
  .children { padding-left: 1.2rem; }
  li.selected > .row { background: color-mix(in srgb, var(--accent) 14%, transparent); }
  .row { display: flex; align-items: center; gap: 0.35rem; padding: 0.15rem 0.2rem; border-radius: 4px; }
  .title { flex: 1 1 auto; min-width: 0; display: flex; gap: 0.3rem; background: transparent; border: 0; color: var(--fg);
    font: inherit; text-align: left; cursor: pointer; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; padding: 0; }
  .derived { font-style: italic; color: var(--fg-muted); }
  .meta { color: var(--fg-muted); font-size: 0.72rem; white-space: nowrap; }
  .back { padding: 0.3rem 0.6rem; border-bottom: 1px solid var(--border); }
  .muted { color: var(--fg-muted); }
  .err { color: var(--usage-crit, #c62828); margin: 0.3rem 0.6rem; }
</style>
```

Adapt to the house components if the reviewer asks (`Modal`, the `btn` classes are already global). Keep test ids stable.

- [ ] **Step 4: Swap it into the sidebar.** `Sidebar.svelte`: replace `import WorkTree from './WorkTree.svelte';` with `import TaskList from './TaskList.svelte';`, and `<WorkTree />` (line ~1179) with `<TaskList />`. Remove the ☑ popover in the same file: `import TasksPanel …` (line 98; keep it if `TasksPanel` is still used elsewhere in the file, which it is not), `let showTasks = $state(false);` (123), the `{showTasks}` / `onOpenTasks={…}` props (1165-1167) and the `{#if showTasks} <Modal …><TasksPanel /></Modal> {/if}` block (1593-1597). In `SidebarFilters.svelte` remove the `showTasks`/`onOpenTasks` props (lines ~70-72, ~94-96) and the `data-testid="tasks-open"` button (259-267). `TasksPanel` stays in `SessionDetails.svelte:627`.

- [ ] **Step 5: Update `WorkViewSwitch.test.ts`:** `'work-tree'` → `'task-list'` (both occurrences), and drop `'tasks-open'` from the "global chrome" list, adding a line after it:

```ts
    // The ☑ Tasks popover is gone: dispatched tasks live in the Work list.
    expect(screen.queryByTestId('tasks-open')).toBeNull();
```

- [ ] **Step 6: Run** — `pnpm vitest run src/lib/TaskList.test.ts src/lib/WorkViewSwitch.test.ts src/lib/WorkTree.test.ts src/lib/TasksPanel.test.ts` → PASS; `pnpm run check` → 0 errors.

- [ ] **Step 7: Commit**

```bash
git add src/lib/TaskList.svelte src/lib/TaskList.test.ts src/lib/Sidebar.svelte src/lib/SidebarFilters.svelte src/lib/WorkViewSwitch.test.ts src/lib/trackers.ts
git commit -m "feat(ui): the Work tab is one task list — To do / Doing / Done, quick add, Start"
```

---

### Task 11: Task detail shows notes and the job's result

**Files:**
- Modify: `src/lib/WorkTaskDetail.svelte`
- Test: `src/lib/WorkTaskDetail.test.ts`

**Interfaces:**
- Consumes: `TaskDetail.notes`, `TaskDetail.job_result` (Task 7/9).

- [ ] **Step 1: Failing test** — add to `WorkTaskDetail.test.ts`, following that file's existing mock of `work_task`:

```ts
  it('shows a native task\'s notes and an agent task\'s result as text', async () => {
    mockTaskDetail({ task: task({ task_id: 'item:3', origin: 'agent', job_state: 'done' }), notes: 'Rebase <b>now</b>', job_result: 'Rebased; 2 conflicts fixed.' });
    render(WorkTaskDetail, { taskId: 'item:3' });
    await flush();
    expect(screen.getByTestId('task-notes').textContent).toBe('Rebase <b>now</b>');
    expect(screen.getByTestId('task-job-result').textContent).toContain('2 conflicts fixed');
  });
```

(`mockTaskDetail` / `flush`: use whatever helper the file already defines to answer `work_task`, and rename these to match it. The assertion that `<b>` renders as text is the point.)

- [ ] **Step 2: Run** — FAIL.

- [ ] **Step 3: Implement** — in `WorkTaskDetail.svelte`, next to where the description renders:

```svelte
{#if detail.notes}
  <section class="notes"><h4>Notes</h4><p data-testid="task-notes">{detail.notes}</p></section>
{/if}
{#if detail.job_result}
  <section class="result"><h4>Agent result</h4><p data-testid="task-job-result">{detail.job_result}</p></section>
{/if}
```

(Plain `{…}` interpolation escapes markup. Never use `{@html}` here.)

- [ ] **Step 4: Run** — `pnpm vitest run src/lib/WorkTaskDetail.test.ts` → PASS.

- [ ] **Step 5: Commit**

```bash
git add src/lib/WorkTaskDetail.svelte src/lib/WorkTaskDetail.test.ts
git commit -m "feat(ui): task detail shows notes and an agent task's result"
```

---

### Task 12: Docs, skill, full CI mirror, PR

**Files:**
- Modify: `skills/claude-fleet-control/SKILL.md` (the work section ~lines 225-269)
- Modify: `docs/work-graph.md` (a short "The task list" section near the top of the user guide)
- Modify: `docs/superpowers/specs/2026-09-29-internal-task-list-design.md` (status line → "implemented")

- [ ] **Step 1: Skill text** — add to `claude-fleet-control`'s work section:

```markdown
- **Tasks you write.** `work_link { action: "create", title, project_id?, notes? }` makes a native task `TASK-<id>` (desktop / master only). `work_link { action: "start", item_id }` starts it in its project with title + notes as the first prompt.
- **Dispatched jobs are tasks too.** Every `dispatch_task` also appears in the Work list as an agent task (🤖) under the requester's primary work; its status follows the job (queued → to do, running → doing, done/failed/cancelled → done).
```

Then copy it to the local install as the repo skill says: `cp skills/claude-fleet-control/SKILL.md ~/.claude/skills/claude-fleet-control/SKILL.md`. On this Mac, `~/.claude` is a symlink into the dotfiles checkout, so this edits the dotfiles repo. Mention it in the PR body and do not commit it there.

- [ ] **Step 2: `docs/work-graph.md`** — add a section:

```markdown
## The task list (2026-09-29)

The Work tab opens on one list, To do / Doing / Done (Done shows the last 7 days):

- **+ New task** writes a task in fleet itself (`TASK-<id>`); ▾ adds a project and notes. **Start** opens a session in that project with the title and notes as its first prompt.
- A job one session dispatches to another (`dispatch_task`) shows as a 🤖 subtask under the requester's task, and follows the job's state.
- Tickets found in prompts, branches and PRs, and tracker items, appear in the same list.
- ⋯ → **Grouped view** is the org → group tree with its filters; Review and Placement rules are in the same menu.
```

- [ ] **Step 3: Full local CI mirror** (Global Constraints). Every command exits 0, except the known pre-existing frontend failures, which must also fail on `origin/main`.

- [ ] **Step 4: Push and open the PR** (merge-commit style, `--admin` while Actions is billing-blocked; ask the owner before merging):

```bash
git push -u origin HEAD
gh pr create --base main --head "$(git branch --show-current)" \
  --title "One internal task list: own + dispatched tasks in To do / Doing / Done" \
  --body-file docs/superpowers/specs/2026-09-29-internal-task-list-design.md
```

- [ ] **Step 5: Commit** any doc changes before the push:

```bash
git add skills/claude-fleet-control/SKILL.md docs/work-graph.md docs/superpowers/specs/2026-09-29-internal-task-list-design.md
git commit -m "docs(work): the task list — skill, user guide, spec status"
```
