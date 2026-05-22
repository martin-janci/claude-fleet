# Session Restore (Ghost Sessions) — Design

**Date:** 2026-05-22
**Status:** Approved

## Problem

When a machine (e.g. `mefistos`) reboots or crashes, its tmux server is wiped. When the machine comes back online, the fleet's reconcile probe succeeds, finds no tmux sessions, and hard-deletes the corresponding DB rows. The user loses all session metadata — project links, worktree keys, names — with no way to recover it.

## Goal

Preserve "last known" session records as ghost sessions when reconcile would delete them. Show them in the UI with a Recreate action that creates a fresh tmux session with the same name on the recovered host, and a Dismiss action to clean up manually.

## Out of scope

- Auto-launching Claude Code in recreated sessions
- Restoring conversation history or PTY output
- Cross-host session migration

---

## Data Layer

### Migration (007)

Add a `lost_at INTEGER` column to `sessions`:

```sql
ALTER TABLE sessions ADD COLUMN lost_at INTEGER;
```

No change to the `status` column type — it is `TEXT` and already accepts arbitrary values. `"ghost"` becomes a documented valid status alongside `"running"`.

### Reconcile change (`store.rs` — `apply_host_reconcile`)

Current behavior when `spec.reachable = true`:
```sql
DELETE FROM sessions WHERE host_alias = ? AND id NOT IN (<keep>)
```

New behavior — two statements in order:

1. **Ghost sessions not yet ghost** (first cycle after machine wipe):
```sql
UPDATE sessions
SET status = 'ghost', lost_at = <now_unix>
WHERE host_alias = ?
  AND status != 'ghost'
  AND id NOT IN (<keep>)
```

2. **Hard-delete sessions already ghost** (second reconcile cycle — machine came back again but session was never recreated):
```sql
DELETE FROM sessions
WHERE host_alias = ?
  AND status = 'ghost'
  AND id NOT IN (<keep>)
```

This gives ghost sessions exactly one reconcile cycle of grace. Implementation note: SQLite raises a syntax error on `id NOT IN ()` with an empty list — the existing reconcile code must guard against this (skip the statement or substitute `WHERE FALSE`); verify the guard covers both new statements. On the next successful probe cycle they are permanently removed, preventing indefinite accumulation.

When `spec.reachable = false`, neither statement runs — sessions are left untouched, exactly as today.

---

## Backend Commands

Both live in `commands/sessions.rs` / `service/sessions.rs`, alongside `kill_session` and `rename_session`.

`E_HOST_OFFLINE` is a new IPC error code — add it to `ipc_error.rs` alongside the existing `E_*` constants.

### `recreate_session(session_id: i64) -> Result<SessionRow>`

1. Load session from DB — return `E_NOT_FOUND` if missing, `E_INVALID_STATE` if `status != "ghost"`
2. Load host — return `E_HOST_OFFLINE` if `reachable = false`
3. Run `tmux new-session -d -s <tmux_name>` on the host via the existing SSH path
4. If tmux exits with "duplicate session" error, treat as success (session already exists)
5. `UPDATE sessions SET status = 'running', lost_at = NULL WHERE id = ?`
6. Emit `SessionUpdated` row event
7. Return updated `SessionRow`

### `dismiss_ghost_session(session_id: i64) -> Result<()>`

1. Load session — return `E_NOT_FOUND` if missing, `E_INVALID_STATE` if `status != "ghost"` (safety guard)
2. `DELETE FROM sessions WHERE id = ?`
3. Emit `SessionRemoved` row event

---

## Frontend

### No query changes needed

`list_sessions` already returns all sessions regardless of status. Ghost sessions arrive via the normal `mergeSession()` path when the status-change event fires during reconcile.

### Session card — ghost branch

When `session.status === "ghost"`:

- **Visual:** Same card shape, reduced opacity or dashed/grey border to signal inactivity
- **Subtitle:** "Last seen [relative time from `lost_at`]" (e.g. "Last seen 2 hours ago")
- **Actions:**
  - **Recreate** button — calls `recreate_session(session.id)`, disabled with tooltip "Host is offline" when `host.reachable === false`
  - **Dismiss** button — calls `dismiss_ghost_session(session.id)`
- **Hidden actions:** attach, kill, rename — not applicable to a ghost

### Host filter behaviour

Ghost sessions remain visible when the user filters to their host. They are hidden only when the user selects a different host filter. This is unchanged from current behaviour since filtering is by `host_alias`, not by status.

### `hostIsReachable` helper

Reuse `hostByAlias` derived store (already exists in `hosts.ts`) to check `host.reachable` inline in the Recreate button component. No new store needed.

---

## Error handling

| Scenario | Behaviour |
|---|---|
| User clicks Recreate while host is offline | Button is disabled; no IPC call made |
| Host comes online between render and click | `recreate_session` returns `E_HOST_OFFLINE`; frontend shows inline error on card |
| tmux name already taken on host | Treated as success — ownership claimed |
| Ghost session dismissed before second reconcile | Hard-delete via `dismiss_ghost_session`; reconcile's second-cycle DELETE is a no-op |

---

## Testing

- `store.rs` unit tests: verify `apply_host_reconcile(reachable=true)` ghosts live sessions and hard-deletes already-ghost sessions in successive calls
- `service/sessions.rs` unit tests: `recreate_session` rejects non-ghost sessions and offline hosts; `dismiss_ghost_session` rejects non-ghost sessions
- Frontend: ghost card renders correctly, Recreate button disabled when `reachable = false`
