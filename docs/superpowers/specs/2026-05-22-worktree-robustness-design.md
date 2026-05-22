# Robust worktree management — missing detection, recreate, `.git` edge cases

Date: 2026-05-22
Status: Approved — ready for implementation plan

## Goal

Make claude-fleet handle git worktrees whose directory has been deleted or
corrupted out-of-band, resolve `.git` correctly in the worktree case, and cover
the related edge cases — end to end (backend + UI feedback).

Concretely:

1. **Detect** a worktree whose working dir is gone (the state behind a
   `fatal: cannot change to '…': No such file or directory`).
2. **Two-phase lifecycle**: mark it `missing` first (kept visible in the UI),
   auto-prune it on a later refresh if still gone.
3. **Recreate** action in the UI to rebuild a missing worktree from its stored
   path + branch.
4. Robust `.git` resolution (worktree `.git` is a *file*, not a directory).

Non-goals: a manual "Prune" button (auto-prune covers removal), a friendly
"worktree missing" rewrite of repo/Files/Branches errors, remote-host worktree
missing-detection, and symlink canonicalization. (See "Out of scope".)

## Context / current state

- **Discovery** (`src-tauri/src/projects.rs`): `scan_projects()` walks
  `~/projects/github.com/<owner>/<repo>` two levels deep, treating a dir as a
  project when `path.join(".git").exists()`. For each project,
  `list_worktrees()` runs `git -C <repo> worktree list --porcelain` and
  `parse_worktree_porcelain()` extracts `path` + `branch`. The main checkout is
  normalized to the name `"main"`; extra worktrees use their dir name. This runs
  on the **local** host only (`service/projects.rs::refresh_projects()`).
- **Data model** (`src-tauri/src/store.rs`): `worktrees(id, project_id, name,
  path, branch)` keyed by `(project_id, name)`. `WorktreeRow` mirrors it.
  `upsert_worktree()` / `list_worktrees_for_project()` /
  `delete_worktrees_not_in()` (keep-list prune after each scan) /
  `worktree_path(id)`.
- **Sessions** carry `worktree_id: Option<i64>`. The ghost-session flow
  (`store.rs::ghost_and_clean_sessions_in_tx`, migration `008`, `lost_at`
  column) is a proven two-phase pattern: mark `ghost` on first miss, hard-delete
  on the second. We mirror it for worktrees.
- **`.git` resolution** (`src-tauri/src/commands/repo.rs::repo_script`): commands
  resolve the repo root via `tmux display-message … pane_current_path` then
  `git -C "$p" rev-parse --show-toplevel`. This already handles a worktree's
  `.git`-as-file. It breaks only when that pane cwd is the deleted worktree.
- **Worktree creation** (`service/sessions.rs`): `worktree_add_script()` +
  `create_worktree_local()` run `git worktree add "$wt" -b "$name" "$def"`,
  auto-detecting `.worktrees` vs `.claude/worktrees`. Idempotent.
- **Frontend**: `WorktreeRow` in `src/lib/projects.ts`; `ProjectDetails.svelte`
  lists worktrees; `NewSessionDialog.svelte` picks one (defaults to
  `project.worktrees[0]`). No missing-state UI today.
- **Decided policy**: *mark missing, then auto-prune*; recovery action =
  *Recreate*.

## 1. Detection — make `git worktree list` authoritative

`git worktree list --porcelain` emits, per entry, the lines `worktree <path>`,
`HEAD <sha>`, `branch <ref>` (or `detached`), and — when the working tree is
gone or unusable — `prunable <reason>` and/or `locked <reason>`.

Changes in `src-tauri/src/projects.rs`:

- Extend the parsed worktree struct (`DiscoveredWorktree` / `make_worktree`
  output) with `is_prunable: bool`. Set it when a `prunable` line is present for
  the entry.
- A worktree is **missing** when: git marked it `prunable`, OR its `path` does
  not exist on disk (`std::path::Path::new(path).exists()` — belt-and-suspenders
  for cases git hasn't noticed yet).
- The main checkout (name `"main"`, the repo root itself) is never considered
  missing.
- Keep the existing `.git`-existence check for *project* discovery, but do not
  assume `.git` is a directory anywhere; rely on `git worktree list` output for
  worktree state.

`list_worktrees()` returns the enriched list; `refresh_projects()` passes the
per-worktree `is_prunable`/missing computation through to the store write.

## 2. Data model — two-phase missing lifecycle

New migration `src-tauri/migrations/009_worktree_missing.sql` (current highest
is `008_ghost_sessions.sql`):

```sql
ALTER TABLE worktrees ADD COLUMN missing_since INTEGER;
```

`missing_since` is null when present, else the unix time it was first observed
missing. `WorktreeRow` gains `missing_since: Option<i64>`.

Refresh write (extend the per-project worktree reconcile in
`store.rs`/`service/projects.rs`), transactional, emit-after-commit like the
existing reconcile:

- **Present now** (in scan, not missing): `upsert_worktree(...)` and clear
  `missing_since` (set to null) if it was set.
- **Missing now, `missing_since IS NULL`** (Phase 1): set `missing_since = now`.
  Keep the row. Emit a worktree-changed event so the UI shows it as missing.
- **Missing now, `missing_since` already set** (Phase 2 — still gone on a later
  cycle): auto-prune —
  1. run `git -C <repo_base_path> worktree prune` (clears git's registration so
     the `cannot change to …` errors stop),
  2. `delete` the worktree row,
  3. ghost the sessions whose `worktree_id` matches (reuse the existing
     ghost/clean path so they follow the same lifecycle), emitting their row
     changes.

The keep-list prune (`delete_worktrees_not_in`) must NOT remove a prunable
worktree just because… it actually still appears in `git worktree list`
(prunable entries are listed), so it stays in the keep-list and the
`missing_since` path owns its lifecycle. Verify this interaction in a test.

## 3. Recreate action

Backend: new Tauri command `recreate_worktree(args: { worktree_id })` in a
command module (extend `commands/...`; logic in `service/sessions.rs` next to
`create_worktree_local`):

- Look up the worktree row → `(project base_path, name, path, branch)`.
- Reuse `worktree_add_script()` / `create_worktree_local()` to run
  `git worktree add <path> -b <branch> <default-base>` (idempotent; also works
  after a Phase-2 `git worktree prune`).
- On success: clear `missing_since`, `upsert_worktree`, reconcile so the row +
  any sessions refresh. Return the recreated `WorktreeRow`.
- Validate `name`/`branch` via the existing `validate::path_component` /
  `validate::git_ref`; shell-quote all interpolated paths (`shq`).

Frontend: a **Recreate** button on a worktree shown as missing (ProjectDetails,
and the NewSessionDialog worktree chip). Calls `recreateWorktree(id)`, then the
row event clears the missing state.

## 4. `.git` location & edge cases

- Worktree `.git` is a file pointing into `<main>/.git/worktrees/<name>` — we
  never assume a directory; `git worktree list` / `rev-parse` abstract it.
- **Temporarily-unmounted dir / transient blip**: Phase-1 grace period prevents
  premature deletion; it auto-prunes only if still missing on a later cycle.
- **Partial delete** (`.git` file removed but dir present, or vice-versa): git
  reports `prunable`, so it's caught.
- **Main checkout** is never marked missing.
- **Reappears** before Phase 2 (dir restored / recreated): present-now branch
  clears `missing_since`.
- **Recreate after auto-prune**: `git worktree add` recreates cleanly because
  prune removed the stale registration.

## 5. UI feedback

- `src/lib/projects.ts`: `WorktreeRow` gains `missing_since: number | null`; a
  derived `missing` boolean for convenience.
- `ProjectDetails.svelte`: render a "missing" badge on such worktrees and a
  **Recreate** button.
- `NewSessionDialog.svelte`: mark missing worktrees in the chip list, do not
  default-select a missing worktree, and offer Recreate inline.
- Event plumbing: worktree row changes flow through the existing
  `subscribeToRowEvents` / `mergeOne`/`removeOne` path so the UI patches in place
  (no full refetch).

## 6. Testing

**Backend (cargo):**
- `parse_worktree_porcelain` sets `is_prunable` when a `prunable` line is
  present, false otherwise; still parses path/branch correctly.
- Two-phase transition: a worktree missing on refresh N gets `missing_since`
  set (row kept); still missing on refresh N+1 → row deleted + its sessions
  ghosted (mirror the existing ghost-session test using a temp git repo).
- A worktree present again clears `missing_since`.
- `recreate_worktree` on a temp repo recreates the dir, clears `missing_since`,
  and is idempotent.
- Keep-list prune does not delete a still-listed prunable worktree out from
  under the `missing_since` flow.

**Frontend (Vitest):**
- `projects.ts` merge keeps/clears the `missing` flag from row events.
- ProjectDetails renders the missing badge + Recreate button; clicking invokes
  `recreate_worktree`.
- NewSessionDialog doesn't default to a missing worktree.

## Out of scope (YAGNI / deferred)

- Manual "Prune"/"Remove" button (auto-prune handles it).
- Rewriting repo/Files/Branches command errors to a friendly "worktree missing"
  message.
- Remote-host worktree missing-detection (discovery is local; remote sessions
  already surface git errors at command time).
- Symlink canonicalization of worktree paths.

## Touch list

- `src-tauri/migrations/009_worktree_missing.sql` — new migration.
- `src-tauri/src/projects.rs` — parse `prunable`, `is_prunable` field, missing
  computation.
- `src-tauri/src/store.rs` — `missing_since` column on `WorktreeRow`, two-phase
  worktree reconcile (mark / auto-prune + ghost sessions), clear-on-present.
- `src-tauri/src/service/projects.rs` — thread missing info into the store write.
- `src-tauri/src/service/sessions.rs` — `recreate_worktree` logic.
- `src-tauri/src/commands/…` + `lib.rs` — register `recreate_worktree`.
- `src/lib/projects.ts` — `missing_since` / `missing` on `WorktreeRow`.
- `src/lib/ProjectDetails.svelte`, `src/lib/NewSessionDialog.svelte` — missing
  badge + Recreate.
