# Git history, branches & branch tree in the Files tab

**Date:** 2026-05-22
**Status:** Design — approved for implementation planning

## Summary

Extend the Files tab (iter 5) with three new views and a set of git actions:

- **History** — a full-width, interactive commit graph (the "branch tree"),
  drawn from structured `git log` data, not git's ASCII `--graph`.
- **Branches** — a list of local/remote branches with ahead/behind and actions.
- **Git actions** — checkout, create/delete branch, stage & commit, checkout a
  commit (detached), and remote fetch/pull/push.

The feature reuses the existing transport pattern (a git script run in the
session's worktree, locally or over SSH) and the existing diff/list UI
components. The commit graph layout is computed on the frontend so it can be
unit-tested and rendered interactively.

## Motivation

The Files tab today is read-only: `git status` (Changed), a worktree listing
(All files), file contents, and per-file diffs vs HEAD. Users managing
long-lived agent sessions also want to see *history* — what the agent has been
committing — across branches, and to perform routine git operations without
dropping into the terminal pane.

## Scope

In scope:

- Read: commit log (all branches by default), branch list, one commit's
  changed files, and a file's diff within a commit.
- Mutations: checkout branch, checkout commit (detached), create branch,
  delete branch, stage/unstage, commit, fetch, pull, push.
- A frontend-computed, interactive commit graph.
- Safety guards and confirmations on mutating actions.

Out of scope:

- Rebase, cherry-pick, merge, reset, stash, tag creation.
- Remote/branch management beyond fetch/pull/push (no remote add, no
  branch rename).
- Conflict resolution UI.
- Persisting any git state in the SQLite store.

## Design decisions (resolved during brainstorming)

| Decision | Choice |
| --- | --- |
| Read-only or mutating? | **Full git actions** (branch ops, commit & stage, checkout commit, remote sync). |
| Layout of History/tree | **Full-width graph view** (option B), not the narrow left-gutter. |
| Commit-detail placement | **Reuse the existing left-list + right-DiffView split** (option C); a "Back to graph" button returns. |
| Graph default scope | **All branches** (`git log --all`), with a toggle to current-branch-only. |
| Safety posture | **Guard + confirm:** block checkout on a dirty worktree (warn the agent has uncommitted work); confirm destructive ops; surface git errors, never `--force`. |
| Implementation approach | **Approach 1:** extend the existing files command module; compute the graph on the frontend. |

## Architecture

### Backend (`src-tauri/src/commands/`)

**Refactor — `commands/repo.rs` (new):** extract the shared git plumbing that
currently lives in `files.rs` so it can be shared without bloating one file:

- `session_target(store, session_id) -> (host_alias, tmux_name)`
- `repo_script(tmux_name, body) -> String` (resolves `$root` via tmux cwd +
  `git rev-parse --show-toplevel`)
- `run_in_repo(ssh, host, script) -> Output` (local `bash -lc` or multiplexed
  SSH)
- `repo_err(&Output) -> IpcError`
- The timeout / size-cap constants.

`files.rs` is updated to import these instead of defining them.

**Read — `commands/history.rs` (new):**

- `repo_log(session_id, all: bool, limit: u32, skip: u32) -> Vec<Commit>`
  Runs `git log [--all] --date=iso-strict
  --format=<RECORD>` where the record encodes
  `%H %h %P %D %an %ad %s` with a NUL field separator and a NUL-NUL record
  terminator (`-z`-style), so subjects with newlines parse cleanly. Pagination
  via `--max-count=<limit> --skip=<skip>`; default limit 200; "Load more"
  fetches the next page and appends. `%P` gives parent hashes (the graph
  edges); `%D` gives the ref decoration parsed into `Ref`s.
- `repo_branches(session_id) -> Vec<Branch>`
  `git for-each-ref --format=…` over `refs/heads` and `refs/remotes`, yielding
  name, is_current (`HEAD` match), is_remote, upstream, ahead/behind
  (`%(upstream:track)` or a follow-up `rev-list --count`), and tip hash.
- `repo_commit(session_id, hash) -> CommitDetail`
  `git show --no-patch` for metadata + `git show --name-status` (or
  `diff-tree`) for the changed-file list (mapped to the existing `ChangedFile`
  shape: path, status, orig_path).
- `repo_commit_diff(session_id, hash, path) -> FileDiff`
  `git show <hash> -- <path>` (first-parent diff for merges), so the existing
  `DiffView` renders a commit's per-file diff. Same caps/binary handling as
  `repo_diff`.

**Mutations — `commands/mutate.rs` (new):**

- `repo_checkout(session_id, branch)` — guarded (see Safety).
- `repo_checkout_commit(session_id, hash)` — detached checkout; guarded +
  confirmed.
- `repo_create_branch(session_id, name, start_point: Option<String>, checkout: bool)`
- `repo_delete_branch(session_id, name, force: bool)`
- `repo_stage(session_id, paths: Vec<String>)` / `repo_unstage(session_id, paths)`
- `repo_commit_create(session_id, message: String, amend: bool)`
- `repo_fetch(session_id)` / `repo_pull(session_id)` /
  `repo_push(session_id, set_upstream: bool)`

All commands register in `commands/mod.rs` and the Tauri `invoke_handler`.

**Validation:**

- New `validate::git_ref(name)` — rejects names git itself forbids
  (`check-ref-format` rules: no `..`, no leading `-`, no control chars,
  etc.); used for branch create/delete/checkout.
- New hex check for commit hashes (`[0-9a-f]{4,40}`).
- File paths keep using `validate::repo_rel_path`.
- Every interpolated value is shell-quoted with `shell::quote` (`shq`).

**Wire types** (serde, camelCase to the frontend):

```rust
struct Commit {
    hash: String,
    short_hash: String,
    parents: Vec<String>,
    refs: Vec<Ref>,
    author: String,
    date: String,        // ISO-8601
    subject: String,
}
struct Ref { name: String, kind: RefKind } // branch | remote | tag | head
struct Branch {
    name: String,
    is_current: bool,
    is_remote: bool,
    upstream: Option<String>,
    ahead: u32,
    behind: u32,
    tip_hash: String,
}
struct CommitDetail {
    hash: String,
    subject: String,
    body: String,
    author: String,
    date: String,
    files: Vec<ChangedFile>, // reuse the existing shape
}
```

`ChangedFile` and `FileDiff` are reused from `files.rs`.

### Graph rendering (frontend)

**`src/lib/graph.ts` (new, pure):** a standard lane-assignment algorithm over
the commits' `parents[]` links.

- Input: commits in `git log` order (reverse-chronological, child before
  parent).
- Maintain a list of *active lanes*, each awaiting a specific commit hash.
- For each commit: find the lane awaiting its hash (or allocate a new lane if
  none — a branch tip). Its first parent inherits that lane; additional parents
  (merges) allocate/claim further lanes. A commit whose lane is no longer
  awaited by anything closes that lane.
- Emit per row: `{ column, color, edges: Edge[] }`, where `edges` describe the
  vertical/diagonal segments to draw from this row's lanes down to the next
  row. Lane colour is assigned by lane index (stable palette).

This is rendered as an SVG gutter beside the commit rows in
**`CommitGraph.svelte`**. Keeping the layout in a pure function is the reason we
avoid git's ASCII `--graph`: it is unit-testable and drives an interactive UI.

### Frontend IPC & state

- **`src/lib/history.ts` (new):** `invokeCmd` wrappers + TS types mirroring the
  wire types (`Commit`, `Ref`, `Branch`, `CommitDetail`) and all command
  wrappers (`repoLog`, `repoBranches`, `repoCommit`, `repoCommitDiff`,
  `repoCheckout`, `repoCreateBranch`, `repoDeleteBranch`, `repoStage`,
  `repoUnstage`, `repoCommitCreate`, `repoFetch`, `repoPull`, `repoPush`).
- State stays **component-local** (like the current `FilesPanel`). Git state is
  not in the SQLite store, so there is no row-event bus integration. After any
  mutation the affected view reloads (`repoLog` / `repoBranches` /
  `repoChanges`); the existing `reloadKey` pattern invalidates viewer caches.

### UI integration

**`FilesPanel.svelte` mode toggle** gains two entries:
`Changed · All files · History · Branches`. `mode` becomes
`'changes' | 'tree' | 'history' | 'branches'`.

**History mode:**

- Renders full-width `CommitGraph.svelte` (graph gutter + commit rows: subject,
  ref labels, author, relative date). Honors the all-branches / current-branch
  toggle. "Load more" pages older commits.
- Selecting a commit row switches the panel into the existing **left-list +
  right-viewer split**: `FileList` shows the commit's changed files
  (`repo_commit`), `FileViewer` shows the per-file diff (`repo_commit_diff`). A
  `← Back to graph` control restores the graph.
- `FileViewer` / `DiffView` gain an optional `commit?: string` prop; when set
  they call `repoCommitDiff(session, commit, path)` instead of the working-tree
  `repoDiff`, and read file content at that commit rather than the worktree.
- Per-row commit actions (hover / context menu): **Create branch from here**,
  **Checkout this commit**.

**Branches mode — `BranchList.svelte` (new):** branches grouped local / remote,
current branch marked, ahead/behind shown, per-row `Checkout` / `Delete`, and a
`+ New branch` action (prompts for name + start point + "checkout after
create").

**Commit & stage (in Changed mode):** per-file stage/unstage checkboxes
(driven by the `staged` flag `repo_changes` already returns) plus a
commit-message textarea and a `Commit` button as a footer. `repo_stage` /
`repo_unstage` / `repo_commit_create` back it.

**Remote toolbar:** `Fetch · Pull · Push` buttons in the panel header,
available in History and Branches modes.

## Error handling & safety

- Mutations flow as `IpcError` with `E_*` codes. Reuse `E_REPO` for git command
  failures (stderr surfaced inline). Add **`E_DIRTY`** for the
  checkout-on-dirty-worktree case.
- **Checkout guard:** before `repo_checkout` / `repo_checkout_commit`, run a
  `git status --porcelain` check; if the worktree is dirty, return `E_DIRTY`.
  The frontend warns ("the agent may have uncommitted work in this session")
  and only proceeds if the user explicitly confirms. We never pass `--force` /
  `-f` to checkout.
- **Confirm dialogs** for destructive ops: delete branch, detached checkout,
  push.
- All git errors are surfaced; no silent failures.

## Testing

**Backend** (unit, pure functions — same style as `parse_status_z`):

- `parse_log` — record/field splitting (NUL separators, multi-line subjects),
  `%P` → `parents`, `%D` → `refs` (branch / remote / tag / HEAD).
- `parse_branches` — for-each-ref output, current marker, ahead/behind, remote
  classification.
- `validate::git_ref` — accepts valid names, rejects `..`, leading `-`, spaces,
  control chars; commit-hash hex validation.

**Frontend** (Vitest):

- `graph.ts` — lane assignment against fixtures: linear history, a
  branch+merge, an octopus merge, and disjoint roots. Assert column/edge
  output.
- `history.ts` — wrapper argument shaping and `Result` unwrapping.
- Component tests following `files_view.test.ts`: mode switching, commit-detail
  drill-in and back, branch-list rendering and action wiring, stage/commit
  footer.

## File-by-file change list

New:

- `src-tauri/src/commands/repo.rs` — shared git plumbing (extracted).
- `src-tauri/src/commands/history.rs` — `repo_log`, `repo_branches`,
  `repo_commit`, `repo_commit_diff`.
- `src-tauri/src/commands/mutate.rs` — checkout / branch / stage / commit /
  remote commands.
- `src/lib/graph.ts` — lane-assignment algorithm.
- `src/lib/history.ts` — IPC wrappers + types.
- `src/lib/CommitGraph.svelte` — full-width graph view.
- `src/lib/BranchList.svelte` — branches view.

Modified:

- `src-tauri/src/commands/files.rs` — import shared plumbing from `repo.rs`.
- `src-tauri/src/commands/mod.rs` + invoke handler — register new commands.
- `src-tauri/src/validate.rs` — `git_ref`, commit-hash check.
- `src/lib/FilesPanel.svelte` — new modes, mode routing, remote toolbar,
  commit-detail drill-in.
- `src/lib/FileList.svelte` — stage/unstage checkboxes + commit footer in
  Changed mode.
- `src/lib/FileViewer.svelte` / `src/lib/DiffView.svelte` — optional `commit`
  prop.

## Risks & open considerations

- **Colliding with the live agent:** mutations run in the same worktree the
  agent is using. The dirty-tree guard and confirmations mitigate the worst
  case (lost uncommitted work on checkout); other ops (commit, branch create)
  are comparatively safe.
- **Large histories:** capped at 200 commits/page with "Load more"; the lane
  algorithm is linear in commits rendered.
- **Remote auth:** fetch/pull/push rely on the session host's existing git
  credentials (SSH keys / cached tokens). Failures surface as `E_REPO`; we do
  not prompt for credentials.
- **Shell-quoting duplication:** this lands more `shq` call sites; the
  long-overdue consolidation of the four quoting copies (per CLAUDE.md) is not
  addressed here but is not made worse — all new sites use `shell::quote`.
