# claude-fleet iter 5 — Files & Diff viewer

**Status:** Draft
**Owner:** Martin Janči (martin-janci)
**Date:** 2026-05-21

## 1. Summary

Add an in-app file browser + viewer for a session's worktree. The user can
list **all files** (the worktree tree) or **just changed files** (git status),
click any file to see its **content** or, for changed files, its **diff**. The
viewer takes over the center + terminal region; a tab strip flips between
`Terminal` and `Files` without detaching the PTY.

## 2. Goals

- See what a session has changed without leaving the app or attaching a shell.
- Browse the full worktree tree on demand.
- Read a file's content; read a unified diff for any changed file.
- Works for **remote** sessions (over the existing SSH ControlMaster) and
  `local` sessions alike.
- Ergonomic: one keystroke / one click to flip Terminal ↔ Files; instant,
  no PTY churn; a filter box; folder-lazy tree; sensible caps so a huge repo or
  file never janks the UI.

## 3. Non-goals (v1)

- Editing files. Read-only viewer.
- Syntax highlighting (diff +/− coloring only — highlighting is a fast-follow).
- Side-by-side diff (unified only).
- Staging / committing / any git mutation.
- Watching the worktree for changes (the list is fetched on open + manual
  refresh).

## 4. Backend

New module `src-tauri/src/commands/files.rs`, four Tauri commands. Each takes a
`session_id`, resolves the session's worktree cwd (worktree path → project
`base_path`, same precedence as `resolve_review_cwd`), and runs `git` either
locally or over SSH via a shared `run_in_repo` helper.

| Command | Shells out to | Returns |
|---|---|---|
| `repo_changes` | `git -C <cwd> status --porcelain=v1 -z --untracked-files=all` | `Vec<ChangedFile>` |
| `repo_tree` | `git -C <cwd> ls-files -z --cached --others --exclude-standard` | `RepoTree { entries: Vec<String>, truncated }` |
| `repo_file` | `head -c <cap+1> -- <cwd>/<path>` | `FileContent { path, content, truncated, binary, size }` |
| `repo_diff` | `git -C <cwd> diff HEAD -- <path>` (untracked → `--no-index`) | `FileDiff { path, diff, binary }` |

`ChangedFile { path, status, staged, orig_path }` — `status` is a friendly
enum string (`modified`/`added`/`deleted`/`renamed`/`untracked`/`conflict`)
computed in Rust from the porcelain XY code.

### 4.1 Safety

- The repo-relative `path` from the frontend is validated by a new
  `validate::repo_rel_path` — rejects empty, absolute paths, any `..`
  component, and control characters. (The list itself comes from `git`, but
  DevTools/IPC can bypass the UI.)
- Every interpolated value (`cwd`, `path`) is shell-quoted (`shell::quote`).
  Remote scripts are quoted as a whole word (the ssh argv-join rule).

### 4.2 Caps (constants in `files.rs`)

- `MAX_FILE_BYTES = 512 KiB` — `repo_file` reads `cap+1` bytes; over → `truncated`.
- `MAX_DIFF_BYTES = 1 MiB` — `repo_diff` output truncated past this.
- `MAX_TREE_ENTRIES = 20_000` — `repo_tree` truncates past this.
- Binary detection: a NUL byte in the read bytes (or `git`'s "Binary files"
  marker for diffs) → `binary: true`, content/diff omitted.
- Timeouts: 10 s per git/file call.

No DB changes, no migration — the viewer is stateless.

## 5. Frontend

### 5.1 Layout

`App.svelte` gains a `filesMode` boolean. A slim segmented control (`Terminal` /
`Files`) sits top-right of the region right of the sidebar. In `filesMode`:

- The center (Details) column collapses to `0`.
- `TerminalView` stays mounted but `hidden` — the PTY and ANSI buffer are
  preserved, so flipping back is instant and free.
- `FilesPanel` mounts in the terminal column and fills it.

Exiting files mode unmounts `FilesPanel` (so re-opening re-fetches fresh git
state). `Esc` exits files mode; the Files tab is disabled when no session is
selected.

### 5.2 Components

- `FilesPanel.svelte` — container; an internal resizable split: `FileList`
  (left) + `FileViewer` (right).
- `FileList.svelte` — a `Changed` / `All files` toggle, a filter box, and the
  list. Changed = flat list with status badges; All = a folder-lazy tree built
  from the flat `repo_tree` (folders collapsed by default, only expanded
  folders render children — keeps a 20 k-entry repo cheap).
- `FileViewer.svelte` — for the selected file: a `Diff` / `File` toggle
  (`Diff` only offered for changed, non-untracked files); shows `DiffView` or
  the plain content with line numbers.
- `DiffView.svelte` — renders a unified diff: hunk headers, `+`/`−` line
  coloring, old/new line-number gutter.

### 5.3 Store

`src/lib/files.ts` — typed wrappers (`repoChanges`, `repoTree`, `repoFile`,
`repoDiff`) over `invokeCmd`. Component-local state holds the current list,
selection, and view toggles; results are cached per `(session, path)` for the
lifetime of the panel so re-selecting a file is instant.

## 6. UX details

- Opening files mode loads the changed-files list immediately; the tree loads
  lazily the first time the user switches to `All files`.
- Selecting a changed file defaults to `Diff`; an unchanged file shows `File`.
- A manual `Refresh` re-runs `repo_changes` / `repo_tree`.
- Errors (not a git repo, host unreachable) render inline in the panel, never
  as a modal.
- Loading states are subtle (a thin top bar / skeleton), not blocking spinners.

## 7. Phasing

1. Backend commands + `validate::repo_rel_path` + tests.
2. `files.ts` + `FilesPanel`/`FileList`/`FileViewer`/`DiffView`.
3. `App.svelte` tab strip + `filesMode` wiring.
4. Polish: filter, lazy tree, caps surfaced in UI, keyboard shortcuts.

Fast-follows (not v1): syntax highlighting, side-by-side diff, worktree
file-watching, per-session persistence of the last-open file/mode.
