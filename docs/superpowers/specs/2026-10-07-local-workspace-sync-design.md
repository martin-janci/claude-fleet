# Local workspace sync — Phase 1 design

Status: Phase 1 built (2026-10-07). Phases 2 and 3 of the handover
(Open in IDE, diff UI, "Ask AI about these changes", agent/developer handoff,
AI review and commit, worktree cleanup) are out of scope here.

## Goal

Bind ONE remote worktree to ONE directory on the machine running the desktop
app, and keep the two in step in both directions, so a developer can open
the task in a local IDE while the agent keeps working on the host. File sync
is never a git operation: nothing here commits, stages, checks out or pushes.

## Identity: the link belongs to the worktree, not to the session

What the code already models (`store/rows.rs`, `service/sessions/paths.rs`):

```
project (projects.id, owner/repo)
 └── worktree on a host   (worktrees: host_alias, project_id, name, path, branch)
      └── session          (sessions: host_alias, project_id, worktree_key, worktree_id)
           └── work item   (work_links: the task / ticket the session works on)
```

A session's `worktree_key` is the worktree's name (`main` for the project
root), and `service::sessions::work_dirs(host, project_id, worktree_key)`
resolves where that worktree lives on the host. Sessions come and go on one
worktree (restart, recreate, a reviewer), so the local workspace is keyed on
the worktree identity `(host_alias, project_id, worktree_key)`; the session
that enabled it is recorded, but losing it does not drop the link. Every
session row on that worktree shows the link's state.

The local directory is a representation of the remote worktree, not a second
checkout: `.git` is never synced, and the local copy has no repository of its
own.

## Data (migration 109)

- `local_workspaces`: one row per link. `host_alias, project_id,
  worktree_key` (UNIQUE), `remote_path` (absolute, resolved when the link is
  made), `local_path` (absolute, UNIQUE; no link may sit inside another),
  `session_id` (who enabled it, `ON DELETE SET NULL`), `paused`, `excludes`
  (JSON list of user patterns), `state`, `last_sync_at`, `last_error`,
  `pending_local`, `pending_remote`, `conflicts`, `created_at`.
- `local_workspace_files`: the BASE — the last state both sides agreed on,
  per path: `sha256`, and each side's `(size, mtime)` when it was recorded.
- `local_workspace_conflicts`: open conflicts per path with their kind
  (`both_modified`, `both_added`, `local_deleted`, `remote_deleted`).

## Sync mechanism: our own three-way engine over the existing SSH layer

Chosen over:

- **rsync**: one direction per run; running it both ways is last-writer-wins
  and cannot see a conflict.
- **Mutagen**: does two-way sync well, but installs its own agent on every
  host and its own daemon on the desktop, and its conflict state would live
  outside the fleet's database and UI.
- **Unison**: needs the same Unison version on both ends.

The engine needs nothing on a host beyond what fleet already uses (`bash`,
`git`, `tar`, `stat`, `sha256sum`/`shasum`), goes through `SshClient` (the
per-host ControlMaster, so a pass costs no new handshake), and keeps its state
in SQLite next to everything else.

One pass for one link:

1. **Scan both sides.** Remote: one script lists `git ls-files -co
   --exclude-standard` (tracked plus untracked-not-ignored, so `.gitignore`,
   nested ignores and `.git/info/exclude` are git's own answer) with size and
   mtime, and says which BASE paths still exist although they are no longer
   listed. Local: a walk with the `ignore` crate honouring the synced
   `.gitignore` files. Both sides then go through the same exclude matcher:
   built-in defaults (`.git`, `target/`, `build/`, `.gradle/`, `node_modules/`,
   `.idea/`, `*.iml`, `.vscode/`, `.DS_Store`, `dist/`, `out/`,
   `__pycache__/`, `.venv/`, `.fleet-sync-*`) plus the link's own patterns
   (gitignore syntax). Symlinks and files over 64 MiB are skipped and counted.
2. **Hash what moved.** A path whose `(size, mtime)` differs from the BASE is
   hashed (the remote ones in one batched call). A file recorded within two
   seconds of its mtime is stored "racy" and re-hashed on the next pass, as
   git does, so a same-size edit in the same second is not missed.
3. **Classify** each path against the BASE (`plan()`, a pure function):

   | local | remote | action |
   |---|---|---|
   | unchanged | unchanged | none |
   | changed / new / deleted | unchanged | push it (or delete it remotely) |
   | unchanged | changed / new / deleted | pull it (or delete it locally) |
   | same content on both | | record the BASE, no transfer |
   | different content | | **conflict**, nothing is written |
   | deleted | modified (or the reverse) | **conflict** |

   A path missing from one side's listing counts as deleted only when it
   truly does not exist there: a file that became ignored on one side is
   never propagated as a deletion.
4. **Transfer, guarded.** Pulls: the host streams a tar of the files; each is
   written to a temp file beside its target and renamed into place only if
   the local file still has the stat the scan saw, otherwise it becomes a
   conflict. Pushes: the desktop sends a tar plus the SHA it expects each
   remote file to have (or "absent"); the host script replaces a file only
   when its current content matches, and reports the others as conflicts.
   Deletions use the same guard. So an edit made on either side during a pass
   is never overwritten.
5. **Record** the new BASE for what succeeded, and the link's state.

Making a link uses the same pass with an empty BASE: into an empty or missing
directory it is a plain download; into a directory that already has files,
identical files are adopted, one-sided files are copied, and differing files
become `both_added` conflicts. Nothing is overwritten.

## States

`state` on the link: `syncing` (a pass is running), `synced`,
`local_changes` / `remote_changes` (changes seen and not yet carried — the
moment between two passes, or a pass that failed half-way), `conflict` (at
least one open conflict; the rest keeps syncing), `paused`, `offline` (the
host did not answer; retried with backoff), `error` (anything else, with
`last_error`).

## Conflicts

A conflicting path is left alone on both sides and listed. Resolution
(Phase 1): **Keep local** (push over the remote), **Keep remote** (pull over
the local), or resolve by hand — once both sides have the same content the
next pass clears it. Compare, Keep both and Ask AI to resolve came in Phase 2
(`2026-10-07-local-workspace-handoff-design.md`).

## Engine and commands

- `fleet-core::service::local_sync` — the engine (scan, `plan`, transfer)
  and a desktop tick: every 5 s each enabled, unpaused link gets a pass (one
  at a time per link, at most a few links in parallel); offline links back
  off to 60 s.
- `store::local_workspaces` — the three tables.
- Desktop commands (Tauri): `list_local_workspaces`,
  `enable_local_workspace { session_id, local_path, excludes? }`,
  `pause_local_workspace`, `resume_local_workspace`, `sync_local_workspace_now`,
  `disconnect_local_workspace` (drops the link and its BASE, never touches
  files on either side), `set_local_workspace_excludes`,
  `resolve_local_workspace_conflict { path, keep: local|remote }`.
  Row events `local_workspace:changed` / `:removed` keep the UI live.
- Hub-client mode: `LocalOnly` in Phase 1. It works on a paired desktop
  since, over this machine's own SSH, as the handoff design's *Hub-client
  mode* says (`2026-10-07-local-workspace-handoff-design.md`).
  Hosts reached through `fleet-agent` are refused (`E_UNSUPPORTED`): the agent
  cannot pipe stdin yet.

## UI

- Session detail: a **Local workspace** card — state dot, local and remote
  path, "Synced N s ago", pending counts, conflict list with Keep local /
  Keep remote, and Sync now / Pause / Resume / Disconnect. When off: a
  folder picker and Enable.
- Sessions list: a small dot per row (green synced, grey off, yellow
  pending, red conflict, hollow paused/offline).
- fleet-mobile: no change in Phase 1 (the sync runs on the desktop).

## Out of scope for Phase 1

Open in IDE, diff/compare, "N local changes detected" with Review / Ask AI /
Commit / Discard, agent handoff, worktree create/attach/merge/archive/clean
from this screen, symlinks, file modes beyond the executable bit carried by
tar, a filesystem watcher (polling is enough at 5 s).
