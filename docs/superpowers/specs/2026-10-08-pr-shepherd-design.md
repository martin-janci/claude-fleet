# PR shepherd: design

Status: step 1 built, off until a person grants a rule (2026-10-08).
Asked for by the owner in the Claude Fleet project ("a Jev-like optimizer
… Dev can babysit … conflict solving and resolving issues faster").

## The problem

Fleet's sessions open pull requests, and the PRs then wait on people for
things an agent can do. On 2026-10-08 a dozen parallel redesign PRs kept
re-conflicting: every merge rewrote the same generated lines (hub verdict
count, MCP tool count, size budget, generated docs), so each remaining PR
went `DIRTY` again. Someone had to notice each one, open the session and
type "merge main, regenerate, push". Red CI waited the same way.

Fleet already knows all of it. The reconcile pass's PR probe
(`service/outcome.rs`, `sessions.pr_evidence`) reads, for every live
session in a GitHub worktree, the PR's `mergeStateStatus`, the check
rollup on the head commit, the review decision and the state. Nothing
acts on that reading except the `ci_failing` attention reason.

## What it does

The shepherd turns each PR reading into at most one next step:

| condition  | read from                      | what the session's Claude is asked |
|------------|--------------------------------|------------------------------------|
| `conflict` | `merge_state = DIRTY`          | merge the base branch in (no rebase, no force-push), resolve, regenerate generated files with the repo's tooling, run the fast checks, push; stop and say so when both sides changed the same logic |
| `behind`   | `merge_state = BEHIND`         | merge the base branch in, run the fast checks, push |
| `ci_red`   | `checks.failing_total > 0`     | read the failing logs, root-cause, fix, run the check locally, push; never skip a test or push an empty commit; say so and stop when the base branch is red too |

The order is the table's: a conflicting PR's CI is moot. A merged or
closed PR has no condition.

The prompt goes to **the session that opened the PR**, which already has
the context, the worktree and the branch. It is sent as fleet's own prompt
(`send_system_prompt`, `Origin::Fleet`), like a safe-kill request.

### Episodes

An episode is one condition on one pushed commit:
`(session, head_oid, condition)`, a row in `pr_shepherd_episodes`
(migration 134). The shepherd acts on an episode once. A push is a new
head and so a new episode, which is what makes "fix, push, still red"
get one more nudge and "nothing changed" get none.

### When it waits

A nudge is not sent, and the episode stays open for a later tick, while:

- the session is working, blocked on a question, stuck, or went idle less
  than `IDLE_GRACE_SECS` (60 s) ago: a turn that just ended may be about
  to push;
- the worktree's `HEAD` differs from the PR's head, or it has commits not
  on its upstream: a fix is under way and the reading is about to be stale;
- a person is attached to the pane (`press_enter`'s check): typing into
  what they are typing is never right.

### When it gives up

These are recorded once and never retried for that episode:
`skipped:no_session` (the session is gone), `skipped:controller` (the
registered controller session), `skipped:budget` (the session had
`MAX_NUDGES_PER_DAY` = 3 nudges in 24 h; a failed send counts). A
`failed:<error>` send is recorded too, and counts against the budget.

Every recorded episode writes a `pr_shepherd` event (`<condition>:<outcome>`)
on the session's timeline (Ops chip).

## Who decides: rules, not modes

"AI proposes, a person confirms; no auto mode." The shepherd's actions are
fleet's own deterministic steps, not a model's answer, but they still act
on a person's work, so nothing happens without a person's **standing
rule** per project (`pr_shepherd_rules`):

| level   | does                                                       |
|---------|------------------------------------------------------------|
| `watch` | records episodes, sends nothing                            |
| `nudge` | also sends the fix prompt                                  |
| `merge` | also merges green PRs through the merge queue (step 3; until then, `nudge`) |

No row, no shepherd: a project without a rule is never even read. A rule
can expire (`expires_at`); an expired rule is absent. Only a person writes
a rule: in step 1 through `fleet-hub shepherd grant` straight into the
database, which no control API action can reach, so an agent with the
master token cannot grant itself one. `fleet-hub shepherd pause-all`
removes every rule, and the shepherd is a pausable loop (`pr_shepherd` in
`service::loops`): `automation.paused` stops it with every other automatic
write, and `fleet_health.loops` shows its last run.

## Steps

1. **Backend core** (this change): migration 134, `store/pr_shepherd.rs`,
   `service/pr_shepherd/` (pure planner, prompts, injected executor), the
   reconcile tick, `fleet-hub shepherd grant | revoke | pause-all | status`,
   the `pr_shepherd` timeline kind, the `pr_shepherd` loop (8.1's registry).
2. **Control API and the Inbox.** `prs { shepherd_rules | shepherd_grant |
   shepherd_revoke }` (a person's act, refused for host and peer tokens;
   isolation matrix rows) and a Needs-you item per open `watch` episode
   with a *Send fix* button, and per `skipped:budget`. Builds on 6.4's
   `pull_requests` table (#524) instead of reading `sessions.pr_evidence`
   directly. Contract bump through Lane A.
3. **Merge queue** (`merge` level). Per repository, at most one merge in
   flight: the oldest PR whose head has every check `success` (none
   pending, none skipped that is required), `mergeStateStatus = CLEAN`,
   review not `CHANGES_REQUESTED`, not draft. Merged with
   `gh pr merge --merge --match-head-commit <sha>` on the session's host,
   never GitHub auto-merge (with no required checks on `main` it merges
   without waiting). The next PR is taken only after the others have
   re-read their merge state against the new base: serialising merges is
   what stops one merge from making every other PR conflict and then
   another merge landing before they recover.
4. **Jev `pr_triage`, shadow.** A closed-set question per red check:
   `fix_in_pr | regenerate | merge_base | flaky_rerun | not_this_pr |
   needs_person`, recorded through the decision envelope
   (`service::decide`) and compared with what the fix turned out to be.
   In `assist` it would only choose which prompt to send; it never sends,
   merges or skips anything itself.
5. **UI.** The Automation view (behind `ui.layout`) lists the shepherd as
   a built-in routine with its rules and episodes; the session's PR chip
   shows the open episode; the phone gets the same Needs-you items.

## Not in scope

- PRs no fleet session opened. The shepherd needs a session to ask.
  Starting a fresh session in the PR's worktree for an orphaned PR is a
  possible later step, as a mission task (`orchestrate/integrate.rs`
  already proposes resolve tasks inside missions).
- Rebasing, force-pushing, re-running CI, disabling tests. The prompts
  forbid them, and the mission guard (`orchestrate/guard.rs`) already
  refuses a worker's `gh pr merge` and pushes to `main`.
- Fixing the generated-lines root cause itself (a separate change makes
  those lines merge cleanly).
