# Recreate a live tmux session (full teardown + rebuild)

**Date:** 2026-05-22
**Status:** Design — approved for implementation planning

## Summary

Let the user tear down and rebuild a session's tmux session in one action —
for when the session is wedged/frozen or eating too much RAM. Concretely,
generalize the existing `recreate_session` command so it works on **running**
sessions (not just `ghost` ones), killing the tmux session and rebuilding a
fresh one in the same worktree with the kind-appropriate launch command. This
also fixes the current ghost-Recreate, which today rebuilds a bare
`new-session -d` with no cwd and no REPL.

## Motivation

A long-lived Claude session can hang or accumulate memory. Today the only
in-place recovery is **Restart** (`tmux respawn-pane -k`), which relaunches the
pane process but keeps the tmux session/window — so it won't clear resources
held at the tmux-session level or recover a wedged session. **Kill** drops the
session to a `ghost` row, and **Recreate** only works on ghosts and brings the
session back as a bare shell in `$HOME` (wrong cwd, no Claude REPL). There is no
one-click "nuke this live session and bring it back fresh in its worktree".

## Scope

In scope:

- `recreate_session(session_id)` works on `running` **and** `ghost` sessions.
- Faithful rebuild: resolve the session's worktree cwd and relaunch the
  kind-appropriate command (Claude REPL for `work`, bare shell for `shell`).
- Kill the live tmux session first (tolerating an already-dead session).
- UI: a "Recreate" action on live sessions (SessionDetails + Sidebar) with a
  confirmation, plus PTY re-attach after rebuild.

Out of scope (deferred):

- Per-session resource metrics (RAM/CPU/PID display).
- Automatic frozen/hung detection.
- Persisting a shell session's custom start command (see Known limitations).

## Design decisions (resolved during brainstorming)

| Decision | Choice |
| --- | --- |
| Recreate behavior on a live session | **Full teardown + rebuild** (`kill-session` → fresh `new-session` in the worktree, relaunch command). |
| Resource metrics (RAM/CPU) | **Out of scope** for now — action only. |
| Implementation approach | **Unify & fix `recreate_session`** to handle both running and ghost sessions, rather than adding a separate command. |

## Architecture

### Backend (`src-tauri/src/service/sessions.rs`)

Generalize `recreate_session(session_id, store, ssh)`:

1. **Look up** the `SessionRow` by id (`E_NOTFOUND` if missing). Drop the
   current ghost-only precondition (lines that reject non-`ghost` status).
   Validate `host_alias` and `tmux_name`.
2. **Resolve the rebuild cwd.** Generalize the existing `resolve_review_cwd(s,
   row)` into a shared `resolve_session_cwd(s, row) -> Result<String, IpcError>`
   that resolves: `worktree_id` → `worktree_path`, else the row's project
   `base_path`, else `E_NOREPO` ("cannot determine a worktree path for this
   session"). `spawn_review` switches to call the shared helper (no behavior
   change for it).
3. **Resolve the pane command** by kind: `work` → `tmux::pane_command()`
   (Claude REPL); `shell` → `tmux::shell_pane_command(None)` (bare shell — the
   original custom command isn't persisted; same as Restart).
4. **Teardown.** `let tmux = exec_for(&host_alias, ssh);` then
   `tmux.kill_session(&name)`, **tolerating** a "no such session" failure (a
   ghost or already-dead session must not abort the rebuild). The kill is what
   frees the old process tree / tmux-session resources.
5. **Rebuild.** `tmux.new_session(&name, &cwd, &pane_cmd)` — the same primitive
   `new_session` uses, so the session comes back detached in the correct cwd
   running the correct command.
6. **Persist + reconcile.** `s.restore_session(row.id)` (status→`running`,
   clears `lost_at`) and bump `last_activity_at`; then
   `reconcile_one_host(store, ssh, &host_alias)` and return the refreshed
   `SessionRow` (`E_NOTFOUND` if it doesn't reappear, mirroring `restart`).

Host-correct over SSH via `exec_for`. No new tmux primitives are needed —
`kill_session`, `new_session`, `restore_session`, and `reconcile_one_host` all
already exist.

### Frontend

- **IPC:** `recreateSession(sessionId): Promise<Result<SessionRow>>` already
  exists in `src/lib/sessions.ts` — unchanged. It now succeeds on live
  sessions; on success it patches the returned row into the store (it already
  does this for the ghost path).
- **Live Recreate action:**
  - `SessionDetails.svelte`: add a **Recreate** control next to Restart/Kill.
  - `Sidebar.svelte`: add Recreate to the non-ghost row actions (`↻ ✎ ×`).
  - Use a distinct glyph (e.g. `♻`/`⟳`) and tooltip: *"Kill the tmux session and
    start it fresh in the same worktree."* Keep the ghost-Recreate `↺` as-is
    (it now also rebuilds faithfully via the same command).
- **Confirmation modal** (mirrors the existing Kill modal): *"Recreate
  `<name>`? This kills the tmux session and the running Claude state, then
  starts a fresh session in the same worktree."* Recreate is destructive
  (the running Claude process and its in-memory state are lost).
- **PTY re-attach:** unlike Restart (which keeps the tmux session via
  `respawn-pane`, so the existing `tmux attach` client stays connected and no
  re-attach is needed), Recreate runs `kill-session`, which **severs** the PTY
  client. Because the recreated session keeps the same `tmux_name`,
  TerminalView's selection-keyed effect (it re-opens the PTY only when
  `$selectedSession.tmux_name !== currentSession`) will **not** auto-fire. So
  after a live Recreate of the currently-selected session, the frontend must
  **force** a PTY re-attach — e.g. by briefly clearing and restoring the
  selection (`selectSession(null)` → `selectSession(row)`), which drives the
  effect to close and re-open the PTY, or an explicit pty close+open. The exact
  mechanism (and confirming the effect's close-on-deselect cleanup) is pinned
  in the plan. If the recreated session is **not** the selected one, nothing
  extra is needed.

## Error handling & edge cases

- Errors flow as `IpcError` with `E_*` codes (frontend unwraps `Result`).
- **Host unreachable:** `kill_session`/`new_session` over SSH fail → the SSH/
  `E_REPO` error is surfaced inline. The Sidebar Recreate button is disabled
  when the host is unreachable (same gating the ghost-Recreate `↺` uses).
- **No resolvable worktree/project:** `E_NOREPO` with a clear message.
- **Already-dead/ghost session:** `kill_session` "no such session" is tolerated
  so the rebuild proceeds.
- **Rebuild fails after kill:** the error is surfaced; reconcile leaves the row
  as `ghost`, so the user can retry Recreate (or Dismiss). No silent partial
  state.

## Testing

- **Backend** (extend `service::sessions` tests, following existing patterns):
  - `resolve_session_cwd` prefers the worktree path, falls back to project
    `base_path`, and errors (`E_NOREPO`) when neither resolves.
  - Pane-command selection by `kind` (`work` → REPL, `shell` → bare shell).
  - The recreate flow issues `kill-session` then `new-session` in order and
    restores the row to `running`, using the existing local/mock test seams.
  - A `ghost` session and a `running` session both recreate successfully
    (the precondition was removed).
- **Frontend:** rely on the existing `sessions.ts` store-merge tests; add a
  thin wrapper test only if the suite covers the sibling wrappers. The button +
  confirm + PTY re-attach are verified in the manual smoke (run the app,
  recreate a live work session, confirm it returns in the right worktree with
  the Claude REPL and the terminal re-attaches).

## File-by-file change list

Modified:

- `src-tauri/src/service/sessions.rs` — generalize `recreate_session`
  (drop ghost-only precondition; resolve cwd + pane command; kill-then-rebuild;
  restore + reconcile); extract `resolve_session_cwd` from `resolve_review_cwd`
  and repoint `spawn_review` at it.
- `src/lib/Sidebar.svelte` — Recreate in the non-ghost row actions, gated on
  host reachability; force PTY re-attach when the recreated session is the
  selected one (the existing ghost `doRecreate` handler is extended, not
  duplicated).
- `src/lib/SessionDetails.svelte` — Recreate button + confirm modal, with the
  same force-re-attach for the selected session.

Unchanged (reused): `recreate_session` Tauri command + `recreateSession` IPC
wrapper, `tmux::kill_session`/`new_session`/`pane_command`/`shell_pane_command`,
`store::restore_session`, `reconcile_one_host`.

## Known limitations

- A `shell` session's **custom start command isn't persisted** (the schema has
  no `start_command` column), so Recreate (like Restart) brings shell sessions
  back as a bare shell. Work sessions — the common case for the RAM/frozen
  scenario — rebuild faithfully with the Claude REPL. Persisting the start
  command is a separate, optional follow-up.
- No resource metrics: the user decides when to recreate by observation, not
  from in-app RAM/CPU figures (deferred).
