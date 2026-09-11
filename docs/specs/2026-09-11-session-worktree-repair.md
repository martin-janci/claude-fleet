# Self-repairing session workspaces (worktree directory + tmux cwd)

**Date:** 2026-09-11
**Status:** Implemented (`src-tauri/src/service/repair.rs`)

## Problem

Session creation and restoration must ALWAYS end with the worktree directory
existing, registered with git on the right branch, and the tmux session
running in it. Before this change:

- `new_session` with an existing worktree row trusted `worktrees.path` (local)
  or "the dir exists" (remote `ensure_remote_project`) — a deleted, pruned or
  moved directory produced a tmux session in a non-existent cwd (tmux falls
  back to `$HOME`) or a `git worktree add` failure.
- `recreate_session` resolved the cwd from the rows and rebuilt tmux there,
  never checking the directory.
- `restart_session` respawned the pane without any cwd (`respawn-pane -k`), so
  a pane whose directory was deleted kept its dead inode as cwd.
- The terminal attach path never looked at the workspace at all.
- Nothing recreated a directory for a row whose directory had disappeared.

## Design

Probe → plan → apply → verify → record, in one new module
`service::repair`:

1. **Probe** — one `bash -lc` script per host (every value goes through
   `shell::quote`), read-only, always exits 0. It prints `key=value` lines
   (`root_exists`, `root_git`, `root_gitdir_ok`, `layout_dot_worktrees`,
   `wt_exists`, `wt_git`, `wt_empty`, `wt_gitdir_ok`, `index_lock`,
   `branch_local`, `branch_remote`, `default_branch`, `tmux_alive`,
   `tmux_cwd`, `tmux_cwd_exists`) followed by the raw
   `git worktree list --porcelain` dump. `tmux has-session -t "=<name>"` uses
   the exact-match form so a prefix cannot match the wrong session.
2. **Plan** — `repair::plan(spec, probe, policy)` is pure and returns the
   ordered `Step`s (`Prune`, `RepairLinks`, `AddWorktree{from}`, `AdoptPath`,
   `TmuxCreate`, `TmuxRespawn`) or refuses with an `E_*` code when the fix
   would need a guess. Every inventory case below is a unit test of this
   function with a hand-built `Probe`.
3. **Apply** — the git steps are rendered into one `set -e` script
   (`render_git_script`); the branch source for a re-added worktree is
   resolved as local branch → `origin/<branch>` → `git fetch origin <branch>`
   then `origin/<branch>` → new branch from the base branch (the spec's
   `base_branch`, else the repo default, else `HEAD`). The script prints an
   `outcome=` line so the report records which path was taken.
4. **Verify** — the probe runs again; the result must be a registered,
   non-prunable worktree at the planned cwd whose `git rev-parse --git-dir`
   succeeds. Otherwise `E_REPAIR_FAILED`. Only then is tmux touched.
5. **Record** — the local `worktrees` row gets the verified path/branch
   (`upsert_worktree` → `worktree:updated`), the session is linked to the row
   (`worktree_id`, re-stamped via `set_worktree_key` → `session:updated`), a
   ghost row whose tmux was recreated is restored (`restore_session` →
   `session:updated`), and one `session_events` row of kind
   `workspace_repaired` (detail: JSON with `cwd`, `actions`, `branch_source`,
   `tmux`, `warnings`) or `workspace_repair_failed` (`E_CODE: message`) lands
   on the timeline.

A healthy workspace costs exactly one probe and writes nothing, so the check
runs unconditionally from every lifecycle entry point.

### tmux policy

- `TmuxPolicy::Leave` — fix the directory only; the caller creates/respawns
  tmux itself. Used by `new_session`, `recreate_session`, `restart_session`.
  The report carries `tmux_alive` and `tmux_cwd_stale` so the caller picks
  `new-session -c` vs `respawn-pane -k -c`.
- `TmuxPolicy::Ensure` — also create a dead tmux session
  (`tmux new-session -c <cwd>`) or respawn a pane whose cwd is gone / was just
  recreated (`tmux respawn-pane -k -c <cwd>`, new `TmuxExec::respawn_pane_in`,
  default impl ignores `cwd` so test doubles need no change). Used by
  `repair_session` (Tauri command, MCP tool, pre-attach check).

A pane keeps a deleted inode as cwd even after the path is recreated, so a
recreated directory ALWAYS respawns the pane; a pane that merely `cd`ed into
a subdirectory is left alone (`tmux_cwd_exists` is true).

### Entry points

| Entry point | Call | tmux |
|---|---|---|
| `new_session` (existing worktree row or main checkout; not a brand-new worktree) | `repair::ensure_for_new_session` before `tmux new-session`; uses the verified cwd; records `workspace_repaired` on the row once reconcile creates it | Leave |
| `recreate_session` | `repair::ensure_session_workspace` replaces the cwd resolution (orphans fall back to the old resolver) | Leave, then kill + `new-session -c` |
| `restart_session` | `repair::ensure_session_workspace`; then `respawn_pane_in(cwd)` (alive) or `new_session(cwd)` (dead) | Leave |
| terminal attach (`TerminalView.openTerm`) | `repair_session` before `pty_open` for project-backed non-bg rows; toast when something was fixed | Ensure |
| `repair_session` Tauri command / MCP tool / "Repair workspace" button | `repair::repair_session` (refuses `E_HOST_OFFLINE` before probing) | Ensure |
| background tick | `repair::ensure_session_workspace(session_id, …)` is the function Track D can call; NOT wired into the tick here | — |

## Edge-case inventory

"Before" is the behaviour on `main` before this change; "now" is the
behaviour of `repair::plan` + `ensure_workspace`. Every row has a unit test
(`repair::tests::case_*`) unless noted.

| # | Case | Before | Now | How |
|---|---|---|---|---|
| a | row + tmux alive, worktree dir deleted | broken: new panes fail, recreate rebuilt at the stale path (tmux fell back to `$HOME`), restart kept the dead inode | fixed | `Prune` → `AddWorktree` (local branch) → verify → `TmuxRespawn -c` |
| b | row exists, tmux gone, dir deleted | ghost → Recreate rebuilt tmux in a non-existent cwd | fixed | `Prune` → `AddWorktree` → `TmuxCreate -c`; `restore_session` clears the ghost |
| c | dir exists but git no longer lists it / `.git` file points to a pruned gitdir | undetected; git ops inside the pane failed | fixed when repairable | `git worktree repair <path>`; verify decides. A `.git` file whose admin dir was pruned cannot be re-linked by git — reported `E_REPAIR_FAILED` with "move it aside"; the directory (possibly holding uncommitted work) is never deleted |
| d | registered but `prunable` (dir missing) | `git worktree add` failed ("already registered") | fixed | `Prune` first, then `AddWorktree` |
| e | branch deleted locally, exists on remote | `worktree add <path> <branch>` failed | fixed | `AddWorktree --track -b <branch> origin/<branch>`; when the remote-tracking ref is also missing, `git fetch origin <branch>` runs first |
| f | branch deleted everywhere | failed | fixed, recorded | new branch from the base branch (`spec.base_branch` → repo default → `HEAD`); `outcome=branch_from_base:<start>` is stored in the report and the `workspace_repaired` event |
| g | branch checked out in another worktree | git refused `worktree add` | decided | a *linked* worktree elsewhere IS the workspace (moved dir / other layout): `AdoptPath` — the row's path is corrected and the pane respawns there. The *main* checkout is the user's: refused with `E_BRANCH_CHECKED_OUT`. Neither `--detach` (loses the branch) nor `-B` (moves the branch under the user) is ever used |
| h | project base path moved or missing | `E_NOREPO`/git error at tmux start; local `refresh_projects` would drop the project | reported | `E_REPO_MISSING`; never `mkdir`. A new remote session still auto-clones via `ensure_remote_project` |
| i | worktree_key collision with a different project | n/a (keys are scoped by `project_id`; reconcile derives `project_id` from owner/repo) | guarded | `spec_for_session` looks up the key only inside the session's project; a `worktree_id` pointing at another project's row is `E_INVALID_STATE`, nothing is touched |
| j | tmux alive, pane cwd no longer exists | new panes/windows failed; restart kept the dead inode | fixed | `TmuxRespawn -k -c <verified cwd>` (`respawn_pane_in`); restart now always passes `-c` |
| k | host unreachable during repair | n/a | clear failure, no writes | `repair_session` checks `hosts.reachable` first (`E_HOST_OFFLINE`); an `E_SSH`/`E_SSH_TIMEOUT` from the probe maps to `E_HOST_OFFLINE` "nothing was changed" before any row write or event |
| l | partial `git worktree add` (dir exists, empty or `.git` missing) | `worktree add` failed ("already exists") | fixed / refused | empty dir: git accepts it, `Prune` → `AddWorktree`; non-empty dir without `.git`: `E_REPAIR_FAILED` "move it aside" — user files are never deleted |
| m | main-checkout session (no worktree) whose project dir vanished | tmux in `$HOME` | reported | `E_REPO_MISSING` (same as h); a healthy root is a no-op; a dead tmux is recreated at the root |
| n | review sessions sharing the source worktree | each session repaired independently, if at all | shared fix, siblings surfaced | the directory fix is shared; `RepairReport.sibling_session_ids` lists alive sessions on the host with the same `project_id` + `worktree_key`, whose panes also hold the dead inode. They are respawned on their own next attach/restart/repair (not automatically in this PR — the tick owner can iterate the list) |
| o | lock files | n/a | reported | worktree `locked` + dir missing: `E_WORKSPACE_LOCKED` (prune skips locked entries and `add` would fail; unlocking silently would defeat the lock's purpose). `index.lock` in the admin dir: a warning in the report; the repair never deletes it |
| p | permissions / disk full | n/a | reported once | the git script fails → `E_REPAIR_FAILED` with git's stderr; single attempt, no retry loop (tests assert exactly one apply script); `workspace_repair_failed` on the timeline |
| — | healthy workspace | — | no-op | one probe, zero writes, zero events (test `healthy_workspace_runs_one_probe_and_writes_nothing`) |
| — | repair failure of any kind | — | row kept | no path deletes or ghosts the session row (tests `git_step_failure_…`, `refusals_record_…`, `tmux_failure_…`) |
| — | branch drift (user checked out another branch in the worktree) | — | reported, row corrected | never "fixed"; the row's `branch` follows what is actually checked out |
| — | guessed path on a `.worktrees/` layout | — | handled | when the path was derived from the `.claude/worktrees/<name>` convention and the repo already uses `.worktrees/`, the checkout is created there |

### Remote hosts

The `worktrees` table holds LOCAL paths only (`refresh_projects` scans the
local disk). A repair on a remote host therefore:

- derives the root from the `~/projects/github.com/<owner>/<repo>` convention
  plus the remote `$HOME` (like `recreate_session`), with the worktree path
  guessed as `.claude/worktrees/<name>` unless git already registers the
  branch elsewhere (adopted) or the repo uses `.worktrees/`;
- never writes a remote path into the local row; only the (portable) branch of
  an existing row is refreshed.

### Path source of truth

- **Local:** the `worktrees.path` row is authoritative. Only when a session
  has a `worktree_key` but no row is the path derived
  (`<project base_path>/.claude/worktrees/<name>`, or `.worktrees/<name>` when
  the repo already uses that layout).
- **Remote:** paths come from `sessions::remote_project_path`, the same
  function `new_session_inner` uses, through one helper
  (`repair::resolve_remote_paths`). Nothing in `repair` hard-codes the
  `~/projects/github.com/...` layout, so a per-host projects-base / layout
  setting that changes that function carries over by updating one call.
- `new_session` hands its already-resolved cwd to the repair, so the create
  path and the repair can never disagree.
- `RepairReport.project_root` (and the `E_REPO_MISSING` message) show the
  resolved root, so a user can see which base-path setting produced a
  missing root (case h).

## New surface

- Service: `repair::ensure_workspace`, `repair::repair_session`,
  `repair::ensure_session_workspace`, `repair::ensure_for_new_session`,
  `repair::spec_for_session`, `repair::plan` (pure), `repair::probe_script`,
  `repair::parse_probe`, `repair::render_git_script`.
- Tauri command: `repair_session({ session_id })` → `RepairReport`.
- MCP tool: `repair_session` (mutating; not in `READONLY_TOOLS`; addressed
  through `resolve_target` like `restart_session`).
- `TmuxExec::respawn_pane_in(name, cwd, pane_cmd)` (default falls back to
  `restart_session`), `tmux::respawn_pane_in`, `tmux::respawn_pane_in_script`.
- `session_events.kind`: `workspace_repaired`, `workspace_repair_failed`.
- Error codes: `E_REPO_MISSING`, `E_BRANCH_CHECKED_OUT`,
  `E_WORKSPACE_LOCKED`, `E_REPAIR_FAILED`. Host unreachability reuses the
  existing `E_HOST_OFFLINE` (the codebase's canonical code; no near-duplicate
  `E_HOST_UNREACHABLE` was added).
- Frontend: `repairSession()` + `RepairReport` in `sessions.ts`, a
  "Repair workspace" button in `SessionDetails.svelte`, the pre-attach check in
  `TerminalView.svelte`.
- No migration.

## Testing

- Pure `plan` tests, one per inventory row, plus no-op / drift / layout cases.
- Probe script + parser tests (quoting, `=` in values, porcelain flags).
- Script rendering tests (order, quoting, `|| exit 1` inside `if` branches
  where `set -e` does not fire).
- End-to-end `ensure_workspace` with a scripted `RepairExec` (`FakeExec`):
  scripted probe outputs → recorded scripts / tmux calls in order, store rows,
  timeline events; failure paths keep the row; host-offline changes nothing.
- One test against a real `git` repository in a tempdir: delete the worktree
  directory, repair brings it back on its branch; second run is a no-op.
- `spec_for_session` / `spec_for_new_session` row-mapping tests, including the
  refusals (orphan, bg, cross-project row, missing row, offline host).
- Frontend: wrapper tests for `repairSession`, a `SessionDetails` test for the
  button gating + toast.

## Follow-ups

- Wire `repair::ensure_session_workspace` into the reconcile tick (Track D),
  iterating `sibling_session_ids` so twin/review panes are respawned together.
- `git worktree repair` cannot resurrect a checkout whose admin dir was pruned
  while the directory (with uncommitted work) survived; a guided "move aside
  and re-add, then restore the diff" flow would close that gap.
