# Task Result, Completion Gate and Follow-up Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Any work item can end with a structured result behind a deterministic gate, shown in one panel, from which a person creates a follow-up task whose fresh session inherits the result's decisions, remaining work and changes.

**Architecture:** The result is the newest finished `tasks` row of a work item that carries `result_json` (report) and `evidence_json` (git). An interactive session gets such a row through an explicit `wrap_up` request that reuses the done-marker pipeline. A pure `completion_gate` reads `done_when` checks and open children. A follow-up is a sibling item with `origin_task_id`; the start brief renders the origin's facts.

**Tech Stack:** Rust (`crates/fleet-core`, `src-tauri`), SQLite migrations, rmcp tool router, Svelte 5 + TypeScript, Vitest.

**Spec:** `docs/superpowers/specs/2026-10-13-task-result-handover-design.md` (decisions D1–D6 are referenced below by number).

## Global Constraints

- Validate with the ladder in `CLAUDE.md`: `cargo fleet-fast-check` after each edit, `scripts/verify.sh` before each commit, `scripts/verify.sh full` before the PR. Never `cargo build`, never `-p <crate>`.
- Narrow tests with a filter: `cargo fleet-test -- <filter>`. On a loaded box prefix `TMPDIR=/dev/shm/fleet-tests` (Linux) for `src-tauri` tests.
- Migration number: take it from `origin/main` (`git ls-tree --name-only origin/main crates/fleet-core/migrations/ | tail -1`). Main took `184` (`184_work_close_dismissals.sql`) on 2026-10-13, so this plan uses `185`; renumber if main moved again.
- SQLite goes through `Store` behind `std::sync::Mutex`; never hold the guard across `.await`.
- Errors are `IpcError` with `E_*` codes. An item or session outside the caller's scope answers exactly as an unknown id (`orgs::not_found`).
- Text served to a per-host token is fenced with `crate::mcp::guard::fence_untrusted`.
- New desktop command = four places: `src-tauri/src/commands/work.rs` (command + `routed::`), `src-tauri/src/lib.rs` `generate_handler!`, `src-tauri/src/backend/verdicts.rs`, and the `(command, tool, action)` table in `crates/fleet-core/src/service/work/mod.rs` (near line 640). Then regenerate: `REGEN_HUB_VERDICTS=1 cargo fleet-test -- verdict_gen` and `REGEN_DOCS=1 cargo fleet-test -- reference_is_current` (each fails once on purpose; run again without the variable).
- A test that reads a file outside its crate uses `repo_files::read`, never `include_str!`.
- No attribution lines in commits. Conventional Commits.

## File Structure

| File | Responsibility |
|---|---|
| `crates/fleet-core/src/store/task_report.rs` (modify) | `TaskReport` gains `done`, `decisions`, `remaining`, `context`; `Store::latest_item_result` |
| `crates/fleet-core/src/service/work/report.rs` (modify) | parser and instruction for the new keys |
| `crates/fleet-core/src/service/work/wrapup.rs` (create) | `wrap_up` request for a live session |
| `crates/fleet-core/src/service/work/gate.rs` (create) | `Gate`, `completion_gate` |
| `crates/fleet-core/src/service/work/status.rs` (modify) | agent `done` refused when the gate is not passed |
| `crates/fleet-core/src/service/work/result.rs` (create) | `TaskResultView`, `result_for_item`, `result_for_session` |
| `crates/fleet-core/migrations/185_followup_origin.sql` (create) | `work_items.origin_task_id` |
| `crates/fleet-core/src/store/followups.rs` (create) | `Store::create_followup`, `Store::followups_of` |
| `crates/fleet-core/src/service/work/followup.rs` (create) | scope fence + title rules for a follow-up |
| `crates/fleet-core/src/service/work/handover.rs` (modify) | `OriginFacts`, rendered first in the brief |
| `crates/fleet-core/src/mcp/tools/orchestration.rs` (modify) | actions `wrap_up`, `followup` on `work_link`; `result` on `work` |
| `src-tauri/src/commands/work.rs`, `lib.rs`, `backend/verdicts.rs` (modify) | commands `request_work_result`, `work_result`, `create_followup_task` |
| `src/lib/rich_blocks.ts` (modify) | TS mirror of `TaskReport` |
| `src/lib/task_result.ts` (create) | types + `workResult`, `requestWorkResult`, `createFollowupTask` |
| `src/lib/TaskResultPanel.svelte` (create) | the panel (D6) |
| `src/lib/rich/ReportCard.svelte`, `TicketCard.svelte`, `WorkTaskDetail.svelte`, `RichText.svelte` (modify) | mount points |
| `docs/work-graph.md`, `docs/status.md` (modify) | what landed |

---

### Task 1: The report carries done, decisions, remaining and context

**Files:**
- Modify: `crates/fleet-core/src/store/task_report.rs:32-49`
- Modify: `crates/fleet-core/src/service/work/report.rs:33-103`
- Modify: `src/lib/rich_blocks.ts:42-50` and its `reportFromValue`
- Modify: `src/lib/rich/ReportCard.svelte`
- Test: `crates/fleet-core/src/service/work/report/tests.rs`, `src/lib/rich_blocks.test.ts`

**Interfaces:**
- Produces: `TaskReport { summary, outcome, done: Vec<String>, decisions: Vec<String>, remaining: Vec<String>, context: Vec<String>, tests_run, warnings, blockers, followups, confidence }`; TS `TaskReport` with the same four `string[]` fields.

- [ ] **Step 1: Write the failing Rust tests** (append to `service/work/report/tests.rs`)

```rust
#[test]
fn a_report_keeps_done_decisions_remaining_and_context() {
    let v = serde_json::json!({
        "summary": "Added the gate.",
        "outcome": "partial",
        "done": ["gate.rs with completion_gate"],
        "decisions": ["the gate never moves an item itself"],
        "remaining": ["Windows path of the probe"],
        "context": ["verify.rs computes on read"],
    });
    let r = report_from_value(&v).unwrap();
    assert_eq!(r.done, vec!["gate.rs with completion_gate"]);
    assert_eq!(r.decisions, vec!["the gate never moves an item itself"]);
    assert_eq!(r.remaining, vec!["Windows path of the probe"]);
    assert_eq!(r.context, vec!["verify.rs computes on read"]);
}

#[test]
fn a_report_stored_before_the_new_keys_still_reads() {
    let old = r#"{"summary":"x","outcome":"done","followups":["a"]}"#;
    let r: crate::store::TaskReport = serde_json::from_str(old).unwrap();
    assert!(r.done.is_empty() && r.decisions.is_empty());
    assert_eq!(r.followups, vec!["a"]);
}

#[test]
fn the_instruction_names_every_key_the_parser_reads() {
    let text = run_instruction("abc");
    for key in ["summary", "outcome", "done", "decisions", "remaining", "context",
                "tests_run", "warnings", "blockers", "followups", "confidence"] {
        assert!(text.contains(&format!("\"{key}\"")), "{key} missing");
    }
}
```

- [ ] **Step 2: Run them, expect a compile failure**

Run: `cargo fleet-test -- work::report::tests`
Expected: FAIL, `no field 'done' on type TaskReport`.

- [ ] **Step 3: Add the fields** in `store/task_report.rs`, after `outcome`:

```rust
    /// What the worker says it did, one short line each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub done: Vec<String>,
    /// Choices the worker made that the next task must keep.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<String>,
    /// Work the task did not finish.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remaining: Vec<String>,
    /// Facts a fresh session needs to continue (where things live, traps).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<String>,
```

In `report.rs::report_from_value`, inside the `TaskReport { .. }` literal:

```rust
        done: list(o.get("done")),
        decisions: list(o.get("decisions")),
        remaining: list(o.get("remaining")),
        context: list(o.get("context")),
```

Replace the body of `run_instruction`:

```rust
    format!(
        "When finished, print exactly {} on its own line, followed by one fenced \
         ```json block with these keys: \"summary\" (one paragraph), \"outcome\" \
         (done | partial | blocked | failed), \"done\" (what you did), \
         \"decisions\" (choices the next task must keep), \"remaining\" (work \
         left), \"context\" (facts a fresh session needs to continue), \
         \"tests_run\" (the commands you ran), \"warnings\", \"blockers\", \
         \"followups\" (each a task title) — all lists of short strings — and \
         \"confidence\" (low | medium | high). Fleet reads your commits and \
         changed files from git itself; do not list them.",
        crate::service::tasks::done_marker(nonce)
    )
```

- [ ] **Step 4: Fix every other `TaskReport { .. }` literal**

Run: `cargo fleet-check 2>&1 | grep -n 'missing fields'`
For each site add `..Default::default()` (the struct derives `Default`).

- [ ] **Step 5: Run the Rust tests**

Run: `cargo fleet-test -- work::report`
Expected: PASS.

- [ ] **Step 6: Mirror in TypeScript.** In `src/lib/rich_blocks.ts` add to `interface TaskReport` the lines `done: string[]; decisions: string[]; remaining: string[]; context: string[];`, and in the function that builds a report from parsed JSON (search `followups:` in the file) add the four fields with the same list helper `followups` uses. Add to `src/lib/rich_blocks.test.ts`:

```ts
it('keeps done, decisions, remaining and context of a report', () => {
  const text = [
    'FLEET_TASK_DONE_abc123',
    '```json',
    '{"summary":"s","outcome":"done","done":["a"],"decisions":["b"],"remaining":["c"],"context":["d"]}',
    '```',
  ].join('\n');
  const block = parseRich(text).find((b) => b.t === 'report');
  expect(block && block.t === 'report' && block.report).toMatchObject({
    done: ['a'], decisions: ['b'], remaining: ['c'], context: ['d'],
  });
});
```

(`parseRich` is the file's exported parser; use the name the existing report tests in that file call.)

- [ ] **Step 7: Show them in `ReportCard.svelte`.** After the summary block and before Blockers, add three groups in the existing markup style, each guarded by `.length`: "Done" (`report.done`, `data-testid="rich-report-done"`), "Decisions" (`rich-report-decisions`), "Left" (`report.remaining`, `rich-report-remaining`). `context` is not shown on the card; it is for the next session.

- [ ] **Step 8: Run frontend checks**

Run: `pnpm check && pnpm exec vitest related --run src/lib/rich_blocks.ts src/lib/rich/ReportCard.svelte`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/fleet-core/src/store/task_report.rs crates/fleet-core/src/service/work/report.rs crates/fleet-core/src/service/work/report/tests.rs src/lib/rich_blocks.ts src/lib/rich_blocks.test.ts src/lib/rich/ReportCard.svelte
git commit -m "feat(work): a task report carries done, decisions, remaining and context"
```

---

### Task 2: Finish task — a wrap-up request for a live session

**Files:**
- Create: `crates/fleet-core/src/service/work/wrapup.rs`
- Modify: `crates/fleet-core/src/service/work/mod.rs` (`pub mod wrapup;`, command table)
- Modify: `crates/fleet-core/src/mcp/tools/orchestration.rs` (next to the `handover` arm at ~1516)
- Modify: `src-tauri/src/commands/work.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/backend/verdicts.rs`
- Test: in `wrapup.rs` (`#[cfg(test)] mod tests`), `crates/fleet-core/src/mcp/tools/tests.rs` reach table (~12092), `tests_isolation.rs`

**Interfaces:**
- Consumes: `report::with_run_instruction(prompt, nonce)`, `tasks::create_task`, `Store::set_task_run(task_id, item_id, attempt, role)`, `tasks::start_task`, `sessions::send_system_prompt` (same call as `agent_handover::request`, `agent_handover.rs:233-320`).
- Produces: `pub const WRAPUP_ROLE: &str = "wrapup";` · `pub fn wrapup_prompt(key: &str) -> String` · `pub fn record_wrapup(s: &Store, session_id: i64, item_id: i64) -> Result<TaskRow, IpcError>` · `pub async fn request(store: &Mutex<Store>, ssh: &Arc<SshClient>, session_id: i64, scope: &OrgScope) -> Result<TaskRow, IpcError>`. Desktop command `request_work_result { session_id } -> TaskRow`. MCP `work_link { action: "wrap_up", session_id }`.

- [ ] **Step 1: Write the failing store-level tests** (in `wrapup.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    /// A session linked (primary, confirmed) to a local item. Copy the
    /// fixture `service::tasks::tests::seed` uses for the session and
    /// `Store::create_native_item` + `link_session_work_as` for the link.
    fn linked(s: &Store) -> (i64, i64) {
        let sid = crate::service::tasks::tests_support::seed(s, "h1", "dev-a");
        let item = s
            .create_native_item(&crate::store::NativeItem { title: "Add gate", ..Default::default() })
            .unwrap();
        s.link_session_work_as(sid, crate::store::WorkTarget::Item(item.id), "manual", true, None)
            .unwrap();
        (sid, item.id)
    }

    #[test]
    fn a_wrapup_is_a_running_task_of_role_wrapup_on_the_sessions_item() {
        let s = Store::open_in_memory().unwrap();
        let (sid, item) = linked(&s);
        let t = record_wrapup(&s, sid, item).unwrap();
        assert_eq!(t.state, "running");
        assert_eq!(t.role.as_deref(), Some(WRAPUP_ROLE));
        assert_eq!(t.work_item_id, Some(item));
        assert_eq!(t.worker_session_id, Some(sid));
        assert_eq!(t.requester_session_id, None);
    }

    #[test]
    fn a_wrapup_is_not_an_implement_attempt() {
        let s = Store::open_in_memory().unwrap();
        let (sid, item) = linked(&s);
        record_wrapup(&s, sid, item).unwrap();
        assert!(s.latest_item_task(item, "implement").unwrap().is_none());
        assert!(s.item_tasks(item, "implement").unwrap().is_empty());
    }

    #[test]
    fn a_second_wrapup_while_one_is_open_is_refused() {
        let s = Store::open_in_memory().unwrap();
        let (sid, item) = linked(&s);
        record_wrapup(&s, sid, item).unwrap();
        let e = record_wrapup(&s, sid, item).unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_EXISTS);
    }

    #[test]
    fn the_prompt_asks_for_no_new_work() {
        let p = wrapup_prompt("CF-12");
        assert!(p.contains("CF-12"));
        assert!(p.contains("Do not start new work"));
    }
}
```

If `service::tasks` has no shared `seed` outside its `tests` module, move its `fn seed` (`tasks.rs:1173`) into a `#[cfg(test)] pub(crate) mod tests_support` in the same file and use it from both.

- [ ] **Step 2: Run, expect failure**

Run: `cargo fleet-test -- work::wrapup`
Expected: FAIL, unresolved `record_wrapup`.

- [ ] **Step 3: Implement the pure and store halves**

```rust
//! `work_link { action: wrap_up, session_id }`: ask a live session for its
//! task result (design 2026-10-13 D2). The request is a `tasks` row of role
//! `wrapup` with the session as its own worker, so the Stop hook resolves it
//! exactly as it resolves a run (`service::tasks::handle_stop_for_worker`).

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs::{self, OrgScope};
use crate::service::tasks;
use crate::ssh::SshClient;
use crate::store::{Store, TaskRow};
use std::sync::{Arc, Mutex};

/// Not in `run::RUN_ROLES`: `work_link { run }` cannot ask for it, and
/// attempts count within `(item, role)`, so it never reads as an attempt.
pub const WRAPUP_ROLE: &str = "wrapup";

/// PURE: what the session is asked, before the report instruction.
pub fn wrapup_prompt(key: &str) -> String {
    format!(
        "Wrap up your work on {key}. Do not start new work. Commit what is \
         finished, then report what you did and what is left."
    )
}

/// Write the request under the lock. `E_EXISTS` while one is open.
pub fn record_wrapup(s: &Store, session_id: i64, item_id: i64) -> Result<TaskRow, IpcError> {
    let open = s
        .open_tasks_for_worker(session_id)?
        .into_iter()
        .any(|t| t.role.as_deref() == Some(WRAPUP_ROLE));
    if open {
        return Err(IpcError::new(
            codes::E_EXISTS,
            "a result is already requested from this session",
        ));
    }
    let key = s
        .get_work_item(item_id)?
        .map(|i| i.key.unwrap_or(i.title))
        .unwrap_or_else(|| format!("item {item_id}"));
    let task = tasks::create_task(s, None, Some(session_id), &wrapup_prompt(&key))?;
    let attempt = s.item_tasks(item_id, WRAPUP_ROLE)?.len() as i64 + 1;
    s.set_task_run(task.id, item_id, attempt, WRAPUP_ROLE)?;
    let task = s
        .get_task(task.id)?
        .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "task vanished after insert"))?;
    tasks::start_task(s, &task)
}
```

- [ ] **Step 4: Run the four tests**

Run: `cargo fleet-test -- work::wrapup`
Expected: PASS.

- [ ] **Step 5: Implement `request`.** Copy the body of `agent_handover::request` (`agent_handover.rs:233-320`) and change only these parts; keep its fences, `refuse_if_operator(.., "wrap up")`, `refusal(&row)` and `conversation_refusal` checks verbatim (make those two helpers `pub(super)`):

```rust
pub async fn request(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    session_id: i64,
    scope: &OrgScope,
) -> Result<TaskRow, IpcError> {
    let (row, task) = {
        let s = lock(store)?;
        let row = s
            .get_session_by_id(session_id)?
            .ok_or_else(|| orgs::not_found("session", session_id))?;
        // … the org/host fence, operator refusal and liveness refusals of
        //   agent_handover::request, unchanged …
        let item_id = s
            .session_work_links(row.id)?
            .into_iter()
            .find(|l| l.is_primary && l.state == "confirmed" && l.ended_at.is_none())
            .and_then(|l| l.item_id)
            .ok_or_else(|| {
                IpcError::new(
                    codes::E_INVALID,
                    format!("{} is not linked to any work; link it first", row.tmux_name),
                )
            })?;
        let task = record_wrapup(&s, row.id, item_id)?;
        (row, task)
    };
    let prompt = crate::service::work::report::with_run_instruction(&task.prompt, &task.nonce);
    if let Err(e) = crate::service::sessions::send_system_prompt(
        &row.host_alias, &row.tmux_name, &prompt, true, store, ssh,
    )
    .await
    {
        if let Ok(s) = store.lock() {
            let _ = tasks::fail_task(&s, task.id, &format!("not delivered: {}", e.message));
        }
        return Err(e);
    }
    Ok(task)
}
```

The fence comment lines in `agent_handover::request` (`// Org half; …`) are read by `scope_guard_tests`; keep them with the copied code.

- [ ] **Step 6: Wire the MCP action and the desktop command.** In `orchestration.rs`, directly after the `handover` arm, add an arm for `args.action == "wrap_up"` that is the `handover` arm with `wrapup::request` and returns the task through the same JSON helper `dispatch_task` uses for a `TaskRow`. Add `("work_link", "wrap_up", &["Drive"])` to the reach table in `mcp/tools/tests.rs` next to the `handover` row, and copy the four `handover` cases in `tests_isolation.rs:1261-1314` for `wrap_up`. Add `("request_work_result", "work_link", "wrap_up")` to the table in `service/work/mod.rs`. In `src-tauri/src/commands/work.rs` copy `RequestWorkHandoverArgs` / `request_work_handover` / `routed::request_work_handover` as `RequestWorkResultArgs` / `request_work_result` / `routed::request_work_result` (action `"wrap_up"`, return `TaskRow`); register in `lib.rs` after `request_work_handover`; add `("request_work_result", Verdict::Routed { tool: "work_link" })` in `verdicts.rs`. Mention `wrap_up` in the `work_link` `#[tool(description)]`.

- [ ] **Step 7: Regenerate and verify**

```bash
REGEN_HUB_VERDICTS=1 cargo fleet-test -- verdict_gen ; cargo fleet-test -- verdict_gen
REGEN_DOCS=1 cargo fleet-test -- reference_is_current ; cargo fleet-test -- reference_is_current
cargo fleet-test -- wrap_up
scripts/verify.sh
```
Expected: the first run of each REGEN pair fails on purpose, the second passes; the rest pass.

- [ ] **Step 8: Commit**

```bash
git add -A crates/fleet-core/src src-tauri/src src/lib/hub_verdicts.generated.json docs/hub.md docs/control-api-reference.md
git commit -m "feat(work): ask a live session for its task result (work_link wrap_up)"
```

---

### Task 3: The completion gate

**Files:**
- Create: `crates/fleet-core/src/service/work/gate.rs`
- Modify: `crates/fleet-core/src/service/work/mod.rs` (`pub mod gate;`)
- Modify: `crates/fleet-core/src/service/work/status.rs:25-45`
- Test: in `gate.rs`; `service/work/status.rs` tests

**Interfaces:**
- Consumes: `verify::verification(s, item, now) -> Option<Verification>`, `verify::CondCheck`, `Store::native_children(parent_id) -> Vec<WorkItemRow>`.
- Produces:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenChild { pub id: i64, pub title: String }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gate {
    /// `passed` | `unverified` | `blocked`.
    pub state: String,
    pub checks: Vec<CondCheck>,
    pub open_children: Vec<OpenChild>,
}
pub fn gate_from(v: Option<&Verification>, children: &[WorkItemRow]) -> Gate; // PURE
pub fn completion_gate(s: &Store, item: &WorkItemRow, now: i64) -> Result<Gate, IpcError>;
pub fn refusal(g: &Gate) -> Option<String>; // PURE: why an agent may not set done
```

- [ ] **Step 1: Write the failing tests** (in `gate.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::work::verify::{CondCheck, Verification};

    fn chk(line: &str, state: &str) -> CondCheck {
        CondCheck { line: line.into(), kind: "ci".into(), state: state.into(),
                    detail: String::new(), by: None, at: None }
    }
    fn ver(checks: Vec<CondCheck>) -> Verification {
        Verification { state: String::new(), checks }
    }

    #[test]
    fn no_lines_and_no_children_passes() {
        let g = gate_from(None, &[]);
        assert_eq!(g.state, "passed");
        assert!(refusal(&g).is_none());
    }

    #[test]
    fn a_failed_check_blocks_and_the_refusal_names_it() {
        let v = ver(vec![chk("ci", "pass"), chk("test:cargo test", "fail")]);
        let g = gate_from(Some(&v), &[]);
        assert_eq!(g.state, "blocked");
        assert!(refusal(&g).unwrap().contains("test:cargo test"));
    }

    #[test]
    fn a_pending_check_is_unverified_and_still_refuses_an_agent() {
        let v = ver(vec![chk("review", "pending")]);
        let g = gate_from(Some(&v), &[]);
        assert_eq!(g.state, "unverified");
        assert!(refusal(&g).unwrap().contains("review"));
    }

    #[test]
    fn an_open_child_blocks_a_done_or_rejected_one_does_not() {
        let s = crate::store::Store::open_in_memory().unwrap();
        let parent = s.create_native_item(&crate::store::NativeItem {
            title: "Parent", ..Default::default() }).unwrap();
        let open = s.create_native_item(&crate::store::NativeItem {
            title: "Child open", parent_id: Some(parent.id), ..Default::default() }).unwrap();
        let done = s.create_native_item(&crate::store::NativeItem {
            title: "Child done", parent_id: Some(parent.id), ..Default::default() }).unwrap();
        s.set_item_status(done.id, "done").unwrap();
        let g = completion_gate(&s, &parent, 0).unwrap();
        assert_eq!(g.state, "blocked");
        assert_eq!(g.open_children, vec![OpenChild { id: open.id, title: "Child open".into() }]);
    }
}
```

Check the real pending state name first: `grep -n '"pass"\|"fail"\|"pending"\|"unknown"' crates/fleet-core/src/service/work/verify.rs | head`. Use the value `check_line` returns for a line that is neither passed nor failed, in the test and in `gate_from`.

- [ ] **Step 2: Run, expect failure**

Run: `cargo fleet-test -- work::gate`
Expected: FAIL, unresolved `gate_from`.

- [ ] **Step 3: Implement**

```rust
//! The completion gate (design 2026-10-13 D3): what stands between a work
//! item and `done`. It reads the item's `done_when` checks (`verify.rs`) and
//! its open native children. It never moves an item; `status::set_status`
//! consults it for an agent, and the result panel shows it to a person.

use super::verify::{self, CondCheck, Verification};
use crate::ipc_error::IpcError;
use crate::store::{Store, WorkItemRow};
use serde::{Deserialize, Serialize};

// … OpenChild and Gate as in Interfaces …

/// PURE. A child counts as open unless it is done or a rejected proposal.
pub fn gate_from(v: Option<&Verification>, children: &[WorkItemRow]) -> Gate {
    let checks: Vec<CondCheck> = v.map(|v| v.checks.clone()).unwrap_or_default();
    let open_children: Vec<OpenChild> = children
        .iter()
        .filter(|c| c.status_category.as_deref() != Some("done"))
        .filter(|c| c.proposal_state.as_deref() != Some("rejected"))
        .map(|c| OpenChild { id: c.id, title: c.title.clone() })
        .collect();
    let state = if checks.iter().any(|c| c.state == "fail") || !open_children.is_empty() {
        "blocked"
    } else if checks.iter().all(|c| c.state == "pass") {
        "passed"
    } else {
        "unverified"
    };
    Gate { state: state.into(), checks, open_children }
}

pub fn completion_gate(s: &Store, item: &WorkItemRow, now: i64) -> Result<Gate, IpcError> {
    let v = verify::verification(s, item, now)?;
    let children = s.native_children(item.id)?;
    Ok(gate_from(v.as_ref(), &children))
}

/// PURE: the sentence an agent gets instead of `done`.
pub fn refusal(g: &Gate) -> Option<String> {
    if g.state == "passed" {
        return None;
    }
    let mut why: Vec<String> = g
        .checks
        .iter()
        .filter(|c| c.state != "pass")
        .map(|c| format!("`{}` is {}", c.line, c.state))
        .collect();
    why.extend(g.open_children.iter().map(|c| format!("subtask \"{}\" is open", c.title)));
    Some(format!(
        "this task is not done yet: {}. Report it instead; a person marks it done.",
        why.join("; ")
    ))
}
```

Confirm the field name of an item's status on `WorkItemRow` (`grep -n 'pub status' crates/fleet-core/src/store/rows.rs`) and use it in the first filter.

- [ ] **Step 4: Run the gate tests**

Run: `cargo fleet-test -- work::gate`
Expected: PASS.

- [ ] **Step 5: Write the failing enforcement test** (in `status.rs` tests; build the host scope the way the neighbouring tests in that module do)

```rust
#[test]
fn an_agent_may_not_set_done_past_an_open_subtask_a_person_may() {
    let store = std::sync::Mutex::new(crate::store::Store::open_in_memory().unwrap());
    let (parent, _child) = {
        let s = store.lock().unwrap();
        let p = s.create_native_item(&crate::store::NativeItem {
            title: "Parent", ..Default::default() }).unwrap();
        let c = s.create_native_item(&crate::store::NativeItem {
            title: "Child", parent_id: Some(p.id), ..Default::default() }).unwrap();
        (p.id, c.id)
    };
    let host = host_scope_seeing(&store, parent); // the module's fixture for OrgScope::Host
    let e = set_status(&store, &host, parent, "done").unwrap_err();
    assert_eq!(e.code, crate::ipc_error::codes::E_INVALID_STATE);
    assert!(e.message.contains("Child"));
    assert!(set_status(&store, &OrgScope::All, parent, "done").is_ok());
}
```

- [ ] **Step 6: Enforce in `set_status`**, before the final `s.set_item_status`:

```rust
    // Design 2026-10-13 D3: an agent does not decide a task is done past its
    // own acceptance lines or open subtasks. A person is never refused.
    if status == "done" && scope.host().is_some() && item.source == "local" {
        let gate = super::gate::completion_gate(&s, &item, crate::service::catalog::now_secs())?;
        if let Some(why) = super::gate::refusal(&gate) {
            return Err(IpcError::new(crate::ipc_error::codes::E_INVALID_STATE, why));
        }
    }
```

- [ ] **Step 7: Run and commit**

Run: `cargo fleet-test -- work::status && cargo fleet-test -- work::gate && scripts/verify.sh`
Expected: PASS. Then check no mission test regressed: `cargo fleet-test -- orchestrate`.

```bash
git add crates/fleet-core/src/service/work/gate.rs crates/fleet-core/src/service/work/mod.rs crates/fleet-core/src/service/work/status.rs
git commit -m "feat(work): completion gate; an agent cannot set done past it"
```

---

### Task 4: The result view

**Files:**
- Create: `crates/fleet-core/src/service/work/result.rs`
- Modify: `crates/fleet-core/src/store/task_report.rs` (add `latest_item_result`)
- Modify: `crates/fleet-core/src/service/work/mod.rs`, `mcp/tools/orchestration.rs` (`work { action: "result" }`), `src-tauri` (command `work_result`)
- Test: in `result.rs`, `store/task_report.rs`

**Interfaces:**
- Consumes: `Gate`, `completion_gate` (Task 3); `TaskReport`, `TaskEvidence`; `Store::pull_request_by_url`.
- Produces:

```rust
impl Store {
    /// The newest `done` task of `item_id` with a stored report, any role.
    pub fn latest_item_result(&self, item_id: i64) -> Result<Option<TaskRow>, IpcError>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultPr { pub url: String, pub number: Option<i64>, pub state: Option<String>,
                      pub ci_status: Option<String>, pub review_decision: Option<String> }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultFollowup { pub text: String, pub item_id: Option<i64> }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskResultView {
    pub task_id: i64,
    pub item_id: i64,
    pub session_id: Option<i64>,
    pub role: String,
    pub at: i64,
    /// The done marker the report followed; the conversation matches on it.
    pub marker: String,
    pub report: TaskReport,
    pub evidence: Option<TaskEvidence>,
    pub prs: Vec<ResultPr>,
    pub gate: Gate,
    pub followups: Vec<ResultFollowup>,
}
pub fn result_for_item(store: &Mutex<Store>, scope: &OrgScope, item_id: i64) -> Result<Option<TaskResultView>, IpcError>;
pub fn result_for_session(store: &Mutex<Store>, scope: &OrgScope, session_id: i64) -> Result<Option<TaskResultView>, IpcError>;
```

Desktop command `work_result { item_id?: i64, session_id?: i64 } -> Option<TaskResultView>`; MCP `work { action: "result", item_id | session_id }`.

- [ ] **Step 1: Write the failing store test** (in `store/task_report.rs` tests, or a new `tests` module there)

```rust
#[test]
fn the_latest_result_is_the_newest_done_task_with_a_report() {
    let s = Store::open_in_memory().unwrap();
    let item = s.create_native_item(&crate::store::NativeItem {
        title: "T", ..Default::default() }).unwrap();
    let mk = |role: &str, report: bool| {
        let t = crate::service::tasks::create_task(&s, None, None, "p").unwrap();
        s.set_task_run(t.id, item.id, 1, role).unwrap();
        s.finish_task(t.id, "done", Some("r"), None).unwrap();
        if report {
            s.set_task_report(t.id, &TaskReport { summary: role.into(),
                outcome: "done".into(), ..Default::default() }).unwrap();
        }
        t.id
    };
    let _plain = mk("implement", false);
    let first = mk("implement", true);
    assert_eq!(s.latest_item_result(item.id).unwrap().unwrap().id, first);
    let second = mk("wrapup", true);
    assert_eq!(s.latest_item_result(item.id).unwrap().unwrap().id, second);
}
```

- [ ] **Step 2: Run, expect failure**

Run: `cargo fleet-test -- the_latest_result_is`
Expected: FAIL, no method `latest_item_result`.

- [ ] **Step 3: Implement the store read** (next to `latest_item_task`, same column helper):

```rust
    pub fn latest_item_result(&self, item_id: i64) -> Result<Option<super::TaskRow>, IpcError> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {} FROM tasks WHERE work_item_id = ?1 AND state = 'done' \
                       AND result_json IS NOT NULL \
                     ORDER BY finished_at DESC, id DESC LIMIT 1",
                    super::rows::TASK_COLUMNS
                ),
                [item_id],
                super::rows::map_task_row,
            )
            .optional()?)
    }
```

Use whichever column constant `latest_item_task` (same file, ~line 128) passes to `format!`.

- [ ] **Step 4: Write the failing view tests** (in `result.rs`)

```rust
#[test]
fn the_view_joins_report_evidence_gate_and_follow_ups() {
    // item with one done wrapup task whose report has followups ["A", "B"]
    // and whose evidence has one commit; a child item is still open.
    let (store, item, task) = fixture();
    let v = result_for_item(&store, &OrgScope::All, item).unwrap().unwrap();
    assert_eq!(v.task_id, task);
    assert_eq!(v.role, "wrapup");
    assert!(v.marker.starts_with("FLEET_TASK_DONE_"));
    assert_eq!(v.evidence.as_ref().unwrap().commits.len(), 1);
    assert_eq!(v.gate.state, "blocked");
    assert_eq!(v.followups, vec![
        ResultFollowup { text: "A".into(), item_id: None },
        ResultFollowup { text: "B".into(), item_id: None },
    ]);
}

#[test]
fn an_item_with_no_report_has_no_result() {
    let (store, item) = bare_item();
    assert!(result_for_item(&store, &OrgScope::All, item).unwrap().is_none());
}

#[test]
fn an_item_outside_the_scope_answers_as_unknown() {
    let (store, item, _) = fixture();
    let other = other_org_scope(&store); // as status.rs / local.rs tests build it
    let e = result_for_item(&store, &other, item).unwrap_err();
    assert_eq!(e.code, crate::ipc_error::codes::E_NOTFOUND);
}
```

Write `fixture`, `bare_item` with `Store::open_in_memory`, `create_native_item`, `create_task` + `set_task_run(.., "wrapup")` + `finish_task` + `set_task_report` + `set_task_evidence`, exactly as Step 1 does.

- [ ] **Step 5: Implement the view.** Visibility is the check `status::set_status` runs (`status.rs:32-42`); lift it into `pub(super) fn item_visible(s, scope, &item) -> Result<bool, IpcError>` in `status.rs` and call it from both. Then:

```rust
pub fn result_for_item(
    store: &Mutex<Store>,
    scope: &OrgScope,
    item_id: i64,
) -> Result<Option<TaskResultView>, IpcError> {
    let s = lock(store)?;
    let Some(item) = s.get_work_item(item_id)? else {
        return Err(orgs::not_found("work item", item_id));
    };
    if !super::status::item_visible(&s, scope, &item)? {
        return Err(orgs::not_found("work item", item_id));
    }
    let Some(task) = s.latest_item_result(item_id)? else { return Ok(None) };
    let Some(mut report) = crate::store::task_report::decode::<TaskReport>(task.result_json.clone())
    else { return Ok(None) };
    if scope.host().is_some() {
        report.summary = crate::mcp::guard::fence_untrusted(
            &report.summary, "a task report", crate::service::work::report::SUMMARY_MAX_CHARS);
    }
    let evidence = crate::store::task_report::decode::<TaskEvidence>(task.evidence_json.clone());
    let gate = super::gate::completion_gate(&s, &item, crate::service::catalog::now_secs())?;
    let prs = task
        .worker_session_id
        .and_then(|sid| s.get_session_by_id(sid).ok().flatten())
        .and_then(|row| row.pr_url)
        .and_then(|url| s.pull_request_by_url(&url).ok().flatten())
        .map(|p| ResultPr { url: p.url, number: p.number, state: p.state,
                            ci_status: p.ci_status, review_decision: p.review_decision })
        .into_iter()
        .collect();
    let made = s.followups_of(task.id).unwrap_or_default(); // Task 5; until then: Vec::new()
    let followups = report.followups.iter().map(|t| ResultFollowup {
        text: t.clone(),
        item_id: made.iter().find(|(title, _)| title == t).map(|(_, id)| *id),
    }).collect();
    Ok(Some(TaskResultView {
        task_id: task.id, item_id, session_id: task.worker_session_id,
        role: task.role.clone().unwrap_or_else(|| "implement".into()),
        at: task.finished_at.unwrap_or(0),
        marker: crate::service::tasks::done_marker(&task.nonce),
        report, evidence, prs, gate, followups,
    }))
}
```

Make `decode` in `store/task_report.rs` `pub(crate)`. In this task write `let made: Vec<(String, i64)> = Vec::new();`; Task 5 replaces the line. Match `ResultPr` field types to `PullRequestRow` (`store/pull_requests.rs`). `result_for_session` resolves the session's primary confirmed link (the lookup in `wrapup::request`) and calls `result_for_item`; a session with no link answers `Ok(None)`.

- [ ] **Step 6: Wire `work { action: "result" }` and the command `work_result`** by copying how `work { action: "task" }` / command `work_task` are wired (search `"task" =>` or `action == "task"` in `orchestration.rs`, and `work_task` in `src-tauri/src/commands/work.rs`, `lib.rs`, `verdicts.rs`, and the table in `service/work/mod.rs`: add `("work_result", "work", "result")`). Reach: the same as `work { task }`.

- [ ] **Step 7: Regenerate, verify, commit**

```bash
REGEN_HUB_VERDICTS=1 cargo fleet-test -- verdict_gen ; cargo fleet-test -- verdict_gen
REGEN_DOCS=1 cargo fleet-test -- reference_is_current ; cargo fleet-test -- reference_is_current
cargo fleet-test -- work::result && scripts/verify.sh
git add -A crates/fleet-core/src src-tauri/src src/lib/hub_verdicts.generated.json docs/hub.md docs/control-api-reference.md
git commit -m "feat(work): one result view per task: report, evidence, gate, PR"
```

---

### Task 5: Create a follow-up task from a result

**Files:**
- Create: `crates/fleet-core/migrations/185_followup_origin.sql`
- Modify: `crates/fleet-core/src/store/schema.rs` (registered after the `184_…` entry)
- Create: `crates/fleet-core/src/store/followups.rs`; modify `store/mod.rs`, `store/work.rs` (`WorkItemRow.origin_task_id`)
- Create: `crates/fleet-core/src/service/work/followup.rs`
- Modify: `service/work/result.rs` (the `made` line), `mcp/tools/orchestration.rs`, `src-tauri`
- Test: `store/followups.rs`, `service/work/followup.rs`, `store/schema/tests_upgrade.rs` if it lists columns

**Interfaces:**
- Consumes: `Store::create_native_item(&NativeItem) -> WorkItemRow`, `TaskResultView` (Task 4).
- Produces:

```rust
impl Store {
    /// A sibling of the task's item, born from that task's result. The same
    /// (task, title) twice answers the first item; `.1` says whether it is new.
    pub fn create_followup(&self, origin_task_id: i64, title: &str) -> Result<(WorkItemRow, bool), IpcError>;
    /// (title, item id) of every follow-up made from `task_id`.
    pub fn followups_of(&self, task_id: i64) -> Result<Vec<(String, i64)>, IpcError>;
}
pub fn create(store: &Mutex<Store>, scope: &OrgScope, task_id: i64, index: Option<usize>, title: Option<&str>) -> Result<WorkItemRow, IpcError>;
```

`WorkItemRow.origin_task_id: Option<i64>`. MCP `work_link { action: "followup", task_id, index? , title? }`; desktop command `create_followup_task { task_id, index?, title? } -> WorkItemRow`.

- [ ] **Step 1: Write the migration**

```sql
-- Design docs/superpowers/specs/2026-10-13-task-result-handover-design.md D4:
-- a follow-up task remembers the task whose result it came from. The origin
-- session, item and result all follow from that `tasks` row.
--
-- `work_items.origin_task_id`  NULL for every item not made from a result.
ALTER TABLE work_items ADD COLUMN origin_task_id INTEGER REFERENCES tasks(id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS idx_work_items_origin_task
  ON work_items(origin_task_id) WHERE origin_task_id IS NOT NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (185);
```

Register it in `store/schema.rs` after the `184` entry, in the guarded form (`already_applied: Some(work_items_has_origin_task_id)`, as migration 117 does for `done_when`): it is an `ADD COLUMN`. Add `origin_task_id` to the work-item column list (`ITEM_COLUMNS`, `ITEM_COLUMN_COUNT`) and mapper in `store/work.rs` and to `WorkItemRow` with `#[serde(default, skip_serializing_if = "Option::is_none")]`.

- [ ] **Step 2: Write the failing store tests** (`store/followups.rs`)

```rust
#[test]
fn a_follow_up_is_a_sibling_that_names_its_origin_task() {
    let s = Store::open_in_memory().unwrap();
    let epic = s.create_native_item(&NativeItem { title: "Epic", ..Default::default() }).unwrap();
    let src = s.create_native_item(&NativeItem {
        title: "Source", parent_id: Some(epic.id), ..Default::default() }).unwrap();
    let task = done_task_on(&s, src.id); // create_task + set_task_run(.., "wrapup") + finish_task
    let (f, new) = s.create_followup(task, "  Add integration   tests ").unwrap();
    assert!(new);
    assert_eq!(f.title, "Add integration tests");
    assert_eq!(f.parent_id, Some(epic.id));
    assert_eq!(f.project_id, src.project_id);
    assert_eq!(f.origin_task_id, Some(task));
    assert_eq!(f.origin.as_deref(), Some("manual"));
}

#[test]
fn the_same_follow_up_twice_is_one_item() {
    let s = Store::open_in_memory().unwrap();
    let src = s.create_native_item(&NativeItem { title: "Source", ..Default::default() }).unwrap();
    let task = done_task_on(&s, src.id);
    let (a, _) = s.create_followup(task, "Add tests").unwrap();
    let (b, new) = s.create_followup(task, "add  TESTS").unwrap();
    assert!(!new);
    assert_eq!(a.id, b.id);
    assert_eq!(s.followups_of(task).unwrap(), vec![("Add tests".to_string(), a.id)]);
}

#[test]
fn a_task_with_no_item_has_no_follow_ups() {
    let s = Store::open_in_memory().unwrap();
    let t = crate::service::tasks::create_task(&s, None, None, "p").unwrap();
    let e = s.create_followup(t.id, "x").unwrap_err();
    assert_eq!(e.code, crate::ipc_error::codes::E_INVALID);
}
```

- [ ] **Step 3: Run, expect failure**

Run: `cargo fleet-test -- store::followups`
Expected: FAIL, no method `create_followup`.

- [ ] **Step 4: Implement the store half**

```rust
//! Follow-up tasks (design 2026-10-13 D4): an item made from a task's
//! result, a sibling of that task's item, naming the task as its origin.

use super::{NativeItem, Store, WorkItemRow};
use crate::ipc_error::{codes, IpcError};

fn norm(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

impl Store {
    pub fn followups_of(&self, task_id: i64) -> Result<Vec<(String, i64)>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT title, id FROM work_items WHERE origin_task_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([task_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn create_followup(
        &self,
        origin_task_id: i64,
        title: &str,
    ) -> Result<(WorkItemRow, bool), IpcError> {
        let source = self
            .get_task(origin_task_id)?
            .and_then(|t| t.work_item_id)
            .map(|id| self.get_work_item(id))
            .transpose()?
            .flatten()
            .ok_or_else(|| {
                IpcError::new(codes::E_INVALID, "that task has no work item to follow up on")
            })?;
        let want = norm(title);
        if let Some((_, id)) = self
            .followups_of(origin_task_id)?
            .into_iter()
            .find(|(t, _)| norm(t) == want)
        {
            let row = self.get_work_item(id)?.ok_or_else(|| {
                IpcError::new(codes::E_INTERNAL, "follow-up vanished")
            })?;
            return Ok((row, false));
        }
        let clean = title.split_whitespace().collect::<Vec<_>>().join(" ");
        let item = self.create_native_item(&NativeItem {
            title: &clean,
            parent_id: source.parent_id,
            project_id: source.project_id,
            notes: None,
        })?;
        self.conn.execute(
            "UPDATE work_items SET origin_task_id = ?1 WHERE id = ?2",
            rusqlite::params![origin_task_id, item.id],
        )?;
        let row = self.get_work_item(item.id)?.ok_or_else(|| {
            IpcError::new(codes::E_INTERNAL, "follow-up vanished")
        })?;
        Ok((row, true))
    }
}
```

A follow-up of a top-level item whose source is a tracker ticket still becomes a local item (`create_native_item` writes `source = local`); that is intended: fleet never creates tickets at a tracker here. If `create_native_item` refuses a `parent_id` that is a tracker item, retry with `parent_id: None` and add the test `a_follow_up_of_a_tracker_subtask_is_top_level`.

- [ ] **Step 5: Run the store tests, then the migration tests**

Run: `cargo fleet-test -- store::followups && cargo fleet-test -- schema`
Expected: PASS (`migrations_are_contiguous_from_one` included).

- [ ] **Step 6: Write the failing service tests** (`service/work/followup.rs`)

```rust
#[test]
fn index_picks_the_reports_follow_up_and_a_title_overrides_it() {
    let (store, task) = fixture_with_followups(&["Add tests", "Fix Windows"]);
    let a = create(&store, &OrgScope::All, task, Some(1), None).unwrap();
    assert_eq!(a.title, "Fix Windows");
    let b = create(&store, &OrgScope::All, task, None, Some("Write docs")).unwrap();
    assert_eq!(b.title, "Write docs");
}

#[test]
fn neither_index_nor_title_or_an_index_out_of_range_is_invalid() {
    let (store, task) = fixture_with_followups(&["Add tests"]);
    for (i, t) in [(None, None), (Some(5), None)] {
        let e = create(&store, &OrgScope::All, task, i, t).unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_INVALID);
    }
}

#[test]
fn a_task_outside_the_scope_answers_as_unknown() {
    let (store, task) = fixture_with_followups(&["Add tests"]);
    let e = create(&store, &other_org_scope(&store), task, Some(0), None).unwrap_err();
    assert_eq!(e.code, crate::ipc_error::codes::E_NOTFOUND);
}
```

- [ ] **Step 7: Implement the service half**

```rust
pub fn create(
    store: &Mutex<Store>,
    scope: &OrgScope,
    task_id: i64,
    index: Option<usize>,
    title: Option<&str>,
) -> Result<WorkItemRow, IpcError> {
    let s = lock(store)?;
    let task = s.get_task(task_id)?.ok_or_else(|| orgs::not_found("task", task_id))?;
    let item = task
        .work_item_id
        .map(|id| s.get_work_item(id))
        .transpose()?
        .flatten()
        .ok_or_else(|| orgs::not_found("task", task_id))?;
    if !super::status::item_visible(&s, scope, &item)? {
        return Err(orgs::not_found("task", task_id));
    }
    let report = crate::store::task_report::decode::<crate::store::TaskReport>(task.result_json.clone());
    let title: String = match (title.map(str::trim).filter(|t| !t.is_empty()), index) {
        (Some(t), _) => t.to_string(),
        (None, Some(i)) => report
            .as_ref()
            .and_then(|r| r.followups.get(i).cloned())
            .ok_or_else(|| IpcError::new(codes::E_INVALID, "no follow-up at that index"))?,
        (None, None) => {
            return Err(IpcError::new(codes::E_INVALID, "a follow-up needs an index or a title"))
        }
    };
    Ok(s.create_followup(task_id, &title)?.0)
}
```

A follow-up text is up to 500 characters; `create_native_item` validates the title length. Cut the report's text to `LOCAL_WORK_TITLE_MAX_CHARS` on a char boundary before passing it, and put the full text in the item's notes when it was cut (pass `notes: Some(&full)` through a `notes` parameter on `create_followup`). Add the test `a_long_follow_up_is_cut_to_a_title_and_kept_whole_in_the_notes`.

- [ ] **Step 8: Replace the `made` line in `result.rs`** with `let made = s.followups_of(task.id)?;` and match by normalised title (move `norm` to `pub(crate)`). Extend `the_view_joins_…` : after `create_followup(task, "A")`, `v.followups[0].item_id` is `Some(_)`.

- [ ] **Step 9: Wire `work_link { action: "followup" }` and the command `create_followup_task`** as Task 2 Step 6 did (`("create_followup_task", "work_link", "followup")`; verdict `Routed { tool: "work_link" }`). Reach: what `work_link { create }` takes (see its row in the reach table of `mcp/tools/tests.rs`); a per-host token is allowed, as it is for `create`. Add `followup` to the `work_link` tool description.

- [ ] **Step 10: Regenerate, verify, commit**

```bash
REGEN_HUB_VERDICTS=1 cargo fleet-test -- verdict_gen ; cargo fleet-test -- verdict_gen
REGEN_DOCS=1 cargo fleet-test -- reference_is_current ; cargo fleet-test -- reference_is_current
cargo fleet-test -- followup && scripts/verify.sh
git add -A crates/fleet-core src-tauri/src src/lib/hub_verdicts.generated.json docs/hub.md docs/control-api-reference.md
git commit -m "feat(work): create a follow-up task from a task's result"
```

---

### Task 6: A follow-up's fresh session inherits the origin's result

**Files:**
- Modify: `crates/fleet-core/src/service/work/handover.rs` (`HandoverInput`, `render`, the function that fills `HandoverInput` from the store)
- Test: the tests module of `handover.rs`

**Interfaces:**
- Consumes: `WorkItemRow.origin_task_id` (Task 5), `TaskReport`, `TaskEvidence`.
- Produces:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct OriginFacts {
    pub key: String,
    pub title: Option<String>,
    pub outcome: String,
    pub summary: String,
    pub decisions: Vec<String>,
    pub remaining: Vec<String>,
    pub context: Vec<String>,
    pub commits: Vec<String>,   // "sha7 subject"
    pub files: Vec<String>,
    pub pr_url: Option<String>,
}
pub fn origin_facts(s: &Store, item: &WorkItemRow) -> Result<Option<OriginFacts>, IpcError>;
```

`HandoverInput.origin: Option<OriginFacts>` with `#[serde(default, skip_serializing_if = "Option::is_none")]`.

- [ ] **Step 1: Write the failing tests**

```rust
fn facts() -> OriginFacts {
    OriginFacts {
        key: "CF-7".into(), title: Some("Add the gate".into()),
        outcome: "partial".into(), summary: "Gate added, Windows left.".into(),
        decisions: vec!["the gate never moves an item".into()],
        remaining: vec!["Windows probe path".into()],
        context: vec!["verify.rs computes on read".into()],
        commits: vec!["abc1234 feat: gate".into()],
        files: vec!["crates/fleet-core/src/service/work/gate.rs".into()],
        pr_url: Some("https://github.com/o/r/pull/9".into()),
    }
}

#[test]
fn a_follow_ups_brief_opens_with_what_the_origin_task_left() {
    let input = HandoverInput { key: "CF-8".into(), origin: Some(facts()), ..Default::default() };
    let out = build_handover(&input);
    let at = |needle: &str| out.find(needle).unwrap_or_else(|| panic!("{needle} missing"));
    assert!(at("Follows CF-7") < at("Decisions to keep"));
    assert!(out.contains("the gate never moves an item"));
    assert!(out.contains("Windows probe path"));
    assert!(out.contains("abc1234 feat: gate"));
    assert!(out.contains("https://github.com/o/r/pull/9"));
}

#[test]
fn an_oversized_origin_still_fits_the_brief() {
    let mut f = facts();
    f.decisions = (0..20).map(|i| format!("{i} {}", "d".repeat(480))).collect();
    f.files = (0..200).map(|i| format!("src/file_{i}.rs")).collect();
    let input = HandoverInput { key: "CF-8".into(), origin: Some(f), ..Default::default() };
    assert!(build_handover(&input).chars().count() <= BRIEF_MAX_CHARS);
}

#[test]
fn a_brief_with_no_origin_is_unchanged() {
    let input = HandoverInput { key: "CF-8".into(), ..Default::default() };
    assert!(!build_handover(&input).contains("Follows"));
}
```

If `HandoverInput` does not derive `Default`, add it (every field is `Option`, `Vec`, `String` or `usize`).

- [ ] **Step 2: Run, expect failure**

Run: `cargo fleet-test -- work::handover`
Expected: FAIL, no field `origin`.

- [ ] **Step 3: Implement.** Add the struct and the field. Add to `Limits` (`handover.rs:105`) the caps `origin_items: usize` and `origin_item_chars: usize`, and give each rung of `BRIEF_LADDER` and `CONTEXT_LADDER` a value: first rung `8` / `300`, each later rung smaller, last rung `2` / `120`. In `render`, directly after the item header and before every other section:

```rust
    if let Some(o) = &input.origin {
        let cut = |s: &str| -> String { s.chars().take(l.origin_item_chars).collect() };
        out.push_str(&format!(
            "\n## Follows {}{}\nIts result ({}): {}\n",
            o.key,
            o.title.as_deref().map(|t| format!(" — {}", cut(t))).unwrap_or_default(),
            o.outcome,
            cut(&o.summary),
        ));
        for (heading, items) in [
            ("Decisions to keep", &o.decisions),
            ("Left undone", &o.remaining),
            ("Context", &o.context),
            ("Its commits", &o.commits),
            ("Files it changed", &o.files),
        ] {
            if items.is_empty() { continue; }
            out.push_str(&format!("{heading}:\n"));
            for it in items.iter().take(l.origin_items) {
                out.push_str(&format!("- {}\n", cut(it)));
            }
            if items.len() > l.origin_items {
                out.push_str(&format!("- … {} more\n", items.len() - l.origin_items));
            }
        }
        if let Some(pr) = &o.pr_url {
            out.push_str(&format!("Its PR: {pr}\n"));
        }
    }
```

Use the string-building style `render` already uses (it may use `writeln!` or a `Vec<String>`); keep one style in the function.

- [ ] **Step 4: Fill it from the store.** Write the failing test `the_start_brief_of_a_follow_up_carries_its_origin` against the function that builds a `HandoverInput` for an item (find it: `grep -n 'HandoverInput {' crates/fleet-core/src/service/work/*.rs`): make an item, a done `wrapup` task with a report and evidence, `create_followup`, build the input for the follow-up, assert `input.origin.unwrap().key` is the source's key. Then implement:

```rust
pub fn origin_facts(s: &Store, item: &WorkItemRow) -> Result<Option<OriginFacts>, IpcError> {
    let Some(task) = item.origin_task_id.map(|id| s.get_task(id)).transpose()?.flatten() else {
        return Ok(None);
    };
    let Some(src) = task.work_item_id.map(|id| s.get_work_item(id)).transpose()?.flatten() else {
        return Ok(None);
    };
    let report = crate::store::task_report::decode::<crate::store::TaskReport>(task.result_json.clone())
        .unwrap_or_default();
    let ev = crate::store::task_report::decode::<crate::store::TaskEvidence>(task.evidence_json.clone());
    let pr_url = task
        .worker_session_id
        .and_then(|sid| s.get_session_by_id(sid).ok().flatten())
        .and_then(|r| r.pr_url);
    Ok(Some(OriginFacts {
        key: src.key.clone().unwrap_or_else(|| format!("#{}", src.id)),
        title: Some(src.title.clone()),
        outcome: report.outcome,
        summary: report.summary,
        decisions: report.decisions,
        remaining: report.remaining,
        context: report.context,
        commits: ev.as_ref().map(|e| e.commits.iter()
            .map(|c| format!("{} {}", c.sha.chars().take(7).collect::<String>(), c.subject))
            .collect()).unwrap_or_default(),
        files: ev.map(|e| e.files.into_iter().map(|f| f.path).collect()).unwrap_or_default(),
        pr_url,
    }))
}
```

and set `origin: origin_facts(&s, &item)?` where the input is built. The report text is third-party to the new session: the brief is already delivered inside the fence the handover uses (`service/hooks.rs:337,715`); confirm by reading those two sites, and if the brief is sent unfenced on the `start` path, wrap the origin section's lists with `crate::mcp::guard::fence_untrusted(.., "a previous task's report", ..)` in `render`.

- [ ] **Step 5: Run and commit**

Run: `cargo fleet-test -- work::handover && cargo fleet-test -- work::resume && scripts/verify.sh`
Expected: PASS.

```bash
git add crates/fleet-core/src/service/work
git commit -m "feat(work): a follow-up's brief opens with what its origin task left"
```

---

### Task 7: The result panel

**Files:**
- Create: `src/lib/task_result.ts`, `src/lib/task_result.test.ts`
- Create: `src/lib/TaskResultPanel.svelte`, `src/lib/TaskResultPanel.test.ts`
- Modify: `src/lib/TicketCard.svelte` (next to "Ask for a handover", ~line 155)
- Modify: `src/lib/WorkTaskDetail.svelte:937-950`
- Modify: `src/lib/rich/RichText.svelte` (where a `report` block draws `ReportCard`)

**Interfaces:**
- Consumes: commands `work_result`, `request_work_result`, `create_followup_task`, `set_work_status` (Tasks 2, 4, 5).
- Produces (`task_result.ts`):

```ts
import type { TaskReport } from './rich_blocks';
export interface GateCheck { line: string; kind: string; state: string; detail: string }
export interface Gate { state: 'passed' | 'unverified' | 'blocked'; checks: GateCheck[]; open_children: { id: number; title: string }[] }
export interface ResultPr { url: string; number: number | null; state: string | null; ci_status: string | null; review_decision: string | null }
export interface TaskEvidence { commits?: { sha: string; subject: string }[]; commits_total: number; files?: { path: string; added?: number; removed?: number }[]; files_total: number; uncommitted?: number; error?: string }
export interface TaskResultView { task_id: number; item_id: number; session_id: number | null; role: string; at: number; marker: string; report: TaskReport; evidence: TaskEvidence | null; prs: ResultPr[]; gate: Gate; followups: { text: string; item_id: number | null }[] }
export function workResult(by: { itemId: number } | { sessionId: number }): Promise<Result<TaskResultView | null>>;
export function requestWorkResult(sessionId: number): Promise<Result<TaskRow>>;
export function createFollowupTask(taskId: number, pick: { index: number } | { title: string }): Promise<Result<WorkItemRow>>;
export function validationLines(v: TaskResultView): { label: string; state: 'pass' | 'fail' | 'pending' | 'reported' }[];
export function headline(v: TaskResultView): { label: string; tone: 'success' | 'warning' | 'danger' };
```

- [ ] **Step 1: Write the failing unit tests** (`task_result.test.ts`)

```ts
import { describe, expect, it } from 'vitest';
import { headline, validationLines, type TaskResultView } from './task_result';

const base: TaskResultView = {
  task_id: 1, item_id: 2, session_id: 3, role: 'wrapup', at: 0, marker: 'FLEET_TASK_DONE_x',
  report: { summary: 's', outcome: 'done', done: [], decisions: [], remaining: [], context: [],
            tests_run: ['cargo fleet-test'], warnings: [], blockers: [], followups: [], confidence: null },
  evidence: null, prs: [], followups: [],
  gate: { state: 'passed', checks: [], open_children: [] },
};

describe('task result', () => {
  it('lists gate checks before reported tests and marks the latter as reported', () => {
    const v = { ...base, gate: { ...base.gate, checks: [{ line: 'ci', kind: 'ci', state: 'pass', detail: '' }] } };
    expect(validationLines(v)).toEqual([
      { label: 'ci', state: 'pass' },
      { label: 'cargo fleet-test', state: 'reported' },
    ]);
  });
  it('says completed only when the report is done and the gate passed', () => {
    expect(headline(base)).toEqual({ label: 'Task completed', tone: 'success' });
    expect(headline({ ...base, gate: { ...base.gate, state: 'blocked' } }))
      .toEqual({ label: 'Completion blocked', tone: 'danger' });
    expect(headline({ ...base, gate: { ...base.gate, state: 'unverified' } }))
      .toEqual({ label: 'Reported done, not verified', tone: 'warning' });
    expect(headline({ ...base, report: { ...base.report, outcome: 'partial' } }))
      .toEqual({ label: 'Partly done', tone: 'warning' });
    expect(headline({ ...base, report: { ...base.report, outcome: 'failed' } }))
      .toEqual({ label: 'Failed', tone: 'danger' });
    expect(headline({ ...base, report: { ...base.report, outcome: 'blocked' } }))
      .toEqual({ label: 'Blocked', tone: 'danger' });
  });
});
```

- [ ] **Step 2: Run, expect failure**

Run: `pnpm exec vitest run src/lib/task_result.test.ts`
Expected: FAIL, cannot resolve `./task_result`.

- [ ] **Step 3: Implement `task_result.ts`**: the types above; the three command wrappers in the form of `requestWorkHandover` (`src/lib/work.ts:1028`), passing `{ args: { item_id } }` / `{ args: { session_id } }` / `{ args: { task_id, index, title } }`; and:

```ts
export function validationLines(v: TaskResultView) {
  const state = (s: string) => (s === 'pass' ? 'pass' : s === 'fail' ? 'fail' : 'pending') as 'pass' | 'fail' | 'pending';
  return [
    ...v.gate.checks.map((c) => ({ label: c.line, state: state(c.state) })),
    ...v.report.tests_run.map((t) => ({ label: t, state: 'reported' as const })),
  ];
}

export function headline(v: TaskResultView): { label: string; tone: 'success' | 'warning' | 'danger' } {
  switch (v.report.outcome) {
    case 'failed': return { label: 'Failed', tone: 'danger' };
    case 'blocked': return { label: 'Blocked', tone: 'danger' };
    case 'partial': return { label: 'Partly done', tone: 'warning' };
  }
  if (v.gate.state === 'blocked') return { label: 'Completion blocked', tone: 'danger' };
  if (v.gate.state === 'unverified') return { label: 'Reported done, not verified', tone: 'warning' };
  return { label: 'Task completed', tone: 'success' };
}
```

- [ ] **Step 4: Write the failing component test** (`TaskResultPanel.test.ts`, with `@testing-library/svelte` as the other `*.test.ts` component tests in `src/lib` do; mock `./task_result`'s `createFollowupTask` with `vi.mock`)

```ts
it('shows the result first and the detail folded', async () => {
  const { getByTestId, queryByTestId } = render(TaskResultPanel, { result: withTwoFollowups, title: 'Add the gate' });
  expect(getByTestId('task-result-headline').textContent).toContain('Task completed');
  expect(getByTestId('task-result-summary')).toBeTruthy();
  expect((getByTestId('task-result-changes') as HTMLDetailsElement).open).toBe(false);
  expect(queryByTestId('task-result-mark-done')).toBeNull(); // gate passed + already done handled by parent
});

it('creates a task from a follow-up and then shows it as created', async () => {
  createFollowupTask.mockResolvedValue({ ok: true, value: { id: 77, title: 'A' } });
  const { getAllByTestId, findByTestId } = render(TaskResultPanel, { result: withTwoFollowups, title: 'T' });
  await fireEvent.click(getAllByTestId('task-result-followup-create')[0]);
  expect(createFollowupTask).toHaveBeenCalledWith(1, { index: 0 });
  expect((await findByTestId('task-result-followup-open-77')).textContent).toContain('Open task');
});

it('a blocked gate names what blocks', () => {
  const { getByTestId } = render(TaskResultPanel, { result: blockedByChild, title: 'T' });
  expect(getByTestId('task-result-gate').textContent).toContain('Child open');
});
```

- [ ] **Step 5: Build `TaskResultPanel.svelte`.** Props: `{ result: TaskResultView; title: string; sessionId?: number | null; onOpenTask?: (id: number) => void; onChanged?: () => void }`. Markup, top to bottom, reusing `StatusChip`, `Badge`, `MarkdownView`, `CopyButton` and the `.card.rich-card` classes `ReportCard` uses:

1. Header: `headline(result)` as a `StatusChip` (`data-testid="task-result-headline"`), the title, the word "reported" as a muted badge.
2. Summary (`task-result-summary`), then "What changed" = `report.done` as a list.
3. Validation: `validationLines(result)`, one row each with a pass / fail / pending / reported mark (`task-result-validation`).
4. Gate, only when not `passed` (`task-result-gate`): failed and pending lines, then open subtasks by title.
5. PRs: one chip per `result.prs` with state, CI and review, opening the URL with `openExternal`.
6. `<details data-testid="task-result-changes">`: commits (`sha.slice(0,7)` + subject), files with `+added −removed`, "N more" from the totals, uncommitted count.
7. `<details>` Decisions, Left (`report.remaining`), Blockers, Warnings: each only when non-empty.
8. Follow-ups (`task-result-followups`): each `result.followups` row shows the text and either a "Create task" button (`task-result-followup-create`, calls `createFollowupTask(result.task_id, { index })`, then sets the row's `item_id` locally and calls `onChanged`) or "Open task" (`task-result-followup-open-<id>`, calls `onOpenTask`). Below: a one-line input with "Create follow-up" calling `createFollowupTask(result.task_id, { title })`.
9. Actions row: "Ask about result" (`insertIntoComposer(sessionId, 'About the result of this task: ')`, only with a `sessionId`); "Mark done" (`task-result-mark-done`, calls the existing `setWorkStatus(item_id, 'done')` from `work.ts`, shown only when the parent passes `canMarkDone`).

A failed command shows its `IpcError.message` in the row it belongs to, as `TicketCard` does for a failed handover request.

- [ ] **Step 6: Mount it in three places.**
  - `TicketCard.svelte`: next to "Ask for a handover" add "Finish task" calling `requestWorkResult(session.id)`; disable it under the same condition the handover button uses, and show the `E_EXISTS` message inline.
  - `WorkTaskDetail.svelte`: load `workResult({ itemId })` with the detail; when it answers a result, draw `<TaskResultPanel>` above "Last outcome" with `canMarkDone = task is local && status !== 'done'`, `onOpenTask` = the page's existing navigation to a task, `onChanged` = the page's reload.
  - `rich/RichText.svelte`: where a `report` block draws `ReportCard`, accept an optional `result: TaskResultView | null` prop threaded from `ConversationPanel` (which loads `workResult({ sessionId })` when the session is linked to work and reloads it on the session's row event); when `result?.marker === block.marker` draw `TaskResultPanel`, else `ReportCard` as today.

- [ ] **Step 7: Run frontend checks**

Run: `pnpm check && pnpm exec vitest related --run src/lib/task_result.ts src/lib/TaskResultPanel.svelte src/lib/TicketCard.svelte src/lib/WorkTaskDetail.svelte src/lib/rich/RichText.svelte`
Expected: PASS, no svelte-check errors.

- [ ] **Step 8: See it in the app.** With a display available: `pnpm tauri dev`, link a session to a local task, press "Finish task", wait for the reply, confirm the panel in the conversation and on the task page, create a follow-up, start it, and read the first prompt of the new session for the "Follows …" section. In a cloud session skip this step and say so in the PR.

- [ ] **Step 9: Commit**

```bash
git add src/lib
git commit -m "feat(ui): one result panel per task, with follow-ups that become tasks"
```

---

### Task 8: Docs, status and the full ladder

**Files:**
- Modify: `docs/work-graph.md` (new section "Task result, gate and follow-ups")
- Modify: `docs/status.md` (landed: result panel, `wrap_up`, gate for agents, follow-ups, origin in the brief; not built: the list in the spec's §4)
- Modify: `skills/` only if a `SKILL.md` there lists `work_link` actions (`grep -rn 'handover' skills/*/SKILL.md`): add `wrap_up` and `followup` beside it

- [ ] **Step 1: Write the docs section.** In `docs/work-graph.md`, state in this order: what a result is (D1) and where it is stored; how a person asks for one (`Finish task` / `work_link { wrap_up }`) and what the session is asked to print (quote `run_instruction`); the gate's three states and who it refuses (D3); what a follow-up inherits and what it does not (D4, D5: no conversation text); the three commands and their MCP actions.

- [ ] **Step 2: Run the whole ladder**

Run: `scripts/verify.sh full`
Expected: exit 0. A failure in `service::add_project` or `fleet-agent`'s `report_frames_stay_under_the_frame_cap_and_carry_the_rest_over` is a known flake: run it alone before changing anything.

- [ ] **Step 3: Check the migration number against main**

Run: `git fetch origin main && scripts/check-migration-numbers.sh`
Expected: exit 0. If main took 185: merge `origin/main`, then `scripts/renumber-migrations.sh`.

- [ ] **Step 4: Commit**

```bash
git add docs skills
git commit -m "docs(work): task result, completion gate and follow-ups"
```

---

## Self-review notes

- Spec coverage: D1 → Tasks 1, 4. D2 → Task 2. D3 → Task 3. D4 → Task 5. D5 → Task 6. D6 → Task 7. §5 trust → fences in Tasks 2, 4, 5, 6. §6 measures need no code: they are queries over `tasks.role`, `tasks.result_json`, `work_items.origin_task_id`.
- Names used across tasks: `WRAPUP_ROLE`, `record_wrapup`, `wrapup::request`, `Gate`, `OpenChild`, `completion_gate`, `gate::refusal`, `status::item_visible`, `latest_item_result`, `TaskResultView`, `ResultPr`, `ResultFollowup`, `result_for_item`, `result_for_session`, `create_followup`, `followups_of`, `followup::create`, `OriginFacts`, `origin_facts`; commands `request_work_result`, `work_result`, `create_followup_task`; actions `wrap_up`, `result`, `followup`.
- Four places name something to confirm in the source before typing it, each with the command that answers it: the pending state name of a check (Task 3 Step 1), the status field of `WorkItemRow` (Task 3 Step 3), the column constant of `latest_item_task` (Task 4 Step 3), the builder of `HandoverInput` (Task 6 Step 4).
