# Native item status Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Work with no ticket can finally be `in_progress` and `done`, so a board has columns, a group roll-up means something, and a release can say what shipped.

**Architecture:** `status_category` keeps its three existing values and stays the stored column. Two new columns record *who* set it. `done` is **stamped once** by the tidy pass that already detects a merged PR, so it survives the session's retirement; `in_progress` is **computed** on read, because it is transient; a person's explicit setting outranks both and is refused on tracker items, whose status the sync owns.

**Tech Stack:** Rust (`crates/fleet-core`), SQLite (rusqlite), rmcp, Svelte 5 frontend.

**Spec:** `docs/superpowers/specs/2026-09-28-sprints-releases-epics-design.md` — §0 records the owner's decisions E1–E7, §2 is what this plan implements, §10 places it as phase 1. Read §2 in full before Task 1.

## Global Constraints

- `status_category` keeps exactly three values: `todo | in_progress | done`. No fourth value, and **`blocked` is deliberately not a status** (§2) — it is a session property (`claude_status`) the row already shows.
- `status_set_by` is `NULL | 'person' | 'derived'`. Precedence: `person` > `derived` > live `in_progress` > `todo`.
- A person's override applies to `source = 'local'` items **only**. On a tracker item, `set_status` is `E_INVALID` naming the ticket — the sync would revert it on the next pass (§2).
- A stamped `done` is **not** undone by new work. Flip-flopping would make the status untrustworthy for a release asking what shipped.
- The live half runs in the Work view's read path. It must be **one join, never a per-row query**, and must be measured against `scale_work_view` (`crates/fleet-core/src/service/work/scale_tests.rs:605`) before the last task merges.
- Next free migration number is **075** (`main` is at `074`). Check `crates/fleet-core/migrations/` first in case another branch landed one.
- A new MCP action never becomes a new tool (C21). `BUDGET_BYTES` is `63_573` in `crates/fleet-core/src/mcp/tools/tests.rs`; measure, then raise deliberately with the number in the commit message.
- After changing any `#[tool(...)]` description or action list: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`.
- A new Tauri command needs a row in `src-tauri/src/backend/verdicts.rs` plus `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen`.
- Every wire enum needs an `Unknown` (`#[serde(other)]`) variant.
- Never hold the `Store` mutex across an `.await`, and never take it twice in one statement.
- Use `/usr/bin/git` for every git command.
- Three test groups fail on the dev box under load for environmental reasons and are not yours: `service::work::scale_tests::*`, `store::schema::tests_upgrade::*`, `src/lib/markdown.test.ts`'s timing case. The gate is `cargo test -p fleet-core -- --skip scale_tests` plus each task's named tests; `scale_work_view` is run deliberately in the last task.

## File Structure

| File | Responsibility |
|---|---|
| `crates/fleet-core/migrations/075_native_item_status.sql` | the two columns |
| `crates/fleet-core/src/store/schema.rs` | the migration's chain entry |
| `crates/fleet-core/src/store/work.rs` | `ITEM_COLUMNS`, `WorkItemRow`, `map_item` |
| `crates/fleet-core/src/store/work_status.rs` | the person override's write and the derived stamp — the only writers of `status_set_by` |
| `crates/fleet-core/src/service/work/status.rs` | the precedence rule as a pure function, plus the live `in_progress` read |
| `crates/fleet-core/src/store/work_tidy.rs` | stamps `done` where it already detects a merged PR (~line 194) |
| `crates/fleet-core/src/service/work/mod.rs` | `set_status` on `WORK_LINK_ACTIONS` and its dispatch |
| `crates/fleet-core/src/service/work/view.rs` | the Work view projects the effective status |
| `docs/work-graph.md` | one user-facing paragraph |

---

### Task 1: The columns

**Files:**
- Create: `crates/fleet-core/migrations/075_native_item_status.sql`
- Modify: `crates/fleet-core/src/store/schema.rs`
- Modify: `crates/fleet-core/src/store/work.rs` (`ITEM_COLUMNS`, `WorkItemRow`, `map_item`)
- Test: `crates/fleet-core/src/store/work/tests.rs` (or the module where `work_items` reads are tested)

**Interfaces:**
- Produces: `WorkItemRow.status_set_by: Option<String>`, `WorkItemRow.status_set_at: Option<i64>`.

- [ ] **Step 1: Write the migration**

```sql
-- Native item status (design 2026-09-28 §2): who decided this item's status.
--
-- `status_category` keeps its three values and stays the stored column; these
-- two say where the value came from, which is what makes the precedence rule
-- expressible: a person outranks a derived stamp, and a derived stamp outranks
-- the live signal.
--
-- `status_set_by`: NULL (nobody — the value is the sync's or the default) |
-- 'person' (an explicit setting, final) | 'derived' (stamped once by the tidy
-- pass that saw a merged PR; stamped rather than computed because
-- `sessions.pr_signals` dies with its session, and a computed `done` would
-- silently revert to `todo` afterwards).
ALTER TABLE work_items ADD COLUMN status_set_by TEXT;
ALTER TABLE work_items ADD COLUMN status_set_at INTEGER;

CREATE INDEX IF NOT EXISTS idx_work_items_status_set
  ON work_items(status_set_by) WHERE status_set_by IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (75);
```

- [ ] **Step 2: Add the chain entry**

In `crates/fleet-core/src/store/schema.rs`, at the end of the migration array:

```rust
    // Native item status (design 2026-09-28 §2): `status_set_by` /
    // `status_set_at`. Two ADD COLUMNs plus a partial index, all idempotent on
    // their own, so no `already_applied` guard.
    Migration::plain(75, include_str!("../../migrations/075_native_item_status.sql")),
```

- [ ] **Step 3: Write the failing store test**

In `crates/fleet-core/src/store/work/tests.rs`:

```rust
#[test]
fn an_items_status_provenance_round_trips_and_defaults_to_none() {
    let s = store();
    let id = s.name_local_item_for_test("auth refactor");
    let row = s.get_work_item(id).unwrap().unwrap();
    assert_eq!(row.status_category, "todo");
    assert_eq!(row.status_set_by, None, "a fresh item has no decision on it");
    assert_eq!(row.status_set_at, None);

    s.conn
        .execute(
            "UPDATE work_items SET status_category = 'done', status_set_by = 'person', \
             status_set_at = 1700 WHERE id = ?1",
            rusqlite::params![id],
        )
        .unwrap();
    let row = s.get_work_item(id).unwrap().unwrap();
    assert_eq!(row.status_category, "done");
    assert_eq!(row.status_set_by.as_deref(), Some("person"));
    assert_eq!(row.status_set_at, Some(1700));
}
```

Use the helpers that module already has (`store()`, `name_local_item_for_test`); do not invent new ones.

- [ ] **Step 4: Run it and watch it fail**

Run: `cargo test -p fleet-core an_items_status_provenance_round_trips`
Expected: FAIL to compile — `WorkItemRow` has no field `status_set_by`.

- [ ] **Step 5: Add the fields**

In `crates/fleet-core/src/store/work.rs`, in `WorkItemRow`, after `status_category`:

```rust
    /// Who decided `status_category`: `None` (the sync's, or the default),
    /// `"person"` (explicit, final) or `"derived"` (stamped from a merged PR).
    pub status_set_by: Option<String>,
    pub status_set_at: Option<i64>,
```

Append both to `ITEM_COLUMNS` **at the end**, so every existing `row.get(n)`
index in `map_item` keeps its meaning:

```rust
pub(super) const ITEM_COLUMNS: &str =
    "id, source, key, title, url, status_category, created_at, updated_at, \
     tracker_id, external_id, aliases, kind, hierarchy_level, status_name, resolution, parent_id, \
     assignees, iteration, updated_ext, status_changed_at, fetched_at, unavailable_at, \
     unavailable_reason, status_set_by, status_set_at";
```

and read them at the two new indices in `map_item`, immediately after the last
existing field, in the same order as the column list.

- [ ] **Step 6: Run the test and the chain**

Run: `cargo test -p fleet-core an_items_status_provenance_round_trips`
Expected: PASS
Run: `cargo test -p fleet-core store::schema`
Expected: PASS — the chain applies on a fresh database and on one already at 74.

- [ ] **Step 7: Commit**

```bash
/usr/bin/git add crates/fleet-core
/usr/bin/git commit -m "feat(work): record who decided an item's status"
```

---

### Task 2: A person sets the status

**Files:**
- Create: `crates/fleet-core/src/store/work_status.rs`
- Modify: `crates/fleet-core/src/store/mod.rs` (module list)
- Modify: `crates/fleet-core/src/service/work/mod.rs` (`WORK_LINK_ACTIONS`, dispatch, `WorkLinkArgs.status`)
- Create: `crates/fleet-core/src/service/work/status.rs`
- Modify: `crates/fleet-core/src/mcp/tools/tests.rs` (the budget constant, if the measurement requires it)
- Modify: `docs/work-graph.md`
- Test: `crates/fleet-core/src/store/work_status/tests.rs`, `crates/fleet-core/src/service/work/status.rs` (inline)

**Interfaces:**
- Consumes: `WorkItemRow.status_set_by` (Task 1); `OrgScope`, `orgs::require_key` (existing).
- Produces:
  - `Store::set_item_status(&self, item_id: i64, status: &str) -> Result<Option<WorkItemRow>, IpcError>` — `Ok(None)` when the id is unknown; `E_INVALID` for a tracker item or a status outside the three values.
  - `service::work::status::set_status(store, scope, item_id, status) -> Result<WorkItemRow, IpcError>`
  - action `work_link { action: set_status, item_id, status }`

- [ ] **Step 1: Write the failing store tests**

In `crates/fleet-core/src/store/work_status/tests.rs`:

```rust
#[test]
fn a_person_can_set_a_local_items_status() {
    let s = store();
    let id = s.name_local_item_for_test("auth refactor");
    let row = s.set_item_status(id, "in_progress").unwrap().unwrap();
    assert_eq!(row.status_category, "in_progress");
    assert_eq!(row.status_set_by.as_deref(), Some("person"));
    assert!(row.status_set_at.is_some());
}

#[test]
fn a_tracker_items_status_belongs_to_its_tracker() {
    let s = store();
    let t = s.add_tracker_for_test("jira");
    let id = s.seed_tracker_item(t, "ABC-1");
    let e = s.set_item_status(id, "done").unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
    assert!(e.message.contains("ABC-1"), "the refusal must name the ticket: {}", e.message);
}

#[test]
fn only_the_three_categories_are_accepted() {
    let s = store();
    let id = s.name_local_item_for_test("x");
    for bad in ["blocked", "review", "", "DONE"] {
        let e = s.set_item_status(id, bad).unwrap_err();
        assert_eq!(e.code, codes::E_INVALID, "accepted {bad:?}");
    }
}

#[test]
fn an_unknown_id_is_not_an_error() {
    let s = store();
    assert!(s.set_item_status(9_999, "done").unwrap().is_none());
}
```

- [ ] **Step 2: Run them and watch them fail**

Run: `cargo test -p fleet-core work_status`
Expected: FAIL to compile — the module does not exist.

- [ ] **Step 3: Write the store module**

```rust
//! Who decided an item's status (design 2026-09-28 §2). The only writers of
//! `work_items.status_set_by` live here: a person's explicit setting, and the
//! derived stamp `work_tidy` makes when it sees a merged PR.

use super::{now_unix, Store, WorkItemRow};
use crate::ipc_error::{codes, IpcError};

/// The three values `status_category` may hold. `blocked` is deliberately not
/// among them: it is a property of a session, which the row already shows.
pub const STATUS_CATEGORIES: [&str; 3] = ["todo", "in_progress", "done"];

impl Store {
    /// A person sets a local item's status. `Ok(None)` when the id is unknown,
    /// so a caller outside the item's scope gets the answer an unknown id gets.
    ///
    /// Refused for a tracker item: `store::tracker_items` writes
    /// `status_category` on every sync, so the setting would be reverted on the
    /// next pass — worse than saying no.
    pub fn set_item_status(
        &self,
        item_id: i64,
        status: &str,
    ) -> Result<Option<WorkItemRow>, IpcError> {
        if !STATUS_CATEGORIES.contains(&status) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "a status is one of {} — `blocked` is a session's state, not an item's",
                    STATUS_CATEGORIES.join(", ")
                ),
            ));
        }
        let Some(before) = self.get_work_item(item_id)? else {
            return Ok(None);
        };
        if before.source != "local" {
            let which = before.key.clone().unwrap_or_else(|| before.title.clone());
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "{which}'s status belongs to its tracker; change it there, \
                     or track this work as a local item"
                ),
            ));
        }
        let now = now_unix();
        self.conn.execute(
            "UPDATE work_items SET status_category = ?1, status_set_by = 'person', \
             status_set_at = ?2, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![status, now, item_id],
        )?;
        self.emit_work_item(
            item_id,
            super::tracker_items::SessionChange {
                primary: true,
                suggested: false,
                rejected: false,
            },
        )?;
        self.get_work_item(item_id)
    }
}

#[cfg(test)]
mod tests;
```

Register the module in `crates/fleet-core/src/store/mod.rs` beside `work_local`.

- [ ] **Step 4: Run the store tests**

Run: `cargo test -p fleet-core work_status`
Expected: PASS

- [ ] **Step 5: Write the failing service tests**

In `crates/fleet-core/src/service/work/status.rs`, inside `mod tests`:

```rust
#[test]
fn a_host_token_may_set_its_own_items_status() {
    let w = seeded_local_item_on_host("mercury");
    let row = set_status(&w.store, &w.host_scope(), w.item, "done").unwrap();
    assert_eq!(row.status_category, "done");
}

#[test]
fn an_item_outside_the_scope_answers_as_unknown() {
    let w = seeded_local_item_on_host("mercury");
    let e = set_status(&w.store, &w.other_org_scope(), w.item, "done").unwrap_err();
    assert_eq!(e.code, codes::E_NOTFOUND);
}
```

Match `OrgScope`'s real variant shape at the call site, and reuse the scope
fence `service::work::local` already applies — a per-host token sees an item
only through work its own host does inside its org.

- [ ] **Step 6: Write the service function**

`crates/fleet-core/src/service/work/status.rs` resolves the item inside
`scope` with the same fence `local.rs` uses, maps `Ok(None)` to `E_NOTFOUND`
(so a refusal says nothing about another org's work), and calls
`Store::set_item_status`. A per-host token **is** allowed here: an agent may
say its own work is done. It may not touch buckets or rules (spec §7).

- [ ] **Step 7: Add the action**

In `crates/fleet-core/src/service/work/mod.rs`: `"set_status"` in
`WORK_LINK_ACTIONS`, the dispatch arm, and on `WorkLinkArgs`:

```rust
    /// set_status: todo | in_progress | done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
```

Extend the `work_link` tool description with
`set_status {item_id, status}: a person's status for work with no ticket`.

- [ ] **Step 8: Regenerate docs and pay the budget**

Run: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`
Run the budget test. If it fails, read the measured number from the failure,
raise `BUDGET_BYTES` to that plus the customary 100 bytes of headroom, and put
the measurement and today's date in the commit message. If it passes, say in
the report that no raise was needed.

- [ ] **Step 9: Document it**

In `docs/work-graph.md`, in the section about work with no ticket:

```markdown
### Status of work with no ticket

Work fleet tracks itself — named work with no ticket — carries one of three
states: to do, in progress, done. You never have to set it: fleet marks work
*in progress* while a session is working on it, and *done* once the pull
request it produced is merged. Setting it yourself overrides that for good; the
same work will not flip back because a session started again.

A ticket's status is not yours to set here — it belongs to Jira, GitHub, Asana
or Linear, and fleet would be overwritten on its next sync. Change it there.
```

- [ ] **Step 10: Run everything and commit**

Run: `cargo test -p fleet-core -- --skip scale_tests`
Run: `cargo fmt --all --check`
Run: `cargo clippy -p fleet-core --all-targets -- -D warnings`
Expected: PASS

```bash
/usr/bin/git add crates/fleet-core docs
/usr/bin/git commit -m "feat(work): work_link { action: set_status } for local work"
```

---

### Task 3: A merged PR stamps `done`

**Files:**
- Modify: `crates/fleet-core/src/store/work_status.rs` (the stamp)
- Modify: `crates/fleet-core/src/store/work_tidy.rs` (call it where `is_merged()` already fires, ~line 194)
- Test: `crates/fleet-core/src/store/work_status/tests.rs`

**Interfaces:**
- Consumes: `PrSignals::is_merged()` (`service/work/detect.rs`), already called at `store/work_tidy.rs:194`.
- Produces: `Store::stamp_derived_done(&self, item_id: i64) -> Result<bool, IpcError>` — `true` when it wrote.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_merged_pr_stamps_a_local_item_done() {
    let s = store();
    let id = s.name_local_item_for_test("auth refactor");
    assert!(s.stamp_derived_done(id).unwrap());
    let row = s.get_work_item(id).unwrap().unwrap();
    assert_eq!(row.status_category, "done");
    assert_eq!(row.status_set_by.as_deref(), Some("derived"));
}

#[test]
fn a_persons_status_outranks_the_stamp() {
    let s = store();
    let id = s.name_local_item_for_test("auth refactor");
    s.set_item_status(id, "in_progress").unwrap();
    assert!(!s.stamp_derived_done(id).unwrap(), "the stamp must not write");
    let row = s.get_work_item(id).unwrap().unwrap();
    assert_eq!(row.status_category, "in_progress");
    assert_eq!(row.status_set_by.as_deref(), Some("person"));
}

#[test]
fn the_stamp_never_touches_a_tracker_item() {
    let s = store();
    let t = s.add_tracker_for_test("jira");
    let id = s.seed_tracker_item(t, "ABC-1");
    assert!(!s.stamp_derived_done(id).unwrap());
}

#[test]
fn stamping_twice_writes_once() {
    let s = store();
    let id = s.name_local_item_for_test("x");
    assert!(s.stamp_derived_done(id).unwrap());
    assert!(!s.stamp_derived_done(id).unwrap(), "already done, no event");
}
```

- [ ] **Step 2: Run them and watch them fail**

Run: `cargo test -p fleet-core work_status`
Expected: FAIL to compile — `stamp_derived_done` not found.

- [ ] **Step 3: Write the stamp**

In `crates/fleet-core/src/store/work_status.rs`:

```rust
impl Store {
    /// Record that a local item's work was delivered, once.
    ///
    /// Stamped rather than computed because the merged-PR signal lives on
    /// `sessions.pr_signals` and dies with its session: a computed `done` would
    /// silently revert to `todo` once the work's session was swept, which is
    /// the worst behaviour for the one status a release depends on.
    ///
    /// Returns whether it wrote. It never overrides a person (`status_set_by =
    /// 'person'`), never touches a tracker item, and never writes twice — so a
    /// tidy pass may call it every tick without emitting an event per tick.
    pub fn stamp_derived_done(&self, item_id: i64) -> Result<bool, IpcError> {
        let wrote = self.conn.execute(
            "UPDATE work_items SET status_category = 'done', status_set_by = 'derived', \
             status_set_at = ?1, updated_at = ?1 \
             WHERE id = ?2 AND source = 'local' \
               AND COALESCE(status_set_by, '') <> 'person' \
               AND NOT (status_category = 'done' AND status_set_by = 'derived')",
            rusqlite::params![now_unix(), item_id],
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
}
```

- [ ] **Step 4: Call it where the merge is already seen**

In `crates/fleet-core/src/store/work_tidy.rs`, at the site that already
computes `.is_some_and(|s| s.is_merged())` (~line 194), call
`stamp_derived_done` for each local item linked to that session. Do **not** add
a second scan of `sessions` or a second parse of `pr_signals` — that site
already holds both the signal and the link.

Add a test in that module asserting a merged PR on a session linked to a local
item leaves the item `done` with `status_set_by = 'derived'` after the pass.

- [ ] **Step 5: Run and commit**

Run: `cargo test -p fleet-core work_status work_tidy -- --skip scale_tests` (run the two filters in separate invocations; `cargo test` takes one positional)
Run: `cargo test -p fleet-core -- --skip scale_tests`
Expected: PASS

```bash
/usr/bin/git add crates/fleet-core
/usr/bin/git commit -m "feat(work): a merged PR stamps local work done, once"
```

---

### Task 4: The live `in_progress`, and the cost of it

**Files:**
- Modify: `crates/fleet-core/src/service/work/status.rs` (the precedence function)
- Modify: `crates/fleet-core/src/service/work/view.rs` (the projection, ~line 1066)
- Test: `crates/fleet-core/src/service/work/status.rs` (inline), `crates/fleet-core/src/service/work/view_tests.rs`

**Interfaces:**
- Consumes: `WorkItemRow.status_set_by` (Task 1).
- Produces: `pub fn effective_status(row: &WorkItemRow, has_working_session: bool) -> &'static str` — a pure function, so the rule is testable without a store.

- [ ] **Step 1: Write the failing precedence tests**

```rust
fn row(status: &str, by: Option<&str>) -> WorkItemRow {
    WorkItemRow {
        status_category: status.into(),
        status_set_by: by.map(Into::into),
        source: "local".into(),
        ..Default::default()
    }
}

#[test]
fn a_person_outranks_everything() {
    assert_eq!(effective_status(&row("todo", Some("person")), true), "todo");
    assert_eq!(effective_status(&row("done", Some("person")), true), "done");
}

#[test]
fn a_stamped_done_is_not_undone_by_new_work() {
    assert_eq!(effective_status(&row("done", Some("derived")), true), "done");
}

#[test]
fn a_working_session_makes_it_in_progress() {
    assert_eq!(effective_status(&row("todo", None), true), "in_progress");
}

#[test]
fn otherwise_it_is_whatever_is_stored() {
    assert_eq!(effective_status(&row("todo", None), false), "todo");
    // A tracker item: the sync's value, untouched by the live signal.
    let mut t = row("in_progress", None);
    t.source = "jira".into();
    assert_eq!(effective_status(&t, false), "in_progress");
}
```

If `WorkItemRow` does not derive `Default`, build the fixtures with the
struct's real constructor or add `#[derive(Default)]` — note which you did.

- [ ] **Step 2: Run them and watch them fail**

Run: `cargo test -p fleet-core effective_status`
Expected: FAIL to compile — function not found.

- [ ] **Step 3: Write the pure function**

```rust
/// The status a reader should see (design 2026-09-28 §2).
///
/// Precedence: a person's setting, then a derived `done` stamp, then the live
/// signal, then whatever is stored. `has_working_session` is supplied by the
/// caller from ONE join — never a query per row.
pub fn effective_status(row: &WorkItemRow, has_working_session: bool) -> &'static str {
    match row.status_set_by.as_deref() {
        Some("person") | Some("derived") => match row.status_category.as_str() {
            "done" => "done",
            "in_progress" => "in_progress",
            _ => "todo",
        },
        _ if has_working_session && row.source == "local" => "in_progress",
        _ => match row.status_category.as_str() {
            "done" => "done",
            "in_progress" => "in_progress",
            _ => "todo",
        },
    }
}
```

The live signal lifts only **local** items: a tracker item's column is its
tracker's (§2, E11), so a working session must not move it.

- [ ] **Step 4: Supply the signal with one join**

In `crates/fleet-core/src/service/work/view.rs`, where the task's
`status_category` is currently taken straight from the item (~line 1066), pass
it through `effective_status`. Obtain `has_working_session` for the whole page
in **one** query, in the shape `store/work_tidy.rs:95` already uses:

```sql
SELECT DISTINCT l.item_id FROM work_links l
  JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL
  JOIN sessions s     ON s.id = p.session_id
 WHERE l.ended_at IS NULL AND l.state = 'confirmed'
   AND s.claude_status = 'working'
```

Collect it into a `BTreeSet<i64>` once per page and look each row up. A query
per row is a plan failure, not an optimisation opportunity.

- [ ] **Step 5: Add the view test**

In `view_tests.rs`: a local item with a working session shows `in_progress`
through `tree(..)`; the same item with `status_set_by = 'person'` and `todo`
still shows `todo`; a tracker item with a working session keeps its synced
status.

- [ ] **Step 6: Measure the cost — this is the gate**

Run: `cargo test -p fleet-core scale_work_view -- --nocapture`
Record the reported p50/p95 **before and after** your change in the report
(stash your change, run, restore, run). The budget is 3000 ms p95 and the test
is load-sensitive on this box, so run it on an otherwise idle machine.

If p95 moved by more than ~10 %, **stop and report it** rather than adjusting
the budget: the one-join rule exists precisely so this does not happen, and a
real regression means the join is in the wrong place.

- [ ] **Step 7: Full verification and commit**

Run: `cargo test -p fleet-core -- --skip scale_tests`
Run: `cargo fmt --all --check`
Run: `cargo clippy -p fleet-core --all-targets -- -D warnings`
Run: `pnpm check && pnpm test`
Expected: PASS

```bash
/usr/bin/git add crates/fleet-core
/usr/bin/git commit -m "feat(work): a working session shows local work in progress"
```

---

## Self-Review

- **Spec coverage.** §2's stored column and three values → Task 1; the person override and its refusal on tracker items → Task 2; the stamped `done` and why it is stamped → Task 3; the live `in_progress`, the precedence order and the one-join rule → Task 4. §2's "`blocked` is not a status" is enforced by Task 2's `only_the_three_categories_are_accepted`. The risk in §11 about the hot path is Task 4 Step 6, written as a gate with a stop condition rather than a suggestion.
- **Out of scope, by the spec's own phasing (§10):** buckets, sprints, releases, epics, the board, and any frontend beyond what `pnpm check` requires. The desktop affordance for `set_status` is a follow-up issue — this plan ships the model, the action and the projection, which is what phases 2–5 build on.
- **Placeholder scan.** None. Three steps name a thing to confirm in the code rather than guess it (Task 1's `map_item` indices, Task 2's `OrgScope` variant shape, Task 4's `WorkItemRow: Default`), each saying exactly what to check.
- **Type consistency.** `status_set_by: Option<String>` with values `person` / `derived` in Tasks 1–4; `set_item_status(item_id, status) -> Result<Option<WorkItemRow>>` in Tasks 2 and 3; `stamp_derived_done(item_id) -> Result<bool>` only in Task 3; `effective_status(&WorkItemRow, bool) -> &'static str` only in Task 4; `STATUS_CATEGORIES` is the single list of the three values.
