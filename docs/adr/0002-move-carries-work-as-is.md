# ADR 0002: `move_session` carries the work as it is

- Status: accepted
- Date: 2026-09-19
- Supersedes: the preflight clause of ADR 0001 (decision 2, first bullet:
  "The worktree must be clean (`E_MOVE_DIRTY`) and the branch pushed with
  nothing unpushed (`E_MOVE_UNPUSHED`)")
- Spec: `docs/superpowers/specs/2026-09-19-move-carry-engine-design.md`

## Context

ADR 0001 shipped `move_session` with the rule "the move never makes a git
decision for the user" and implemented it as a refusal: a dirty worktree or
an unpushed branch stopped the move. In practice almost every live session
is dirty or ahead of origin, some branches have no usable remote at all, and
a target host may not be able to reach or authenticate to origin. The move
was correct and rarely usable.

## Decision

The principle stays; the refusal goes.

> The move never pushes, never commits to or moves a user branch on the
> source, never stashes, and never modifies the source working tree or
> index. It carries the work as it is.

- The source worktree is snapshotted through a temporary index into two
  commits parked under `refs/fleet/transfer/<claude_id>/`. A `git bundle` of
  what the target lacks is relayed through the orchestrator (the only path
  that is guaranteed to exist), fetched into the target's main clone, and
  replayed with `git read-tree` so that uncommitted stays uncommitted,
  staged stays staged, untracked stays untracked and unpushed stays
  unpushed. The target's `git status --porcelain` is compared with the
  source's; a mismatch fails the move before the target session starts.
- Origin is never required. A target with no clone gets one by `git clone`,
  or by `git init` plus the bundle when origin is unreachable.
- Small git-ignored files travel in a separate archive under size caps and a
  deny-list; what stays behind is reported.
- What the move writes on the source: unreferenced git objects, the private
  ref namespace and a temp directory. The refs and the directory are removed
  when the move ends, on success and on failure.
- `strict: true` restores the ADR 0001 refusals for callers that want the
  guarantee that origin holds everything.

## Consequences

- A successful move leaves a copy of the uncommitted work in the source
  worktree. It is reported, not cleaned up: deleting user work is a separate,
  explicit action.
- An operation in progress (merge, rebase, cherry-pick, revert, bisect) is
  refused (`E_MOVE_MIDOP`); its state is not carried.
- Submodule contents, LFS objects, stashes, hooks and per-repo config do not
  travel. Untracked nested repositories fail the porcelain check rather than
  being dropped silently.
- Payloads pass through the orchestrator in 8 MiB chunks and a `0600` temp
  file, bounded by `move.max_bundle_mb`.
