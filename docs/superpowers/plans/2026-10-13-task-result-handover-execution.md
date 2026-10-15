# Task Result & Handover: multi-agent execution plan

> **For the orchestrating session:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. You are the orchestrator: you dispatch the agents below, merge their branches, and are the only one who asks Buildkite for a build.

**What this is:** the order and the rules for executing `docs/superpowers/plans/2026-10-13-task-result-handover.md` (the *task plan*) with several agents at once. The task plan says WHAT each piece is, with its tests and code. This file says WHO does which piece, WHEN, in which files, and how it is verified. Where the two differ on order or file ownership, this file wins; on code and tests, the task plan wins.

**Spec:** `docs/superpowers/specs/2026-10-13-task-result-handover-design.md`.

**Feature branch:** `feature/cloud-fleet-task-execution-bce8a1`. Everything merges into it. One PR from it to `main` at the end.

## Rules (owner's, 2026-10-13)

1. **Rust is compiled only on Buildkite.** No agent runs anything that compiles Rust on its machine: no `cargo build`, `check`, `clippy`, `test`, `fleet-fast-check`, `fleet-check`, `fleet-lint`, `fleet-test`, no `scripts/verify.sh` without `remote`, no `scripts/ci-local.sh`, no `pnpm tauri dev|build`, no rust-analyzer check. Allowed locally: `cargo fmt --all` (it formats, it does not compile).
2. **Do not enable the git hooks** in an agent worktree (`core.hooksPath` stays unset): the pre-commit hook runs clippy locally. Run `cargo fmt --all --check` by hand before each Rust commit instead.
3. **Frontend runs locally:** `pnpm install --frozen-lockfile`, `pnpm check`, `pnpm exec vitest related --run <files>`, `pnpm test`.
4. **Android may be built on the machine.** This repository has no Android target (fleet-mobile is its own repository), so no step here needs it. If a task turns out to need a fleet-mobile change, stop and report it; it is a separate branch in that repository.
5. **One Buildkite build at a time, only of the feature branch, only by the orchestrator.** The builder has concurrency 1 and builds what GitHub has (`docs/buildkite.md`):

   ```bash
   git push origin feature/cloud-fleet-task-execution-bce8a1
   scripts/verify.sh remote
   ```

   It needs `BUILDKITE_API_TOKEN` and `BUILDKITE_ORG` in the environment. Without them, stop and ask the owner; do not fall back to a local build.
6. **Generated files cannot be regenerated locally** (`REGEN_*` runs are `cargo test`). The wiring agent (Wave C) edits them by hand in the format of their neighbours: `src/lib/hub_verdicts.generated.json`, the refusal table in `docs/hub.md`, `docs/control-api-reference.md`. Buildkite's `verdict_gen` and `reference_is_current` tests are the check; on a mismatch the client prints the failing job's log, which shows the expected text. Fix from the log and build again.
7. Commits: Conventional Commits, no attribution lines. No `--no-verify`.

## Why the waves look like this

Agents cannot compile, so a compile error is found only by a Buildkite build (3–4 min warm, one at a time). The plan therefore minimises builds, not agent time:

- Work is cut by **file ownership**, so parallel branches merge without conflicts.
- The three tasks that each touch `mcp/tools/orchestration.rs`, `src-tauri/src/commands/work.rs`, `lib.rs`, `verdicts.rs`, the command table and the generated docs (task plan Tasks 2, 4, 5) do **service code only** in Waves A and B. One agent does all the wiring in Wave C.
- One build per wave: **4 planned builds** for 8 tasks, plus fix builds.

```
W0 scaffold (orchestrator)
 └─ Wave A: A1 report · A2 wrapup · A3 gate · A4 follow-up store · A5 frontend core   → build 1
     └─ Wave B: B1 result view · B2 follow-up service · B3 origin brief · B4 mounts   → build 2
         └─ Wave C: C1 wiring · C2 docs                                               → build 3
             └─ Wave D: review, PR                                                    → build 4 (final HEAD)
```

## Branches and merging

- Each agent works in its own worktree on `tr/<id>` (for example `tr/a3-gate`), branched from the feature branch's HEAD **at the start of its wave**. Dispatch with `isolation: "worktree"`.
- An agent commits to its branch and reports the branch name and commit. It does not push and does not merge.
- The orchestrator merges each `tr/*` into the feature branch with `git merge --no-ff tr/<id>` in the order listed in the wave, then pushes once and builds once.
- A conflict means a file-ownership rule was broken. Resolve it in the feature branch, then tell the owning agent what changed.
- After a failed build: the orchestrator reads the log, sends each error to the agent that owns the file (SendMessage to the same agent, so it keeps its context), merges the fixes, builds again. **At most two fix builds per wave**; a third failure stops the run with a report to the owner.

## Brief every agent gets

Paste this, then the agent's own section from below:

> You implement one slice of `docs/superpowers/plans/2026-10-13-task-result-handover.md` in the claude-fleet repo. Read the spec `docs/superpowers/specs/2026-10-13-task-result-handover-design.md` and your task in the plan before writing. **You must not compile Rust**: no cargo command except `cargo fmt --all`. You cannot run Rust tests; write them exactly, then read your code against the signatures it calls (`graft skeleton <file>`, `graft grep "<symbol>"`) and check every name, argument order and type by reading the definition. Where the task plan says "confirm in the source", do it and use what you find. Touch only the files listed as yours. Frontend checks (`pnpm check`, `pnpm exec vitest related --run …`) you do run. Commit to your branch with a Conventional Commit, no attribution line, hooks stay disabled. Report: branch, commit, files changed, every place you could not confirm a name by reading, and anything in the plan that did not match the source.

## W0. Scaffold (orchestrator, no agent)

So that no two agents edit the same registration lines. One commit on the feature branch, `chore(work): scaffold task-result modules`:

- `crates/fleet-core/src/service/work/mod.rs`: add `pub mod wrapup; pub mod gate; pub mod result; pub mod followup;` beside the other `pub mod` lines, and create the four files, each holding only a `//!` line naming its purpose.
- `crates/fleet-core/src/store/mod.rs`: add `mod followups;` in the form its neighbours use, and create `store/followups.rs` with a `//!` line.
- `crates/fleet-core/migrations/185_followup_origin.sql` with the SQL of task plan Task 5 Step 1, registered in `store/schema.rs` after the `184` entry (main took 184 on 2026-10-13), with an `already_applied` guard. Confirm the number first: `git fetch origin main && git ls-tree --name-only origin/main crates/fleet-core/migrations/ | tail -1`.
- `crates/fleet-core/src/store/work.rs`: `WorkItemRow.origin_task_id: Option<i64>` in the struct, the column list and the mapper (follow `done_when`).

No build for W0 alone; Wave A's build covers it.

## Wave A (five agents in parallel)

| Id | Task plan | Owns (only these files) |
|---|---|---|
| A1 | Task 1 Steps 1–5 (Rust only); Task 4 Steps 1–3 (`latest_item_result`); make `decode` `pub(crate)` | `store/task_report.rs`, `service/work/report.rs`, `service/work/report/tests.rs`; `..Default::default()` at other `TaskReport { .. }` literals found with `graft grep "TaskReport {"` |
| A2 | Task 2 Steps 1–5 (service only, no wiring) | `service/work/wrapup.rs`, `service/work/agent_handover.rs` (two helpers to `pub(super)`), `service/tasks.rs` (only the `tests_support` move) |
| A3 | Task 3, all steps; plus lift `item_visible` out of `set_status` (task plan Task 4 Step 5, first sentence) | `service/work/gate.rs`, `service/work/status.rs` |
| A4 | Task 5 Steps 2–5 (store half) | `store/followups.rs` |
| A5 | Task 1 Steps 6–8 (TS mirror, `ReportCard`); Task 7 Steps 1–5 (`task_result.ts`, panel, tests) | `src/lib/rich_blocks.ts`, `src/lib/rich_blocks.test.ts`, `src/lib/rich/ReportCard.svelte`, `src/lib/task_result.ts`, `src/lib/task_result.test.ts`, `src/lib/TaskResultPanel.svelte`, `src/lib/TaskResultPanel.test.ts` |

Notes for the briefs:

- **A1** also touches literals in files other agents own only if `graft grep` finds one there; in that case it reports the line instead of editing, and the orchestrator adds `..Default::default()` at merge.
- **A2** stops before Step 6 of Task 2. Its `request` returns `TaskRow`.
- **A3**: `item_visible(s: &Store, scope: &OrgScope, item: &WorkItemRow) -> Result<bool, IpcError>`, `pub(super)`, in `status.rs`. B1 and B2 call it.
- **A4** makes `norm` `pub(crate)` (B1 uses it) and gives `create_followup` the `notes: Option<&str>` parameter the task plan's Step 7 asks for, so its signature is final in Wave A: `create_followup(&self, origin_task_id: i64, title: &str, notes: Option<&str>) -> Result<(WorkItemRow, bool), IpcError>`.
- **A5** runs `pnpm check` and vitest itself and reports green before it hands over. The command wrappers call commands that do not exist until Wave C; its tests mock them.

Merge order: A1, A3, A4, A2, A5. Then **build 1**.

## Wave B (four agents in parallel, branched after build 1 is green)

| Id | Task plan | Owns |
|---|---|---|
| B1 | Task 4 Steps 4–5 (view and its tests; no wiring). Use `s.followups_of(task.id)?` directly, no stub | `service/work/result.rs` |
| B2 | Task 5 Steps 6–7 (service half) | `service/work/followup.rs` |
| B3 | Task 6, all steps | `service/work/handover.rs` and the one file that builds `HandoverInput` (it names it in its report before editing) |
| B4 | Task 7 Step 6 (mounts) and Step 7 | `src/lib/TicketCard.svelte`, `src/lib/WorkTaskDetail.svelte`, `src/lib/rich/RichText.svelte`, `src/lib/ConversationPanel.svelte` |

Notes:

- **B1** and **B2** both need a scope for "another org" in tests. Each builds it inside its own test module, copying the fixture of `service/work/local.rs`'s tests; no shared helper file.
- **B3**: if the file that builds `HandoverInput` is `service/work/resume.rs` or `service/trackers/tickets.rs`, it owns only the lines that fill the struct.
- **B4** keeps `ReportCard` as the fallback and runs the frontend checks.

Merge order: B1, B2, B3, B4. Then **build 2**.

## Wave C (two agents in parallel, branched after build 2 is green)

| Id | Task plan | Owns |
|---|---|---|
| C1 | All wiring: Task 2 Step 6, Task 4 Step 6, Task 5 Step 9; the hand-edited generated files (Rule 6) | `mcp/tools/orchestration.rs`, `mcp/tools/params.rs` if the args struct needs `task_id` / `index` / `title`, `mcp/tools/tests.rs` (reach table), `mcp/tools/tests_isolation.rs`, `service/work/mod.rs` (command table only), `src-tauri/src/commands/work.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/backend/verdicts.rs`, `src/lib/hub_verdicts.generated.json`, `docs/hub.md` (refusal table), `docs/control-api-reference.md` |
| C2 | Task 8 Step 1 | `docs/work-graph.md`, `docs/status.md`, any `skills/*/SKILL.md` that lists `work_link` actions |

Notes:

- **C1** is one agent on purpose: the three commands share every file. Three actions (`wrap_up`, `result`, `followup`) and three commands (`request_work_result`, `work_result`, `create_followup_task`). It copies the `handover` arm for `wrap_up`, the `task` arm for `result`, the `create` arm for `followup`, with each one's reach row and isolation cases.
- **C1** expects build 3 to fail once on a generated file: that is Rule 6 working. It fixes from the log.
- `skills/*/SKILL.md`: three of them are embedded in fleet-core, so an edit recompiles it on the builder; that is fine.

Merge order: C1, C2. Then **build 3**.

## Wave D (orchestrator)

1. Dispatch one review agent with the diff `origin/main...HEAD` and the spec, not with your conclusions: correctness of the scope fences (an item or task outside the caller's scope answers as unknown), the `wrapup` role not read as an attempt anywhere (`graft grep "work_item_id"` consumers that do not filter by role), and that the brief stays under `BRIEF_MAX_CHARS`. Fix what it confirms.
2. `git fetch origin main`; if main took migration 195, merge `origin/main` and run `scripts/renumber-migrations.sh` (a shell script, it compiles nothing).
3. Push, **build 4** on the final HEAD.
4. Open one PR to `main`. In its body say what was not done: task plan Task 7 Step 8 (seeing the panel in the running app) needs `pnpm tauri dev`, which compiles Rust locally and is excluded by Rule 1. A person runs it, or the owner lifts the rule for that one step. Reticle is wired (`vite.config.ts`, `src/reticle-dev.ts`) and connects by itself as soon as the dev app runs.
5. Do not merge the PR; the owner does.

## Build budget

| Build | Covers | Expected |
|---|---|---|
| 1 | W0 + Wave A | most compile errors land here: five agents wrote Rust blind |
| 2 | Wave B | fewer: it builds on compiled interfaces |
| 3 | Wave C | one planned failure on generated files |
| 4 | final HEAD after review and main merge | green |

Planned 4, ceiling 4 + 2 fix builds per wave = 12. Stop and report at the ceiling.

## Stop conditions

Stop and report to the owner, without working around it, when: the Buildkite variables are missing; a wave fails its third build; an agent reports that a signature in the task plan does not exist in the source and no equivalent does; a task needs a fleet-mobile change; main moved the migration number and `renumber-migrations.sh` fails.
