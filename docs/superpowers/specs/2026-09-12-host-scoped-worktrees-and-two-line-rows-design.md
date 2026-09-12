# Host-scoped worktree picker and two-line session rows

Date: 2026-09-12. Status: approved in conversation, pending spec review.

## Problem

1. **The New-session dialog offers the local checkout's worktrees for every
   host.** Picking a local-only worktree for a remote host makes the backend
   run `git worktree add … <branch>` on that host with a branch that only
   exists on the Mac, which fails with the opaque
   `couldn't ensure <owner>/<repo> on <host>: fatal: invalid reference`.
   The DB holds no remote worktree rows to show instead: they only arrive
   through the per-host Claude hook (`EnterWorktree`) over the SSH tunnel,
   which is currently failing on every host, and even when it works a
   worktree is only recorded after a session has entered it.
2. **Session rows are one crowded line.** Status dot, host badge, name,
   status chip, context %, cost, effort, PR, CI and the hover actions all
   share one flex row, so the name is the first thing to be ellipsised.

## Part A — worktree picker lists the chosen host's worktrees

### Backend

New service function `service::worktrees::list_host_worktrees` and a thin
Tauri command `list_host_worktrees` (registered in `lib.rs`; regenerate
`docs/control-api-reference.md` with `REGEN_DOCS=1`). Not an MCP tool.

```rust
pub struct ListHostWorktreesArgs { pub host_alias: String, pub project_id: i64 }
pub struct HostWorktrees {
    pub host_alias: String,
    pub project_id: i64,
    /// false when the project root is not a git checkout on the host yet.
    pub cloned: bool,
    /// Host-scoped rows, `main` first, then by name.
    pub worktrees: Vec<WorktreeRow>,
}
pub async fn list_host_worktrees(
    args: ListHostWorktreesArgs, store: &Mutex<Store>, ssh: &Arc<SshClient>,
) -> Result<HostWorktrees, IpcError>
```

- `host_alias == "local"`: return `Store::list_worktrees_for_project`
  (unchanged behaviour), `cloned: true`.
- Remote host: resolve the project root with the existing
  `repair::resolve_remote_paths` (owner/repo from `fetch_owner_repo`, base
  and layout from `projects::project_base_for` / `layout`). Run one
  `bash -lc` script over SSH with a 15 s wall clock:
  `if [ -d <root>/.git ]; then git -C <root> worktree list --porcelain; else echo __NOT_CLONED__; fi`
  (every path through `crate::shell::quote`). Parse with
  `repair::parse_porcelain`. Map entries to rows: the entry whose path is
  the root becomes `name = "main"`; every other entry's name is its last
  path component; `branch` from the entry (`None` when detached). Bare and
  prunable entries are skipped.
- Cache: upsert each row with `Store::upsert_worktree_on(host, …)` (keyed
  project+host+name, so it never touches local rows), then delete that
  host's rows for the project that the scan did not report
  (a new `Store::delete_host_worktrees_not_in(host, project_id, &names)`;
  refuses to delete a row with alive sessions, same guard as
  `delete_worktree_if_unused`). Return the rows as stored, so ids are stable
  for `new_session`'s `worktree_id`.
- Errors: SSH/host failures surface as the usual `IpcError` (`E_SSH_TIMEOUT`,
  `E_HOST_OFFLINE`, …). A non-zero git exit is `E_GIT_SETUP` with stderr.
  No lock is held across an `.await`.

### `new_session` with a remote `worktree_id`

`new_session_inner`'s existing-worktree arm reads the row by id. A remote
row's `path` is the real path the scan recorded on that host, and it becomes
the pane's cwd directly — a worktree under `.worktrees/` or anywhere else
git has it registered therefore opens correctly, rather than being derived
as `<project_root>/.claude/worktrees/<name>`. `ensure_remote_project` checks
and, if missing, creates exactly that path, so a present worktree is a
no-op. Because the path is authoritative, the repair spec marks it
`path_is_guess: false` and its guess resolver leaves it alone.

A row belonging to another host (stale frontend, MCP caller) is rejected up
front on BOTH the local and the remote arm, with code `E_INVALID` (the
codebase has no `E_INVALID_ARG`): "worktree <name> is a checkout on
<row_host>; pick one that exists on <target_host> or start a new worktree".

Known gap, tracked separately: if the worktree directory was deleted on the
host but its registration remains, `git worktree add` still fails with git's
own "already used by worktree at …" and `ensure_remote_project` returns
before `repair::ensure_for_new_session` could unregister and recreate it.

### Frontend

`src/lib/projects.ts`: `listHostWorktrees(hostAlias, projectId)` wrapper
plus the `HostWorktrees` type.

`NewSessionDialog.svelte`:

- New state `hostWorktrees: { status: 'loading' | 'ready' | 'error'; rows: WorktreeRow[]; cloned: boolean; error?: string }`.
  Local: `rows = project.worktrees`, `status = 'ready'` synchronously (no IPC).
  Remote: an `$effect` keyed on `chosenHost` calls `listHostWorktrees`,
  guarded by a request counter so a slow earlier scan cannot overwrite a
  later host's result.
- The picker's items come from `hostWorktrees.rows` instead of
  `project.worktrees`. A `wt-status` line ABOVE the picker (not a row inside
  it) carries the state: "Scanning <host>…" while the scan runs, "Not cloned
  on <host> yet — it is cloned on the first session" for `cloned: false`, and
  the error message on a failure. In all three the picker offers only
  "+ new worktree", and the selection falls back to new-mode, so a row from
  the previous host can never be submitted with the new host.
- Selection rules on host switch: if the remembered/selected worktree id is
  not in the new host's rows, fall back to that host's `main` row when
  present, else to "+ new worktree". The per-project memory becomes
  per-project-per-host: key `newsession.project.<id>` keeps its shape but
  `worktree` is stored under `hosts[<alias>]`; an old flat value is read
  once as the local host's memory.
- `worktreeDir`, `pathPreview`, `takenSlugs` and `defaultFriendly` read
  from `hostWorktrees.rows` so previews match the host.
- The "in use" meta keeps matching `s.worktree_id === wt.id`; remote rows
  now have ids, so occupancy shows for them too.

### Tests

- Rust: `parse` → row mapping (main vs named, detached, bare skipped);
  cache upsert/prune with the alive-session guard; the `E_INVALID_ARG`
  rejection; script quoting for a path with a quote and a space.
- Frontend (`NewSessionDialog.test.ts`): local host never invokes the
  command; switching to a remote host invokes it once and swaps the list;
  loading and not-cloned states; stale-response guard; fallback to `main`
  / new when the remembered id is absent; memory is per host.

## Part B — two-line session rows with a details toggle

### Layout (`SessionRowItem.svelte`)

Live (non-ghost) rows become a column of two lines:

- Line 1: status dot · kind badges (🔗n, 🔍, ▶, 🤖) · name taking all
  remaining width, single line with ellipsis · one status chip (stuck
  outranks claude_status) · `row-actions` on hover/selection. The host
  badge leaves line 1. **The friendly name is primary**: line 1 shows
  `friendly_name` whenever the session has one (and the friendly-names
  pref is on, as today), else the tmux name.
- Line 2 (`sess-details`, 0.65rem, muted): host · tmux name (always, when
  line 1 shows a friendly name; otherwise the worktree key ONLY when the
  tmux name does not already end in `--<worktree key>` — repeating the tail
  of the name already on line 1 is noise and costs a quarter of the line's
  width) · elapsed · context meter · cost · effort · PR↗ + CI · last-prompt
  preview last.
  Items are joined by ` · ` separators rendered as real `<span>` elements,
  never a CSS `::before` on the item itself: the badges are bordered or
  filled boxes and a generated separator lands *inside* them, shifting the
  context meter's label and adding a stray dot inside the effort chip and
  the PR link's hit area. Absent items and their separators are omitted.
  The line wraps rather than clipping: at the default 280px sidebar the
  full set of badges does not fit on one row, and silently hiding the
  prompt preview and CI would defeat the point of the line. Height is the
  user's choice because the whole line collapses.
  `rowMeta` splits into `rowElapsed` and `rowPrompt` so the preview can be
  placed last; `rowMeta` itself is then unused and goes.
- `row-actions` must not consume layout width on line 1: they are absolutely
  positioned at the row's right edge and revealed on hover/selection, so the
  name keeps its full width and never collapses when the pointer enters.
- Ghost rows, select-mode checkbox and the rename input are unchanged
  (the rename input replaces line 1 and hides line 2).
- `data-testid="sess-row"` stays on the outer element; line 2 gets
  `data-testid="sess-details"`. Existing testids (`host-badge`,
  `claude-chip`, `context-badge`, `cost-badge`, `ci-badge`, `sess-meta`)
  keep their names inside line 2, so current tests need only layout
  adjustments.

### Toggle

`sessions.ts`: `export const showRowDetails = writable<boolean>(readPref('rows.details', true, isBool))`,
persisted like `showBgAgents`. `SidebarFilters.svelte` gets a "details"
pill (testid `toggle-row-details`, `aria-pressed`) next to the friendly-names
pill. When false, line 2 is not rendered; the selected row is not exempt
(one rule, no jumping). Keyboard: the pill is a normal button.

### Tests

- `SessionRowItem` rendering through `Sidebar.test.ts`: line 2 present by
  default with host and prompt preview; hidden when the pref is off; the
  pill flips the pref and persists it; stuck chip still outranks the
  status chip on line 1; the name is on line 1 and the host badge on line 2.

## Out of scope

- Fixing the failing hook tunnel (ssh exit 255 restart loop) — separate.
- Mirroring an unpushed local branch to a remote host (background task
  `task_ceb602b6` covers the origin-aware script and clearer error).
- Per-row or per-project collapse; the toggle is global by decision.
