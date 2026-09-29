# Result evidence: implementation plan

Design: `docs/specs/2026-09-29-result-evidence-design.md`. Baseline: `main` at
`3c8861d8`. Every step is one commit and leaves `scripts/ci-local.sh` green.
Paths are relative to `crates/fleet-core/src/` unless they start with `src/`,
`docs/` or `crates/`.

## Phase 1: probe and store

### Step 1. Parse the new PR fields (pure)

- `service/outcome.rs`: `PR_FIELDS` += `headRefOid,reviewDecision,
  mergeStateStatus,isDraft`. New `pub struct PrEvidence` (serde, the JSON in
  design §2) and `PrInfo.evidence: Option<PrEvidence>`, `None` on a basic
  answer (same rule as `signals`).
- `summarize_checks(&[Value]) -> CheckSummary`: `total`, `pending`,
  `skipped` (`SKIPPED`, `NEUTRAL`), `failing` ≤ 5 with `name`/`context` and
  `detailsUrl`/`targetUrl`. It uses the same match arms as `reduce_ci_status`,
  so the two cannot disagree. A test asserts that `failing` is non-empty
  exactly when `reduce_ci_status` returns `failing`, over every fixture.
- Tests: fixtures from real answers (claude-fleet #381: nine successful check
  runs, `reviewDecision: ""`, merged `UNKNOWN`), plus hand-made failing,
  pending, status-context and empty rollups.

### Step 2. The worktree's git line (pure + script)

- `build_pr_probe_script`: after the trailers line, print
  `__FLEET_GIT__\t<name>\t<head>\t<ahead>\t<dirty>` using design §1's three
  reads. A missing upstream makes `ahead` empty and the parser stores `None`
  (not `0`).
- `parse_pr_probe_output`: a `GIT_PREFIX` arm. Like the trailers, git state
  is attached only to a target that has a PR observation. Without a PR there
  is nothing to assess.
- Tests: the existing script-shape test is extended. A parser test covers
  clean, dirty, ahead 2, no upstream and a garbled line.
- Check that the script still runs under `sh` on the hub's container image
  (`scripts/hub-e2e.sh`), not only zsh or bash.

### Step 3. Migration and write path

- `crates/fleet-core/migrations/082_result_evidence.sql`: `sessions.pr_evidence`
  and `sessions.pr_checked_at`, plus `sessions_row_version_bump` rebuilt from
  065 with both columns watched. One guard (`sessions_has_pr_evidence`).
- The write rides the reconcile upsert (`store/reconcile.rs`), not a second
  UPDATE: `pr_evidence` under `ci_status`'s rule (authoritative when
  `pr_observed`), `pr_checked_at` stamped for a new or changed reading and
  otherwise once it is `PR_CHECKED_REFRESH_SECS` old. No PR means no stamp.
  One event per pass, as today.
- `SessionRow.pr_evidence` / `pr_checked_at` (`SESSION_COLUMNS` + mapper;
  malformed JSON reads as none), `ReconcileSession.pr_evidence` from
  `PrInfo.evidence`, TS mirror in `src/lib/sessions.ts`. The phone view
  (`PHONE_SESSION_FIELDS`) does not grow. Full MCP rows carry the fields
  for PR sessions only (`skip_serializing_if`).
- Tests: the store test covers first sight, not probed, steady inside the
  window (no event), steady after the window (one event), changed, old
  `gh`, no PR and malformed JSON. The fails-late rollback test now also
  covers evidence rolling back. The existing trigger-coverage test passes
  with both columns watched.

## Phase 2: assess and show

### Step 4. The assessment, on both sides (pure)

- `service/evidence.rs`: `assess(pr_url, &PrEvidence, checked_at, now) ->
  Option<Assessment>` with `Verdict` and `Reason` codes (design §5 and its
  phase-2 refinements), plus `is_stale`.
- `src/lib/evidence.ts`: the mirror, and `describeReason` for the wording.
- `service/testdata/evidence_cases.json`: 29 hand-written cases, run by both
  suites. A Rust test fails when a reason code has no case.
- `PrEvidence.state` (OPEN | CLOSED | MERGED), read by the probe.

### Step 5. UI

- `PrResult.svelte` as the **Result** row in `SessionDetails.svelte`.
- `SessionRowItem.svelte`: the CI badge dims when `isStale`, and its tooltip
  says when it was last checked.
- `WorkTaskDetail.svelte`: a verdict chip beside each live linked session's PR.
- Vitest: the card per verdict, the stale badge in the sidebar, and the Work
  chip (live session only).

(The on-demand read planned here moved to phase 3, with the MCP tool that
also gives it a hub path.)

## Phase 3: fresh reads, tasks and agents

### Step 6. The on-demand read (moved from phase 2)

- `evidence::refresh(store, shell, session_id)`: the one-target probe script
  that ignores the TTL, then `gh api 'repos/{owner}/{repo}/pulls/<n>/reviews'`
  for the review commits (`gh pr view` leaves `latestReviews[].commit.oid`
  empty). Keep the latest review per login and mark it stale when its commit
  is not the head. Write back through the reconcile upsert's rule, then
  assess, with a new `review_stale` reason.
- Exposed as the fresh-read MCP tool of Step 8. The desktop's refresh button
  routes to it on a hub and calls the service locally.

### Step 7. `result_commit` on dispatch tasks

- Its own migration: `tasks.result_commit`, `tasks.result_dirty`.
- `service/tasks.rs::handle_stop_for_worker`: before the store lock, run one
  command in `cwd` (design §4), bounded by the existing SSH timeout.
  `complete_task` takes `Option<(String, bool)>` and passes it to
  `finish_task`.
- `TaskRow` + the `list_tasks` / `wait_for_task` output + the `task_result`
  inbox body line.
- Tests: completion with a commit, dirty, and a failed read, where the task
  is still `done` and the commit is `NULL`.

### Step 8. MCP surface (batch with other tool edits)

- Full `list_sessions` rows carry `pr_evidence` and `pr_checked_at`. The
  `session_ops.rs` description mentions them.
- A fresh-read tool, or a `fresh: true` flag on an existing read tool;
  decide when batching. Then `REGEN_DOCS=1 cargo test -p fleet-core
  reference_is_current`.
- `skills/claude-fleet-control/SKILL.md`: before merging a session's PR,
  make a fresh evidence read and act on its reasons.

### Step 9. Docs

- `docs/work-graph.md`: the Evidence line in the work item detail.
- `docs/concepts.md`: the "done vs verified" distinction and the two kinds
  of task.
- `CHANGELOG.md` through the release script.

## Order and size

Steps 1 to 3 are one PR (S–M). Nothing visible changes, and data starts
accumulating. Steps 4 to 6 are the second PR (M), the first visible result.
Steps 7 to 9 are the third, and Step 8 waits for a tool-surface batch.
