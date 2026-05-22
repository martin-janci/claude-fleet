# Claude Session Intelligence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enrich every fleet session with Claude-specific live state — working/blocked/completed status, effort level, PR URL, current activity — by running `claude agents --json` on each host during reconcile and storing the results.

**Architecture:** A new `ClaudeAgentRow` type (parsed from `claude agents --json`) is added to `TmuxExec` as `list_claude_agents()`. The reconcile loop calls both `list_sessions()` and `list_claude_agents()` in parallel per host, matches by tmux session name, and writes 5 new columns (`claude_session_id`, `claude_status`, `effort_level`, `pr_url`, `current_activity`) into the sessions table via migration 009. The Sidebar shows a status chip, effort badge, and PR link on each session card.

**Tech Stack:** Rust/SQLite (rusqlite), serde_json, Tauri 2 IPC, Svelte 5 runes, TypeScript

---

## File Map

| Action | File |
|---|---|
| Create | `src-tauri/migrations/009_claude_agent_fields.sql` |
| Create | `src-tauri/src/claude_agents.rs` |
| Modify | `src-tauri/src/lib.rs` — add `mod claude_agents` |
| Modify | `src-tauri/src/store.rs` — `SessionRow`, `ReconcileSession`, `migrate()`, `upsert_session_in_tx`, `apply_host_reconcile`, SELECT queries |
| Modify | `src-tauri/src/tmux.rs` — add `list_claude_agents()` to `TmuxExec` trait + both impls |
| Modify | `src-tauri/src/service/sessions.rs` — parallel `list_claude_agents` call in reconcile |
| Modify | `src/lib/sessions.ts` — 5 new optional fields on `SessionRow` |
| Modify | `src/lib/Sidebar.svelte` — claude status chip, effort badge, PR link |

---

## Task 1: Migration 009 — add Claude agent columns

**Files:**
- Create: `src-tauri/migrations/009_claude_agent_fields.sql`
- Modify: `src-tauri/src/store.rs` (migrate function, ~line 185)

- [ ] **Step 1: Create the migration file**

```sql
-- src-tauri/migrations/009_claude_agent_fields.sql
ALTER TABLE sessions ADD COLUMN claude_session_id TEXT;
ALTER TABLE sessions ADD COLUMN claude_status TEXT;
ALTER TABLE sessions ADD COLUMN effort_level TEXT;
ALTER TABLE sessions ADD COLUMN pr_url TEXT;
ALTER TABLE sessions ADD COLUMN current_activity TEXT;
INSERT OR IGNORE INTO schema_version (version) VALUES (9);
```

- [ ] **Step 2: Add `v < 9` block to `Store::migrate()`**

In `store.rs`, after the `if v < 8` block (~line 190), add:

```rust
        if v < 9 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../migrations/009_claude_agent_fields.sql"))?;
            tx.commit()?;
        }
```

- [ ] **Step 3: Update the version-related tests**

In `store.rs` tests, find all assertions that check `schema_version == 8` and change them to `== 9`:
- `migrate_is_idempotent` test (contains `assert_eq!(store.schema_version().expect("version"), 8)`)
- `schema_version_is_seven_after_migration` test (misleadingly named, contains `assert_eq!(s.schema_version().expect("version"), 8)`)
- `schema_version` check in `apply_host_reconcile_rollback` test
- In `health.rs`, find `assert_eq!(v, 8, ...)` and change to `9`

- [ ] **Step 4: Run tests to verify migration works**

```bash
cd src-tauri && cargo test migrate -- --nocapture 2>&1 | tail -10
```

Expected: all migration tests pass

- [ ] **Step 5: Commit**

```bash
git add src-tauri/migrations/009_claude_agent_fields.sql src-tauri/src/store.rs src-tauri/src/service/health.rs
git commit -m "feat(store): migration 009 — claude agent fields on sessions"
```

---

## Task 2: `ClaudeAgentRow` struct and parser

**Files:**
- Create: `src-tauri/src/claude_agents.rs`
- Modify: `src-tauri/src/lib.rs` (~line 13)

`claude agents --json` outputs a JSON array like:
```json
[{"pid":1234,"cwd":"/Users/u/project","kind":"session","startedAt":"2026-05-22T10:00:00Z","sessionId":"abc123def","name":"my-session","status":"working"}]
```

The `status` field can be: `"working"`, `"blocked"`, `"completed"`, `"failed"`, `"stopped"`, `"idle"`. The `name` field matches the fleet's `tmux_name` when the session was created by the fleet.

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/claude_agents.rs`:

```rust
//! Parse the JSON emitted by `claude agents --json`.

use serde::Deserialize;

/// One row from `claude agents --json`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ClaudeAgentRow {
    /// The Claude-internal session ID (used to call `claude logs <id>`).
    #[serde(rename = "sessionId", default)]
    pub session_id: Option<String>,
    /// Display name, matches tmux session name when created with `--name`.
    #[serde(default)]
    pub name: Option<String>,
    /// "working" | "blocked" | "completed" | "failed" | "stopped" | "idle"
    #[serde(default)]
    pub status: Option<String>,
    /// Working directory of the Claude session.
    #[serde(default)]
    pub cwd: Option<String>,
}

/// Parse the stdout of `claude agents --json`. Returns empty vec on any parse
/// failure (the fleet treats missing data as degraded-gracefully, not an error).
pub fn parse_claude_agents_json(json: &str) -> Vec<ClaudeAgentRow> {
    serde_json::from_str(json).unwrap_or_default()
}

/// Find the first `ClaudeAgentRow` whose `name` matches `tmux_name`.
pub fn find_by_name<'a>(rows: &'a [ClaudeAgentRow], tmux_name: &str) -> Option<&'a ClaudeAgentRow> {
    rows.iter().find(|r| r.name.as_deref() == Some(tmux_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_array() {
        assert_eq!(parse_claude_agents_json("[]"), vec![]);
    }

    #[test]
    fn parse_invalid_json_returns_empty() {
        assert_eq!(parse_claude_agents_json("not json"), vec![]);
    }

    #[test]
    fn parse_full_row() {
        let json = r#"[{"pid":1234,"cwd":"/Users/u/proj","kind":"session","startedAt":"2026-05-22T10:00:00Z","sessionId":"abc123","name":"my-sess","status":"working"}]"#;
        let rows = parse_claude_agents_json(json);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id.as_deref(), Some("abc123"));
        assert_eq!(rows[0].name.as_deref(), Some("my-sess"));
        assert_eq!(rows[0].status.as_deref(), Some("working"));
        assert_eq!(rows[0].cwd.as_deref(), Some("/Users/u/proj"));
    }

    #[test]
    fn parse_row_missing_optional_fields() {
        let json = r#"[{"pid":999,"cwd":"/tmp","kind":"session","startedAt":"2026-05-22T10:00:00Z"}]"#;
        let rows = parse_claude_agents_json(json);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, None);
        assert_eq!(rows[0].name, None);
        assert_eq!(rows[0].status, None);
    }

    #[test]
    fn find_by_name_matches() {
        let rows = vec![
            ClaudeAgentRow { session_id: Some("s1".into()), name: Some("alpha".into()), status: Some("working".into()), cwd: None },
            ClaudeAgentRow { session_id: Some("s2".into()), name: Some("beta".into()), status: Some("blocked".into()), cwd: None },
        ];
        let hit = find_by_name(&rows, "beta").unwrap();
        assert_eq!(hit.session_id.as_deref(), Some("s2"));
    }

    #[test]
    fn find_by_name_no_match_returns_none() {
        let rows: Vec<ClaudeAgentRow> = vec![];
        assert!(find_by_name(&rows, "missing").is_none());
    }
}
```

- [ ] **Step 2: Add `mod claude_agents` to `lib.rs`**

In `src-tauri/src/lib.rs`, after the existing `mod` declarations (~line 13), add:

```rust
mod claude_agents;
```

- [ ] **Step 3: Run the tests**

```bash
cd src-tauri && cargo test claude_agents -- --nocapture
```

Expected: 5 tests pass

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/claude_agents.rs src-tauri/src/lib.rs
git commit -m "feat(claude_agents): parse claude agents --json output"
```

---

## Task 3: `list_claude_agents()` on `TmuxExec` trait

**Files:**
- Modify: `src-tauri/src/tmux.rs`

Add `list_claude_agents()` to the `TmuxExec` trait and both `LocalTmux` and `RemoteTmux` implementations. It runs `claude agents --json 2>/dev/null || echo '[]'` and returns `Vec<ClaudeAgentRow>`.

- [ ] **Step 1: Add the method to the trait**

In `tmux.rs`, find the `TmuxExec` trait definition and add after `list_sessions`:

```rust
/// Run `claude agents --json` on this host and return parsed session info.
/// Returns an empty vec if claude CLI is not installed or the command fails —
/// the fleet treats missing Claude agent data as degraded-gracefully.
async fn list_claude_agents(&self) -> Vec<crate::claude_agents::ClaudeAgentRow>;
```

- [ ] **Step 2: Implement for `LocalTmux`**

In `impl TmuxExec for LocalTmux`, add:

```rust
async fn list_claude_agents(&self) -> Vec<crate::claude_agents::ClaudeAgentRow> {
    let output = tokio::process::Command::new("claude")
        .args(["agents", "--json"])
        .output()
        .await
        .ok();
    let json = output
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_else(|| "[]".to_string());
    crate::claude_agents::parse_claude_agents_json(&json)
}
```

- [ ] **Step 3: Implement for `RemoteTmux`**

In `impl TmuxExec for RemoteTmux`, add:

```rust
async fn list_claude_agents(&self) -> Vec<crate::claude_agents::ClaudeAgentRow> {
    let script = "claude agents --json 2>/dev/null || echo '[]'";
    let output = self.remote_bash(script).await.ok();
    let json = output
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_else(|| "[]".to_string());
    crate::claude_agents::parse_claude_agents_json(&json)
}
```

- [ ] **Step 4: Add stub to `SleepyTmux` and `FakeTmux` in `service/sessions.rs` tests**

In `src-tauri/src/service/sessions.rs`, find the test structs `SleepyTmux` and `FakeTmux` (they implement `TmuxExec` for tests). Add to each:

```rust
async fn list_claude_agents(&self) -> Vec<crate::claude_agents::ClaudeAgentRow> {
    vec![]
}
```

- [ ] **Step 5: Verify it compiles**

```bash
cd src-tauri && cargo check 2>&1 | grep "^error" | head -10
```

Expected: no errors

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/tmux.rs src-tauri/src/service/sessions.rs
git commit -m "feat(tmux): add list_claude_agents to TmuxExec trait"
```

---

## Task 4: Extend `SessionRow` and `ReconcileSession` with Claude fields

**Files:**
- Modify: `src-tauri/src/store.rs`

Add the 5 new columns to `SessionRow` and carry them through every query.

- [ ] **Step 1: Update `SessionRow` struct**

Find `pub struct SessionRow` (~line 28) and add 5 fields at the end:

```rust
pub struct SessionRow {
    // … existing fields …
    pub lost_at: Option<i64>,
    // NEW:
    pub claude_session_id: Option<String>,
    pub claude_status: Option<String>,
    pub effort_level: Option<String>,
    pub pr_url: Option<String>,
    pub current_activity: Option<String>,
}
```

- [ ] **Step 2: Update `ReconcileSession` struct**

Find `pub struct ReconcileSession` (~line 77) and add:

```rust
pub struct ReconcileSession<'a> {
    pub tmux_name: &'a str,
    pub project_id: Option<i64>,
    pub created_at: i64,
    pub last_activity_at: i64,
    pub account_uuid: Option<String>,
    pub worktree_key: Option<String>,
    // NEW — from claude agents --json:
    pub claude_session_id: Option<String>,
    pub claude_status: Option<String>,
    pub effort_level: Option<String>,
    pub pr_url: Option<String>,
    pub current_activity: Option<String>,
}
```

- [ ] **Step 3: Update `upsert_session_in_tx` to write new fields**

Find the `upsert_session_in_tx` function. Its INSERT/ON CONFLICT statement needs to include the 5 new columns. Change:

```rust
conn.execute(
    "INSERT INTO sessions (tmux_name, host_alias, project_id, worktree_id, created_at,
            last_activity_at, status, account_uuid, worktree_key, lost_at)
     VALUES (?1,?2,?3,?4,?5,?6,'running',?7,?8,NULL)
     ON CONFLICT(tmux_name, host_alias) DO UPDATE SET
         project_id=excluded.project_id,
         last_activity_at=excluded.last_activity_at,
         account_uuid=COALESCE(excluded.account_uuid, account_uuid),
         worktree_key=COALESCE(excluded.worktree_key, worktree_key),
         status=CASE WHEN status='ghost' THEN 'running' ELSE status END,
         lost_at=NULL",
    rusqlite::params![
        s.tmux_name, host_alias, s.project_id, worktree_id,
        s.created_at, s.last_activity_at, s.account_uuid, s.worktree_key,
    ],
)?;
```

To:

```rust
conn.execute(
    "INSERT INTO sessions (tmux_name, host_alias, project_id, worktree_id, created_at,
            last_activity_at, status, account_uuid, worktree_key, lost_at,
            claude_session_id, claude_status, effort_level, pr_url, current_activity)
     VALUES (?1,?2,?3,?4,?5,?6,'running',?7,?8,NULL,?9,?10,?11,?12,?13)
     ON CONFLICT(tmux_name, host_alias) DO UPDATE SET
         project_id=excluded.project_id,
         last_activity_at=excluded.last_activity_at,
         account_uuid=COALESCE(excluded.account_uuid, account_uuid),
         worktree_key=COALESCE(excluded.worktree_key, worktree_key),
         status=CASE WHEN status='ghost' THEN 'running' ELSE status END,
         lost_at=NULL,
         claude_session_id=COALESCE(excluded.claude_session_id, claude_session_id),
         claude_status=COALESCE(excluded.claude_status, claude_status),
         effort_level=COALESCE(excluded.effort_level, effort_level),
         pr_url=COALESCE(excluded.pr_url, pr_url),
         current_activity=COALESCE(excluded.current_activity, current_activity)",
    rusqlite::params![
        s.tmux_name, host_alias, s.project_id, worktree_id,
        s.created_at, s.last_activity_at, s.account_uuid, s.worktree_key,
        s.claude_session_id, s.claude_status, s.effort_level, s.pr_url, s.current_activity,
    ],
)?;
```

- [ ] **Step 4: Update the private `fetch_session` function**

The private `fetch_session` free function builds a `SessionRow` from a SQL row. Add the 5 new column reads at indices 14–18:

```rust
fn fetch_session(row: &rusqlite::Row<'_>) -> Result<SessionRow, rusqlite::Error> {
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
        claude_session_id: row.get(14)?,
        claude_status: row.get(15)?,
        effort_level: row.get(16)?,
        pr_url: row.get(17)?,
        current_activity: row.get(18)?,
    })
}
```

- [ ] **Step 5: Update `fetch_session_by_id` with the same 5 columns**

Find `fetch_session_by_id` and update its SELECT and `SessionRow` construction the same way (add `claude_session_id, claude_status, effort_level, pr_url, current_activity` to the SELECT list and `.get(14)?` through `.get(18)?` to the struct).

- [ ] **Step 6: Update `list_all_sessions`, `list_sessions_for_host`, `list_related_sessions`**

For each of the three list queries, add the 5 columns to the SELECT:

```sql
SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
       last_activity_at, status, notes, account_uuid, kind, reviews_session_id,
       worktree_key, lost_at,
       claude_session_id, claude_status, effort_level, pr_url, current_activity
FROM sessions ...
```

And add `row.get(14)?` through `row.get(18)?` to the `SessionRow` construction in each.

- [ ] **Step 7: Fix `ReconcileSession` construction in `service/sessions.rs`**

The `reconcile_write_one_host` function creates `ReconcileSession { ... }`. Add the 5 new fields with `None` values (they'll be filled in Task 5):

```rust
sessions.push(ReconcileSession {
    tmux_name: &sess.name,
    project_id,
    created_at: sess.created,
    last_activity_at: sess.last_activity,
    account_uuid,
    worktree_key,
    claude_session_id: None,  // filled by claude_agents lookup in Task 5
    claude_status: None,
    effort_level: None,
    pr_url: None,
    current_activity: None,
});
```

- [ ] **Step 8: Fix all test fixtures that construct `ReconcileSession`**

Search for `ReconcileSession {` in store.rs tests and add the 5 new None fields to each.

- [ ] **Step 9: Fix all test fixtures that construct `SessionRow`**

Search for `SessionRow {` in test files and add the 5 new None fields.

- [ ] **Step 10: Run all tests**

```bash
cd src-tauri && cargo test -- --nocapture 2>&1 | tail -15
```

Expected: all tests pass (new columns default to NULL, no behavior change yet)

- [ ] **Step 11: Commit**

```bash
git add src-tauri/src/store.rs src-tauri/src/service/sessions.rs
git commit -m "feat(store): add claude agent fields to SessionRow and all queries"
```

---

## Task 5: Feed `ClaudeAgentRow` data into reconcile

**Files:**
- Modify: `src-tauri/src/service/sessions.rs`

Extend the reconcile JoinSet to also call `list_claude_agents()` per host, then cross-reference by name to fill in the new `ReconcileSession` fields.

- [ ] **Step 1: Update `reconcile_sessions` to call `list_claude_agents` in parallel**

In `service/sessions.rs`, find the JoinSet spawn block (~line 108). Change it from:

```rust
set.spawn(async move {
    let tmux = exec_for(&host.alias, &ssh_arc);
    let result = tmux.list_sessions().await;
    (host, result)
});
```

To:

```rust
set.spawn(async move {
    let tmux = exec_for(&host.alias, &ssh_arc);
    let tmux_result = tmux.list_sessions().await;
    let agent_rows = tmux.list_claude_agents().await;
    (host, tmux_result, agent_rows)
});
```

- [ ] **Step 2: Update the probed collection to carry agent rows**

Find `let mut probed: Vec<(HostRow, Result<...>)>` and change to:

```rust
let mut probed: Vec<(HostRow, Result<Vec<crate::tmux::TmuxSession>, IpcError>, Vec<crate::claude_agents::ClaudeAgentRow>)> = Vec::new();
while let Some(join) = set.join_next().await {
    match join {
        Ok((host, res, agent_rows)) => probed.push((host, res, agent_rows)),
        Err(e) => eprintln!("[reconcile] probe task panicked: {e}"),
    }
}
```

- [ ] **Step 3: Pass `agent_rows` into `reconcile_write_one_host`**

Change the signature of `reconcile_write_one_host`:

```rust
fn reconcile_write_one_host(
    s: &mut Store,
    host: &HostRow,
    res: &Result<Vec<crate::tmux::TmuxSession>, IpcError>,
    projects: &[ProjectRow],
    agent_rows: &[crate::claude_agents::ClaudeAgentRow],
) -> Result<(), IpcError> {
```

And update the call site to pass `&agent_rows`.

- [ ] **Step 4: Use `find_by_name` to fill claude fields in `reconcile_write_one_host`**

In the `Ok(live)` arm of `reconcile_write_one_host`, change the `ReconcileSession` push to:

```rust
let agent = crate::claude_agents::find_by_name(agent_rows, &sess.name);
sessions.push(ReconcileSession {
    tmux_name: &sess.name,
    project_id,
    created_at: sess.created,
    last_activity_at: sess.last_activity,
    account_uuid,
    worktree_key,
    claude_session_id: agent.and_then(|a| a.session_id.clone()),
    claude_status: agent.and_then(|a| a.status.clone()),
    effort_level: None,  // not in claude agents --json; set separately
    pr_url: None,        // not in claude agents --json; set separately
    current_activity: None,
});
```

- [ ] **Step 5: Update the probed loop that calls `reconcile_write_one_host`**

Find the loop over `probed` and pass `&agent_rows`:

```rust
for (host, res, agent_rows) in &probed {
    let mut s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    if let Err(e) = reconcile_write_one_host(&mut s, host, res, &projects, agent_rows) {
        eprintln!("[reconcile] write failed for {}: {e}", host.alias);
    }
}
```

- [ ] **Step 6: Verify compile**

```bash
cd src-tauri && cargo check 2>&1 | grep "^error" | head -10
```

Expected: no errors

- [ ] **Step 7: Write a test for the cross-reference logic**

In `service/sessions.rs` test module (or store.rs tests), add:

```rust
#[test]
fn reconcile_writes_claude_session_id_when_name_matches() {
    use crate::claude_agents::ClaudeAgentRow;
    // Build a fake agent row with name = "my-session"
    let agent_rows = vec![ClaudeAgentRow {
        session_id: Some("abc123".into()),
        name: Some("my-session".into()),
        status: Some("working".into()),
        cwd: None,
    }];
    let hit = crate::claude_agents::find_by_name(&agent_rows, "my-session");
    assert_eq!(hit.unwrap().session_id.as_deref(), Some("abc123"));
    let miss = crate::claude_agents::find_by_name(&agent_rows, "other");
    assert!(miss.is_none());
}
```

- [ ] **Step 8: Run all tests**

```bash
cd src-tauri && cargo test -- --nocapture 2>&1 | tail -15
```

Expected: all tests pass

- [ ] **Step 9: Commit**

```bash
git add src-tauri/src/service/sessions.rs
git commit -m "feat(reconcile): populate claude_session_id and claude_status from claude agents"
```

---

## Task 6: Frontend `sessions.ts` — 5 new fields

**Files:**
- Modify: `src/lib/sessions.ts`

- [ ] **Step 1: Add 5 new optional fields to `SessionRow`**

In `sessions.ts`, find the `SessionRow` interface and add after `lost_at`:

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
  // Claude agent fields — null when claude CLI not installed or session not managed by Claude Code
  claude_session_id: string | null;
  claude_status: string | null;
  effort_level: string | null;
  pr_url: string | null;
  current_activity: string | null;
}
```

- [ ] **Step 2: Run type-check**

```bash
pnpm check 2>&1 | tail -20
```

Expected: no new errors (pre-existing errors in test files are okay)

- [ ] **Step 3: Update test fixture files**

Search for `lost_at: null` in test files — each has a `SessionRow` fixture. Add the 5 new fields with `null` after `lost_at: null` in each of:
- `src/lib/sessions.test.ts`
- `src/lib/Sidebar.test.ts`
- `src/lib/SessionDetails.test.ts`
- `src/lib/ReviewDialog.test.ts`
- `src/lib/PromptComposer.test.ts`
- `src/lib/NewSessionDialog.test.ts`

For each fixture add:
```typescript
claude_session_id: null,
claude_status: null,
effort_level: null,
pr_url: null,
current_activity: null,
```

- [ ] **Step 4: Run frontend tests**

```bash
pnpm test 2>&1 | tail -15
```

Expected: all 230 tests pass

- [ ] **Step 5: Commit**

```bash
git add src/lib/sessions.ts src/lib/sessions.test.ts src/lib/Sidebar.test.ts src/lib/SessionDetails.test.ts src/lib/ReviewDialog.test.ts src/lib/PromptComposer.test.ts src/lib/NewSessionDialog.test.ts
git commit -m "feat(frontend): add claude agent fields to SessionRow interface"
```

---

## Task 7: Sidebar — Claude status chip, effort badge, PR link

**Files:**
- Modify: `src/lib/Sidebar.svelte`

Add visual indicators to each non-ghost session card: a colored status chip for `claude_status`, an effort badge for `effort_level`, and a PR link for `pr_url`.

- [ ] **Step 1: Add `claudeStatusColor` helper in the `<script>` block**

After the `timeAgo` helper, add:

```typescript
function claudeStatusColor(status: string | null): string {
  switch (status) {
    case 'working': return '#50c86e';   // green — active
    case 'blocked': return '#f0b429';   // yellow — needs input
    case 'completed': return '#6c8ebf'; // blue — done
    case 'failed': return '#e64a4a';    // red
    case 'idle': return '#888';         // grey
    default: return 'transparent';
  }
}

function claudeStatusLabel(status: string | null): string {
  switch (status) {
    case 'working': return '⚡ working';
    case 'blocked': return '⏸ blocked';
    case 'completed': return '✓ done';
    case 'failed': return '✗ failed';
    case 'idle': return '· idle';
    default: return '';
  }
}
```

- [ ] **Step 2: Add the status chip and PR link to the non-ghost `{:else}` branch**

Inside the non-ghost `{:else}` branch of `{#snippet sessionRow}`, after the `<span class="sess-name">` line and before `<div class="row-actions">`, add:

```svelte
          {#if sess.claude_status}
            <span
              class="claude-chip"
              style="background: {claudeStatusColor(sess.claude_status)}22; color: {claudeStatusColor(sess.claude_status)}; border-color: {claudeStatusColor(sess.claude_status)}44;"
              title="Claude: {sess.claude_status}{sess.current_activity ? ' — ' + sess.current_activity : ''}"
            >{claudeStatusLabel(sess.claude_status)}</span>
          {/if}
          {#if sess.effort_level}
            <span class="effort-badge" title="Effort: {sess.effort_level}">{sess.effort_level}</span>
          {/if}
          {#if sess.pr_url}
            <a
              class="pr-link"
              href={sess.pr_url}
              onclick={(e) => e.stopPropagation()}
              title="Open pull request"
              target="_blank"
              rel="noreferrer"
            >PR↗</a>
          {/if}
```

- [ ] **Step 3: Add CSS in the `<style>` block**

After the existing `.lost-at` rule, add:

```css
  .claude-chip {
    font-size: 0.65rem;
    padding: 0.05rem 0.3rem;
    border-radius: 3px;
    border: 1px solid;
    flex-shrink: 0;
    white-space: nowrap;
  }
  .effort-badge {
    font-size: 0.6rem;
    padding: 0.05rem 0.25rem;
    border-radius: 3px;
    background: color-mix(in srgb, var(--fg) 10%, transparent);
    color: var(--fg-muted);
    flex-shrink: 0;
    white-space: nowrap;
    text-transform: uppercase;
  }
  .pr-link {
    font-size: 0.65rem;
    color: var(--accent);
    text-decoration: none;
    flex-shrink: 0;
    white-space: nowrap;
  }
  .pr-link:hover { text-decoration: underline; }
```

- [ ] **Step 4: Run type-check**

```bash
pnpm check 2>&1 | tail -10
```

Expected: no new errors

- [ ] **Step 5: Run frontend tests**

```bash
pnpm test 2>&1 | tail -10
```

Expected: all tests pass

- [ ] **Step 6: Commit**

```bash
git add src/lib/Sidebar.svelte
git commit -m "feat(ui): claude status chip, effort badge, and PR link on session cards"
```

---

## Task 8: Final verification

- [ ] **Step 1: Run full Rust test suite**

```bash
cd src-tauri && cargo test -- --nocapture 2>&1 | tail -20
```

Expected: all tests pass

- [ ] **Step 2: Run clippy**

```bash
cd src-tauri && cargo clippy --all-targets -- -D warnings 2>&1 | head -20
```

Expected: no warnings

- [ ] **Step 3: Run frontend tests**

```bash
pnpm test 2>&1 | tail -10
```

Expected: all tests pass

- [ ] **Step 4: Commit any fixups**

```bash
git add -p && git commit -m "chore: clippy and test fixes for claude session intelligence"
```
