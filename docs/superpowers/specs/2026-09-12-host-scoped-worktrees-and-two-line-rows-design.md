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
row's `path` is the host path from the scan, so `remote_project_path_for`
keeps deriving the cwd from the row name as today. `ensure_remote_project`
sees the worktree directory already present and skips `worktree add`.
A local row id sent for a remote host (stale frontend, MCP caller) is
rejected up front with `E_INVALID_ARG`: "worktree <name> is a local
checkout; pick one that exists on <host> or start a new worktree".

### Frontend

`src/lib/projects.ts`: `listHostWorktrees(hostAlias, projectId)` wrapper
plus the `HostWorktrees` type.

`NewSessionDialog.svelte`:

- New state `hostWorktrees: { status: 'idle' | 'loading' | 'ready' | 'error'; rows: WorktreeRow[]; cloned: boolean; error?: string }`.
  Local: `rows = project.worktrees`, `status = 'ready'` synchronously (no IPC).
  Remote: an `$effect` keyed on `chosenHost` calls `listHostWorktrees`,
  guarded by a request counter so a slow earlier scan cannot overwrite a
  later host's result.
- The picker's items come from `hostWorktrees.rows` instead of
  `project.worktrees`. While loading, the list shows one non-selectable row
  "Scanning <host>…" under the still-selectable "+ new worktree". On
  `cloned: false` it shows "Not cloned on <host> yet — cloned on first
  session" and offers only "+ new worktree" (the backend clones and adds).
  On error it shows the message inline and "+ new worktree" stays usable.
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
- Line 2 (`sess-details`, 0.65rem, muted, single line, ellipsis): host ·
  tmux name (always, when line 1 shows a friendly name; otherwise the
  worktree key when it differs from the tmux name) · elapsed · context
  meter · cost · effort · PR↗ + CI · last-prompt preview last, so it
  absorbs the truncation.
  Items are joined by ` · ` separators rendered as spans; absent items and
  their separators are omitted. `rowMeta` splits into `rowElapsed` and
  `rowPrompt` so the preview can be placed last.
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
