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

- `crates/fleet-core/migrations/0NN_result_evidence.sql` (next free number):
  the four columns of design §2. Add the migrate arm and bump the
  schema-version assertions.
- The `row_version` trigger (latest definition: migration 065; drop and re-create it): add
  `OR NEW.pr_evidence IS NOT OLD.pr_evidence`, and **not** `pr_checked_at`.
- `store/work_detect.rs` (or a new `store/evidence.rs`):
  `set_pr_evidence(host, tmux, Option<&str>)`, which writes `pr_evidence`
  when it changed and always stamps `pr_checked_at`.
- `service/sessions/reconcile.rs`: in the loop that writes `pr_signals`
  (about line 979), write evidence for every observed target. A definite "no
  PR" clears `pr_evidence` and still stamps `pr_checked_at`.
- `SessionRow` + `src/lib/sessions.ts`: `pr_evidence: PrEvidence | null`,
  `pr_checked_at: number | null`. Check `mcp/tools/views.rs`: the compact
  row must not grow (token budget). Full rows get the fields in phase 3.
- Tests: a store test for change vs stamp-only, and a trigger test that a
  stamp-only write leaves `row_version` alone while an evidence change bumps
  it. A reconcile test with the fake `HostShell` checks that a failed probe
  leaves both columns untouched.

## Phase 2: assess and show

### Step 4. `service/evidence.rs`: the assessment (pure)

- `Evidence` (stored `PrEvidence` + `checked_at` + optional `reviews`),
  `assess(&Evidence, now, ttl) -> Assessment` as in design §5.
- Table test: one row per line of the §5 table, plus combinations
  (failing + dirty keeps both reasons, dirty first; old + passing reads
  Unknown).

### Step 5. The on-demand read

- `evidence::refresh(store, shell, session_id) -> Assessment`: take the row
  and its cwd under the lock, drop it, run the one-target probe script, then
  run `gh api 'repos/{owner}/{repo}/pulls/<n>/reviews' --jq '[.[] | {login:
  .user.login, state, commit_id, submitted_at}]'` when `review_decision` is
  set or the PR has reviews. Keep the latest review per login. Write back
  through `set_pr_evidence`, then assess.
- Every interpolated value goes through `shell::quote`. `<n>` is parsed as
  an integer before it is interpolated.
- Tauri command `session_evidence_refresh` in `src-tauri/src/commands/`
  (thin, validates the id). Hub path: add a verdict row if the hub proxies
  it, then `REGEN_HUB_VERDICTS=1`.
- Tests: fake `HostShell` answers for no PR, PR without reviews, a stale
  review, and gh failing (returns Unknown with the stored reading, no write).

### Step 6. UI

- `src/lib/evidence.ts`: TS mirror of `Assessment`, plus a client-side
  `isStale(pr_checked_at, now)` for the badge.
- `src/lib/EvidenceCard.svelte` in `SessionDetails.svelte` (replaces the CI
  chip at about line 551): verdict + short commit, reasons, "checked N min
  ago", a refresh button. Refresh once on open when the reading is older
  than the TTL.
- `SessionRowItem.svelte`: dim the CI badge when `isStale`.
- Work item detail: one line per linked session with a PR, worst verdict
  first. The `work_keys.ts` roll-up stays untouched.
- Vitest: card rendering per verdict, the refresh-on-open rule, and the
  stale dimming.

## Phase 3: tasks and agents

### Step 7. `result_commit` on dispatch tasks

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
