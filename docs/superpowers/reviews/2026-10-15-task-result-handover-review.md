# Task result, completion gate and follow-ups: independent review (CF-22, step 4.1)

Date: 2026-10-15. Branch `feature/cloud-fleet-task-execution-bce8a1` at
48c6f79d34, reviewed against
`docs/superpowers/specs/2026-10-13-task-result-handover-design.md` and the
two plans. The review was done by reading the code, in four parts: scope
fences, the `wrapup` role and the gate, follow-ups and the brief, and the
frontend. Rust was not compiled (rule 1 of the execution plan), so build 4 is
the check for every Rust fix below. The frontend checks were run.

## Fixed on this branch

| # | Finding | Fix |
|---|---|---|
| 1 | `work_link { followup }` read the task's report with only the org fence. A person who may not read the task (another person's session, a detached task) got its follow-up text back in the new item. `work { result }` hides the same text since 48c6f79d34. | The MCP arm checks `task_visible_at(.., Reach::Read)` first, and a task the caller may not read answers like an unknown one. The reach table moves `followup` from `WORK_ACTION_NO_GATE` to `WORK_ACTION_REACH` at `Read`. |
| 2 | A follow-up's origin brief (`handover::origin_facts`, which `work { handover\|context }` and the start brief use) was gated on the org half only, so the same text leaked to whoever started or read the follow-up. | `origin_facts` takes the whole `ViewScope` and drops the origin unless `tasks::task_visible_in_scope` passes. `tickets::with_native_defaults` now takes the `ViewScope`, and its tests pass `ViewScope::internal()` (narrowed with `with_org` where they used to pass an org). |
| 3 | The operator and a peer hub, which `Caller::work_decider` counts as agents, have `OrgScope::All`, so `set_status` let them set a local item to `done` past the gate. | `status::refuse_agent_done` (the same `refuse_past_gate` that `set_status` uses) is called from the `set_status` arm when `work_decider() == Agent`. |
| 4 | A `wrapup` row was read as a mission run: `is_mission_worker` (so the worker guard stopped a person's session from `git push` / `gh pr merge` once it had pressed Finish task), `mission_task_counts` (spent attempts and slots, and tripped the no-progress brake after one ignored wrap-up), `mission_cost_micros` (added a person's whole session cost to the mission) and `run_cost_estimate`. | All four leave out `role = 'wrapup'`. There is a test in `store/mission_loop/tests.rs`. |
| 5 | The panel crashed (TypeError) on any report with an empty list. Rust leaves out empty lists and unset options (`skip_serializing_if`), and `task_result.ts` read them as required arrays. A PR with no number also read `#undefined`. | `workResult` normalises the view (`normalizeResult`, report through `reportFromValue`). The PR label uses `!= null`. There is a test with the sparse payload. |
| 6 | The task page showed the result panel *and* "Last outcome", where D6 says the panel replaces it. | "Last outcome" shows only when there is no result. The test asserts it is gone, and `docs/work-graph.md` now says so. |
| 7 | A report follow-up longer than a title (stored cut) was never matched in the view, so "Create task" stayed offered after the task was made. | The view matches on `followup::cut_title`, as `create` stores it. There is a test. |
| 8 | The brief's worst case (a long key, PR URL, title and summary beside five origin lists) went over `BRIEF_MAX_CHARS` at the last rung. `cap()` then cut the closing untrusted fence and the footer. | There is one more rung, `origin_spare`: the origin's lists shrink to their "… N more" lines before anything is cut, in `fit` and in `origin_section`. There is a worst-case test that checks the footer and that every fence is closed. |
| 9 | `work { result }` fenced commit subjects for a per-host token but not evidence file paths, which the origin brief does fence. | The paths are fenced too. |

## Verified, no change

- **D1:** the four `TaskReport` lists are `#[serde(default)]`, so old rows decode. `latest_item_result` returns the newest `done` task with a report, any role.
- **D2:**
  - `wrap_up` takes `Reach::Drive` plus the org fence and refuses the operator session.
  - A second open request is `E_EXISTS`, and a failed delivery fails the task.
  - `wrapup` is not in `RUN_ROLES`.
  - Attempt and retry readers filter by role.
  - A marker does not move the item's status.
- **D3:** `gate_from` matches the table, and the refusal names the lines and subtasks, with hidden titles blanked.
- **D4:**
  - Migration 195 follows main's 194, with an `already_applied` guard.
  - `ITEM_COLUMNS` and `map_item` carry `origin_task_id`.
  - `create_followup` is idempotent under the store lock.
- **D5:**
  - The origin is fenced and every line defused.
  - No conversation text is carried.
  - The lists are capped (at most 8 per list in practice, stricter than the spec's 10 / 15).
- **Wiring:**
  - All three commands are `Routed`, and `hub_verdicts.generated.json` agrees.
  - `control-api-reference.md` lists the new parameters.
  - The isolation matrix has rows for `wrap_up`, `result` and `followup`.
- **Reticle:** it is dev-only and safe in a production bundle (`isReticleDisabledBuild`; `src/reticle-dev.ts` is behind `import.meta.env.DEV`).

## Left for the owner (not fixed here)

- **A scoped caller can make a top-level local item.** A per-host token or an org-bound client may `followup` from a task whose item has no parent. That creates a top-level local item, which `create` refuses to a scoped caller. The isolation matrix encodes the current behaviour. Decide whether it stays or follows `create`'s two guards.
- **Plan import bypasses the gate.** `work_link { mission_import }` sets a row's status straight through `set_item_status`. The operator could set `done` past the gate that way. It is a person's table import in practice.
- **The PR shown is the session's current one.** The result and the origin brief read the worker session's PR at read time. A session that opens another PR after its wrap-up shows the newer one. `TaskEvidence` records no PR.
- **The start brief cuts the rest without a fence.** `with_native_defaults` cuts the text after the origin section with a plain `.take(max)`. A parent ticket's description can lose its closing fence there; the same cut existed for subtask briefs before this work.
- **A cut follow-up title can merge two lines.** Two report follow-ups whose first 120 characters match collapse into one item, and the second line's full text is not kept.
- **A control character blocks one follow-up.** A report follow-up with a control character cannot be created by index, because `cut_title` only folds whitespace and the title check refuses it.
- **Panel buttons ignore hub-offline.** The panel's Mark done and Create task are not disabled when the hub is offline, as WorkTaskDetail's own Mark done is. The error shows inline after the click.
- **Archived subtasks still block.** An archived open subtask blocks the gate (`native_children` does not filter `archived_at`).
- **A finish request blocks the next one for a day.** A wrap-up the session answered without its marker blocks a new Finish task until the 24 h TTL or `cancel_task`.
- **The graph shows a wrap-up as running.** `work::graph` reads an open `wrapup` as the member `running` and shows it as the item's attempt.
- **Unrelated dev tooling.** The Reticle wiring (`vite.config.ts`, `src/reticle-dev.ts`, two devDependencies) has nothing to do with this work and could go in its own PR.
- **The panel was never seen running.** Task 7 Step 8 (seeing the panel in the running app) needs `pnpm tauri dev`, which rule 1 excludes.
