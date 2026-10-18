# Task result, completion gate and follow-up: design

Date: 2026-10-13. Source: the owner's handover "Cloud Fleet — jednotný Task
Execution, Handover & Orchestration systém" (31 sections, pasted into the
session of 2026-10-13). This document covers its first vertical slice (§31 of
the handover) and, in §7, the order of everything after it.

Plan: `docs/superpowers/plans/2026-10-13-task-result-handover.md`.

## 1. What the slice delivers

```
task A → work → "Finish task" → structured result + gate → panel
       → "Create task" on a follow-up → task B → fresh session whose brief
         carries A's decisions, remaining work, commits and files
```

Every piece sits on code that exists. Nothing here adds a second task model.

## 2. What exists today (read from source, 2026-10-13)

| Piece | Where | Limit today |
|---|---|---|
| Structured report | `TaskReport` in `crates/fleet-core/src/store/task_report.rs` → `tasks.result_json` | asked only of `work_link { run }` and headless checks; no decisions, remaining work or context |
| Git evidence | `TaskEvidence` → `tasks.evidence_json`, `service/work/report.rs::collect_evidence` | same tasks only |
| Marker pipeline | `service/tasks.rs::handle_stop_for_worker` resolves `FLEET_TASK_DONE_<nonce>`, parses the report, collects evidence | needs a `tasks` row; an interactive session has none |
| Acceptance lines | `work_items.done_when`, `service/work/verify.rs::verification` | enforced only by the mission loop |
| Status write | `service/work/status.rs::set_status` → `Store::set_item_status` | checks nothing |
| Follow-ups | `TaskReport.followups: Vec<String>` | no reader turns one into an item |
| Brief | `service/work/handover.rs::build_handover` (≤ 4000 chars, deterministic) | knows nothing of a previous task's result |
| Cards | `src/lib/rich/ReportCard.svelte`, `HandoverCard.svelte`, `src/lib/PrResult.svelte` | three cards, "Ask" only fills the composer |

## 3. Decisions

**D1. The result is a `tasks` row, not a new table.** A task result is the
newest finished task of a work item that carries `result_json`:
report + evidence + the gate computed on read. `TaskReport` gains four lists:
`done`, `decisions`, `remaining`, `context`. All are `#[serde(default)]`, so
stored rows and older hubs still read.

**D2. An interactive session finishes through an explicit request.** A person
presses "Finish task" (`work_link { action: "wrap_up", session_id }`). Fleet
writes a `tasks` row with role `wrapup` for the session's primary item, with
the session as worker and no requester, and types the run instruction into
the pane. The Stop hook then does what it does for a run. Rejected: guessing
the end of a task from a Stop (`turn_outcome` is a Jev guess and is off), and
a new MCP tool the agent calls (a second parsing path, and the agent decides
when it is done).

`tasks.attempt` counts within `(work_item_id, role)` (migration 110) and
`latest_item_task` filters by role, so a `wrapup` row is never read as an
implement attempt. `wrapup` is not added to `RUN_ROLES`: `work_link { run }`
cannot ask for it.

**D3. The gate is one pure function, enforced where an agent could skip it.**
`service/work/gate.rs::completion_gate(store, item, now) -> Gate` reads the
item's `done_when` checks and its open native children. `blocked` when a
check failed or a child is open; `unverified` when lines exist and not all
passed; `passed` otherwise. Enforcement:

- A per-host token (an agent) setting a local item to `done` is refused with
  `E_INVALID_STATE` unless the gate is `passed`. The refusal names the lines.
- A person is never refused: the panel shows the gate and a "Mark done"
  button. Fleet does not move an item to `done` on a report's say.
- The mission loop keeps its own close step (`orchestrate/steps.rs`).

**D4. A follow-up is a sibling item with one lineage column.**
`work_items.origin_task_id` (migration 196) points at the task that carried
the result. Origin session, origin item and origin result all follow from
that row. The new item takes the source item's `parent_id` and `project_id`,
origin `manual`. Creating the same follow-up twice answers the existing item.

**D5. Inherited context is computed at start, not copied.** `HandoverInput`
gains `origin: Option<OriginFacts>`; `build_handover` renders it first. It
holds, from the origin task: key and title, outcome, decisions, remaining
work, context lines, commits (≤ 10), files (≤ 15) and the PR link. No
conversation text travels. Every list is capped, so the brief stays under
`BRIEF_MAX_CHARS`.

**D6. One panel.** `src/lib/TaskResultPanel.svelte` shows outcome, what was
done, validation (reported tests + gate checks), changes, PR, follow-ups with
"Create task", and actions. It replaces `ReportCard` in the conversation for
the turn that carries the result, and "Last outcome" on the task page when a
result exists. `ReportCard` stays for reports with no task behind them.

## 4. Out of this slice

Explain / translate mini-sessions, sub-agent results, action detection,
Create PR / Merge PR buttons, policy inheritance, hooks, skills per task,
Jev context selection, project memory. §7 orders them.

## 5. Trust and scope

- The report is what the worker said. The panel labels it "reported"; the
  gate and the evidence are what fleet read itself.
- A result served to a per-host token is fenced with
  `mcp::guard::fence_untrusted`, as `TaskDetail.last_outcome` is.
- `wrap_up` types into a pane: it takes `Reach::Drive` and the org fence, as
  `work_link { handover }` does, and is refused for the operator session.
- `followup` and `result` answer an item outside the caller's scope exactly
  as an unknown id.

## 6. Measures of success

After two weeks of use, read from the store:

- share of work items moved to `done` that have a task with `result_json`;
- share of `wrapup` tasks that ended `failed` (no marker within the TTL) or
  with no parsable report;
- number of items with `origin_task_id`, and how many were started.

## 7. After this plan

Ordered by what each step unlocks and what it depends on. Each is its own
spec and plan.

1. **Results from every dispatch, and child results to the parent.**
   `dispatch_task` gets the structured instruction (one-paragraph result
   stays as the fallback). A per-host agent may start a `run` under its own
   item, with its own worktree, capped per parent. The parent reads child
   results, never transcripts. The panel gets the collapsed "N agents" row.
   Depends on: this plan (D1). Unlocks handover §10–11.
2. **Ask about a result.** A tool-less `claude -p` over one result, its
   evidence and the files it names: explain, simplify, translate. Nothing is
   stored but the question's cost. Reuses `service/work/summary.rs`.
   Depends on: D1. Unlocks §5–6.
3. **Buttons that act, and PR signals in the Inbox.** Card actions that call
   a command instead of filling the composer; "Create PR" and a manual
   "Merge PR" behind the shepherd's own preconditions
   (`pr_shepherd/merge.rs`); Attention reasons `pr_ready` and
   `changes_requested`. Depends on: nothing new. Unlocks §16, §23.
4. **One automation policy.** A single policy record resolved
   Fleet → Org → Project → Task → Session with "inherited from" in the
   answer, and three presets. It replaces, in place, the shepherd rule
   (`watch|nudge|merge`), the agent rule (`watch|propose|act`), the mission
   level and grant, the routine `autonomy` and `automation.paused`. This is
   the largest structural step: do it before adding any new automatic
   behaviour, or each one adds a sixth mechanism. Unlocks §19–22.
5. **PR babysitter, complete.** Store review threads and reviews; classify a
   comment (Jev `pr_triage` out of shadow, closed set); autofix by policy
   with the mechanical / semantic split; automerge checks approvals and
   unresolved threads and takes a merge mode. Depends on: 3 and 4.
6. **Lifecycle hook points and skills per task.** A registry of fleet-side
   points (`before_context_build`, `before_complete`, `before_pr_create` …)
   whose result is allow / deny, warnings, required actions and an
   instruction patch; the gate of D3 becomes the first `before_complete`
   hook. Skills attach by rule on what a task touches. Depends on: D3, 4.
7. **Jev context selection.** KEEP / SUMMARIZE / REFERENCE / DROP over what
   D5 does not already keep deterministically. Run it in `shadow` against
   D5's brief first; turn it on only where `decision_runs` shows it beats
   the rule. Depends on: D5 in use, org consent for the text sent.
8. **Project memory.** Decisions, known problems and conventions distilled
   from results into a per-project store the brief draws from.
   Depends on: enough results (measure §6), 7.
9. **One provenance record.** A single "why did fleet do this" row per
   automatic action: policy source, checks, AI judgement or none. Depends
   on: 4, 6.
10. **Task-first navigation.** Work as the default view, sessions as a
    detail of a task. Last, because it is only right once 1–3 make a task
    page enough to work from.

Decision points before each: after 1, whether agents may spawn runs without a
person's grant; before 4, whether policy lives in settings (`SPECS`) or its
own table; before 7, the Jev consent and budget.
