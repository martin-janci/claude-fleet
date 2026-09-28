# Merging duplicate local work items Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two local work items that are really one can be merged, without losing a link, a placement, a child or any history.

**Architecture:** The loser stays as a tombstone (`merged_into_id`), so its id and its key keep resolving — to the winner, one hop. Merge repoints the three things that point at an item by id (`work_links.item_id`, `work_items.parent_id`, `work_placements.task_id`); the journal needs nothing, because `work_journal` is keyed by `claude_session_id` and follows the links for free. Tracker items are never mergeable: their identity is the tracker's (C24).

**Tech Stack:** Rust (fleet-core), SQLite (rusqlite), Tauri command layer, rmcp.

**Spec:** `docs/superpowers/specs/2026-09-24-work-graph-design.md` — §0 is authoritative; §0.2 (the schema) and §0.5 (the API surface) are what this plan deltas. The re-examination that produced this plan is in *Findings*; the owner declined a separate umbrella spec (2026-09-27), so this section is the delta of record.

## Findings (why this plan exists)

`LocalWorkItem.key` is `Option<String>` and `service::work::local` guards only **keys** with `E_EXISTS` — a keyless local item has nothing to collide with. So "auth refactor" named on Monday and "refactor authentication" named on Wednesday are two items, and nothing notices. The failure is silent and compounding:

- links split across two items, so neither shows the whole work;
- the Today digest and the Work view count the work twice;
- Resume finds half the journal, because it follows one item's links.

There is no repair primitive. `work_link { unlink }` + `link` moves one link at a time and leaves the empty item behind.

Decision taken by the owner on 2026-09-27: **build merge, decline split.** Split is served by per-link relink, which exists; a split primitive would have to decide which conversations follow which half, and that is a per-link judgement the review sheet already asks a person to make.

## Global Constraints

- Take the next unused migration number in `crates/fleet-core/migrations/` (**069** if the visible-truncation plan's 068 has landed, 068 if it has not). The two plans are independent and may land in either order.
- Only `source = 'local'` items merge. A tracker item, on either side, is refused.
- `E_*` codes come from `crate::ipc_error::codes`; new refusals reuse `E_INVALID`, `E_NOT_FOUND`, `E_EXISTS`.
- Any change to a `#[tool(...)]` description or an action list: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`.
- A new Tauri command needs a row in `src-tauri/src/backend/verdicts.rs` and `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen`.
- The MCP tool-description budget (C21) is asserted in `crates/fleet-core/src/mcp/tools/tests.rs`: measure, then raise with the number in the comment.
- Never hold the `Store` mutex across an `.await`.
- Verify with `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.

## File Structure

| File | Responsibility |
|---|---|
| `crates/fleet-core/migrations/069_work_item_merge.sql` | `work_items.merged_into_id`, `merged_at` |
| `crates/fleet-core/src/store/work_local.rs` | `merge_local_work_items` — the one transaction that repoints and tombstones |
| `crates/fleet-core/src/store/work.rs` | the two key resolvers and `resolve_work_target` follow a tombstone |
| `crates/fleet-core/src/service/work/local.rs` | `merge` — scope, guards, the answer |
| `crates/fleet-core/src/service/work/mod.rs` | `WORK_LINK_ACTIONS` gains `merge` |
| `src-tauri/src/commands/work.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/backend/verdicts.rs` | `merge_local_work_items`, Routed |
| `docs/work-graph.md` | a *Merging duplicates* paragraph |

**Out of scope, deliberately:** the desktop affordance (a *Merge into…* entry on a local task's row in the Work view). This plan ships the primitive and its MCP/Tauri surface, which is what makes the repair possible at all; the row menu is a follow-up issue, filed at the end of Task 5.

---

### Task 1: The tombstone column

**Files:**
- Create: `crates/fleet-core/migrations/069_work_item_merge.sql`
- Test: `crates/fleet-core/src/store/tests.rs` (or wherever the migration-chain test lives)

**Interfaces:**
- Produces: `work_items.merged_into_id INTEGER`, `work_items.merged_at INTEGER`.

- [ ] **Step 1: Write the migration**

```sql
-- Work graph: merging two LOCAL work items that are really one.
--
-- The loser is not deleted. It keeps its row, its key and its created_at,
-- and gains `merged_into_id` — so an id or a key someone still holds (a
-- bookmark, an older client, a link fleet has not repointed because it was
-- written in the same second) resolves to the winner instead of vanishing.
-- A merge always points at the FINAL winner: merging a tombstone's target
-- rewrites the tombstones that pointed at it, so a lookup never follows
-- more than one hop.
--
-- Tracker items never merge: their identity is `(tracker_id, external_id)`,
-- the tracker's own (review C24). The guard is in
-- `store::work_local::merge_local_work_items`, not here, so the refusal
-- carries a message.
ALTER TABLE work_items ADD COLUMN merged_into_id INTEGER REFERENCES work_items(id) ON DELETE SET NULL;
ALTER TABLE work_items ADD COLUMN merged_at INTEGER;

CREATE INDEX IF NOT EXISTS idx_work_items_merged
  ON work_items(merged_into_id) WHERE merged_into_id IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (69);
```

- [ ] **Step 2: Run the migration chain test**

Run: `cargo test -p fleet-core migrations`
Expected: PASS — the chain applies cleanly on a fresh database and on one already at 68.

- [ ] **Step 3: Commit**

```bash
git add crates/fleet-core/migrations
git commit -m "feat(work): a merged_into_id tombstone on work items"
```

---

### Task 2: The merge transaction

**Files:**
- Modify: `crates/fleet-core/src/store/work_local.rs`
- Test: `crates/fleet-core/src/store/work_local/tests.rs`

**Interfaces:**
- Consumes: `WorkItemRow`, `Store::get_work_item`, `Store::emit_work_item`, `tracker_items::SessionChange` (all existing).
- Produces: `Store::merge_local_work_items(&self, loser: i64, winner: i64) -> Result<Option<MergeOutcome>, IpcError>` and `pub struct MergeOutcome { pub winner: WorkItemRow, pub links_moved: usize, pub children_moved: usize, pub placement_moved: bool }`. `Ok(None)` means one of the two ids is not a local item.

- [ ] **Step 1: Write the failing tests**

In `crates/fleet-core/src/store/work_local/tests.rs`:

```rust
#[test]
fn a_merge_moves_links_children_and_the_placement() {
    let s = store();
    let a = s.name_local_item_for_test("auth refactor");
    let b = s.name_local_item_for_test("refactor authentication");
    let l1 = s.link_for_test(a, session(&s, "one"));
    let child = s.name_local_item_for_test("a subtask");
    s.set_parent_for_test(child, a);
    s.seed_placement(&format!("item:{a}"), Some("Mine"), None);

    let out = s.merge_local_work_items(a, b).unwrap().unwrap();

    assert_eq!(out.winner.id, b);
    assert_eq!(out.links_moved, 1);
    assert_eq!(out.children_moved, 1);
    assert!(out.placement_moved);
    assert_eq!(s.get_work_link(l1).unwrap().unwrap().item_id, Some(b));
    assert_eq!(s.get_work_item(child).unwrap().unwrap().parent_id, Some(b));
    assert_eq!(s.placement_of(&format!("item:{b}")).unwrap().unwrap().group_label.as_deref(), Some("Mine"));
    assert!(s.placement_of(&format!("item:{a}")).unwrap().is_none());
    // The loser stays, as a tombstone.
    let dead = s.get_work_item(a).unwrap().unwrap();
    assert_eq!(dead.merged_into_id, Some(b));
    assert!(dead.merged_at.is_some());
}

#[test]
fn a_winners_own_placement_is_not_overwritten() {
    let s = store();
    let a = s.name_local_item_for_test("loser");
    let b = s.name_local_item_for_test("winner");
    s.seed_placement(&format!("item:{a}"), Some("Loser group"), None);
    s.seed_placement(&format!("item:{b}"), Some("Winner group"), None);
    let out = s.merge_local_work_items(a, b).unwrap().unwrap();
    assert!(!out.placement_moved);
    assert_eq!(
        s.placement_of(&format!("item:{b}")).unwrap().unwrap().group_label.as_deref(),
        Some("Winner group")
    );
}

#[test]
fn merging_a_tracker_item_is_refused() {
    let s = store();
    let t = s.add_tracker_for_test("jira");
    let tracker_item = s.seed_tracker_item(t, "ABC-1");
    let local = s.name_local_item_for_test("mine");
    assert!(s.merge_local_work_items(tracker_item, local).unwrap().is_none());
    assert!(s.merge_local_work_items(local, tracker_item).unwrap().is_none());
}

#[test]
fn a_merge_never_leaves_a_chain() {
    let s = store();
    let a = s.name_local_item_for_test("a");
    let b = s.name_local_item_for_test("b");
    let c = s.name_local_item_for_test("c");
    s.merge_local_work_items(a, b).unwrap().unwrap();
    s.merge_local_work_items(b, c).unwrap().unwrap();
    // a pointed at b, which is now a tombstone: a must point at c.
    assert_eq!(s.get_work_item(a).unwrap().unwrap().merged_into_id, Some(c));
    assert_eq!(s.get_work_item(b).unwrap().unwrap().merged_into_id, Some(c));
}

#[test]
fn merging_an_item_into_itself_is_refused() {
    let s = store();
    let a = s.name_local_item_for_test("a");
    let e = s.merge_local_work_items(a, a).unwrap_err();
    assert_eq!(e.code, codes::E_INVALID);
}
```

Use the helpers this test module already has; add `name_local_item_for_test`, `set_parent_for_test`, `placement_of` and `seed_tracker_item` only if no equivalent exists, and keep them in the test module.

- [ ] **Step 2: Run them and watch them fail**

Run: `cargo test -p fleet-core work_local`
Expected: FAIL to compile — `merge_local_work_items` not found.

- [ ] **Step 3: Write the transaction**

In `crates/fleet-core/src/store/work_local.rs`:

```rust
/// What a merge moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutcome {
    pub winner: WorkItemRow,
    pub links_moved: usize,
    pub children_moved: usize,
    pub placement_moved: bool,
}

impl Store {
    /// Merge the LOCAL item `loser` into the local item `winner`.
    ///
    /// `Ok(None)` when either id is missing or is not `source = 'local'` —
    /// the caller turns that into the answer an unknown id gets, so a merge
    /// attempt says nothing about an item outside the caller's scope.
    ///
    /// What moves: every link (`work_links.item_id`), every child
    /// (`work_items.parent_id`) and the placement (`work_placements.task_id`
    /// is the text `item:<id>`, with no foreign key — a missed repoint would
    /// silently drop the task out of its group). The journal needs nothing:
    /// `work_journal` is keyed by `claude_session_id`, so it follows the
    /// links it already belonged to.
    ///
    /// A placement is moved only when the winner has none of its own: the
    /// winner is the row that survives, and its own placement is the more
    /// deliberate of the two.
    pub fn merge_local_work_items(
        &self,
        loser: i64,
        winner: i64,
    ) -> Result<Option<MergeOutcome>, IpcError> {
        if loser == winner {
            return Err(IpcError::new(
                codes::E_INVALID,
                "an item cannot be merged into itself",
            ));
        }
        let (Some(dead), Some(live)) = (self.get_work_item(loser)?, self.get_work_item(winner)?)
        else {
            return Ok(None);
        };
        if dead.source != "local" || live.source != "local" {
            return Ok(None);
        }
        if live.merged_into_id.is_some() {
            return Err(IpcError::new(
                codes::E_INVALID,
                "that work was itself merged away; merge into the item it points at",
            ));
        }
        let now = now_unix();
        let links_moved = self.conn.execute(
            "UPDATE work_links SET item_id = ?1 WHERE item_id = ?2",
            rusqlite::params![winner, loser],
        )?;
        let children_moved = self.conn.execute(
            "UPDATE work_items SET parent_id = ?1 WHERE parent_id = ?2",
            rusqlite::params![winner, loser],
        )?;
        let placement_moved = self.conn.execute(
            "UPDATE work_placements SET task_id = ?1, updated_at = ?2 \
             WHERE task_id = ?3 \
               AND NOT EXISTS (SELECT 1 FROM work_placements WHERE task_id = ?1)",
            rusqlite::params![format!("item:{winner}"), now, format!("item:{loser}")],
        )? == 1;
        // The loser's own placement is dropped when the winner already had
        // one: two rows for one task would break the PRIMARY KEY on the next
        // merge, and the winner's is the one that survives.
        self.conn.execute(
            "DELETE FROM work_placements WHERE task_id = ?1",
            rusqlite::params![format!("item:{loser}")],
        )?;
        // One hop, always: anything that pointed at the loser now points at
        // the winner too.
        self.conn.execute(
            "UPDATE work_items SET merged_into_id = ?1 WHERE merged_into_id = ?2",
            rusqlite::params![winner, loser],
        )?;
        self.conn.execute(
            "UPDATE work_items SET merged_into_id = ?1, merged_at = ?2, updated_at = ?2 \
             WHERE id = ?3",
            rusqlite::params![winner, now, loser],
        )?;
        let change = super::tracker_items::SessionChange {
            primary: true,
            suggested: true,
            rejected: false,
        };
        self.emit_work_item(loser, change)?;
        self.emit_work_item(winner, change)?;
        Ok(Some(MergeOutcome {
            winner: self.get_work_item(winner)?.ok_or_else(|| {
                IpcError::new(codes::E_INTERNAL, "the merge target vanished")
            })?,
            links_moved,
            children_moved,
            placement_moved,
        }))
    }
}
```

Wrap the whole body in whatever transaction helper `Store` already uses for multi-statement writes (look at a neighbouring multi-write method in `store/work.rs`); if there is none, use `self.conn.unchecked_transaction()?` and `tx.commit()?`.

- [ ] **Step 4: Add `merged_into_id` / `merged_at` to `WorkItemRow`**

In `crates/fleet-core/src/store/work.rs`, add both to `WorkItemRow`, to `ITEM_COLUMNS` and to `map_item`, in the same order.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p fleet-core work_local`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/src/store
git commit -m "feat(work): merge two local work items"
```

---

### Task 3: Resolution follows a tombstone

**Files:**
- Modify: `crates/fleet-core/src/store/work.rs` (`local_work_item_by_key`, `work_item_by_key`, `resolve_work_target`)
- Modify: `crates/fleet-core/src/store/work_local.rs` (`local_work_items`)
- Test: `crates/fleet-core/src/store/work_local/tests.rs`

**Interfaces:**
- Consumes: `merged_into_id` (Task 2).
- Produces: `Store::follow_merge(&self, row: WorkItemRow) -> Result<WorkItemRow, IpcError>` — one hop, or the row unchanged.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_merged_items_key_resolves_to_the_winner() {
    let s = store();
    let a = s.name_local_item_with_key_for_test("OLD-1", "auth refactor");
    let b = s.name_local_item_for_test("refactor authentication");
    s.merge_local_work_items(a, b).unwrap().unwrap();
    assert_eq!(s.local_work_item_by_key("OLD-1").unwrap().unwrap().id, b);
    assert_eq!(s.work_item_by_key("OLD-1").unwrap().unwrap().id, b);
}

#[test]
fn a_tombstone_is_not_listed_as_local_work() {
    let s = store();
    let a = s.name_local_item_for_test("a");
    let b = s.name_local_item_for_test("b");
    s.merge_local_work_items(a, b).unwrap().unwrap();
    let ids: Vec<i64> = s.local_work_items().unwrap().into_iter().map(|i| i.id).collect();
    assert_eq!(ids, vec![b]);
}
```

- [ ] **Step 2: Run them and watch them fail**

Run: `cargo test -p fleet-core work_local`
Expected: FAIL — the key resolves to the tombstone, and the listing has two rows.

- [ ] **Step 3: Write `follow_merge` and use it**

In `crates/fleet-core/src/store/work.rs`:

```rust
    /// The row a merge points at, or the row itself. One hop only: a merge
    /// rewrites the tombstones that pointed at its loser, so a chain cannot
    /// form (`merge_local_work_items`).
    pub fn follow_merge(&self, row: WorkItemRow) -> Result<WorkItemRow, IpcError> {
        match row.merged_into_id {
            Some(into) => Ok(self.get_work_item(into)?.unwrap_or(row)),
            None => Ok(row),
        }
    }
```

Then wrap the two resolvers' results: `local_work_item_by_key` and `work_item_by_key` end with `.map_err(IpcError::from)`; add `?` and, when `Some(row)`, return `Some(self.follow_merge(row)?)`. Do the same where `resolve_work_target` turns a key into an `item_id`.

- [ ] **Step 4: Exclude tombstones from the listing**

In `local_work_items`, add `AND merged_into_id IS NULL` to the `WHERE`.

Also check every other query in `store/work_local.rs` and `store/work_view.rs` that filters `source = 'local'` and decide, per query, whether a tombstone belongs in its answer. The rule: a **listing** excludes tombstones; a **lookup by id** returns them (so a stale id still answers).

- [ ] **Step 5: Run the tests, then the crate**

Run: `cargo test -p fleet-core work_local`
Expected: PASS
Run: `cargo test -p fleet-core`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/src/store
git commit -m "feat(work): a merged item's key resolves to its winner"
```

---

### Task 4: `work_link { action: merge }`

**Files:**
- Modify: `crates/fleet-core/src/service/work/local.rs`
- Modify: `crates/fleet-core/src/service/work/mod.rs` (`WORK_LINK_ACTIONS`, the dispatch, the tool description)
- Modify: `crates/fleet-core/src/mcp/tools/tests.rs` (the budget)
- Test: `crates/fleet-core/src/service/work/local/tests.rs`

**Interfaces:**
- Consumes: `Store::merge_local_work_items`, `MergeOutcome` (Task 2); `OrgScope`, `orgs::require_key` (existing); `WorkLinkArgs.item_id`, and a new `WorkLinkArgs.into_item_id: Option<i64>`.
- Produces: `service::work::local::merge(store, scope, args) -> Result<LocalWorkItem, IpcError>` — the winner as the caller sees it.

- [ ] **Step 1: Write the failing service tests**

```rust
#[test]
fn a_merge_answers_with_the_winner() {
    let w = seeded_two_local_items();
    let out = merge(&w.store, &OrgScope::All, args(w.a, w.b)).unwrap();
    assert_eq!(out.id, w.b);
    assert_eq!(out.live_sessions, 1); // the loser's live session came along
}

#[test]
fn a_host_token_may_not_merge() {
    let w = seeded_two_local_items();
    let e = merge(&w.store, &w.host_scope(), args(w.a, w.b)).unwrap_err();
    assert_eq!(e.code, codes::E_FORBIDDEN);
}

#[test]
fn merging_across_orgs_is_refused_without_the_flag() {
    let w = seeded_two_local_items_in_different_orgs();
    let e = merge(&w.store, &OrgScope::All, args(w.a, w.b)).unwrap_err();
    assert!(e.message.contains("different organisation"));
}

#[test]
fn an_item_outside_the_scope_answers_as_unknown() {
    let w = seeded_two_local_items();
    let e = merge(&w.store, &w.other_org_scope(), args(w.a, w.b)).unwrap_err();
    assert_eq!(e.code, codes::E_NOT_FOUND);
}
```

- [ ] **Step 2: Run them and watch them fail**

Run: `cargo test -p fleet-core work::local`
Expected: FAIL to compile — no `merge`.

- [ ] **Step 3: Write the service function**

In `crates/fleet-core/src/service/work/local.rs`, following the module's existing scope rules verbatim:

- a per-host token may not merge at all (`E_FORBIDDEN`, "merging work is not something a session decides"): a merge rewrites what every other session sees;
- both items must be visible in `scope`, or the answer is exactly the one an unknown id gets (`E_NOT_FOUND`), so a refusal says nothing about another org's work;
- both must be local, or `E_INVALID` with "that work has a ticket; link it instead" — the same wording `name` uses for a taken key;
- when the two items carry different `org_id`s, refuse with `E_INVALID` naming both, unless `args.force_cross_org` is set (the parameter `work_link` already has), and then the winner's org stands and a `cross_org` review item is raised exactly as a forced cross-org link does (D32).

- [ ] **Step 4: Add the action and the parameter**

In `crates/fleet-core/src/service/work/mod.rs`: `"merge"` in `WORK_LINK_ACTIONS`, the dispatch arm, and on `WorkLinkArgs`:

```rust
    /// Merge: the item that survives (`item_id` is the one merged away).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub into_item_id: Option<i64>,
```

Extend the `work_link` tool description with `merge {item_id, into_item_id}: fold one local work item into another`.

- [ ] **Step 5: Regenerate docs and pay the budget**

Run: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`
Run the budget test, read the measured size from its failure, raise the constant to that plus the existing headroom, and extend its comment with the date and number.

- [ ] **Step 6: Run and commit**

Run: `cargo test -p fleet-core`
Expected: PASS

```bash
git add crates/fleet-core docs/control-api-reference.md
git commit -m "feat(work): work_link { action: merge }"
```

---

### Task 5: The desktop command, the guide, and the follow-up

**Files:**
- Modify: `src-tauri/src/commands/work.rs`
- Modify: `src-tauri/src/lib.rs` (the `generate_handler!` list)
- Modify: `src-tauri/src/backend/verdicts.rs`
- Modify: `docs/work-graph.md`

**Interfaces:**
- Consumes: `service::work::local::merge` (Task 4).
- Produces: Tauri command `merge_local_work_items(args) -> LocalWorkItem`, `Verdict::Routed { tool: "work_link" }`.

- [ ] **Step 1: Copy the neighbouring command**

`src-tauri/src/commands/work.rs` already carries `name_session_work` as both a `#[tauri::command]` and a `routed::name_session_work` inner that either routes to the hub or calls `work::local::…` directly. Add `merge_local_work_items` in exactly that shape, next to it.

- [ ] **Step 2: Register it**

Add `commands::work::merge_local_work_items,` to the `generate_handler!` list in `src-tauri/src/lib.rs`, and to `src-tauri/src/backend/verdicts.rs`:

```rust
    ("merge_local_work_items", Verdict::Routed { tool: "work_link" }),
```

- [ ] **Step 3: Regenerate the verdict table**

Run: `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen`
Expected: PASS, with `src/lib/hub_verdicts.generated.json` and `docs/hub.md` updated.

- [ ] **Step 4: Document it**

In `docs/work-graph.md`, under the section about local work items:

```markdown
### Merging duplicates

Two pieces of named work that turn out to be one can be merged: every
session, branch and note of the one you merge away moves to the one you
keep, and its old name keeps working — it points at the item you kept.

Only work without a ticket merges. Work that has a Jira, GitHub, Asana or
Linear ticket is identified by that ticket, so fleet will not fold two
tickets together; link the sessions to the ticket you mean instead.

Merging cannot be undone in one step. What it moved can be moved back
link by link, and the name you merged away still resolves.
```

- [ ] **Step 5: Full verification**

Run: `cargo fmt --all --check`
Run: `cargo clippy --workspace --all-targets -- -D warnings`
Run: `cargo test --workspace`
Run: `pnpm check && pnpm test`
Expected: PASS

- [ ] **Step 6: Commit and file the follow-up**

```bash
git add src-tauri docs src/lib/hub_verdicts.generated.json
git commit -m "feat(work): merge_local_work_items on the desktop"
```

Then open an issue: **"Work view: Merge into… on a local task's row"** — the row menu that calls `merge_local_work_items`, with a confirm step naming what will move (the counts `MergeOutcome` already returns) and the cross-org refusal in words. Reference this plan.

---

## Self-Review

- **Coverage.** Findings' three consequences map to tasks: split links → Task 2's link repoint; double counting → Task 3's listing exclusion; half a journal → nothing needed, and Task 2's doc comment says why (`work_journal` is keyed by conversation). The owner's decision (merge yes, split no) is in *Findings* and in the guide text of Task 5 Step 4.
- **Placeholders.** None. Task 2 Step 3 defers the transaction helper to a named neighbour rather than guessing its name; Task 3 Step 4 states the rule for the queries it cannot enumerate without reading them, which is a decision, not a TODO.
- **Type consistency.** `merge_local_work_items(loser, winner)` — loser first — in Tasks 2, 3, 4 and 5; `MergeOutcome { winner, links_moved, children_moved, placement_moved }` only in Task 2 and referenced in Task 5's follow-up; `follow_merge(row) -> WorkItemRow` only in Task 3; `into_item_id` is the winner everywhere.
