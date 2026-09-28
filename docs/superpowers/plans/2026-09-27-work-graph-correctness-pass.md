# Work graph correctness pass Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close four invariants the work graph relies on but does not enforce, and give a rejection that is meant to be permanent somewhere durable to live.

**Architecture:** Three of the four are guards and tests, not features: a trigger pair for the `source` / `tracker_id` invariant, a test for the orphan-link path, and a consistency test over the resolver's `(rule, strength)` pairs. The fourth is small but real: a `work_never` table the resolver reads as extra input, written by `work_link { action: reject, scope: "repo" }` and cleared by `scope: "none"`.

**Tech Stack:** Rust (fleet-core), SQLite (rusqlite), rmcp.

**Spec:** `docs/superpowers/specs/2026-09-24-work-graph-design.md` — §0 is authoritative; §0.2 (the schema) and §0.3.1 (the resolver's rules, R9 in particular) are what this plan deltas. The re-examination that produced this plan is in *Findings*; the owner declined a separate umbrella spec (2026-09-27), so this section is the delta of record.

## Findings (why this plan exists)

**F1 — `source` and `tracker_id` say the same thing, and an index trusts one of them.** `ux_work_items_local_key ON work_items(key) WHERE source = 'local'` is the uniqueness of the local key namespace. Nothing in the schema forbids `source = 'local' AND tracker_id IS NOT NULL`, or `source = 'jira'` on a row whose tracker is Linear. The invariant is real and unstated.

**F2 — a retired participant whose `session_id` is already NULL leaves a live link behind.** `046`'s `trg_work_links_end_on_retire` carries `AND OLD.session_id IS NOT NULL`. The participant sweep (`store/participants.rs`) then deletes the participant outright, and `work_links.participant_id` is `ON DELETE SET NULL` — so such a link would end up `participant_id IS NULL AND ended_at IS NULL`: a live link no session owns, still in `idx_work_links_live`, never shown and never cleaned. Whether the path is reachable is unproven; one test settles it.

**F3 — `work_links.strength` and `work_links.rule` are both stored.** On a first read this looks like redundancy, but it is not: a candidate carries its `Strength` from the signal that produced it, and `rule` is stamped at the outcome. Neither derives from the other. What is missing is the guarantee that a *stored* pair is one the resolver can actually emit — a test, not a schema change.

**F4 — R9's stickiness lapses with the participant.** A rejection is `(participant, target)`; the sweep deletes retired participants, so detection can later re-propose exactly what a person rejected. Decision taken by the owner on 2026-09-27: **keep both** — the session-scoped rejection stays as it is, and a person may additionally say "never this item in this repo", which outlives the session, is visible, and can be cleared. The durable claim exists only where someone deliberately made it.

**F5 — doc drift.** `service/work/local.rs` opens with "A local item has no org of its own; each link to it takes its session's". Migration `066` added `work_items.org_id` and `structure.rs` added `assign_org` (D33), so the comment is now wrong.

## Global Constraints

- Take the next unused migration number in `crates/fleet-core/migrations/`. The visible-truncation plan wants 068 and the merge plan 069; this plan takes the next free one after whichever have landed.
- SQLite cannot `ALTER TABLE … ADD CHECK`, and rebuilding `work_items` is not worth it here: use `BEFORE INSERT` / `BEFORE UPDATE` triggers with `RAISE(ABORT, …)`.
- Any change to a `#[tool(...)]` description or an action list: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`.
- The MCP tool-description budget (C21) is asserted in `crates/fleet-core/src/mcp/tools/tests.rs`: measure, then raise with the number in the comment. This plan adds one read action and one parameter — keep them terse.
- `service::work::resolve::resolve` is a **pure function**. New input arrives as a field on its input struct; it never reads the store.
- Verify with `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.

## File Structure

| File | Responsibility |
|---|---|
| `crates/fleet-core/migrations/0NN_work_invariants.sql` | the `source`/`tracker_id` triggers, and `work_never` |
| `crates/fleet-core/src/store/work.rs` | the orphan-link assertion's query; `work_never` reads and writes |
| `crates/fleet-core/src/store/participants.rs` | (only if F2 proves reachable) |
| `crates/fleet-core/src/service/work/resolve.rs` | `Input.never`, the R9 filter, the `(rule, strength)` table test |
| `crates/fleet-core/src/service/work/detect.rs` | load `never` into the resolver's input |
| `crates/fleet-core/src/service/work/mod.rs` | `reject`'s `scope` parameter; `WorkAction::Rejections` |
| `crates/fleet-core/src/service/work/local.rs` | the stale doc comment |
| `docs/work-graph.md` | *Never this work in this repo* |

---

### Task 1: The `source` / `tracker_id` invariant

**Files:**
- Create: `crates/fleet-core/migrations/0NN_work_invariants.sql` (the first half; `work_never` joins it in Task 4)
- Test: `crates/fleet-core/src/store/work/tests.rs` (or the module where `work_items` writes are tested)

**Interfaces:**
- Produces: two triggers, `trg_work_items_source_insert` and `trg_work_items_source_update`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_local_item_may_not_carry_a_tracker() {
    let s = store();
    let t = s.add_tracker_for_test("jira");
    let e = s
        .conn
        .execute(
            "INSERT INTO work_items (source, tracker_id, title, status_category, created_at, updated_at) \
             VALUES ('local', ?1, 'wrong', 'todo', 1, 1)",
            rusqlite::params![t],
        )
        .unwrap_err();
    assert!(e.to_string().contains("source"), "{e}");
}

#[test]
fn a_tracker_item_may_not_claim_to_be_local() {
    let s = store();
    let e = s
        .conn
        .execute(
            "INSERT INTO work_items (source, title, status_category, created_at, updated_at) \
             VALUES ('jira', 'wrong', 'todo', 1, 1)",
            [],
        )
        .unwrap_err();
    assert!(e.to_string().contains("source"), "{e}");
}

#[test]
fn the_existing_writes_still_work() {
    let s = store();
    let t = s.add_tracker_for_test("jira");
    s.seed_tracker_item(t, "ABC-1");
    s.name_local_item_for_test("mine");
    // A removed tracker leaves its items with a dangling tracker_id
    // (`ON DELETE SET NULL` is NOT used there: the rows are kept for the
    // links that point at them). That must stay legal.
    s.remove_tracker_for_test(t);
    assert!(s.work_item_by_key("ABC-1").unwrap().is_some());
}
```

The third test matters more than the first two: read `store/trackers.rs`'s remove path before writing the trigger, and make the invariant exactly "`tracker_id IS NULL` if and only if `source = 'local'`" — *not* "`tracker_id` names a live tracker", which the remove path deliberately breaks.

- [ ] **Step 2: Run them and watch the first two fail**

Run: `cargo test -p fleet-core a_local_item_may_not_carry_a_tracker`
Expected: FAIL — the insert succeeds.

- [ ] **Step 3: Write the triggers**

```sql
-- Work graph: the invariant `ux_work_items_local_key` already depends on.
--
-- `source` and `tracker_id` say the same thing twice: a local item has no
-- tracker, a tracker item has one. The unique index that keeps the local key
-- namespace honest is `WHERE source = 'local'`, so a row that lies about
-- `source` escapes it. SQLite cannot ADD CHECK to an existing table and
-- rebuilding `work_items` (16 columns, five indexes, an FK from
-- `work_links`) is not worth it, so the invariant is two triggers.
--
-- Deliberately NOT asserted: that `tracker_id` names a live tracker. When a
-- tracker is removed its items are kept for the links that point at them,
-- with the id dangling, and `work_item_by_key` orders those last on purpose.
CREATE TRIGGER IF NOT EXISTS trg_work_items_source_insert
BEFORE INSERT ON work_items
WHEN (NEW.tracker_id IS NULL) <> (NEW.source = 'local')
BEGIN
  SELECT RAISE(ABORT, 'work_items: source must be local exactly when tracker_id is null');
END;

CREATE TRIGGER IF NOT EXISTS trg_work_items_source_update
BEFORE UPDATE OF source, tracker_id ON work_items
WHEN (NEW.tracker_id IS NULL) <> (NEW.source = 'local')
BEGIN
  SELECT RAISE(ABORT, 'work_items: source must be local exactly when tracker_id is null');
END;
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p fleet-core work`
Expected: PASS. If an existing test or a migration backfill trips the trigger, that is a row the invariant says is wrong — fix the writer, and only then consider narrowing the trigger.

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core
git commit -m "fix(work): assert source and tracker_id agree"
```

---

### Task 2: Is an orphan live link reachable?

**Files:**
- Test: `crates/fleet-core/src/store/work/tests.rs`
- Modify (only if the test fails): `crates/fleet-core/migrations/046_work_graph.sql` is immutable — the fix is a new trigger in this plan's migration, replacing the old one by name.

**Interfaces:**
- Produces: the assertion `no live link has a null participant`, as a test helper other tests can call.

- [ ] **Step 1: Write the assertion**

```rust
/// The graph's standing invariant: a link is live (`ended_at IS NULL`) only
/// while a participant owns it. `idx_work_links_live` is built on that.
fn assert_no_orphan_live_links(s: &Store) {
    let n: i64 = s
        .conn
        .query_row(
            "SELECT COUNT(*) FROM work_links WHERE ended_at IS NULL AND participant_id IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 0, "a live link with no participant");
}

#[test]
fn a_participant_retired_without_a_session_still_ends_its_links() {
    let s = store();
    let sid = session(&s, "one");
    let pid = s.participant_of_session(sid).unwrap().unwrap();
    let link = s.link_for_test(s.name_local_item_for_test("work"), sid);
    // The shape 046's trigger skips: session_id already NULL at retirement.
    s.conn
        .execute("UPDATE participants SET session_id = NULL WHERE id = ?1", rusqlite::params![pid])
        .unwrap();
    s.retire_participant_for_test(pid);
    assert!(s.get_work_link(link).unwrap().unwrap().ended_at.is_some());
    assert_no_orphan_live_links(&s);
}

#[test]
fn the_participant_sweep_leaves_no_orphan_live_link() {
    let s = store();
    let sid = session(&s, "one");
    let pid = s.participant_of_session(sid).unwrap().unwrap();
    s.link_for_test(s.name_local_item_for_test("work"), sid);
    s.retire_participant_for_test(pid);
    s.sweep_participants(now_unix() + 10_000_000).unwrap();
    assert_no_orphan_live_links(&s);
}
```

Use the sweep's real method name from `store/participants.rs`; add `retire_participant_for_test` and `participant_of_session` to the test module only if no equivalent exists.

- [ ] **Step 2: Run them**

Run: `cargo test -p fleet-core orphan`
Expected: one of two outcomes, and BOTH are a result worth committing:
- **PASS** → the path is unreachable; keep the tests as the regression guard, and add one line to `046`'s comment in this plan's migration header explaining why the `AND OLD.session_id IS NOT NULL` clause is safe.
- **FAIL** on the first test → continue to Step 3.

- [ ] **Step 3: If it failed, replace the trigger**

In this plan's migration, after the Task 1 triggers:

```sql
-- 046's trg_work_links_end_on_retire skipped a participant whose session_id
-- was already NULL, which the sweep then deleted — leaving a link both live
-- and unowned. The snapshot columns already COALESCE, so dropping the clause
-- costs nothing when there is no session left to copy from.
DROP TRIGGER IF EXISTS trg_work_links_end_on_retire;
```

followed by the whole `CREATE TRIGGER trg_work_links_end_on_retire` body copied from `046` with `AND OLD.session_id IS NOT NULL` removed from its `WHEN`. Copy it verbatim otherwise — the `COALESCE`s and the `json_group_array` subquery included.

- [ ] **Step 4: Run again, then the crate**

Run: `cargo test -p fleet-core orphan`
Expected: PASS
Run: `cargo test -p fleet-core`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/fleet-core
git commit -m "test(work): no live link without a participant"
```

---

### Task 3: A stored `(rule, strength)` pair is one the resolver can emit

**Files:**
- Test: `crates/fleet-core/src/service/work/resolve/tests.rs`

**Interfaces:**
- Consumes: `resolve::resolve`, `Strength`, and the outcome types (existing).
- Produces: `RULE_STRENGTHS: &[(&str, &[Strength])]` in the test module — the resolver's table, written down.

- [ ] **Step 1: Write the table and the test**

```rust
/// §0.3.1's table, as code. A rule and a strength are stored side by side on
/// `work_links`; neither derives from the other (a candidate carries its
/// strength from its signal, a rule is stamped at the outcome), so the only
/// thing that can be guaranteed is that a stored pair is one the resolver is
/// able to produce. Add a row here when you add a rule.
const RULE_STRENGTHS: &[(&str, &[Strength])] = &[
    ("R2", &[Strength::Explicit]),
    ("R3", &[Strength::Strong]),
    ("R3b", &[Strength::Strong]),
    ("R3u", &[Strength::Strong]),
    ("R4", &[Strength::Strong]),
    ("R5", &[Strength::Strong]),
    ("R6", &[Strength::Weak]),
    ("R11", &[Strength::Inferred]),
];

#[test]
fn every_rule_the_resolver_emits_is_in_the_table() {
    // Drive the resolver over the scenarios this module already has and
    // collect (rule, strength) from everything it proposes or confirms.
    for (rule, strength) in observed_pairs() {
        let row = RULE_STRENGTHS
            .iter()
            .find(|(r, _)| *r == rule)
            .unwrap_or_else(|| panic!("rule {rule} has no row in RULE_STRENGTHS"));
        assert!(
            row.1.contains(&strength),
            "rule {rule} emitted strength {strength:?}, which its row does not allow"
        );
    }
}
```

Build `observed_pairs()` by running every existing scenario helper in this test module and harvesting the outcomes — do not invent new scenarios for it. Correct the constant's rows against what the resolver actually does; the list above is read off §0.3.1 and the doc table at the top of `resolve.rs`, and either may be stale.

- [ ] **Step 2: Run it**

Run: `cargo test -p fleet-core every_rule_the_resolver_emits_is_in_the_table`
Expected: PASS after the constant matches reality. A mismatch found here is a finding: record it in the test's comment and fix whichever side is wrong.

- [ ] **Step 3: Commit**

```bash
git add crates/fleet-core/src/service/work
git commit -m "test(work): the resolver's rule and strength pairs are enumerated"
```

---

### Task 4: A rejection that outlives the session

**Files:**
- Modify: `crates/fleet-core/migrations/0NN_work_invariants.sql` (add `work_never`)
- Modify: `crates/fleet-core/src/store/work.rs`
- Modify: `crates/fleet-core/src/service/work/resolve.rs` (`Input.never`, the R9 filter)
- Modify: `crates/fleet-core/src/service/work/detect.rs` (load it)
- Test: `crates/fleet-core/src/service/work/resolve/tests.rs`, `crates/fleet-core/src/store/work/tests.rs`

**Interfaces:**
- Produces:
  - table `work_never(id, target, repo, created_at, created_by)`
  - `Store::never_targets_for_repo(&self, repo: &str) -> Result<Vec<String>, IpcError>`
  - `Store::set_never(&self, target: &str, repo: &str, by: &str) -> Result<(), IpcError>`
  - `Store::clear_never(&self, target: &str, repo: &str) -> Result<usize, IpcError>`
  - `Store::all_never(&self) -> Result<Vec<NeverRow>, IpcError>` with `pub struct NeverRow { pub target: String, pub repo: String, pub created_at: i64, pub created_by: String }`
  - `resolve::Input.never: Vec<String>`

- [ ] **Step 1: Write the migration half**

```sql
-- Work graph, decision F4/"keep both" (2026-09-27): a rejection a person
-- means permanently.
--
-- R9's rejection is `(participant, target)` and lapses when the participant
-- sweep removes the participant — which is right for what it says ("not this
-- session's work": once the session is gone the statement has no subject),
-- but it means detection can re-propose what someone rejected. A person who
-- means "never this work in this repo" now has somewhere to say it: durable,
-- listed in Settings, and clearable.
--
-- `target` is a normalised key or `item:<id>`, the same spelling
-- `work_links.ref_key` / `item_id` resolve from; `repo` is the project's
-- `owner/name`, so the claim is scoped to where it was made and says nothing
-- about another repo.
CREATE TABLE IF NOT EXISTS work_never (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  target     TEXT    NOT NULL,
  repo       TEXT    NOT NULL,
  created_at INTEGER NOT NULL,
  created_by TEXT    NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_work_never ON work_never(target, repo);
```

- [ ] **Step 2: Write the failing resolver test**

```rust
#[test]
fn a_never_in_this_repo_survives_a_swept_participant() {
    let mut input = branch_key_scenario("ABC-123"); // the module's existing helper
    input.rejected_links.clear();                   // the session-scoped rejection is gone
    input.never = vec!["ABC-123".into()];
    let out = resolve(&input);
    assert!(out.created.is_empty(), "R9's durable half must silence it");
    assert!(out.suggested.is_empty());
}

#[test]
fn a_never_in_another_repo_does_not_silence_this_one() {
    let mut input = branch_key_scenario("ABC-123");
    input.never = vec![]; // the store filters by repo, so nothing arrives here
    let out = resolve(&input);
    assert!(!out.created.is_empty());
}
```

Match the real field names on the resolver's input struct; `rejected_links` above is a placeholder for whatever it calls the links it filters on, and the test must use the real one.

- [ ] **Step 3: Run it and watch it fail**

Run: `cargo test -p fleet-core a_never_in_this_repo_survives`
Expected: FAIL to compile — `Input` has no `never`.

- [ ] **Step 4: Add the field and the filter**

In `crates/fleet-core/src/service/work/resolve.rs`, on the input struct:

```rust
    /// Targets a person said "never, in this repo" about (`work_never`,
    /// already filtered to this session's repo by the caller). Consulted
    /// after R1 and before every proposal, exactly as R9's session-scoped
    /// half is.
    pub never: Vec<String>,
```

and extend R9's filter — the `!rejected.contains(...)` predicate — to also reject anything in `never`. Update the R9 row of the module's doc table to name both halves.

- [ ] **Step 5: Load it in `detect`**

In `crates/fleet-core/src/service/work/detect.rs`, where the resolver's input is assembled, fill `never` from `never_targets_for_repo(repo)` for the session's project. When the session has no repo, pass an empty vec — a claim scoped to a repo cannot apply where there is none.

- [ ] **Step 6: Run the resolver tests**

Run: `cargo test -p fleet-core resolve`
Expected: PASS

- [ ] **Step 7: Add the write and read surfaces**

`work_link { action: reject }` gains:

```rust
    /// reject: session (default) — this session only, lapsing with it;
    /// repo — never in this repo, until cleared; none — clear that claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
```

`scope: "repo"` writes `set_never` *and* the session-scoped rejection; `"none"` calls `clear_never` and leaves the session-scoped one alone; anything else, and the absent case, behave exactly as today. A per-host token may write `session` but not `repo` or `none` (`E_FORBIDDEN`, "a session does not decide what the repo never links") — the same reasoning that keeps rules and views away from host tokens in `structure.rs`.

`WorkAction::Rejections` returns `all_never()` for the Settings list, scoped like the other reads: the master and a paired client see all of it, a bound client only its own org's repos, a per-host token nothing.

- [ ] **Step 8: Regenerate the docs and pay the budget**

Run: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`
Then the budget test; raise the constant to the measured value plus the existing headroom and extend its comment.

- [ ] **Step 9: Document it**

In `docs/work-graph.md`, in the section about correcting a link:

```markdown
**Not this, ever.** *Not this* tells fleet the current session is not working
on that ticket, and it holds for as long as the session exists. If fleet keeps
guessing the same wrong thing in one repository, choose *…and never in this
repo*: that is remembered until you clear it, and you can see every such
claim under Settings → Work. It applies to the repository you made it in, and
to no other.
```

- [ ] **Step 10: Run and commit**

Run: `cargo test --workspace`
Expected: PASS

```bash
git add crates/fleet-core docs
git commit -m "feat(work): never this work in this repo"
```

---

### Task 5: The stale comment

**Files:**
- Modify: `crates/fleet-core/src/service/work/local.rs`

- [ ] **Step 1: Correct it**

Replace

```
//! **Who sees a local item.** A local item has no org of its own; each link
//! to it takes its session's.
```

with

```
//! **Who sees a local item.** A local item's org is its own when one was
//! assigned (`work_items.org_id`, migration 066; set through
//! `structure::assign_org` behind the impact preview, D33) and otherwise its
//! links' — each link takes its session's.
```

Read `structure.rs`'s `assign_org` and the `066` migration comment before writing the replacement, and say what the code does now, not what this plan expects.

- [ ] **Step 2: Verify and commit**

Run: `cargo test -p fleet-core work::local`
Expected: PASS

```bash
git add crates/fleet-core/src/service/work/local.rs
git commit -m "docs(work): a local item can carry its own org"
```

---

## Self-Review

- **Coverage.** F1 → Task 1; F2 → Task 2 (with both outcomes defined, so the task cannot end undecided); F3 → Task 3, reframed as a consistency test after reading `resolve.rs` rather than the schema change the finding first suggested; F4 → Task 4, in the "keep both" shape the owner chose; F5 → Task 5.
- **Placeholders.** None. Three steps name a placeholder identifier and say explicitly that the real one must be read from the code (Task 2's sweep method, Task 3's `observed_pairs`, Task 4's `rejected_links`) — that is an instruction, not a TODO, and each says where to look.
- **Type consistency.** `assert_no_orphan_live_links(&Store)` only in Task 2; `RULE_STRENGTHS` only in Task 3; `never_targets_for_repo` / `set_never` / `clear_never` / `all_never` and `Input.never: Vec<String>` are used with the same signatures in Task 4's Steps 4, 5 and 7; `scope` is `session | repo | none` in both the parameter doc and the guide text.
