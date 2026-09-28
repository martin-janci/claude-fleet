# Result evidence: tying "done" to a commit

Date: 2026-09-29. Baseline: `main` at `3c8861d8`.

## The problem

Fleet can tell that an agent *says* it finished and, roughly, what CI thinks.
It cannot tell whether the two describe the same code, or how old either
reading is. Three facts from the code:

1. **A delegated task is `done` on the agent's word.** `tasks::complete_task`
   flips the row to `done` when `FLEET_TASK_DONE_<nonce>` appears in the
   worker's transcript or pane (`service/tasks.rs:80`, `:324`). The `tasks`
   table (migration 020) stores the result paragraph and no commit. A task
   that finished with uncommitted changes, or whose commit never reached the
   PR, looks exactly like one that shipped.
2. **The CI badge has no commit and no clock.** `outcome.rs` asks `gh pr
   view` for `url,statusCheckRollup,headRefName,title,body,
   closingIssuesReferences,state` and reduces the rollup to `passing |
   failing | pending` (`reduce_ci_status`). `headRefOid` is not read, so
   the badge cannot say which commit it describes. The probe's timestamp
   lives only in the in-memory `PrProbeCache` (`Instant`, lost on restart).
   `sessions.ci_status` is a bare string, and `pr_signals_at` is stamped
   only when the signals *change*, so neither says when fleet last looked.
3. **"Unknown" is not visible.** A failed probe (offline, rate limit)
   correctly keeps the stored value (`classify_probe_record` → `None`), but
   the UI then shows yesterday's `passing` as if it were current. A PR with
   no checks reads `ci_status = NULL`, which the UI renders exactly like "no
   PR".

Nothing reads `reviewDecision`, `mergeStateStatus` or `isDraft` either
(no occurrence anywhere in the repository).

### Which "task"

Fleet has two things the UI or the API calls a task:

| | `tasks` (dispatch) | `work_items` (Work view "tasks") |
|---|---|---|
| Made by | `dispatch_task`, agent → agent | tracker sync, local work, detection |
| "Done" means | the worker printed the marker | ticket done in the tracker, or PR merged (tidy `done_idle`, `pr_merged_idle`) |
| Has a PR / CI | no | yes, through linked sessions' `pr_url` / `ci_status` |

The evidence card belongs on the **work item** and on the **session**, where
PR and CI already live. The dispatch task gets something smaller: the
worker's commit when it reported done (§4).

## Principles

- **`done` keeps its meaning.** Evidence is a separate reading beside it,
  never a gate on it and never a rewrite of it.
- **Every piece of evidence carries its source, the commit it describes,
  and when it was observed.** A reading without a commit or a time is shown
  as such, not as a pass.
- **Unknown is a first-class answer.** "Not checked since 14:32" and "no
  checks configured" are both distinct from "passing".
- **The background probe stays cheap; the decisive read is on demand.**
  The 20 s reconcile keeps its one-round-trip-per-host budget. Opening the
  card, or an agent about to merge, asks for a fresh read that ignores the
  TTL.
- **Fleet explains, it does not decide.** No auto-merge, no blocking. The
  output is a list of reasons a person can read in one glance.

## 1. What the background probe adds

Same script, same round trip (`build_pr_probe_script`). Two additions:

**More PR fields.** `PR_FIELDS` gains `headRefOid,reviewDecision,
mergeStateStatus,isDraft`. The basic fallback (`PR_FIELDS_BASIC`, for an old
`gh`) is unchanged; evidence from a basic answer is simply absent.

**The worktree's own state.** Per target, after the `gh` call, three local
git reads that cost no network:

```sh
head="$(git rev-parse HEAD 2>/dev/null)"
ahead="$(git rev-list --count '@{u}..HEAD' 2>/dev/null)"
git diff --quiet HEAD -- 2>/dev/null
case $? in 0) dirty=0;; 1) dirty=1;; *) dirty='';; esac
```

printed as `__FLEET_GIT__\t<name>\t<head>\t<ahead>\t<dirty 0|1>`, next to the
existing `__FLEET_TRAILERS__` line. `git diff --quiet HEAD` looks at tracked
files only, so a stray build artefact does not read as "uncommitted work". It
exits 1 for a change and above 1 when it cannot tell, and that case is empty,
not "clean". A missing upstream prints no count, which is stored as "unknown"
and never as 0.

**Failing checks by name.** `reduce_ci_status` keeps its return value (the
badge, Attention's `CiFailing`, the work roll-up and Today all read it). A
sibling `summarize_checks` returns `{ total, failing: [{name, url}] (≤ 5),
pending, skipped }` from the same rollup. Check runs carry `name` and
`detailsUrl`; commit statuses carry `context` and `targetUrl`.

The rollup GitHub returns is for the PR's head commit, so the check summary
is stamped with `headRefOid`.

## 2. Storage

Migration 082 (phase 1; the task columns of §4 come with phase 3):

```sql
ALTER TABLE sessions ADD COLUMN pr_evidence TEXT;      -- JSON, see below
ALTER TABLE sessions ADD COLUMN pr_checked_at INTEGER; -- last observation, see below
```

`pr_evidence` is written by the reconcile upsert under `ci_status`'s rule. It
is authoritative when the probe ran this pass: a `None` clears it, so an old
`gh` that answers only the basic fields leaves no reading that looks current.
It is kept when the probe did not run. It is kept apart from `pr_signals` on
purpose. A `pr_signals` change re-runs work resolution
(`detect::resolve_session`). Check results and the local HEAD change far more
often than branch names and ticket keys, and must not trigger link resolution.
Riding the upsert also means an evidence change is part of the one
`session:updated` event the pass already emits, not a second one.

```json
{
  "head_oid": "1490bc3…",
  "local_head": "1490bc3…",
  "ahead": 0,
  "dirty": false,
  "draft": false,
  "review_decision": "APPROVED",
  "merge_state": "BLOCKED",
  "checks": { "total": 9, "pending": 0, "skipped": 1,
              "failing": [{ "name": "rust (ubuntu-24.04)", "url": "https://…" }] }
}
```

Both columns are `SessionRow` fields and both are watched by the `row_version`
trigger (082 rebuilds 065's). An unwatched `pr_checked_at` was the first draft,
and it is wrong. The client would only ever hold the stamp from the last
*visible* change, so a PR that sits green would dim as "stale" a quarter of an
hour after its last change, however often it was probed.

Stamping every observation would cost a row event per PR session per probe
(every five minutes, forever, for an idle green PR). The stamp is therefore
exact when the reading changes, and otherwise refreshed only once it is
`PR_CHECKED_REFRESH_SECS` (2 × TTL = 600 s) old. While probes succeed, the
stored stamp is at most about 640 s old (refresh + one TTL + tick jitter), below
`PR_EVIDENCE_STALE_SECS` (3 × TTL = 900 s), the age at which a reading counts
as the past. A steady PR thus costs one event per ten minutes, and dimming
never fires falsely. A session without a PR has no stamp at all, so the many
worktree sessions with no PR emit nothing.

## 3. The on-demand read

`service::evidence::refresh(session_id)` runs for one session and ignores
`PrProbeCache`:

1. the probe script for that one target (§1), and
2. only when the PR has reviews: `gh api 'repos/{owner}/{repo}/pulls/<n>/reviews'`.
   The number comes from `gh pr view --json number`, and `gh api` expands
   `{owner}/{repo}` from the worktree's remote.

The REST call exists because `gh pr view --json latestReviews` leaves
`commit.oid` **empty**. Measured on `martin-janci/property-management#2036`:
`latestReviews[].commit.oid == ""` for both reviews, while
`GET /pulls/2036/reviews` returns `commit_id` for each: the Copilot review at
`f10e866`, the approval at head `1490bc3`. Without it, "review is from an
older commit" cannot be answered.

Reviews are reduced to the latest one per reviewer: `{ login, state,
commit_id, stale: commit_id != head_oid }`. They are returned, not stored. They
are only true at the moment of reading, and the card is the only thing that
shows them.

The result is written back like a background probe (so the row badge catches
up), stamps `pr_checked_at`, and returns `Evidence` (below) to the caller.

It is exposed as a Tauri command for the card. MCP exposure is phase 3 (§7):
it is the read an agent should make before merging, but a new `#[tool]`
changes the tool-definition prefix for every client, and that change should
be batched.

## 4. Dispatch tasks: the commit behind "done"

`tasks::handle_stop_for_worker` already runs off the hook handler with an SSH
client and the worker's `cwd`. Before it takes the store lock to complete
tasks, one command reads the same `head` and `dirty` pair as §1:

```sh
cd <cwd> && git rev-parse HEAD && git status --porcelain --untracked-files=no | head -n 1
```

`complete_task` stores `result_commit` and `result_dirty`. `list_tasks` and
`wait_for_task` return them, and the `task_result` inbox message gets one line
`commit: 1490bc3 (clean)` / `commit: 1490bc3 (uncommitted changes)`.

Best-effort throughout: a worker outside a git repo, or a failed read,
completes the task with `result_commit = NULL`, exactly as today. The task's
state never depends on it.

## 5. From evidence to reasons

One pure function, `evidence::assess(&Evidence, now) -> Assessment`:

```rust
pub struct Assessment {
    pub verdict: Verdict,          // Ready | Waiting | Blocked | Unknown
    pub reasons: Vec<Reason>,      // ordered, most decisive first
    pub commit: Option<String>,    // the commit the verdict is about
    pub checked_at: Option<i64>,
}
```

The rules, in order. Each adds a reason; the verdict is the worst one seen.

| Condition | Reason text (UI) | Verdict |
|---|---|---|
| never observed, or `pr_checked_at` older than 3 × TTL | "Not checked since 14:32" | Unknown |
| basic-fields answer (old `gh`) | "Host's gh is too old to report evidence" | Unknown |
| `dirty` | "Uncommitted changes in the worktree" | Blocked |
| `ahead > 0` | "2 commits not pushed; CI and review describe 1490bc3" | Blocked |
| `local_head ≠ head_oid`, `ahead == 0` | "Worktree is on a different commit than the PR" | Unknown |
| `checks.failing` non-empty | "Failing: rust (ubuntu-24.04), hub-headless" | Blocked |
| `checks.pending > 0` | "3 checks still running" | Waiting |
| `checks.total == 0` | "No checks configured" (not "passing") | Waiting |
| `draft` | "Draft PR" | Waiting |
| `review_decision == CHANGES_REQUESTED` | "Changes requested" | Blocked |
| `review_decision == REVIEW_REQUIRED` | "Review required" | Waiting |
| (on-demand only) a latest review with `stale` | "Review by X is from f10e866, head is 1490bc3" | Waiting |
| `merge_state == DIRTY` | "Merge conflicts" | Blocked |
| `merge_state == BEHIND` | "Branch is behind base" | Waiting |
| `merge_state == UNKNOWN` | "GitHub is still computing mergeability" | Unknown |
| none of the above | "Checks passed for 1490bc3" | Ready |

`Ready` is only reachable from a reading within the TTL: an old reading is
reported as Unknown whatever it contained.

`reviewDecision` is `""` on repositories without required reviews (measured on
claude-fleet #381). That reads as "no requirement", not as a reason.

## 6. Where it shows

- **Session details** (`SessionDetails.svelte`, today the PR link plus the CI
  chip): an **Evidence** card. Verdict and commit on the first line, reasons
  below, "checked 3 min ago" and a refresh button that calls §3. Opening the
  card triggers one refresh if the stored reading is older than the TTL.
- **Work item detail**: one line per linked session with a PR, worst verdict
  first. The group-header roll-up (`work_keys.ts`, worst `ci_status`) is
  unchanged in phase 1.
- **Session row badge**: unchanged. It gets a dimmed style when the row's
  `pr_checked_at` is older than 3 × TTL, so an old `passing` stops looking
  current.
- **Dispatch tasks**: the commit line in `list_tasks` / the inbox message (§4).

Attention is unchanged. `CiFailing` still reads `ci_status`, and "ready to
merge" is not something that needs a person urgently.

## 7. Phases

1. **Probe and store** (§1, §2): fields, git line, check summary, migration,
   write path. Nothing new in the UI yet, but the data starts accumulating.
2. **Assess and show** (§3, §5, §6): the pure assessment, the on-demand read,
   the card, the dimmed badge.
3. **Tasks and agents** (§4, MCP): `result_commit`, then the evidence on the
   full `list_sessions` row plus a fresh-read MCP tool, batched with other
   tool-surface changes.

## Not in scope

- **Acceptance criteria.** The report that motivated this asks for "one
  criterion still unconfirmed". Fleet has no source for criteria today. The
  natural one is the tracker ticket (checkbox lists in Jira and GitHub issue
  bodies), read by the tracker sync. That is a separate design.
- **Deciding or merging.** No auto-merge, no "done" gate, no blocking of
  `dispatch_task` chains on evidence (that is the dependency work, which
  should come after this).
- **Non-GitHub forges.** The probe is `gh`-only today, and so is this.

## Cost

- Background: no extra round trip. Per probed session, three local git reads
  and four more JSON fields in a call already made.
- On demand: one to two `gh` calls per refresh, only when a person opens the
  card or an agent asks.
- Store: two columns per session (phase 1), two per task (phase 3).
- Events: one `session:updated` per PR session per ten minutes while
  nothing changes (the `pr_checked_at` refresh), plus one per real change.

## Open questions

1. ~~Should `pr_checked_at` bump `row_version`?~~ Decided in phase 1: yes,
   with the sparse refresh of §2.
2. Should a `stale` review count as Waiting, or only as a note? Recommended:
   Waiting. A stale approval is the case the card exists to catch.
3. Should `ahead > 0` with CI green on the older commit be Blocked or Waiting?
   Recommended: Blocked. The work that would be merged is not the work that
   was checked.
