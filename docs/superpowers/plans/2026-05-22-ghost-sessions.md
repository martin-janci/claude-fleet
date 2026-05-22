# Ghost Sessions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When a machine reboots and loses its tmux sessions, preserve the last-known session records as "ghost" rows that show in the UI with Recreate and Dismiss actions.

**Architecture:** Add a `lost_at` column to `sessions`; replace the hard-delete in `apply_host_reconcile` with a two-phase ghost-then-cleanup; add two service functions (`recreate_session`, `dismiss_ghost_session`) backed by a new `bare_new_session` tmux primitive; surface ghost sessions in the sidebar with distinct UI.

**Tech Stack:** Rust/SQLite (rusqlite), Tauri 2 IPC, Svelte 5 runes, TypeScript

---

## File Map

| Action | File |
|---|---|
| Create | `src-tauri/migrations/008_ghost_sessions.sql` |
| Modify | `src-tauri/src/store.rs` — `SessionRow`, `migrate()`, `upsert_session_in_tx`, new `fetch_session_by_id`, `ghost_and_clean_sessions_in_tx`, `apply_host_reconcile`, `restore_session` |
| Modify | `src-tauri/src/tmux.rs` — add `bare_new_session` to trait + both impls |
| Modify | `src-tauri/src/service/sessions.rs` — add `recreate_session`, `dismiss_ghost_session` |
| Modify | `src-tauri/src/commands/sessions.rs` — add command wrappers |
| Modify | `src-tauri/src/lib.rs` — register new commands |
| Modify | `src/lib/sessions.ts` — extend `SessionRow`, add `recreateSession`, `dismissGhostSession` |
| Modify | `src/lib/Sidebar.svelte` — ghost card UI + CSS |

---

## Task 1: Migration 008 — add `lost_at` column

**Files:**
- Create: `src-tauri/migrations/008_ghost_sessions.sql`
- Modify: `src-tauri/src/store.rs:154-183` (the `migrate()` method)

- [ ] **Step 1: Write the migration file**

```sql
-- src-tauri/migrations/008_ghost_sessions.sql
ALTER TABLE sessions ADD COLUMN lost_at INTEGER;
INSERT OR IGNORE INTO schema_version (version) VALUES (8);
```

- [ ] **Step 2: Add the `v < 8` block to `Store::migrate()`**

In `store.rs`, after the `if v < 7` block (line ~179), add:

```rust
        if v < 8 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../migrations/008_ghost_sessions.sql"))?;
            tx.commit()?;
        }
```

- [ ] **Step 3: Write the failing test**

In the `#[cfg(test)]` block in `store.rs`, add after the existing migration tests:

```rust
#[test]
fn migration_008_adds_lost_at_column() {
    let store = Store::open_in_memory().expect("store");
    let v: i64 = store
        .conn
        .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, 8, "schema_version should be 8 after migration");
    // Column exists and defaults to NULL
    store.upsert_host("alpha").unwrap();
    store
        .upsert_session("s1", "alpha", None, None, 1, 1, "running", None)
        .unwrap();
    let lost: Option<i64> = store
        .conn
        .query_row(
            "SELECT lost_at FROM sessions WHERE tmux_name='s1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(lost, None, "lost_at should be NULL for a fresh session");
}
```

- [ ] **Step 4: Run the test**

```bash
cd src-tauri && cargo test migration_008 -- --nocapture
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/migrations/008_ghost_sessions.sql src-tauri/src/store.rs
git commit -m "feat(store): migration 008 — add lost_at column to sessions"
```

---

## Task 2: Extend `SessionRow` and all SELECT queries

**Files:**
- Modify: `src-tauri/src/store.rs`

The `SessionRow` struct and every `SELECT` statement that maps session columns must include `lost_at` at column index 13 (0-based).

- [ ] **Step 1: Add `lost_at` to `SessionRow` struct**

In `store.rs`, update the struct definition (currently at line ~33):

```rust
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionRow {
    pub id: i64,
    pub tmux_name: String,
    pub host_alias: String,
    pub project_id: Option<i64>,
    pub worktree_id: Option<i64>,
    pub created_at: i64,
    pub last_activity_at: i64,
    pub status: String,
    pub notes: Option<String>,
    pub account_uuid: Option<String>,
    pub kind: String,
    pub reviews_session_id: Option<i64>,
    pub worktree_key: Option<String>,
    pub lost_at: Option<i64>,
}
```

- [ ] **Step 2: Update `upsert_session_in_tx` ON CONFLICT clause to clear `lost_at` on revival**

Find the ON CONFLICT clause in `upsert_session_in_tx` (line ~1025) and add `lost_at=NULL`:

```rust
        tx.execute(
            "INSERT INTO sessions (tmux_name, host_alias, project_id, worktree_id,
                                   created_at, last_activity_at, status, account_uuid,
                                   worktree_key)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(host_alias, tmux_name) DO UPDATE SET
               project_id=excluded.project_id,
               worktree_id=excluded.worktree_id,
               last_activity_at=excluded.last_activity_at,
               status=excluded.status,
               account_uuid=excluded.account_uuid,
               worktree_key=excluded.worktree_key,
               lost_at=NULL",
```

- [ ] **Step 3: Update `fetch_session` (line ~1219) to include `lost_at`**

```rust
fn fetch_session(
    conn: &Connection,
    tmux_name: &str,
    host_alias: &str,
) -> Result<Option<SessionRow>, rusqlite::Error> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
                last_activity_at, status, notes, account_uuid, kind, reviews_session_id,
                worktree_key, lost_at
         FROM sessions WHERE tmux_name=?1 AND host_alias=?2",
    )?;
    let mut rows = stmt.query_map(rusqlite::params![tmux_name, host_alias], |row| {
        Ok(SessionRow {
            id: row.get(0)?,
            tmux_name: row.get(1)?,
            host_alias: row.get(2)?,
            project_id: row.get(3)?,
            worktree_id: row.get(4)?,
            created_at: row.get(5)?,
            last_activity_at: row.get(6)?,
            status: row.get(7)?,
            notes: row.get(8)?,
            account_uuid: row.get(9)?,
            kind: row.get(10)?,
            reviews_session_id: row.get(11)?,
            worktree_key: row.get(12)?,
            lost_at: row.get(13)?,
        })
    })?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}
```

- [ ] **Step 4: Add private `fetch_session_by_id` helper (needed for ghost logic)**

Add this function immediately after `fetch_session`:

```rust
fn fetch_session_by_id(
    conn: &Connection,
    id: i64,
) -> Result<Option<SessionRow>, rusqlite::Error> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
                last_activity_at, status, notes, account_uuid, kind, reviews_session_id,
                worktree_key, lost_at
         FROM sessions WHERE id=?1",
    )?;
    let mut rows = stmt.query_map(rusqlite::params![id], |row| {
        Ok(SessionRow {
            id: row.get(0)?,
            tmux_name: row.get(1)?,
            host_alias: row.get(2)?,
            project_id: row.get(3)?,
            worktree_id: row.get(4)?,
            created_at: row.get(5)?,
            last_activity_at: row.get(6)?,
            status: row.get(7)?,
            notes: row.get(8)?,
            account_uuid: row.get(9)?,
            kind: row.get(10)?,
            reviews_session_id: row.get(11)?,
            worktree_key: row.get(12)?,
            lost_at: row.get(13)?,
        })
    })?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}
```

- [ ] **Step 5: Update `get_session_by_id` to delegate to `fetch_session_by_id`**

Replace the body of `get_session_by_id` (line ~869):

```rust
pub fn get_session_by_id(&self, id: i64) -> Result<Option<SessionRow>, rusqlite::Error> {
    fetch_session_by_id(&self.conn, id)
}
```

- [ ] **Step 6: Update `list_all_sessions`, `list_sessions_for_host`, `list_related_sessions`**

In each of these three methods, add `, lost_at` to the SELECT column list and `lost_at: row.get(13)?` to the `SessionRow` mapping. The pattern is identical for all three — find the SELECT string and add the column, then add the field in the `Ok(SessionRow { ... })` closure.

For `list_all_sessions` (line ~764):
```rust
"SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
        last_activity_at, status, notes, account_uuid, kind, reviews_session_id,
        worktree_key, lost_at
 FROM sessions ORDER BY last_activity_at DESC"
```

For `list_sessions_for_host` (line ~734):
```rust
"SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
        last_activity_at, status, notes, account_uuid, kind, reviews_session_id,
        worktree_key, lost_at
 FROM sessions WHERE host_alias=?1 ORDER BY last_activity_at DESC"
```

For `list_related_sessions` (line ~808):
```rust
"SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
        last_activity_at, status, notes, account_uuid, kind, reviews_session_id,
        worktree_key, lost_at
 FROM sessions
 WHERE project_id=?1 AND worktree_key=?2 AND id<>?3
 ORDER BY host_alias ASC, tmux_name ASC"
```

Add `lost_at: row.get(13)?` to each `SessionRow` construction in these methods.

- [ ] **Step 7: Run existing tests to verify no regressions**

```bash
cd src-tauri && cargo test -- --nocapture 2>&1 | tail -20
```

Expected: all existing tests pass (the new field defaults to NULL and nothing breaks)

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/store.rs
git commit -m "feat(store): add lost_at field to SessionRow and all session queries"
```

---

## Task 3: Ghost logic in `apply_host_reconcile`

**Files:**
- Modify: `src-tauri/src/store.rs`

Replace the `delete_sessions_not_in_in_tx` call with a two-phase ghost-then-cleanup function.

- [ ] **Step 1: Write the failing test (ghost on first reconcile cycle)**

Add in `#[cfg(test)]`:

```rust
#[test]
fn reconcile_ghosts_sessions_on_first_empty_probe_then_deletes_on_second() {
    let (mut store, bus) = store_with_recorder();
    store.upsert_host("alpha").unwrap();
    store
        .upsert_session("s1", "alpha", None, None, 1, 10, "running", None)
        .unwrap();
    let s1_id = store.get_session("s1", "alpha").unwrap().unwrap().id;
    bus.take(); // drain setup events

    // First reachable probe with no sessions — s1 should become ghost
    store
        .apply_host_reconcile(HostReconcile {
            alias: "alpha",
            reachable: true,
            claude_version: None,
            tmux_version: None,
            last_pinged_at: 100,
            sessions: &[],
            keep: &[],
        })
        .unwrap();

    let s1 = store.get_session_by_id(s1_id).unwrap().unwrap();
    assert_eq!(s1.status, "ghost", "first empty probe should ghost s1");
    assert!(s1.lost_at.is_some(), "lost_at must be set");

    let evts = bus.take();
    assert!(
        evts.iter().any(|e| e.starts_with("session:updated:")),
        "ghost transition should emit session:updated; got: {evts:?}"
    );
    assert!(
        !evts.iter().any(|e| e.starts_with("session:killed:")),
        "no kill event on first cycle; got: {evts:?}"
    );

    // Second reachable probe with no sessions — ghost s1 should be deleted
    store
        .apply_host_reconcile(HostReconcile {
            alias: "alpha",
            reachable: true,
            claude_version: None,
            tmux_version: None,
            last_pinged_at: 200,
            sessions: &[],
            keep: &[],
        })
        .unwrap();

    assert!(
        store.get_session_by_id(s1_id).unwrap().is_none(),
        "second empty probe should hard-delete the ghost"
    );
    let evts2 = bus.take();
    assert!(
        evts2.contains(&format!("session:killed:{s1_id}")),
        "second cycle must emit session:killed; got: {evts2:?}"
    );
}
```

- [ ] **Step 2: Run test to confirm it fails**

```bash
cd src-tauri && cargo test reconcile_ghosts -- --nocapture
```

Expected: FAIL (current code hard-deletes on first cycle)

- [ ] **Step 3: Add `ghost_and_clean_sessions_in_tx` function**

Add this private function in `store.rs` after `delete_sessions_not_in_in_tx` (around line 1106):

```rust
/// Phase 1: sessions not in `keep_names` that are currently live (`status !=
/// 'ghost'`) are soft-deleted by setting `status='ghost'` and `lost_at=now`.
/// Phase 2: sessions that are already ghost (from a previous cycle) and still
/// not in `keep_names` are hard-deleted.
///
/// Both phases are no-ops when a session's `tmux_name` IS in `keep_names`
/// (the session reappeared in tmux — the preceding upsert loop already set it
/// back to `status='running'` with `lost_at=NULL`).
fn ghost_and_clean_sessions_in_tx(
    tx: &rusqlite::Transaction,
    host_alias: &str,
    keep_names: &[String],
    now: i64,
    out: &mut Vec<RowChange>,
) -> Result<(), rusqlite::Error> {
    // ── Phase 1: ghost live sessions not in keep ──────────────────────────
    let ghost_ids: Vec<i64> = if keep_names.is_empty() {
        let mut stmt = tx.prepare_cached(
            "UPDATE sessions SET status='ghost', lost_at=?1
             WHERE host_alias=?2 AND status!='ghost'
             RETURNING id",
        )?;
        stmt.query_map(rusqlite::params![now, host_alias], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        let phs = keep_names
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "UPDATE sessions SET status='ghost', lost_at=?1
             WHERE host_alias=?2 AND status!='ghost' AND tmux_name NOT IN ({phs})
             RETURNING id"
        );
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&now, &host_alias];
        for n in keep_names {
            params.push(n);
        }
        let mut stmt = tx.prepare(&sql)?;
        stmt.query_map(params.as_slice(), |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };

    for id in &ghost_ids {
        if let Some(row) = fetch_session_by_id(tx, *id)? {
            out.push(RowChange::SessionUpdated(row));
        }
    }

    // ── Phase 2: hard-delete sessions already ghost from a prior cycle ────
    let kill_ids: Vec<i64> = if keep_names.is_empty() {
        let mut stmt = tx.prepare_cached(
            "DELETE FROM sessions WHERE host_alias=?1 AND status='ghost' RETURNING id",
        )?;
        stmt.query_map(rusqlite::params![host_alias], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        let phs = keep_names
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "DELETE FROM sessions
             WHERE host_alias=?1 AND status='ghost' AND tmux_name NOT IN ({phs})
             RETURNING id"
        );
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&host_alias];
        for n in keep_names {
            params.push(n);
        }
        let mut stmt = tx.prepare(&sql)?;
        stmt.query_map(params.as_slice(), |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };

    for id in &kill_ids {
        out.push(RowChange::SessionKilled(*id));
    }

    Ok(())
}
```

- [ ] **Step 4: Wire `ghost_and_clean_sessions_in_tx` into `apply_host_reconcile`**

In `apply_host_reconcile` (line ~1155), replace:

```rust
Self::delete_sessions_not_in_in_tx(tx, spec.alias, spec.keep, &mut out)?;
```

with:

```rust
let now = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs() as i64;
Self::ghost_and_clean_sessions_in_tx(tx, spec.alias, spec.keep, now, &mut out)?;
```

- [ ] **Step 5: Run the test**

```bash
cd src-tauri && cargo test reconcile_ghosts -- --nocapture
```

Expected: PASS

- [ ] **Step 6: Run all store tests**

```bash
cd src-tauri && cargo test -- --nocapture 2>&1 | tail -20
```

Expected: `apply_host_reconcile_happy_path_persists_all_and_emits_after_commit` **will fail** because it expects `session:killed:{stale_id}` but ghosts now emit `session:updated:` instead, and the ghost row is still present. Update that test as follows:

```rust
// Replace the two assertions that check for stale:
// OLD:
//   assert!(evts.contains(&format!("session:killed:{stale_id}")), ...);
// NEW:
assert!(
    evts.contains(&format!("session:updated:{stale_id}")),
    "stale becomes ghost (session:updated); got: {evts:?}"
);

// Also replace the row-list check:
// OLD: assert_eq!(names, vec!["fresh", "keep-existing"], "stale pruned, two live");
// NEW:
let live: Vec<String> = store
    .list_sessions_for_host("alpha")
    .unwrap()
    .into_iter()
    .filter(|r| r.status != "ghost")
    .map(|r| r.tmux_name)
    .collect();
assert_eq!(live, vec!["fresh", "keep-existing"], "two live sessions");
let ghosts: Vec<String> = store
    .list_sessions_for_host("alpha")
    .unwrap()
    .into_iter()
    .filter(|r| r.status == "ghost")
    .map(|r| r.tmux_name)
    .collect();
assert_eq!(ghosts, vec!["stale"], "stale is now ghost");
```

Run all tests again after this fix.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/store.rs
git commit -m "feat(store): ghost sessions on reconcile instead of hard-delete"
```

---

## Task 4: `Store::restore_session` method

**Files:**
- Modify: `src-tauri/src/store.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn restore_session_clears_ghost_status_and_lost_at() {
    let (store, bus) = store_with_recorder();
    store.upsert_host("alpha").unwrap();
    store
        .upsert_session("s1", "alpha", None, None, 1, 1, "running", None)
        .unwrap();
    let id = store.get_session("s1", "alpha").unwrap().unwrap().id;
    // Manually ghost it
    store
        .conn
        .execute(
            "UPDATE sessions SET status='ghost', lost_at=999 WHERE id=?1",
            rusqlite::params![id],
        )
        .unwrap();
    bus.take(); // drain

    let row = store.restore_session(id).unwrap().expect("row must exist");
    assert_eq!(row.status, "running");
    assert_eq!(row.lost_at, None);

    let evts = bus.take();
    assert!(
        evts.iter().any(|e| e.starts_with("session:updated:")),
        "restore must emit session:updated; got: {evts:?}"
    );
}
```

- [ ] **Step 2: Run test to confirm it fails**

```bash
cd src-tauri && cargo test restore_session_clears -- --nocapture
```

Expected: FAIL (method doesn't exist yet)

- [ ] **Step 3: Add `restore_session` to `Store`**

Add after `set_worktree_key` (around line 866):

```rust
/// Transition a ghost session back to running. Called after `bare_new_session`
/// successfully recreates the tmux session on the host.
pub fn restore_session(&self, id: i64) -> Result<Option<SessionRow>, rusqlite::Error> {
    self.conn.execute(
        "UPDATE sessions SET status='running', lost_at=NULL WHERE id=?1",
        rusqlite::params![id],
    )?;
    let row = fetch_session_by_id(&self.conn, id)?;
    if let Some(ref r) = row {
        self.bus.session_updated(r);
    }
    Ok(row)
}
```

- [ ] **Step 4: Run the test**

```bash
cd src-tauri && cargo test restore_session_clears -- --nocapture
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/store.rs
git commit -m "feat(store): add restore_session to clear ghost status"
```

---

## Task 5: Add `bare_new_session` to `TmuxExec` trait

**Files:**
- Modify: `src-tauri/src/tmux.rs`

Creates a detached tmux session with just a name — no working directory or pane command. Used by `recreate_session` to resurrect a ghost.

- [ ] **Step 1: Add method to `TmuxExec` trait**

In `tmux.rs`, add to the `TmuxExec` trait (after `capture_pane`):

```rust
/// Create a detached session with the given name and no initial command.
/// Returns `Ok(())` if the session was created or already exists.
async fn bare_new_session(&self, name: &str) -> Result<(), IpcError>;
```

- [ ] **Step 2: Implement for `LocalTmux`**

In the `impl TmuxExec for LocalTmux` block:

```rust
async fn bare_new_session(&self, name: &str) -> Result<(), IpcError> {
    let output = tokio::process::Command::new("tmux")
        .args(["new-session", "-d", "-s", name])
        .output()
        .await
        .map_err(|e| IpcError::new("E_TMUX", format!("spawn tmux failed: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("duplicate session") {
        return Ok(());
    }
    Err(IpcError::new("E_TMUX", stderr.trim()))
}
```

- [ ] **Step 3: Implement for `RemoteTmux`**

In the `impl TmuxExec for RemoteTmux` block:

```rust
async fn bare_new_session(&self, name: &str) -> Result<(), IpcError> {
    let script = format!("tmux new-session -d -s {}", shell_quote(name));
    let output = self.remote_bash(&script).await?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("duplicate session") {
        return Ok(());
    }
    Err(IpcError::new("E_TMUX", stderr.trim()))
}
```

- [ ] **Step 4: Verify it compiles**

```bash
cd src-tauri && cargo check 2>&1 | grep -E "error|warning" | head -20
```

Expected: no errors

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/tmux.rs
git commit -m "feat(tmux): add bare_new_session to TmuxExec trait"
```

---

## Task 6: Service functions `recreate_session` and `dismiss_ghost_session`

**Files:**
- Modify: `src-tauri/src/service/sessions.rs`

- [ ] **Step 1: Write failing tests**

Add at the bottom of `service/sessions.rs` (or its test module if it has one):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn recreate_session_rejects_non_ghost() {
        let store = std::sync::Mutex::new(Store::open_in_memory().unwrap());
        {
            let mut s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
                .unwrap();
        }
        let id = store
            .lock()
            .unwrap()
            .get_session("dev", "local")
            .unwrap()
            .unwrap()
            .id;
        let args = RecreateSessionArgs { session_id: id };
        // SshClient::new() is the no-arg constructor; the call never reaches SSH
        // because the non-ghost guard fires first.
        let ssh = std::sync::Arc::new(crate::ssh::SshClient::new());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(recreate_session(args, &store, &ssh));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, "E_INVALID_STATE");
    }

    #[test]
    fn dismiss_ghost_rejects_non_ghost() {
        let store = std::sync::Mutex::new(Store::open_in_memory().unwrap());
        {
            let mut s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
                .unwrap();
        }
        let id = store
            .lock()
            .unwrap()
            .get_session("dev", "local")
            .unwrap()
            .unwrap()
            .id;
        let args = DismissGhostSessionArgs { session_id: id };
        let result = dismiss_ghost_session(args, &store);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, "E_INVALID_STATE");
    }
}
```

Note: `SshClient::new_noop()` may not exist. If it doesn't, skip the `recreate_session_rejects_non_ghost` test or create the args struct and only test the store-layer guard (steps below explain the guard occurs before SSH is touched).

- [ ] **Step 2: Run tests to confirm they fail**

```bash
cd src-tauri && cargo test service::sessions::tests -- --nocapture 2>&1 | head -20
```

Expected: compile error (structs/functions don't exist yet)

- [ ] **Step 3: Add `RecreateSessionArgs`, `DismissGhostSessionArgs`, and both service functions**

Add after `spawn_review` in `service/sessions.rs`:

```rust
#[derive(Deserialize)]
pub struct RecreateSessionArgs {
    pub session_id: i64,
}

pub async fn recreate_session(
    args: RecreateSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<SessionRow, IpcError> {
    // Load session and validate it is a ghost.
    let (sess, host) = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        let sess = s
            .get_session_by_id(args.session_id)?
            .ok_or_else(|| IpcError::new("E_NOTFOUND", "session not found"))?;
        if sess.status != "ghost" {
            return Err(IpcError::new(
                "E_INVALID_STATE",
                format!("session {} is not a ghost (status={})", sess.id, sess.status),
            ));
        }
        let host = s
            .get_host_row(&sess.host_alias)?
            .ok_or_else(|| IpcError::new("E_NOTFOUND", "host not found"))?;
        if !host.reachable {
            return Err(IpcError::new(
                "E_HOST_OFFLINE",
                format!("host {} is not reachable", host.alias),
            ));
        }
        (sess, host)
    };

    // Create the bare tmux session on the host.
    let tmux = exec_for(&host.alias, ssh);
    tmux.bare_new_session(&sess.tmux_name).await?;

    // Restore the DB row and return it.
    let row = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?
        .restore_session(sess.id)?
        .ok_or_else(|| IpcError::new("E_INTERNAL", "session vanished after restore"))?;
    Ok(row)
}

#[derive(Deserialize)]
pub struct DismissGhostSessionArgs {
    pub session_id: i64,
}

pub fn dismiss_ghost_session(
    args: DismissGhostSessionArgs,
    store: &Mutex<Store>,
) -> Result<(), IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let sess = s
        .get_session_by_id(args.session_id)?
        .ok_or_else(|| IpcError::new("E_NOTFOUND", "session not found"))?;
    if sess.status != "ghost" {
        return Err(IpcError::new(
            "E_INVALID_STATE",
            format!("session {} is not a ghost (status={})", sess.id, sess.status),
        ));
    }
    s.delete_session(sess.id)?;
    Ok(())
}
```

`store.get_host_row` does not yet exist — `get_host` is private and `list_hosts` returns all hosts. Add this public method to `Store` in `store.rs` (alongside `get_session_by_id`):

```rust
pub fn get_host_row(&self, alias: &str) -> Result<Option<HostRow>, rusqlite::Error> {
    fetch_host(&self.conn, alias)
}
```

Add the call `git add src-tauri/src/store.rs` to the Task 6 commit.

- [ ] **Step 4: Run the tests**

```bash
cd src-tauri && cargo test service::sessions::tests -- --nocapture
```

Expected: PASS (or adjust if `SshClient::new_noop()` doesn't exist — just remove the SSH test)

- [ ] **Step 5: Run all backend tests**

```bash
cd src-tauri && cargo test -- --nocapture 2>&1 | tail -20
```

Expected: all pass

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/store.rs src-tauri/src/service/sessions.rs
git commit -m "feat(service): add recreate_session and dismiss_ghost_session"
```

---

## Task 7: Command wrappers and `lib.rs` registration

**Files:**
- Modify: `src-tauri/src/commands/sessions.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add command wrappers to `commands/sessions.rs`**

Add these two imports to the existing use block at the top:

```rust
use crate::service::sessions::{
    self, DismissGhostSessionArgs, KillSessionArgs, NewSessionArgs, RecreateSessionArgs,
    RelatedSessionsArgs, RenameSessionArgs, RestartSessionArgs, SendPromptArgs, SpawnReviewArgs,
};
```

Then add after `spawn_review`:

```rust
#[tauri::command]
pub async fn recreate_session(
    args: RecreateSessionArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    sessions::recreate_session(args, &store, &ssh).await
}

#[tauri::command]
pub fn dismiss_ghost_session(
    args: DismissGhostSessionArgs,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<(), IpcError> {
    sessions::dismiss_ghost_session(args, &store)
}
```

- [ ] **Step 2: Register commands in `lib.rs`**

In `lib.rs` (line ~293), add to the `invoke_handler` list after `commands::sessions::spawn_review`:

```rust
commands::sessions::recreate_session,
commands::sessions::dismiss_ghost_session,
```

- [ ] **Step 3: Verify compile**

```bash
cd src-tauri && cargo check 2>&1 | grep "error" | head -20
```

Expected: no errors

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/commands/sessions.rs src-tauri/src/lib.rs
git commit -m "feat(commands): expose recreate_session and dismiss_ghost_session"
```

---

## Task 8: Frontend — `sessions.ts`

**Files:**
- Modify: `src/lib/sessions.ts`

- [ ] **Step 1: Add `lost_at` to `SessionRow` interface**

In `sessions.ts`, update the `SessionRow` interface:

```typescript
export interface SessionRow {
  id: number;
  tmux_name: string;
  host_alias: string;
  project_id: number | null;
  worktree_id: number | null;
  created_at: number;
  last_activity_at: number;
  status: string;
  notes: string | null;
  account_uuid: string | null;
  kind: string;
  reviews_session_id: number | null;
  worktree_key: string | null;
  lost_at: number | null;
}
```

- [ ] **Step 2: Add `recreateSession` and `dismissGhostSession` functions**

Add after `spawnReview`:

```typescript
export async function recreateSession(sessionId: number): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('recreate_session', {
    args: { session_id: sessionId },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

export async function dismissGhostSession(sessionId: number): Promise<Result<void>> {
  const r = await invokeCmd<void>('dismiss_ghost_session', {
    args: { session_id: sessionId },
  });
  if (r.ok) removeSession(sessionId);
  return r;
}
```

- [ ] **Step 3: Run frontend type-check**

```bash
pnpm check 2>&1 | tail -20
```

Expected: no errors

- [ ] **Step 4: Run frontend tests**

```bash
pnpm test 2>&1 | tail -20
```

Expected: no new failures (pre-existing `localStorage is undefined` failures are expected; they're documented in `CLAUDE.md`)

- [ ] **Step 5: Commit**

```bash
git add src/lib/sessions.ts
git commit -m "feat(frontend): add recreateSession and dismissGhostSession to sessions store"
```

---

## Task 9: Frontend — `Sidebar.svelte` ghost card

**Files:**
- Modify: `src/lib/Sidebar.svelte`

- [ ] **Step 1: Add imports and handlers**

In `Sidebar.svelte`, update the sessions import (line ~5):

```typescript
import {
  sessions,
  loadSessions,
  killSession,
  renameSession,
  restartSession,
  recreateSession,
  dismissGhostSession,
  type SessionRow,
} from './sessions';
```

Add import for `hostByAlias` (line ~18, after the existing hosts import):

```typescript
import { hosts, hostFilter, hostByAlias } from './hosts';
```

Add these two handler functions in the `<script>` block (after `cancelKill`):

```typescript
async function doRecreate(sess: SessionRow, e?: Event) {
  e?.stopPropagation();
  actionError = null;
  const r = await recreateSession(sess.id);
  if (!r.ok) actionError = r.error.message;
}

async function doDismissGhost(sess: SessionRow, e?: Event) {
  e?.stopPropagation();
  actionError = null;
  const r = await dismissGhostSession(sess.id);
  if (!r.ok) actionError = r.error.message;
}
```

Add a helper derived value (alongside the existing `relatedCountById` derivations):

```typescript
function hostIsReachable(alias: string): boolean {
  return $hostByAlias.get(alias)?.reachable ?? false;
}
```

- [ ] **Step 2: Update the `sessionRow` snippet to handle ghost status**

Replace the content of the `{:else}` branch inside `{#snippet sessionRow}` (the block starting at line ~388 with `<span class="status-dot...`) with:

```svelte
{#if sess.status === 'ghost'}
  <span class="status-dot status-ghost" title="ghost — session lost" aria-hidden="true"></span>
  <span class="host-badge" data-testid="host-badge">[{sess.host_alias}]</span>
  <span class="sess-name">{sess.tmux_name}</span>
  {#if sess.lost_at}
    <span class="lost-at" title="Lost at {new Date(sess.lost_at * 1000).toLocaleString()}">
      lost {timeAgo(sess.lost_at)}
    </span>
  {/if}
  <div class="row-actions">
    <button
      class="icon-btn small"
      onclick={(e) => doRecreate(sess, e)}
      disabled={!hostIsReachable(sess.host_alias)}
      title={hostIsReachable(sess.host_alias) ? 'Recreate tmux session' : 'Host is offline'}
      aria-label="Recreate"
    >↺</button>
    <button
      class="icon-btn small danger"
      onclick={(e) => doDismissGhost(sess, e)}
      title="Dismiss ghost session"
      aria-label="Dismiss"
    >×</button>
  </div>
{:else}
  <span class="status-dot status-{sess.status}" title={sess.status} aria-hidden="true"></span>
  {#if relatedCountFor(sess) > 0}
    <span
      class="related-badge"
      data-testid="related-badge"
      role="img"
      title="{relatedCountFor(sess)} related session(s)"
      aria-label="{relatedCountFor(sess)} related sessions"
    >🔗{relatedCountFor(sess)}</span>
  {/if}
  {#if sess.kind === 'review'}
    <span class="review-badge" role="img" title="review session" aria-label="review session">🔍</span>
  {/if}
  {#if sess.kind === 'shell'}
    <span class="shell-badge" title="shell session">▶</span>
  {/if}
  <span class="host-badge" data-testid="host-badge">[{sess.host_alias}]</span>
  <span class="sess-name">{sess.tmux_name}</span>
  <div class="row-actions">
    <button class="icon-btn small" onclick={(e) => doRestart(sess, e)} title="Restart claude in this session" aria-label="Restart">↻</button>
    <button class="icon-btn small" onclick={(e) => beginRename(sess, e)} title="Rename session" aria-label="Rename">✎</button>
    <button class="icon-btn small danger" onclick={(e) => askKill(sess, e)} title="Kill session" aria-label="Kill">×</button>
  </div>
{/if}
```

- [ ] **Step 3: Add `timeAgo` helper function in the script block**

Add in the `<script>` block (e.g., after the `hostIsReachable` helper):

```typescript
function timeAgo(unixSecs: number): string {
  const diffMs = Date.now() - unixSecs * 1000;
  const diffMins = Math.floor(diffMs / 60_000);
  if (diffMins < 1) return 'just now';
  if (diffMins < 60) return `${diffMins}m ago`;
  const diffHours = Math.floor(diffMins / 60);
  if (diffHours < 24) return `${diffHours}h ago`;
  const diffDays = Math.floor(diffHours / 24);
  return `${diffDays}d ago`;
}
```

- [ ] **Step 4: Add CSS for ghost status**

In the `<style>` block, after `.status-dot.status-orphan`:

```css
  .status-dot.status-ghost { background: rgb(160, 120, 200); opacity: 0.55; }
  .lost-at {
    font-size: 0.7em;
    opacity: 0.6;
    margin-left: auto;
    padding-right: 0.25rem;
    white-space: nowrap;
  }
```

- [ ] **Step 5: Run type-check**

```bash
pnpm check 2>&1 | tail -20
```

Expected: no errors

- [ ] **Step 6: Run frontend tests**

```bash
pnpm test 2>&1 | tail -20
```

Expected: no new failures

- [ ] **Step 7: Commit**

```bash
git add src/lib/Sidebar.svelte
git commit -m "feat(ui): ghost session card with Recreate and Dismiss actions"
```

---

## Task 10: Final verification

- [ ] **Step 1: Run full backend test suite**

```bash
cd src-tauri && cargo test -- --nocapture 2>&1 | tail -30
```

Expected: all tests pass

- [ ] **Step 2: Run clippy**

```bash
cd src-tauri && cargo clippy --all-targets -- -D warnings 2>&1 | head -30
```

Expected: no warnings

- [ ] **Step 3: Run frontend tests**

```bash
pnpm test 2>&1 | tail -20
```

Expected: no new failures vs baseline

- [ ] **Step 4: Final commit if anything was touched**

```bash
git add -p
git commit -m "chore: clippy and test fixes for ghost sessions"
```
