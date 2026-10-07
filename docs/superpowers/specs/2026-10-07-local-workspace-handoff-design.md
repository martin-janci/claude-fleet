# Local workspace — Phases 2 and 3 design

Status: built (2026-10-07). Builds on Phase 1
(`2026-10-07-local-workspace-sync-design.md`), which keeps one remote
worktree and one local folder in step. Phases 2 and 3 make that pair a
shared workspace: the developer opens it in an IDE, sees what changed on
which side, and hands the work to the agent and back.

Git is still not sync: nothing here commits, discards or merges unless a
person presses the button that says so.

## What each side changed: the activity log

The sync engine already knows the direction of every change it carries.
A push is a local edit reaching the host; a pull is the agent's (or
anyone's on the host) reaching the folder. Phase 2 records that per path:

- `local_workspace_activity (workspace_id, path, origin, change, at)`:
  `origin` is `local` (pushed) or `remote` (pulled), `change` is
  `added`, `modified` or `deleted`. One row per path; the latest
  carry wins.
- A row means "not looked at yet". It goes away when the person hands
  those changes to the agent, commits them, discards them, or dismisses
  the list.

The card says **"7 local changes"** / **"3 agent changes"** from these rows,
and the session list's dot turns yellow with *changes* while local rows
exist.

## Phase 2

### Open in IDE

`open_local_workspace { id, app }`, `app` one of `folder` (the OS file
manager: `open`, `xdg-open` or `explorer`), `vscode` (`code`, or
`open -a "Visual Studio Code"` on macOS), `intellij`
(`idea`, or `open -a "IntelliJ IDEA"` on macOS), `terminal` (Terminal on
macOS, `wt -d` on Windows, `x-terminal-emulator` on Linux). The program is
built by `local_sync::open::command_for` (pure, tested per OS) and spawned
through `proc::std_command`. A missing `code`/`idea` launcher is reported
with how to install it.

### Review changes (diff UI)

- `local_workspace_changes { id }`: runs a pass first (unless paused), then
  `git status --porcelain=v1 -z` in the worktree on the host, so the list
  is git's own answer: uncommitted changes, whoever made them. Each entry
  carries the activity row's `origin` when there is one, so the list can
  say *you* / *agent* / *earlier*.
- `local_workspace_diff { id, path }`: `git diff HEAD -- path` there
  (`--no-index` against `/dev/null` for an untracked file), shown with the
  existing `DiffView`.

These read the worktree by its path, not through a session's pane, so they
work while no session is running on it.

### Ask AI about these changes

`ask_ai_about_local_changes { id, intent, question?, paths? }` sends the
session on that worktree a prompt naming the changed files (at most 200,
then "and N more") and what to do with them. It does not paste the diff:
the agent runs `git diff` itself, in the worktree that already has the
changes, so the prompt stays small and always current.

Intents: `explain`, `review`, `continue` (carry on with the task from the
developer's edits), `tests` (write tests for them), `commit` (propose and
make a commit), `merge` (get the branch ready to merge into its base),
`resolve` (a conflict, below), `custom` (the person's own question). The
target is the
link's own session if it is still alive, else any live work session on the
same host and worktree; with none, the command says so (start one first).
Local activity rows for the named paths are cleared when the agent is asked
to act on them (`continue`, `tests`, `commit`); a question leaves them.

## Phase 3

### Handoff

A link has a `driver`: `shared` (default: both edit, sync resolves),
`developer` or `agent`.

- **Take over** (`driver = developer`): fleet tells the agent, as a system
  prompt, that the developer is editing this worktree locally and it must
  not change files until handed back. The card shows *You're driving*.
- **Hand back to AI** (`driver = agent`): fleet sends the agent the list of
  files the developer changed since taking over (the local activity rows)
  and asks it to continue from them; those rows are cleared. The card shows
  *Agent is driving*, and the agent's own changes as they arrive.
- **Shared** again: no message.

Commands: `set_local_workspace_driver { id, driver }`.

### AI review and commit

The `review` and `commit` intents above, plus two direct git actions on the
card's change list, each behind a confirmation:

- `commit_local_workspace { id, message, paths }`: `git add -A -- paths`
  then `git commit -m message -- paths` in the worktree on the host. Only
  the chosen paths; returns the new commit.
- `discard_local_workspace_changes { id, paths }`: back to `HEAD` on the
  host (`git reset` then `git checkout HEAD --`, which older hosts have
  where `git restore` is missing; a file `HEAD` lacks is removed); the next pass carries that to the folder, under the same
  write guards as any other change.

### Advanced conflict resolution

- **Compare** (`compare_local_conflict { id, path }`): the local file is
  sent to the host and `git diff --no-index` runs there, so the person sees
  local → remote as a diff, binary files reported as such.
- **Keep both** (`keep_both_local_conflict { id, path }`): the local version
  is copied to `<path>.local-copy` (an extension no build picks up), then
  the conflict resolves to the remote side. Both versions survive and sync.
- **Ask AI to resolve**: the `resolve` intent writes the local version next
  to the remote one on the host (`<path>.fleet-local`, which the
  sync never carries; the prompt asks the agent to delete it after) and
  asks the agent to merge the two into `path`; once the agent's result
  syncs, *Keep remote* takes it.
- Keep local, Keep remote and by-hand resolution stay as in Phase 1.

### Multi-worktree management and cleanup

A **Local workspaces** overview (opened from the card)
lists every link on this machine: project, worktree, folder, state, change
counts, driver, and why it is **stale** when it is: its folder is gone, the
worktree folder is gone on the host, or no live session uses the worktree.
From there: Open, Sync now, Pause/Resume, Disconnect, and **Clean up stale**
(disconnects them; files stay on both sides).

Worktree lifecycle maps onto what fleet already has rather than a second
copy of it: *create* and *attach* are New session (new or existing
worktree), *archive/delete* is Safe remove (which removes the worktree and
keeps unpushed work), *merge* is the `merge` intent (the agent merges its
branch into the base branch and reports). The overview links each.

## Hub-client mode

Every new command is `LocalOnly`, as in Phase 1: the folder is on this
desktop.

## Not here

A filesystem watcher (polling at 5 s stays), three-way merge UI inside
fleet, symlinks, fleet-agent hosts, the mobile app (it has no local folder).
