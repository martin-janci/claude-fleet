# Cross-host session memory + prompt transfer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cache each session's claude account on creation, surface "related sessions" cross-host via SessionDetails panel + sidebar 🔗N badge, and let the user send plain-text prompts to any reachable target session via `tmux send-keys -l`.

**Architecture:** Migration 004 adds `sessions.account_uuid` (nullable FK to accounts). Reconciliation captures the host's current `account_uuid` ONLY for newly-discovered sessions (preserves the value for already-known rows so a user-driven host re-auth doesn't rewrite history). Frontend computes related-session lists from the existing `$sessions` store via `$derived` — no extra IPC for the badge/panel. The new `send_prompt` Tauri command issues two tmux `send-keys` invocations (one with `-l` for the literal text, one for `Enter`) routed locally or through SSH depending on host_alias. A new `PromptComposer.svelte` modal drives multi-target sends sequentially with per-target inline error surfacing.

**Tech Stack:** Rust + Tauri 2 backend, Svelte 5 (runes) frontend, SQLite via rusqlite, existing portable-pty + SshClient infrastructure from iter 1.

**Spec:** `docs/specs/2026-05-20-cross-host-sessions-and-transfer-design.md`

---

## File Structure

**Created:**
- `src-tauri/migrations/004_session_account.sql` — schema change (sessions.account_uuid)
- `src/lib/PromptComposer.svelte` — modal: targets picker + textarea + Send
- `src/lib/PromptComposer.test.ts` — 4 component tests

**Modified:**
- `src-tauri/src/store.rs` — `SessionRow.account_uuid` field; `list_sessions_for_host` SELECT widening; `upsert_session` signature gains `account_uuid: Option<&str>`; `get_session_account` helper; `list_related_sessions` helper; `migrate()` applies 004; 5 new tests
- `src-tauri/src/commands/health.rs` — bump expected `schema_version` 3 → 4
- `src-tauri/src/commands/sessions.rs` — `reconcile_sessions` passes preserved-or-current account_uuid to upsert; `related_sessions` Tauri command; `send_prompt` Tauri command + helpers; `build_send_commands` + `shell_quote_str` pure helper; 4 new tests
- `src-tauri/src/lib.rs` — register `related_sessions` + `send_prompt` invoke handlers
- `src/lib/sessions.ts` — `SessionRow` interface gains `account_uuid: string | null`; `relatedSessions(sessionId)` + `sendPrompt(host, name, prompt)` wrappers
- `vitest.setup.ts` — mocks for `related_sessions` + `send_prompt`; existing `list_sessions` fixtures include the new field
- `src/lib/Sidebar.svelte` — derive `relatedCountFor(sess)`; insert 🔗N badge in session rows (project group + orphan); CSS for `.related-badge`
- `src/lib/Sidebar.test.ts` — 2 new tests (badge visible / hidden)
- `src/lib/SessionDetails.svelte` — derive related list; render Related sessions panel; CSS; `→ Send prompt` button opens PromptComposer
- `src/lib/SessionDetails.test.ts` — 3 new tests (panel renders / hidden when alone / orphans hidden)

---

## Task 1: Migration 004 + SessionRow + schema bump

**Files:**
- Create: `src-tauri/migrations/004_session_account.sql`
- Modify: `src-tauri/src/store.rs` (migrate + SessionRow + upsert_session + list_sessions_for_host + tests)
- Modify: `src-tauri/src/commands/health.rs` (assertion 3 → 4)

- [ ] **Step 1: Write the failing tests in `src-tauri/src/store.rs` `mod tests`**

Append inside the existing `mod tests` block:

```rust
    #[test]
    fn migration_004_adds_account_uuid_column_to_sessions() {
        let s = Store::open_in_memory().expect("open");
        let mut stmt = s
            .conn
            .prepare("SELECT name FROM pragma_table_info('sessions')")
            .unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.iter().any(|c| c == "account_uuid"),
            "expected `account_uuid` column on sessions; got: {cols:?}"
        );
    }

    #[test]
    fn schema_version_is_four_after_migration() {
        let s = Store::open_in_memory().expect("open");
        assert_eq!(s.schema_version().expect("version"), 4);
    }
```

Update existing `migrate_is_idempotent` to assert `== 4`:

```rust
    #[test]
    fn migrate_is_idempotent() {
        let store = Store::open_in_memory().expect("open");
        store.migrate().expect("re-migrate");
        assert_eq!(store.schema_version().expect("version"), 4);
    }
```

If a `schema_version_is_three_after_migration` test exists (it was added during iter 2), rename it to `schema_version_is_four_after_migration` and update the assertion to `4`.

- [ ] **Step 2: Run tests, expect failures**

```bash
cd /Users/martinjanci/projects/github.com/martin-janci/claude-fleet
cargo test --manifest-path src-tauri/Cargo.toml --lib store::tests 2>&1 | tail -20
```

Expected: at least 3 failures (`migration_004_adds_account_uuid_column_to_sessions`, `schema_version_is_four_after_migration`, `migrate_is_idempotent`).

- [ ] **Step 3: Create `src-tauri/migrations/004_session_account.sql`**

```sql
-- Migration 004: cached account binding per session.
-- Step 3 of the multi-host iteration (cross-host session memory). When a
-- new tmux session is discovered, we capture the host's current account_uuid
-- on the session row. Existing rows are NOT rewritten on re-probe — the
-- preservation invariant lets the UI show the account a session was
-- originally created under even if the host later re-auths.

ALTER TABLE sessions ADD COLUMN account_uuid TEXT REFERENCES accounts(uuid);

INSERT OR IGNORE INTO schema_version (version) VALUES (4);
```

- [ ] **Step 4: Extend `migrate()` in `src-tauri/src/store.rs`**

Find the existing `migrate` body and replace it:

```rust
    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        self.conn
            .execute_batch(include_str!("../migrations/001_init.sql"))?;
        let v: i64 = self
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap_or(0);
        if v < 2 {
            self.conn
                .execute_batch(include_str!("../migrations/002_hosts_ssh.sql"))?;
        }
        if v < 3 {
            self.conn
                .execute_batch(include_str!("../migrations/003_accounts.sql"))?;
        }
        if v < 4 {
            self.conn
                .execute_batch(include_str!("../migrations/004_session_account.sql"))?;
        }
        Ok(())
    }
```

- [ ] **Step 5: Extend `SessionRow` struct in `src-tauri/src/store.rs`**

Find the existing `SessionRow` struct and add `account_uuid`:

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
}
```

- [ ] **Step 6: Extend `upsert_session` signature + SQL in `src-tauri/src/store.rs`**

Find the existing `upsert_session` function. Add the new arg and update the INSERT statement:

```rust
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_session(
        &self,
        tmux_name: &str,
        host_alias: &str,
        project_id: Option<i64>,
        worktree_id: Option<i64>,
        created_at: i64,
        last_activity_at: i64,
        status: &str,
        account_uuid: Option<&str>,
    ) -> Result<i64, rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO sessions (tmux_name, host_alias, project_id, worktree_id,
                                   created_at, last_activity_at, status, account_uuid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(host_alias, tmux_name) DO UPDATE SET
               project_id=excluded.project_id,
               worktree_id=excluded.worktree_id,
               last_activity_at=excluded.last_activity_at,
               status=excluded.status,
               account_uuid=excluded.account_uuid",
            rusqlite::params![tmux_name, host_alias, project_id, worktree_id,
                              created_at, last_activity_at, status, account_uuid],
        )?;
        self.conn.query_row(
            "SELECT id FROM sessions WHERE host_alias=?1 AND tmux_name=?2",
            rusqlite::params![host_alias, tmux_name],
            |row| row.get(0),
        )
    }
```

The conflict-update DOES rewrite `account_uuid` — preservation is the CALLER's responsibility (Task 2 will pass the preserved value when applicable).

- [ ] **Step 7: Update `list_sessions_for_host` SELECT in `src-tauri/src/store.rs`**

Find the existing `list_sessions_for_host` function and widen the SELECT + row mapping:

```rust
    pub fn list_sessions_for_host(
        &self,
        host_alias: &str,
    ) -> Result<Vec<SessionRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
                    last_activity_at, status, notes, account_uuid
             FROM sessions WHERE host_alias=?1 ORDER BY last_activity_at DESC",
        )?;
        let rows = stmt.query_map(rusqlite::params![host_alias], |row| {
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
            })
        })?;
        rows.collect()
    }
```

- [ ] **Step 8: Patch existing `upsert_session` call sites in `store.rs` tests**

The existing tests in `mod tests` call `upsert_session("dev-a", "local", None, None, 1, 1, "running")` — these now need a trailing `None` arg. Find every test that calls `upsert_session` and add `None` as the 8th arg.

Use:

```bash
grep -n "upsert_session" src-tauri/src/store.rs
```

For each call site inside `mod tests`, append `, None` before the closing `)`. Example fix:

Before: `s.upsert_session("dev-foo", "local", None, None, 1000, 2000, "running")`
After:  `s.upsert_session("dev-foo", "local", None, None, 1000, 2000, "running", None)`

- [ ] **Step 9: Update `src-tauri/src/commands/health.rs` test expectation**

Find:

```rust
        assert_eq!(h.schema_version, 3);
```

Change to:

```rust
        assert_eq!(h.schema_version, 4);
```

- [ ] **Step 10: Run all store + health tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib store:: 2>&1 | tail -15
cargo test --manifest-path src-tauri/Cargo.toml --lib commands::health:: 2>&1 | tail -5
```

Expected: all pass.

- [ ] **Step 11: Patch any other in-tree callers of `upsert_session`**

```bash
grep -rn "upsert_session" src-tauri/src --include='*.rs'
```

Check `src-tauri/src/commands/sessions.rs` — the `reconcile_sessions` function calls `upsert_session`. Add a trailing `None` arg as a TEMPORARY placeholder (Task 2 will replace this with the proper preserved/captured value):

Find the existing call (look for `s.upsert_session(`) and append `, None` to its arg list. There should be exactly one call site in commands/sessions.rs.

- [ ] **Step 12: Run full lib suite**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -5
```

Expected: `test result: ok. NN passed; 0 failed` where NN is the previous count + 2 (the two new schema tests).

- [ ] **Step 13: Commit**

```bash
git add src-tauri/migrations/004_session_account.sql src-tauri/src/store.rs src-tauri/src/commands/health.rs src-tauri/src/commands/sessions.rs
git commit -m "store: migration 004 (sessions.account_uuid) + SessionRow update"
```

---

## Task 2: Reconciliation captures host account on new sessions (preserves existing)

**Files:**
- Modify: `src-tauri/src/store.rs` — add `get_session_account` helper + test
- Modify: `src-tauri/src/commands/sessions.rs` — `reconcile_sessions` reads existing account_uuid before upsert; tests

- [ ] **Step 1: Add failing test in `src-tauri/src/store.rs` `mod tests`**

```rust
    #[test]
    fn get_session_account_returns_none_for_missing_then_some_after_upsert() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        // No session yet → None
        assert!(s.get_session_account("h", "dev-foo").unwrap().is_none());
        // Upsert with an account uuid
        s.upsert_account(&AccountRow {
            uuid: "u1".into(),
            email: None,
            display_name: None,
            organization_name: None,
            organization_uuid: None,
            seat_tier: None,
            last_seen_at: None,
        })
        .unwrap();
        s.upsert_session("dev-foo", "h", None, None, 1, 1, "running", Some("u1"))
            .unwrap();
        assert_eq!(s.get_session_account("h", "dev-foo").unwrap().as_deref(), Some("u1"));
    }
```

- [ ] **Step 2: Run test, expect compile failure** (`get_session_account` doesn't exist)

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib store::tests::get_session_account 2>&1 | tail -5
```

- [ ] **Step 3: Add `get_session_account` helper to `impl Store`**

Insert near the other session helpers in `src-tauri/src/store.rs`:

```rust
    pub fn get_session_account(
        &self,
        host_alias: &str,
        tmux_name: &str,
    ) -> Result<Option<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT account_uuid FROM sessions WHERE host_alias=?1 AND tmux_name=?2",
        )?;
        let mut rows = stmt.query_map(
            rusqlite::params![host_alias, tmux_name],
            |row| row.get::<_, Option<String>>(0),
        )?;
        match rows.next() {
            Some(r) => Ok(r?),
            None => Ok(None),
        }
    }
```

- [ ] **Step 4: Run the test, verify passing**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib store::tests::get_session_account 2>&1 | tail -5
```

Expected: PASS.

- [ ] **Step 5: Update `reconcile_sessions` in `src-tauri/src/commands/sessions.rs` to preserve existing account_uuid**

Find the per-session loop inside `reconcile_sessions`:

```rust
        for sess in &live {
            keep.push(sess.name.clone());
            let project_id = find_project_id_for_path(s, &host.alias, &sess.path);
            s.upsert_session(
                &sess.name,
                &host.alias,
                project_id,
                None,
                sess.created,
                sess.last_activity,
                "running",
                None,  // placeholder from Task 1
            )?;
            if let Some(pid) = project_id {
                s.touch_project_last_session_at(pid, sess.last_activity)?;
            }
        }
```

Replace the `None` with preservation logic:

```rust
        for sess in &live {
            keep.push(sess.name.clone());
            let project_id = find_project_id_for_path(s, &host.alias, &sess.path);
            // Preservation invariant: if the session already has an
            // account_uuid in the DB, keep it; only capture the host's
            // current account for newly-discovered sessions.
            let account_uuid = s
                .get_session_account(&host.alias, &sess.name)?
                .or_else(|| host.account_uuid.clone());
            s.upsert_session(
                &sess.name,
                &host.alias,
                project_id,
                None,
                sess.created,
                sess.last_activity,
                "running",
                account_uuid.as_deref(),
            )?;
            if let Some(pid) = project_id {
                s.touch_project_last_session_at(pid, sess.last_activity)?;
            }
        }
```

- [ ] **Step 6: Verify prune-then-upsert order in `reconcile_sessions`**

Read the function body. Confirm the order is:
1. `tmux.list_sessions()` (get live names)
2. Inside loop: upsert each live session
3. `s.delete_sessions_not_in(&host.alias, &keep)` AFTER the loop

That's current behavior. The kill-then-recreate-same-name edge case noted in the spec means: if a user kills a session and creates a new one with the same name BEFORE another reconcile runs, the upsert hits an existing DB row and preserves its account_uuid. Acceptable for iter 3 — document this as a known minor quirk in the commit message.

- [ ] **Step 7: Add an integration-style test in `src-tauri/src/commands/sessions.rs`**

This task can't easily mock `TmuxExec`, so instead test the preservation logic at the store level. Append in the existing `#[cfg(test)] mod tests` block in `commands/sessions.rs`:

```rust
    #[test]
    fn upsert_session_preserves_account_uuid_when_passed_existing_value() {
        use crate::store::{AccountRow, Store};
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        s.upsert_account(&AccountRow {
            uuid: "u1".into(), email: None, display_name: None,
            organization_name: None, organization_uuid: None,
            seat_tier: None, last_seen_at: None,
        }).unwrap();
        // First reconcile captures host's account
        s.upsert_session("dev-a", "h", None, None, 1, 100, "running", Some("u1")).unwrap();
        // Host re-auths into a different account
        s.upsert_account(&AccountRow {
            uuid: "u2".into(), email: None, display_name: None,
            organization_name: None, organization_uuid: None,
            seat_tier: None, last_seen_at: None,
        }).unwrap();
        // Second reconcile: caller reads existing account before upsert
        let preserved = s.get_session_account("h", "dev-a").unwrap();
        s.upsert_session(
            "dev-a", "h", None, None, 1, 200, "running",
            preserved.as_deref(),  // u1
        ).unwrap();
        // Verify session kept the ORIGINAL account
        assert_eq!(s.get_session_account("h", "dev-a").unwrap().as_deref(), Some("u1"));
    }

    #[test]
    fn upsert_session_captures_new_account_for_fresh_row() {
        use crate::store::{AccountRow, Store};
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        s.upsert_account(&AccountRow {
            uuid: "u1".into(), email: None, display_name: None,
            organization_name: None, organization_uuid: None,
            seat_tier: None, last_seen_at: None,
        }).unwrap();
        // Brand new session — no existing row
        assert!(s.get_session_account("h", "dev-new").unwrap().is_none());
        let preserved = s.get_session_account("h", "dev-new").unwrap();
        let account = preserved.or(Some("u1".to_string()));
        s.upsert_session(
            "dev-new", "h", None, None, 1, 100, "running",
            account.as_deref(),
        ).unwrap();
        assert_eq!(s.get_session_account("h", "dev-new").unwrap().as_deref(), Some("u1"));
    }
```

- [ ] **Step 8: Run all tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -8
```

Expected: all pass + 3 new tests (1 in store::tests, 2 in commands::sessions::tests).

- [ ] **Step 9: Commit**

```bash
git add src-tauri/src/store.rs src-tauri/src/commands/sessions.rs
git commit -m "sessions: reconcile captures host account on new sessions (preserves existing)"
```

---

## Task 3: related_sessions Tauri command + frontend wrapper

**Files:**
- Modify: `src-tauri/src/store.rs` — add `list_related_sessions` helper + 3 tests
- Modify: `src-tauri/src/commands/sessions.rs` — `related_sessions` Tauri command
- Modify: `src-tauri/src/lib.rs` — register the new command
- Modify: `src/lib/sessions.ts` — `SessionRow.account_uuid` + `relatedSessions` wrapper
- Modify: `vitest.setup.ts` — mock `related_sessions` and update `list_sessions` fixture

- [ ] **Step 1: Add failing test in `src-tauri/src/store.rs`**

```rust
    #[test]
    fn list_related_sessions_returns_siblings_with_same_project_and_worktree() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_host("mefistos").unwrap();
        // Three sessions: A and B share (proj=1, wt=10); C is on a different worktree.
        let a = s.upsert_session("dev-a", "local", Some(1), Some(10), 1, 1, "running", None).unwrap();
        let _b = s.upsert_session("dev-b", "mefistos", Some(1), Some(10), 1, 1, "running", None).unwrap();
        let _c = s.upsert_session("dev-c", "local", Some(1), Some(20), 1, 1, "running", None).unwrap();
        let related = s.list_related_sessions(a).unwrap();
        assert_eq!(related.len(), 1, "expected only dev-b as related; got: {:?}", related.iter().map(|r| &r.tmux_name).collect::<Vec<_>>());
        assert_eq!(related[0].tmux_name, "dev-b");
    }

    #[test]
    fn list_related_sessions_matches_null_worktree() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let a = s.upsert_session("dev-a", "h", Some(1), None, 1, 1, "running", None).unwrap();
        let _b = s.upsert_session("dev-b", "h", Some(1), None, 1, 1, "running", None).unwrap();
        let _c = s.upsert_session("dev-c", "h", Some(1), Some(10), 1, 1, "running", None).unwrap();
        let related = s.list_related_sessions(a).unwrap();
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].tmux_name, "dev-b");
    }

    #[test]
    fn list_related_sessions_excludes_orphans() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let a = s.upsert_session("dev-a", "h", None, None, 1, 1, "running", None).unwrap();
        let _b = s.upsert_session("dev-b", "h", None, None, 1, 1, "running", None).unwrap();
        // Source has project_id=None → no relateds (orphans are not grouped).
        let related = s.list_related_sessions(a).unwrap();
        assert!(related.is_empty(), "orphans should not match each other; got: {:?}", related.iter().map(|r| &r.tmux_name).collect::<Vec<_>>());
    }
```

- [ ] **Step 2: Run tests, expect compile errors**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib store::tests::list_related 2>&1 | tail -10
```

- [ ] **Step 3: Add `list_related_sessions` to `impl Store`**

In `src-tauri/src/store.rs`, near the other session helpers:

```rust
    pub fn list_related_sessions(
        &self,
        session_id: i64,
    ) -> Result<Vec<SessionRow>, rusqlite::Error> {
        // Look up source's (project_id, worktree_id) first.
        let (proj, wt): (Option<i64>, Option<i64>) = self.conn.query_row(
            "SELECT project_id, worktree_id FROM sessions WHERE id=?1",
            rusqlite::params![session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        // Orphans (project_id=NULL) have no relateds — they share no identity.
        let Some(project_id) = proj else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare(
            "SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
                    last_activity_at, status, notes, account_uuid
             FROM sessions
             WHERE project_id=?1
               AND ((?2 IS NULL AND worktree_id IS NULL) OR worktree_id=?2)
               AND id<>?3
             ORDER BY host_alias ASC, tmux_name ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![project_id, wt, session_id], |row| {
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
            })
        })?;
        rows.collect()
    }
```

- [ ] **Step 4: Run store tests, verify passing**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib store::tests::list_related 2>&1 | tail -10
```

Expected: 3 passes.

- [ ] **Step 5: Add `related_sessions` Tauri command in `src-tauri/src/commands/sessions.rs`**

Insert near the existing session commands:

```rust
#[derive(Deserialize)]
pub struct RelatedSessionsArgs {
    pub session_id: i64,
}

#[tauri::command]
pub fn related_sessions(
    args: RelatedSessionsArgs,
    store: State<'_, Mutex<Store>>,
) -> Result<Vec<SessionRow>, IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.list_related_sessions(args.session_id).map_err(IpcError::from)
}
```

- [ ] **Step 6: Register `related_sessions` in `src-tauri/src/lib.rs`**

Find the `generate_handler![...]` block. Add `commands::sessions::related_sessions,` adjacent to the existing `commands::sessions::*` entries.

- [ ] **Step 7: Run full Rust suite**

```bash
cargo build --manifest-path src-tauri/Cargo.toml 2>&1 | tail -5
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -5
```

Expected: clean build + all tests pass.

- [ ] **Step 8: Update `src/lib/sessions.ts`**

Add `account_uuid: string | null` to `SessionRow` interface and add the new wrapper:

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
}

// ... existing functions ...

export async function relatedSessions(sessionId: number): Promise<Result<SessionRow[]>> {
  return invokeCmd<SessionRow[]>('related_sessions', { args: { session_id: sessionId } });
}
```

- [ ] **Step 9: Update `vitest.setup.ts`**

Find the existing `if (cmd === 'list_sessions') return [];` mock and (since some tests may rely on a fixture) add `account_uuid: null` to any inline session-shaped objects already in the file.

Append the new mock before the final `return null`:

```ts
    if (cmd === 'related_sessions') return [];
```

- [ ] **Step 10: Patch inline session fixtures across `src/lib/*.test.ts`**

Run:

```bash
grep -rnE "tmux_name:.*host_alias" src/lib/*.test.ts | head -20
```

For each session-shaped literal that uses the `SessionRow` shape, add `account_uuid: null` after `notes: ...`. Typical fixtures live in `Sidebar.test.ts`, `SessionDetails.test.ts`, `sessions.test.ts`, `NewSessionDialog.test.ts`. If `pnpm vitest run` reports TS errors after the SessionRow widening, the missing field will be flagged at compile.

- [ ] **Step 11: Run frontend tests**

```bash
pnpm vitest run 2>&1 | tail -8
```

Expected: green (any failures are fixture gaps from Step 10 — patch them).

- [ ] **Step 12: Commit**

```bash
git add src-tauri/src/store.rs src-tauri/src/commands/sessions.rs src-tauri/src/lib.rs src/lib/sessions.ts vitest.setup.ts $(git diff --name-only -- src/lib/ | grep .test.ts)
git commit -m "sessions: related_sessions command + frontend wrapper"
```

---

## Task 4: Sidebar 🔗N related-count badge

**Files:**
- Modify: `src/lib/Sidebar.svelte` — derive `relatedCountFor`; insert badge in two session-row spots; CSS
- Modify: `src/lib/Sidebar.test.ts` — 2 new tests

- [ ] **Step 1: Add `relatedCountFor` helper in `src/lib/Sidebar.svelte`**

Read the existing component first to find the script block structure. Below the existing `$derived` declarations and before the template, add:

```ts
  function relatedCountFor(sess: SessionRow): number {
    if (sess.project_id === null) return 0;
    return $sessions.filter(
      (s) =>
        s.id !== sess.id &&
        s.project_id === sess.project_id &&
        s.worktree_id === sess.worktree_id,
    ).length;
  }
```

- [ ] **Step 2: Insert the badge in the project-grouped session row**

Find the existing session-row template that has `<span class="status-dot ...">` and the `[host]` badge from iter 1. Insert the 🔗N badge between the status dot and the host badge:

```svelte
                    <span class="status-dot status-{sess.status}" ...></span>
                    {#if relatedCountFor(sess) > 0}
                      <span
                        class="related-badge"
                        data-testid="related-badge"
                        title="{relatedCountFor(sess)} related session(s)"
                      >🔗{relatedCountFor(sess)}</span>
                    {/if}
                    <span class="host-badge" ...>[{sess.host_alias}]</span>
```

Apply the SAME insertion in the orphan-section session-row loop later in the file.

- [ ] **Step 3: Add CSS for `.related-badge` in `src/lib/Sidebar.svelte` `<style>` block**

```css
  .related-badge {
    font-size: 0.65rem;
    color: var(--fg-muted);
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    padding: 0.05rem 0.3rem;
    border-radius: 3px;
    flex-shrink: 0;
  }
```

- [ ] **Step 4: Add 2 tests in `src/lib/Sidebar.test.ts`**

In the existing `describe('Sidebar (sessions-grouped view)')` block, append:

```typescript
  it('renders 🔗N badge for sessions with related siblings', async () => {
    const a = sessionFor(1, 'dev-a');
    a.worktree_id = 10;
    const b = sessionFor(1, 'dev-b');
    b.worktree_id = 10;
    mockBackend(fakeProjects, [a, b]);
    render(Sidebar);
    for (let i = 0; i < 8; i++) await tick();
    const badges = screen.queryAllByTestId('related-badge');
    expect(badges).toHaveLength(2); // each session sees one sibling
    expect(badges[0].textContent).toContain('1');
  });

  it('omits 🔗 badge for solo sessions', async () => {
    const solo = sessionFor(1, 'dev-solo');
    solo.worktree_id = 10;
    mockBackend(fakeProjects, [solo]);
    render(Sidebar);
    for (let i = 0; i < 8; i++) await tick();
    expect(screen.queryAllByTestId('related-badge')).toHaveLength(0);
  });
```

If `sessionFor` doesn't currently default `worktree_id: null`, check the helper at the top of `Sidebar.test.ts` and confirm it sets `worktree_id: null` initially; the tests above mutate it before passing to `mockBackend`.

- [ ] **Step 5: Run Sidebar tests**

```bash
pnpm vitest run src/lib/Sidebar.test.ts 2>&1 | tail -10
```

Expected: all sidebar tests pass.

- [ ] **Step 6: Full vitest sweep**

```bash
pnpm vitest run 2>&1 | tail -5
```

Expected: green (count increased by 2).

- [ ] **Step 7: Commit**

```bash
git add src/lib/Sidebar.svelte src/lib/Sidebar.test.ts
git commit -m "Sidebar: 🔗N related-count badge per session row"
```

---

## Task 5: SessionDetails Related sessions panel

**Files:**
- Modify: `src/lib/SessionDetails.svelte` — derive related list; render panel; CSS
- Modify: `src/lib/SessionDetails.test.ts` — 3 new tests

- [ ] **Step 1: Update `src/lib/SessionDetails.svelte` imports + derived**

Read the existing file first to find the script block layout. Make sure `sessions` and `selectSession` are imported:

```ts
  import { sessions, type SessionRow } from './sessions';
  import { selectSession } from './selection';
```

(Both likely already imported from earlier work; verify.)

Add a derived `related`:

```ts
  const related = $derived.by(() => {
    if (session.project_id === null) return [];
    return $sessions.filter(
      (s) =>
        s.id !== session.id &&
        s.project_id === session.project_id &&
        s.worktree_id === session.worktree_id,
    );
  });
```

Add a helper for the account text within a related row (reuse `accountFor` from earlier in the file if defined; otherwise add):

```ts
  function accountForRow(s: SessionRow): AccountRow | null {
    if (!s.account_uuid) return null;
    return $accounts.find((a) => a.uuid === s.account_uuid) ?? null;
  }
```

(Confirm `accounts` import and `AccountRow` type are present; they should be from iter 2.)

- [ ] **Step 2: Add the panel after the existing meta-grid**

Below the existing `<dl class="meta">…</dl>` (but inside the same root `<article>`), insert:

```svelte
{#if related.length > 0}
  <section class="related" data-testid="related-sessions">
    <h3>Related sessions ({related.length})</h3>
    <ul class="related-list">
      {#each related as r (r.id)}
        <li>
          <button
            class="related-row"
            data-testid="related-row"
            onclick={() => selectSession(r)}
          >
            <span class="host-badge">[{r.host_alias}]</span>
            <span class="account">{accountText(accountForRow(r))}</span>
            <span class="status-dot status-{r.status}" title={r.status}></span>
            <span class="sess-name">{r.tmux_name}</span>
            <span class="age">{formatRelative(r.last_activity_at)}</span>
          </button>
        </li>
      {/each}
    </ul>
  </section>
{/if}
```

`accountText` and `formatRelative` should already exist in this file from iter 2; if `accountText` only handles `AccountRow | null` and `accountForRow` returns the same, the call is direct. If `accountText` was scoped to the source-session account, factor a small helper:

```ts
  function accountText(a: AccountRow | null): string {
    if (!a) return '—';
    const email = a.email ?? a.uuid;
    return a.seat_tier ? `${email} (${a.seat_tier})` : email;
  }
```

(If this helper is already defined elsewhere in the file from iter 2, REUSE it; do not duplicate.)

- [ ] **Step 3: Add CSS to `src/lib/SessionDetails.svelte` `<style>` block**

```css
  .related {
    border-top: 1px solid var(--border);
    padding-top: 0.6rem;
    margin-top: 0.6rem;
  }
  .related h3 {
    font-size: 0.7rem;
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin: 0 0 0.4rem 0;
  }
  .related-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }
  .related-row {
    width: 100%;
    text-align: left;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0.35rem 0.5rem;
    color: var(--fg);
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.82rem;
  }
  .related-row:hover {
    border-color: var(--accent);
    background: var(--bg-pane);
  }
  .related .host-badge {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.7rem;
    color: var(--fg-muted);
    border: 1px solid var(--border);
    padding: 0.05rem 0.3rem;
    border-radius: 3px;
  }
  .related .account {
    color: var(--fg-muted);
    font-size: 0.75rem;
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .related .sess-name {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.78rem;
  }
  .related .age {
    color: var(--fg-muted);
    font-size: 0.7rem;
  }
```

- [ ] **Step 4: Add 3 tests in `src/lib/SessionDetails.test.ts`**

Inside the existing `describe('SessionDetails')` block, append:

```typescript
  it('shows Related sessions panel when siblings exist', async () => {
    const source = { ...sampleSession, id: 1, project_id: 1, worktree_id: 10 };
    const sibling = { ...sampleSession, id: 2, tmux_name: 'dev-sib', host_alias: 'mefistos', project_id: 1, worktree_id: 10 };
    hosts.set([
      { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null },
    ]);
    accounts.set([]);
    sessions.set([source, sibling]);
    render(SessionDetails, { props: { session: source } });
    await tick();
    const rows = await screen.findAllByTestId('related-row');
    expect(rows).toHaveLength(1);
    expect(rows[0].textContent).toContain('dev-sib');
  });

  it('hides Related panel when session has no siblings', async () => {
    const lone = { ...sampleSession, id: 1, project_id: 1, worktree_id: 10 };
    sessions.set([lone]);
    render(SessionDetails, { props: { session: lone } });
    await tick();
    expect(screen.queryByTestId('related-sessions')).toBeNull();
  });

  it('hides Related panel for orphan sessions (project_id=null)', async () => {
    const orphan = { ...sampleSession, id: 1, project_id: null, worktree_id: null };
    const otherOrphan = { ...sampleSession, id: 2, tmux_name: 'dev-other', project_id: null, worktree_id: null };
    sessions.set([orphan, otherOrphan]);
    render(SessionDetails, { props: { session: orphan } });
    await tick();
    expect(screen.queryByTestId('related-sessions')).toBeNull();
  });
```

Make sure `sessions` is imported at the top of `SessionDetails.test.ts`:

```ts
import { sessions } from './sessions';
```

- [ ] **Step 5: Run tests**

```bash
pnpm vitest run src/lib/SessionDetails.test.ts 2>&1 | tail -10
```

Expected: all pass.

- [ ] **Step 6: Full vitest sweep**

```bash
pnpm vitest run 2>&1 | tail -5
```

Expected: green.

- [ ] **Step 7: Commit**

```bash
git add src/lib/SessionDetails.svelte src/lib/SessionDetails.test.ts
git commit -m "SessionDetails: Related sessions panel"
```

---

## Task 6: send_prompt Tauri command

**Files:**
- Modify: `src-tauri/src/commands/sessions.rs` — `build_send_commands` helper + `send_prompt` command + 3 tests
- Modify: `src-tauri/src/lib.rs` — register `send_prompt`

- [ ] **Step 1: Add failing test for `build_send_commands`**

In the existing `#[cfg(test)] mod tests` in `src-tauri/src/commands/sessions.rs`:

```rust
    #[test]
    fn build_send_commands_emits_literal_text_then_enter() {
        let cmds = build_send_commands("dev-foo", "hello world");
        assert_eq!(cmds.len(), 2);
        assert!(cmds[0].starts_with("tmux send-keys -t "));
        assert!(cmds[0].contains(" -l "));
        assert!(cmds[0].contains("'hello world'"));
        assert!(cmds[1].ends_with(" Enter"));
    }

    #[test]
    fn build_send_commands_escapes_embedded_quotes() {
        let cmds = build_send_commands("dev-foo", "it's a test");
        // shell_quote_str uses the '\''..  dance for embedded singles.
        assert!(cmds[0].contains("'it'\\''s a test'"));
    }

    #[test]
    fn build_send_commands_quotes_session_name_with_dashes() {
        let cmds = build_send_commands("dev-with-dashes", "x");
        assert!(cmds[0].contains("'dev-with-dashes'"));
    }
```

- [ ] **Step 2: Run tests, expect compile errors**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib commands::sessions::tests::build_send 2>&1 | tail -8
```

- [ ] **Step 3: Add the helpers and command to `src-tauri/src/commands/sessions.rs`**

Insert after the existing helper functions (above the `#[cfg(test)]` block):

```rust
/// Conservative single-quote shell escape (local copy — iter 4 will extract
/// to a shared module). Wraps in `'...'`, replaces embedded `'` with the
/// canonical `'\''` dance.
fn shell_quote_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Build the two tmux invocations that together send a prompt to a session:
///   1. send-keys -t <name> -l <prompt>   (literal, no key-name translation)
///   2. send-keys -t <name> Enter         (real Enter to submit)
pub fn build_send_commands(tmux_name: &str, prompt: &str) -> Vec<String> {
    vec![
        format!("tmux send-keys -t {} -l {}", shell_quote_str(tmux_name), shell_quote_str(prompt)),
        format!("tmux send-keys -t {} Enter", shell_quote_str(tmux_name)),
    ]
}

#[derive(Deserialize)]
pub struct SendPromptArgs {
    pub host_alias: String,
    pub tmux_name: String,
    pub prompt: String,
}

#[tauri::command]
pub fn send_prompt(
    args: SendPromptArgs,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    let cmds = build_send_commands(&args.tmux_name, &args.prompt);
    if args.host_alias == "local" {
        for cmd in &cmds {
            let out = std::process::Command::new("bash")
                .args(["-c", cmd])
                .output()
                .map_err(|e| IpcError::new("E_TMUX", format!("spawn bash: {e}")))?;
            if !out.status.success() {
                return Err(IpcError::new(
                    "E_TMUX",
                    String::from_utf8_lossy(&out.stderr).trim().to_string(),
                ));
            }
        }
    } else {
        for cmd in &cmds {
            let quoted = shell_quote_str(cmd);
            let out = ssh.run(
                &args.host_alias,
                &["bash", "-lc", &quoted],
                std::time::Duration::from_secs(10),
            )?;
            if !out.status.success() {
                return Err(IpcError::new(
                    "E_TMUX",
                    String::from_utf8_lossy(&out.stderr).trim().to_string(),
                ));
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run unit tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib commands::sessions::tests::build_send 2>&1 | tail -8
```

Expected: 3 passes.

- [ ] **Step 5: Register `send_prompt` in `src-tauri/src/lib.rs`**

Add to the `generate_handler![...]` block:

```rust
            commands::sessions::send_prompt,
```

(Place adjacent to other `commands::sessions::*` entries.)

- [ ] **Step 6: Full lib suite**

```bash
cargo build --manifest-path src-tauri/Cargo.toml 2>&1 | tail -5
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -5
```

Expected: clean build + tests pass.

- [ ] **Step 7: Add a sanity smoke test against a local tmux session (optional but recommended)**

```bash
# Start a transient local tmux session
tmux new-session -d -s send-prompt-smoke 'cat'
# Build the binary and run a quick check (skip if build is heavy — the unit
# tests already cover correctness of the command shape).
tmux capture-pane -t send-prompt-smoke -p
# After send_prompt would run, the pane should contain the test text.
tmux kill-session -t send-prompt-smoke
```

Not a step that needs to be checked in — just verifies your understanding. Skip if short on time.

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/commands/sessions.rs src-tauri/src/lib.rs
git commit -m "sessions: send_prompt command (tmux send-keys + Enter)"
```

---

## Task 7: PromptComposer modal + SessionDetails wiring

**Files:**
- Create: `src/lib/PromptComposer.svelte`
- Create: `src/lib/PromptComposer.test.ts`
- Modify: `src/lib/sessions.ts` — `sendPrompt` wrapper
- Modify: `src/lib/SessionDetails.svelte` — `→ Send prompt` button opens PromptComposer
- Modify: `vitest.setup.ts` — mock `send_prompt`

- [ ] **Step 1: Add `sendPrompt` to `src/lib/sessions.ts`**

```typescript
export async function sendPrompt(
  hostAlias: string,
  tmuxName: string,
  prompt: string,
): Promise<Result<void>> {
  return invokeCmd<void>('send_prompt', {
    args: { host_alias: hostAlias, tmux_name: tmuxName, prompt },
  });
}
```

- [ ] **Step 2: Update `vitest.setup.ts`**

Append before the final `return null`:

```ts
    if (cmd === 'send_prompt') return null;
```

- [ ] **Step 3: Create `src/lib/PromptComposer.svelte`**

```svelte
<script lang="ts">
  import { sessions, sendPrompt, type SessionRow } from './sessions';
  import { hosts, type HostRow } from './hosts';
  import { accounts, type AccountRow } from './accounts';

  let {
    source,
    onClose,
  }: {
    source: SessionRow;
    onClose: () => void;
  } = $props();

  let prompt = $state('');
  let showAllFleet = $state(false);
  let sending = $state(false);
  // Per-target error map: tmux_name@host → message
  let errors = $state<Record<string, string>>({});
  // Per-target success map: tmux_name@host → true
  let succeeded = $state<Record<string, boolean>>({});

  // Default: related sessions for the source. Toggle expands to all fleet.
  const relatedTargets = $derived(
    source.project_id === null
      ? []
      : $sessions.filter(
          (s) =>
            s.id !== source.id &&
            s.project_id === source.project_id &&
            s.worktree_id === source.worktree_id,
        ),
  );
  const allOtherTargets = $derived(
    $sessions.filter((s) => s.id !== source.id),
  );

  // List of (session, isRelated). When showAllFleet=false → relatedTargets.
  // When true → all sessions (related ones marked).
  const displayTargets = $derived(
    showAllFleet ? allOtherTargets : relatedTargets,
  );

  // Track which targets are checked (default: all relateds checked).
  let checked = $state<Record<number, boolean>>({});

  // Initialise checked map when relatedTargets changes.
  $effect(() => {
    for (const r of relatedTargets) {
      if (checked[r.id] === undefined) checked[r.id] = true;
    }
  });

  function targetKey(s: SessionRow): string {
    return `${s.tmux_name}@${s.host_alias}`;
  }

  function accountForRow(s: SessionRow): AccountRow | null {
    if (!s.account_uuid) return null;
    return $accounts.find((a) => a.uuid === s.account_uuid) ?? null;
  }

  function accountText(a: AccountRow | null): string {
    if (!a) return '—';
    const email = a.email ?? a.uuid;
    return a.seat_tier ? `${email} (${a.seat_tier})` : email;
  }

  const hasChecked = $derived(
    Object.entries(checked).some(([_, v]) => v),
  );
  const canSend = $derived(prompt.trim().length > 0 && hasChecked && !sending);

  async function send() {
    sending = true;
    errors = {};
    succeeded = {};
    const targets = displayTargets.filter((t) => checked[t.id]);
    for (const t of targets) {
      const key = targetKey(t);
      const r = await sendPrompt(t.host_alias, t.tmux_name, prompt);
      if (r.ok) {
        succeeded[key] = true;
      } else {
        errors[key] = r.error.message;
      }
    }
    sending = false;
    // Auto-close on full success
    if (Object.keys(errors).length === 0) {
      setTimeout(() => onClose(), 600);
    }
  }
</script>

<div class="modal-backdrop" onclick={onClose} role="presentation">
  <div class="dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-label="Send prompt">
    <h3>Send prompt to session(s)</h3>

    <section class="targets">
      <h4>Targets</h4>
      {#if displayTargets.length === 0}
        <p class="muted" data-testid="composer-no-targets">
          No other sessions available{showAllFleet ? '' : ' for this worktree'}.
        </p>
      {:else}
        <ul>
          {#each displayTargets as t (t.id)}
            {@const key = targetKey(t)}
            <li class="target-row">
              <label>
                <input
                  type="checkbox"
                  bind:checked={checked[t.id]}
                  disabled={sending}
                  data-testid="target-checkbox-{t.id}"
                />
                <span class="host-badge">[{t.host_alias}]</span>
                <span class="account">{accountText(accountForRow(t))}</span>
                <span class="sess-name">{t.tmux_name}</span>
                {#if t.status !== 'running'}
                  <span class="warn" title="session may not be in claude REPL">⚠</span>
                {/if}
                {#if succeeded[key]}
                  <span class="ok">✓</span>
                {/if}
                {#if errors[key]}
                  <span class="err" data-testid="target-err-{t.id}">✗ {errors[key]}</span>
                {/if}
              </label>
            </li>
          {/each}
        </ul>
      {/if}
      <label class="show-all">
        <input
          type="checkbox"
          bind:checked={showAllFleet}
          disabled={sending}
          data-testid="show-all-fleet"
        />
        Show all fleet sessions
      </label>
    </section>

    <section class="prompt-section">
      <h4>Prompt</h4>
      <textarea
        bind:value={prompt}
        disabled={sending}
        rows="8"
        placeholder="Type a prompt to send to selected sessions…"
        data-testid="composer-textarea"
      ></textarea>
    </section>

    <div class="actions">
      <button onclick={onClose} disabled={sending}>Cancel</button>
      <button
        class="primary"
        disabled={!canSend}
        onclick={send}
        data-testid="composer-send"
      >{sending ? 'Sending…' : 'Send →'}</button>
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed; inset: 0; background: rgba(0,0,0,0.4);
    display: flex; align-items: center; justify-content: center;
    z-index: 20;
  }
  .dialog {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 1rem;
    width: 520px;
    max-height: 80vh;
    overflow: auto;
    color: var(--fg);
    display: flex;
    flex-direction: column;
    gap: 0.8rem;
  }
  .dialog h3 { margin: 0; font-size: 1rem; }
  .dialog h4 {
    margin: 0 0 0.3rem 0;
    font-size: 0.7rem;
    color: var(--fg-muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .muted { color: var(--fg-muted); font-size: 0.85rem; margin: 0; }

  .targets ul {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }
  .target-row label {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.3rem;
    border: 1px solid transparent;
    border-radius: 4px;
    font-size: 0.85rem;
    cursor: pointer;
  }
  .target-row label:hover { border-color: var(--border); background: var(--bg-pane); }
  .host-badge {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.7rem;
    color: var(--fg-muted);
    border: 1px solid var(--border);
    padding: 0.05rem 0.3rem;
    border-radius: 3px;
  }
  .account { color: var(--fg-muted); font-size: 0.75rem; flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .sess-name { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 0.78rem; }
  .warn { color: #d4a017; }
  .ok { color: rgb(80, 200, 110); }
  .err { color: #e64a4a; font-size: 0.75rem; }

  .show-all {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    margin-top: 0.4rem;
    font-size: 0.8rem;
    color: var(--fg-muted);
    cursor: pointer;
  }

  .prompt-section textarea {
    width: 100%;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.85rem;
    padding: 0.5rem;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    border-radius: 4px;
    resize: vertical;
    min-height: 6rem;
  }

  .actions { display: flex; gap: 0.4rem; justify-content: flex-end; }
  .actions button {
    font-size: 0.85rem;
    padding: 0.3rem 0.8rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg);
    border-radius: 4px;
    cursor: pointer;
  }
  .actions button:disabled { opacity: 0.5; cursor: not-allowed; }
  .actions button.primary {
    border-color: var(--accent);
    color: var(--fg);
  }
</style>
```

- [ ] **Step 4: Wire `→ Send prompt` button in `src/lib/SessionDetails.svelte`**

Find the existing action button row (where Rename / Restart / Kill buttons live). Insert a new button:

```svelte
  <section class="block actions">
    <!-- existing Rename, Restart, Kill buttons remain -->
    <button class="ghost" onclick={openComposer} data-testid="send-prompt-from-details">
      → Send prompt
    </button>
  </section>

  {#if composerOpen}
    <PromptComposer source={session} onClose={() => (composerOpen = false)} />
  {/if}
```

Add to imports + script:

```ts
  import PromptComposer from './PromptComposer.svelte';

  let composerOpen = $state(false);
  function openComposer() {
    composerOpen = true;
  }
```

- [ ] **Step 5: Create `src/lib/PromptComposer.test.ts`**

```typescript
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import PromptComposer from './PromptComposer.svelte';
import { sessions, type SessionRow } from './sessions';
import { hosts } from './hosts';
import { accounts } from './accounts';

const source: SessionRow = {
  id: 1,
  tmux_name: 'dev-source',
  host_alias: 'local',
  project_id: 1,
  worktree_id: 10,
  created_at: 1,
  last_activity_at: 1,
  status: 'running',
  notes: null,
  account_uuid: null,
};

const sibling: SessionRow = {
  id: 2,
  tmux_name: 'dev-sibling',
  host_alias: 'mefistos',
  project_id: 1,
  worktree_id: 10,
  created_at: 1,
  last_activity_at: 1,
  status: 'running',
  notes: null,
  account_uuid: null,
};

const unrelated: SessionRow = {
  id: 3,
  tmux_name: 'dev-other',
  host_alias: 'local',
  project_id: 99,
  worktree_id: 100,
  created_at: 1,
  last_activity_at: 1,
  status: 'running',
  notes: null,
  account_uuid: null,
};

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  sessions.set([source, sibling, unrelated]);
  hosts.set([]);
  accounts.set([]);
});

describe('PromptComposer', () => {
  it('defaults to showing related targets only', async () => {
    render(PromptComposer, { props: { source, onClose: () => {} } });
    await tick();
    // sibling is related; unrelated must not appear by default
    expect(screen.queryByText('dev-sibling')).toBeInTheDocument();
    expect(screen.queryByText('dev-other')).toBeNull();
  });

  it('toggling Show all fleet expands the targets list', async () => {
    render(PromptComposer, { props: { source, onClose: () => {} } });
    await tick();
    const toggle = screen.getByTestId('show-all-fleet') as HTMLInputElement;
    await fireEvent.click(toggle);
    await tick();
    expect(screen.queryByText('dev-other')).toBeInTheDocument();
  });

  it('Send is disabled until prompt + at least one target are set', async () => {
    render(PromptComposer, { props: { source, onClose: () => {} } });
    await tick();
    const send = screen.getByTestId('composer-send') as HTMLButtonElement;
    expect(send.disabled).toBe(true); // prompt is empty
    const textarea = screen.getByTestId('composer-textarea') as HTMLTextAreaElement;
    await fireEvent.input(textarea, { target: { value: 'hello' } });
    await tick();
    // sibling is auto-checked by default → send is now enabled
    expect((screen.getByTestId('composer-send') as HTMLButtonElement).disabled).toBe(false);
  });

  it('clicking Send calls send_prompt for each checked target', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'send_prompt') return null;
      return null;
    });
    render(PromptComposer, { props: { source, onClose: () => {} } });
    await tick();
    const textarea = screen.getByTestId('composer-textarea') as HTMLTextAreaElement;
    await fireEvent.input(textarea, { target: { value: 'echo hi' } });
    await tick();
    await fireEvent.click(screen.getByTestId('composer-send'));
    for (let i = 0; i < 8; i++) await tick();
    const sendCalls = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'send_prompt');
    expect(sendCalls).toHaveLength(1);
    const [, payload] = sendCalls[0] as [string, { args: { host_alias: string; tmux_name: string; prompt: string } }];
    expect(payload.args.host_alias).toBe('mefistos');
    expect(payload.args.tmux_name).toBe('dev-sibling');
    expect(payload.args.prompt).toBe('echo hi');
  });
});
```

- [ ] **Step 6: Run PromptComposer tests**

```bash
pnpm vitest run src/lib/PromptComposer.test.ts 2>&1 | tail -10
```

Expected: 4 passes.

- [ ] **Step 7: Full vitest sweep**

```bash
pnpm vitest run 2>&1 | tail -8
```

Expected: green.

- [ ] **Step 8: Commit**

```bash
git add src/lib/PromptComposer.svelte src/lib/PromptComposer.test.ts src/lib/sessions.ts src/lib/SessionDetails.svelte vitest.setup.ts
git commit -m "PromptComposer: targets picker + textarea + Send action"
```

---

## Task 8: Live verify + final review

This is manual but scripted for reproducibility.

- [ ] **Step 1: Build the release bundle**

```bash
cd /Users/martinjanci/projects/github.com/martin-janci/claude-fleet
pnpm tauri build --bundles app 2>&1 | tail -8
```

Expected: clean build.

- [ ] **Step 2: Restart claude-fleet**

```bash
pkill -f claude-fleet 2>/dev/null; sleep 1
open -a /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/src-tauri/target/release/bundle/macos/claude-fleet.app
```

- [ ] **Step 3: Create two local test sessions in the same worktree**

Use the UI: `+ New session` → claude-fleet project → host=local → Create. Then `+ New session` again with the same project but a different worktree to verify the badge logic. Then create another session in the SAME worktree as the first (use the same worktree picker) so you have two siblings.

Verify in the sidebar: each of the two sibling sessions should show `🔗1`. The unrelated session should not.

- [ ] **Step 4: Test SessionDetails Related panel**

Click one of the sibling sessions. SessionDetails should now show a `Related sessions (1)` section below the meta-grid. Click the sibling row → terminal pane should switch.

- [ ] **Step 5: Test the PromptComposer**

In SessionDetails, click `→ Send prompt`. Modal opens. The sibling is pre-checked. Type `echo hello from claude-fleet` and Send. The target session's tmux should receive the text + Enter; verify via:

```bash
tmux capture-pane -t <target-session-name> -p | tail -5
```

You should see the typed text echoed back in the pane.

- [ ] **Step 6: Test "Show all fleet sessions" toggle**

Re-open the composer, click `Show all fleet sessions`. The unrelated session (from a different worktree) should now appear. Don't actually send to it; just confirm it's visible.

- [ ] **Step 7: Test the multi-line prompt edge case**

In the composer, type:

```
line 1
line 2 with 'single quotes'
line 3 with $HOME
```

Send. Verify the target pane received all three lines verbatim, with no shell expansion.

- [ ] **Step 8: (When reachable) Test cross-host with mefistos**

If mefistos is online, create a session on mefistos in the same project+worktree as a local session. Verify the 🔗 badge appears on both, the Related panel cross-host, and prompt transfer works ssh→remote.

- [ ] **Step 9: Final test sweep**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -5
pnpm vitest run 2>&1 | tail -5
```

Expected: all green.

- [ ] **Step 10: Push to origin**

```bash
git push origin main 2>&1 | tail -3
```

- [ ] **Step 11: Tag iter 3 commits if any anomalies surfaced during live testing**

If live verification surfaced unexpected behavior, document in the spec's "Open risks" section:

```bash
git add docs/specs/2026-05-20-cross-host-sessions-and-transfer-design.md
git commit -m "docs: iter 3 spec — live verification notes"
git push
```

If no anomalies: just push the existing commits and move on.

---

## Self-Review (filled in by plan author)

**Spec coverage check:**
- Migration 004 + SessionRow.account_uuid + schema bump → Task 1 ✓
- Reconciliation preservation (capture for new, preserve for existing) → Task 2 ✓
- related_sessions Tauri command + Store helper → Task 3 ✓
- Frontend SessionRow type update + `relatedSessions` wrapper → Task 3 ✓
- Sidebar 🔗N badge per session row → Task 4 ✓
- SessionDetails Related sessions panel with click-to-switch → Task 5 ✓
- send_prompt backend (build_send_commands + local/remote dispatch) → Task 6 ✓
- PromptComposer modal (targets, textarea, sequential send, per-target errors) → Task 7 ✓
- SessionDetails `→ Send prompt` button + integration → Task 7 ✓
- Live verify on local + (when reachable) mefistos → Task 8 ✓

**Placeholder scan:** every code-bearing step has the actual code. The single `// placeholder from Task 1` comment is a transitional state INSIDE Task 1 that Task 2 explicitly resolves. No "TBD" / "TODO" anywhere.

**Type consistency:**
- `SessionRow.account_uuid: Option<String>` in Rust ↔ `account_uuid: string | null` in TS — consistent
- `build_send_commands(tmux_name, prompt) -> Vec<String>` referenced identically in Tasks 6 + 8
- `list_related_sessions(session_id) -> Vec<SessionRow>` ↔ TS `relatedSessions(sessionId): Promise<Result<SessionRow[]>>` — consistent
- `send_prompt` args camelCase frontend → snake_case Rust per existing serde defaults
- `PromptComposer.svelte` props `{ source, onClose }` typed consistently with SessionDetails caller

**Scope:** 8 tasks, ~half day of focused work. Commits are atomic and incremental. Live verify covers both new features and acknowledges mefistos's intermittent availability.
