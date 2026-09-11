# Self-repairing session workspaces (worktree directory + tmux cwd)

**Date:** 2026-09-11
**Status:** Implemented (`src-tauri/src/service/repair.rs`), revised after the
PR #49 review.

## Problem

Session creation and restoration must end with the worktree directory
existing, registered with git on the right branch, and the tmux session
running in it. Before this change:

- `new_session` with an existing worktree row trusted `worktrees.path`
  (local) or "the dir exists" (remote) — a deleted directory produced a tmux
  session in a missing cwd (tmux silently falls back to `$HOME`).
- `recreate_session`, `restart_session` and `spawn_review` resolved the cwd
  from rows or from the remote convention `~/projects/github.com/<o>/<r>/.claude/worktrees/<name>`
  and never checked it; on a host that uses `.worktrees/<name>` that guess is
  simply wrong (live: a review session running Claude in `/home/mjanci`).
- `restart_session` respawned the pane without a cwd, so a pane whose
  directory was deleted kept its dead inode.
- Nothing recreated a directory for a row whose directory had disappeared.

## The rule: automatic repair only creates

**Any probe can be wrong.** So a repair that runs as a side effect of a
lifecycle action may only *create what is confirmed missing*. Anything that
can destroy, unregister, redirect or rebranch runs only when a person asked
for it.

| | Automatic (`Policy::Auto`) | Explicit (`Policy::Explicit`) |
|---|---|---|
| Who | new session (existing worktree), restart, recreate, spawn review, terminal attach | **Repair workspace** button; MCP `repair_session` (behind `mcp.confirm_destructive`) |
| Read-only probe | yes | yes |
| `git worktree add` from an existing local or `origin/` branch, target absent on disk and not registered | yes | yes |
| `tmux new-session` when `tmux has-session` confirms the session is gone | attach only (other callers own tmux) | yes |
| `git worktree remove --force -- <path>` for this worktree's own stale entry (never a blanket prune) | only when the [vanished-directory guard](#vanished-directory-guard-automatic) holds, including a parent `dev:inode` matching the one recorded while healthy; the expected pair is re-checked in the apply script before the remove and before the add, which follows immediately from the existing branch; otherwise warning / `E_REPAIR_REQUIRED` | yes |
| `git worktree add` into an empty leftover dir | no — warning | yes |
| Adopt the branch's checkout elsewhere, re-path the row | no — warning | yes, guarded |
| Recreate a branch that exists nowhere locally (`ls-remote`, fetch, or fork from base) | no — warning | yes |
| `git worktree repair -- <path>` | no — warning | yes |
| `tmux respawn-pane -k -c` of a live pane | no — warning | only when the pane's reported cwd is confirmed missing |
| Rewrite the row's path / branch | no | adoption / branch follows checkout |
| Record a missing `worktrees` row, link the session FK | yes | yes |

An automatic run that needs an explicit step applies nothing git-side and
returns `needs_explicit_repair` with the warnings and the `deferred` steps.
New session, restart, recreate and spawn review turn that into
`E_REPAIR_REQUIRED` before they touch tmux; the attach path shows a notice
and attaches anyway.

**Ambiguous means no action.** Unknown tmux state (tmux missing / not
answering), an unreadable pane cwd, no registration at the expected path while
the branch is checked out elsewhere, a failed `ls-remote` or fetch — each is a
warning, never a step.

### Vanished-directory guard (automatic)

The common real case is `rm -rf` of a worktree directory: git still lists the
entry, so a plain add would fail. Any automatic entry point (new session,
spawn review, restart, recreate, attach, the opt-in reconcile tick) may remove
that one entry (`git worktree remove --force -- <path>`, never `prune`) and
re-add it from the existing local or `origin/` branch, but only when every
condition holds (`repair::VanishedGuard`, evaluated in `plan_with`). The
primary one is the **parent fingerprint** (9): without it, the removal stays
explicit-only as #49 designed. A context-free plan (plain `plan()`, no store)
never removes (`AutoContext::allow_auto_unregister` is false there).

1. `dir_absent` — the probe reported a non-empty canonical path (`pwd -P` of
   the nearest existing parent plus the remainder) and `test -e` / `test -L`
   false for it. A missing key or an empty / failed probe never counts.
2. `parent_exists` — the directory that should contain the worktree exists.
3. `repo_ok` — the project root exists and `git rev-parse --git-dir` works
   there (a missing root is refused earlier with `E_REPO_MISSING`).
4. `under_root` — the canonical path lies strictly under the canonical
   project root (which covers `.worktrees/` and `.claude/worktrees/`), with no
   `.` / `..` component.
5. `not_locked` — the entry is not locked (a locked entry whose directory is
   gone is refused earlier with `E_WORKSPACE_LOCKED`, in every policy).
6. `no_other_session` — no other live or ghost session on the host maps to
   the worktree (same project and `worktree_key`, or a `worktree_id` whose
   row has that name or canonical path). Unknown (no project id, a store
   error) counts as mapped.
7. `same_filesystem` — the parent directory's device id equals the project
   root's (`stat -L -c %d` on Linux / BusyBox, `stat -L -f %d` on macOS; the
   probe picks the flavour by trying `-c` first). Either stat failing fails
   the guard.
8. `siblings_present` — every OTHER registered worktree whose canonical path
   shares this one's parent still exists on disk (the probe prints
   `present 1|0` after each porcelain entry). An unmounted volume makes all
   of its worktrees vanish at once; an `rm -rf` leaves the others in place.
   The refusal names the missing siblings: a stale registration of a
   worktree deleted long ago also trips it (prune it, or use Repair).
9. `fingerprint_matches` — the canonical parent's current `dev:inode`
   (`stat -L -c '%d:%i'`, falling back to `stat -L -f '%d:%i'`) equals the
   one recorded while this worktree was healthy. Every probe that finds the
   registered worktree healthy (create, attach, restart, recreate, explicit
   repair, and the verify after a repair) records it, keyed by host and
   canonical worktree path (migration `023_worktree_parent_fingerprints`).
   `fingerprint_check` says which: `match`, `mismatch` (remounted or
   replaced parent), `missing` (never seen healthy), `stat_failed`. Only
   `match` passes.

If any condition fails the run stays explicit-only (`E_REPAIR_REQUIRED` for
new session / restart / recreate / spawn review, a notice on attach) and the
warning names the failed conditions. The guard is recorded in the
`workspace_repaired` event detail (`vanished_guard`) and the `RepairReport`.

**Unmounted volumes.** A worktree on a volume that is not mounted looks
exactly like a deleted one: the path is gone. Removing its registration would
detach a checkout that still exists on the unmounted disk. The
`parent_exists` condition blocks that case: when the volume or its mountpoint
is missing, the parent directory is missing too, so nothing is removed. The
same holds when a whole `.worktrees/` directory is gone. `same_filesystem`
blocks a parent that is another volume: a mounted `.worktrees/` volume whose
worktree vanished, or an autofs mountpoint (autofs has its own device id even
before it mounts). A person decides through the explicit Repair workspace.

**The parent fingerprint closes the unmounted-mountpoint hole.** A plain
(non-autofs) mountpoint that is currently unmounted is an ordinary empty
directory on the root's filesystem, so `same_filesystem` passes; but its
inode is the mountpoint directory's, not the mounted volume root's that was
recorded while the worktree was healthy, so `fingerprint_matches` fails. A
remount, or a parent directory deleted and recreated, likewise gets a new
inode. The apply script re-checks the expected pair (quoted into the script)
right before `git worktree remove` and again before `git worktree add`, so a
volume that unmounts or remounts between the probe and the apply refuses
before the next git step (`E_REPAIR_REQUIRED`: "reappeared" before the
remove, "nothing was re-added" before the add).

Known residuals: a worktree that vanished before any probe saw it healthy has
no fingerprint and needs one Repair click (then it is recorded); a legitimate
rename or recreation of the parent directory also needs one Repair click; an
inode number reused by a new directory at the same path on the same
filesystem would match (a remount of a different filesystem cannot, since
the device id differs); and a bind mount shares the underlying directory's
`dev:inode`.

**Re-check at apply time.** The probe runs one round trip before the apply,
and a late-mounting path (autofs, NFS) can reappear in between; `git worktree
remove --force` would then delete whatever is there. So the apply script
re-checks in the same shell, immediately before the remove: if the path is a
symlink, exists as anything but an empty directory, or its parent is gone, it
prints `reappeared or parent missing; not removing` and exits before any git
step. That maps to `E_REPAIR_REQUIRED` ("reappeared"), which the reconcile
tick backs off like any refusal. (An empty directory is allowed so the
explicit repair of an empty leftover keeps working.) An interrupted apply
(`E_REPAIR_FAILED`, "may be partially applied") is retried at the next tick
interval instead; the interval has a 60 s floor.

## Design

Probe → plan → apply → verify → record, in `service::repair`:

1. **Probe** — one read-only `bash -lc` script per host, every value through
   `shell::quote`, always exits 0. It prints `key=value` lines
   (`wt_path`, `root_exists`, `root_git`, `root_gitdir_ok`, `root_canon`,
   `wt_canon`, `layout_dot_worktrees`, `wt_exists`, `wt_entry_exists`,
   `wt_parent_exists`, `root_dev`, `wt_parent_dev`, `wt_git`, `wt_empty`,
   `wt_gitdir_ok`, `index_lock`, `branch_local`, `branch_remote`,
   `default_branch`, `tmux_alive`, `tmux_dead`, `tmux_cwd`,
   `tmux_cwd_exists`) and then `git worktree list --porcelain`, with a
   `canon <path>` line after every `worktree` entry.
   - **Canonical paths.** `canon` is `pwd -P` of the nearest existing
     ancestor plus the missing remainder (`realpath` is absent on stock macOS
     < 13). Every comparison — our registration, "checked out elsewhere", the
     main-checkout guard, the post-repair verify — is canonical to canonical.
     Rows, `cwd` and reports keep the user-facing form; `cwd_physical` carries
     the host's form.
   - **Layout.** For a convention-derived (guessed) path the probe takes the
     first of `<root>/.worktrees/<name>`, `<root>/.claude/worktrees/<name>`
     that exists or that git has registered (in either path form), and
     reports it as `wt_path`.
   - **tmux.** `tmux_dead=1` only when `has-session` failed while the server
     answered (or no server runs). `tmux_cwd` absent means unknown.
2. **Plan** — `repair::plan(spec, probe, policy)` is pure. Refusals apply in
   every policy: `E_REPO_MISSING`, `E_WORKSPACE_LOCKED` (checked first — real
   git reports a locked, missing worktree as `locked`, not `prunable`),
   `E_BRANCH_CHECKED_OUT` (the branch is in the main checkout),
   `E_REPAIR_FAILED` (a non-empty directory that is not a worktree). Each
   step is tagged automatic or explicit-only; under `Auto` any explicit-only
   step defers the whole git side.
3. **Apply** — the git steps render into one `set -e` script with `--`
   separators, run with a 180 s wall clock (local runs are bounded too). An
   interrupted apply is `E_REPAIR_FAILED` "may be partially applied — run
   Repair workspace again", never "nothing was changed".
   - Branch recreation (explicit): `git ls-remote --exit-code --heads --
     origin refs/heads/<b>` → `0`: fetch it and track `origin/<b>`; `2` (or no
     `origin`): fork from the base branch, then the default branch, then
     `HEAD`, recorded as `branch_from_base:<start>`; anything else: abort. A
     failed fetch aborts.
4. **Verify** — the probe runs again on exactly the planned directory; it
   must be a registered, non-prunable worktree whose `git rev-parse` works.
   Adoption is verified too. Only then is tmux touched.
5. **Record** — rows within the policy (see the table); one
   `workspace_repaired` event (detail: `cwd`, `actions`, `branch_source`,
   `tmux`, `warnings`, and `vanished_guard` when our registered directory was
   missing) or `workspace_repair_failed` (`E_CODE: message`).

### Adoption guard (explicit)

Adopting re-paths the row, and two rows on one path would let a safe-kill of
either delete the other's tree. Adoption is refused with
`E_BRANCH_CHECKED_OUT` when another worktree row of the project claims the
branch, or when any other running or not-yet-dismissed ghost session on the
host maps to that checkout (by the portable `worktree_key`, or by canonical
local path through its `worktree_id`). A key equal to ours is our own
workspace group (reviews, twins).

### Entry points

| Entry point | Call | Policy |
|---|---|---|
| `new_session` (existing worktree row or main checkout) | `repair::ensure_for_new_session` before `tmux new-session`; its cwd is used | `Entry::NewSession` → Auto |
| `spawn_review` | `repair::ensure_session_workspace(source, Entry::SpawnReview)`; the review starts in the resolved dir | Auto |
| `restart_session` | `ensure_session_workspace(id, Entry::Restart)`; then `respawn-pane -k -c <cwd>` (the user asked to restart), or `new-session -c <cwd>` when tmux is gone | Auto |
| `recreate_session` | `ensure_session_workspace(id, Entry::Recreate)`; kill + `new-session -c <cwd>` | Auto |
| terminal attach (`TerminalView.openTerm`) | `repair_session({ explicit: false })` before `pty_open`; notice when an explicit repair is needed | `Entry::Attach` → Auto, creates a confirmed-dead session |
| Repair workspace button | `repair_session({ explicit: true })` | `Entry::Explicit` |
| MCP `repair_session` | resolve + host-bind the target, `confirm_gate`, `repair::repair_session(id, true)` | `Entry::Explicit` |
| reconcile tick (opt-in `repair.auto_on_tick`) | `service::repair_tick`: one batched `test -d` per host, then `ensure_session_workspace(id, Entry::Restart)` for vanished dirs, ≤ 5 per run; refusals are backed off per workspace signature | Auto, tmux untouched |

## Edge-case inventory

"Before" is `main` before this PR; "now" is `repair::plan` + `ensure_workspace`.

| # | Case | Before | Automatic now | Explicit now |
|---|---|---|---|---|
| a | row + tmux alive, worktree dir deleted | new panes fail; rebuilt at the stale path | guard holds (parent fingerprint matches): remove our entry → add → verify (live pane left, `tmux_cwd_stale`); otherwise warning, `E_REPAIR_REQUIRED` for lifecycles | remove our entry → add → verify → respawn (pane cwd confirmed missing) |
| b | tmux gone, dir deleted, entry gone | tmux in a missing cwd | add from the existing branch; attach creates tmux | same, plus create tmux |
| c | dir present, git no longer lists it / stale `.git` link | undetected | warning | `git worktree repair --`; verify decides; a checkout whose admin dir was pruned is reported, never deleted |
| d | registered but directory missing (`prunable`, or older git without the flag) | `worktree add` failed | guard holds (parent fingerprint matches): remove our entry only → add; otherwise warning (an empty leftover dir is not "absent") | remove our entry only → add |
| e | branch only on the remote (`origin/<b>` present) | failed | add with `--track` | same |
| f | branch deleted everywhere | failed | warning | `ls-remote` confirms absent (or no origin) → fork from base, recorded; unreachable origin / failed fetch → `E_REPAIR_FAILED` |
| g | branch checked out elsewhere | git refused | main checkout: `E_BRANCH_CHECKED_OUT`; linked worktree: warning | main checkout: `E_BRANCH_CHECKED_OUT`; linked: adopt after the guard + verify |
| h/m | project root missing or not a repo | tmux in `$HOME` | `E_REPO_MISSING` (resolved root in the message and `project_root`) | same; never `mkdir` |
| i | worktree key collision with another project | n/a | keys are scoped by project; a `worktree_id` of another project is `E_INVALID_STATE` | same |
| j | tmux alive, pane cwd gone | restart kept the dead inode | warning (attach never kills a live pane) | respawn only when the reported cwd is confirmed missing; an empty cwd is "unknown" |
| k | host unreachable | n/a | probe failure → `E_HOST_OFFLINE`, nothing changed; `repair_session` checks reachability first | apply interrupted → `E_REPAIR_FAILED` "may be partially applied" |
| l | interrupted `worktree add` (dir empty or no `.git`) | failed | empty: warning; non-empty: refused | empty: add (TOCTOU is benign: git refuses a non-empty target, nothing deletes); non-empty: refused, never deleted |
| n | reviews sharing the source worktree | n/a | `sibling_session_ids` reported; `spawn_review` uses the resolved dir | same; siblings are not respawned automatically |
| o | locked worktree with missing dir | n/a | `E_WORKSPACE_LOCKED` (checked before anything else) | same; `index.lock` is only a warning |
| p | permissions / disk full | n/a | the git failure is reported once, no retry | same |
| — | symlinked root (macOS `/var` → `/private/var`; `~/projects` → `/mnt/…`; mixed git path forms) | — | canonical comparison: healthy stays a no-op; the main checkout is recognised (never adopted) | same |
| — | remote host using `.worktrees/<name>` | wrong cwd, tmux fell back to `$HOME` | the probe resolves the layout; the resolved dir is used | same |
| — | branch drift inside a healthy worktree | — | warning only | warning; the row's branch follows the checkout |

## New surface

- Service: `repair::{ensure_workspace, repair_session, ensure_session_workspace,
  ensure_for_new_session, spec_for_session, plan, probe_script, parse_probe,
  render_git_script, policy_for, event_detail, require_no_explicit}`,
  `Policy`, `Entry`.
- Tauri command: `repair_session({ session_id, explicit })` → `RepairReport`
  (`explicit` defaults to `false`).
- MCP tool: `repair_session` (mutating, in `CONFIRM_TOOLS`, addressed through
  `resolve_target`, `confirm_nonce` param).
- `TmuxExec::respawn_pane_in(name, cwd, pane_cmd)`.
- `session_events.kind`: `workspace_repaired`, `workspace_repair_failed`.
- Error codes: `E_REPO_MISSING`, `E_BRANCH_CHECKED_OUT`, `E_WORKSPACE_LOCKED`,
  `E_REPAIR_FAILED`, `E_REPAIR_REQUIRED`. Unreachable hosts reuse
  `E_HOST_OFFLINE`.
- Frontend: `repairSession(id, { explicit })` + `RepairReport` in
  `sessions.ts`; the Repair workspace button (explicit, shows the branch
  source); the automatic pre-attach check in `TerminalView.svelte`.
- `repair::{plan_with, AutoContext, VanishedGuard, ensure_workspace_with,
  ensure_session_workspace_for_tick, render_git_script_expecting, parse_fp,
  fp_string}`; `RepairReport.vanished_guard`.
- Store: `worktree_parent_fingerprints` (migration 023),
  `Store::{record_parent_fingerprint, parent_fingerprint}`.
- Reconcile tick: `service::repair_tick` (settings `repair.auto_on_tick`,
  `repair.tick_interval_secs`; migration `021_repair_backoff` for the
  per-session backoff stamp). The repair itself needs no migration.

## Path source of truth

- Local: the `worktrees.path` row, compared canonically. A key-only session's
  path is resolved by the probe (both layouts).
- Remote: `sessions::remote_project_path` (the function `new_session_inner`
  uses) through one helper, `repair::resolve_remote_paths`, for the root; the
  worktree directory is resolved on the host. The local row is never written
  with a remote path. When the per-host projects-base setting (G3) changes
  `remote_project_path`, only that helper follows, and the probe's resolved
  directory still wins at runtime.
- `refresh_projects` stores git's own path form; since every comparison is
  canonical, repair never rewrites a row just because the forms differ.

## Testing

- `plan()` tests per inventory row, in both policies, including the symlinked
  root (healthy no-op, main checkout refused, drift, deleted dir), mixed
  logical/physical path forms, the `.worktrees` layout, locked-only case o,
  empty pane cwd, unknown tmux state.
- Probe script / parser tests; git script rendering asserted exactly.
- `ensure_workspace` with a scripted executor: exact scripts in order,
  tmux calls, row writes, events; automatic runs never apply explicit steps;
  failures keep the row; probe vs apply transport failures map differently.
- Vanished-directory guard: `plan_with` removes-then-adds under every
  automatic entry point only with a matching parent fingerprint (plan tests
  for match / mismatch / missing / failed stat; a click-driven path with no
  recorded fingerprint or a mismatch stays explicit-only); the apply script
  re-checks the quoted pair before remove and add; a healthy probe records
  it; real git: a sole `rm -rf`'d worktree is re-added, a replaced parent
  directory refuses, two siblings vanishing together refuse; each condition alone
  (parent missing, absence unconfirmed, outside the root, `..`, another
  session mapped, mapping unknown) leaves no steps and `E_REPAIR_REQUIRED`;
  root missing and locked are refused earlier; the event detail carries the
  guard.
- Real `git` in tempdirs: deleted worktree (explicit repair, second run a
  no-op; automatic repair recreates it on the same branch, second run a
  no-op; a missing parent blocks the automatic removal), symlinked root (healthy no-op, deleted dir, branch in the main
  checkout refused with the row untouched), `.worktrees` layout, another
  worktree's stale registration survives, unreachable origin aborts, branch
  recreated from base when there is no origin.
- Wiring: `policy_for` per entry point, and each call site's entry.
- Frontend: the wrapper's `explicit` flag, the button (explicit, branch
  source in the toast), the attach path (automatic, before `pty_open`).

## Follow-ups

- Only the active pane of the session's current window is respawned.
- `git worktree repair` cannot resurrect a checkout whose admin dir was pruned
  while the directory survived; a guided "move aside, re-add, restore the
  diff" flow would close that gap.
