# Worktree-aware related matching — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make related-session matching worktree-aware and portable across hosts — two sessions are related iff same repo AND same worktree *name*.

**Architecture:** Derive a portable `worktree_key` (worktree name) from each session's cwd; store it on `sessions` (migration 006); reconcile populates it via the existing `apply_host_reconcile` batch; both match sites (backend `list_related_sessions` + frontend grouping) switch from `worktree_id` to `worktree_key`.

**Tech Stack:** Rust + Tauri 2 (`rusqlite`, `regex`/`once_cell`), Svelte 5 runes.

**Reference spec:** `docs/specs/2026-05-21-cross-host-worktree-key-design.md`

---

## File Structure

| File | Responsibility | Status |
|---|---|---|
| `src-tauri/migrations/006_session_worktree_key.sql` | Add `worktree_key` column, bump schema_version. | Created |
| `src-tauri/src/store.rs` | `SessionRow.worktree_key`; all projections; `set_worktree_key`; `upsert_session_in_tx` writes the key; `list_related_sessions` matches on it; migration wired. | Modified |
| `src-tauri/src/commands/health.rs` | schema_version assertion → 6. | Modified |
| `src-tauri/src/commands/sessions.rs` | `worktree_key_for_path` helper; `ReconcileSession.worktree_key`; both reconcile callers compute + pass it. | Modified |
| `src/lib/sessions.ts` | `SessionRow.worktree_key`. | Modified |
| `src/lib/Sidebar.svelte` | `relatedCountById` groups by `worktree_key`. | Modified |
| `src/lib/SessionDetails.svelte` | related-panel `$derived` matches on `worktree_key`. | Modified |

---

## M1 — Backend

### Task 1: Migration 006 + SessionRow.worktree_key + worktree_key_for_path + set_worktree_key

**Files:**
- Create: `src-tauri/migrations/006_session_worktree_key.sql`
- Modify: `src-tauri/src/store.rs`, `src-tauri/src/commands/health.rs`, `src-tauri/src/commands/sessions.rs`

- [ ] **Step 1: Migration file**

Create `src-tauri/migrations/006_session_worktree_key.sql`:

```sql
ALTER TABLE sessions ADD COLUMN worktree_key TEXT;
INSERT OR IGNORE INTO schema_version (version) VALUES (6);
```

- [ ] **Step 2: Wire into `migrate()`**

In `src-tauri/src/store.rs`, after the `if v < 5 { ... }` block:

```rust
if v < 6 {
    self.conn
        .execute_batch(include_str!("../migrations/006_session_worktree_key.sql"))?;
}
```

- [ ] **Step 3: Add field to `SessionRow`**

After `reviews_session_id`:

```rust
    pub worktree_key: Option<String>,
```

- [ ] **Step 4: Update EVERY SessionRow projection**

```bash
cd /Users/martinjanci/projects/github.com/martin-janci/claude-fleet
grep -n "SessionRow {" src-tauri/src/store.rs
```

For each (`get_session`, `get_session_by_id`, `list_sessions_for_host`, `list_related_sessions`, and any others), add `worktree_key` to the SELECT column list (after `reviews_session_id`) and `worktree_key: row.get(12)?,` to the builder (index = the new last column; adjust to each query's actual order). A missed projection is a runtime panic.

- [ ] **Step 5: `set_worktree_key` setter**

In `impl Store` (mirrors `set_session_kind`):

```rust
/// Set a session's portable worktree key (derived from its cwd by reconcile).
/// Emits `session_updated` so the frontend patches in place.
pub fn set_worktree_key(&self, id: i64, key: Option<&str>) -> Result<(), rusqlite::Error> {
    self.conn.execute(
        "UPDATE sessions SET worktree_key = ?1 WHERE id = ?2",
        rusqlite::params![key, id],
    )?;
    if let Some(row) = self.get_session_by_id(id)? {
        self.bus.session_updated(&row);
    }
    Ok(())
}
```

- [ ] **Step 6: `worktree_key_for_path` helper**

In `src-tauri/src/commands/sessions.rs`, near `extract_owner_repo`:

```rust
/// Derive a portable worktree name from a session's cwd. Host-path-independent:
///   - <repo>/.claude/worktrees/<name>[/…]  → Some("<name>")
///   - <repo> root or any other subdir       → Some("main")
///   - path without a github.com repo segment → None (orphan)
fn worktree_key_for_path(path: &str) -> Option<String> {
    static RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"/projects/github\.com/[^/]+/[^/]+(/.*)?$").expect("static regex")
    });
    let caps = RE.captures(path)?;
    let remainder = caps.get(1).map(|m| m.as_str()).unwrap_or("");
    if let Some(idx) = remainder.find("/.claude/worktrees/") {
        let after = &remainder[idx + "/.claude/worktrees/".len()..];
        if let Some(name) = after.split('/').next() {
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    Some("main".to_string())
}
```

- [ ] **Step 7: Tests for the derivation**

In `commands/sessions.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn worktree_key_root_is_main_local_and_remote() {
    assert_eq!(
        worktree_key_for_path("/Users/martinjanci/projects/github.com/martin-janci/claude-fleet"),
        Some("main".to_string())
    );
    assert_eq!(
        worktree_key_for_path("/home/mjanci/projects/github.com/martin-janci/claude-fleet"),
        Some("main".to_string())
    );
}

#[test]
fn worktree_key_extracts_named_worktree() {
    assert_eq!(
        worktree_key_for_path("/Users/x/projects/github.com/o/r/.claude/worktrees/feat-auth"),
        Some("feat-auth".to_string())
    );
    assert_eq!(
        worktree_key_for_path("/home/mjanci/projects/github.com/o/r/.claude/worktrees/feat-auth/src"),
        Some("feat-auth".to_string())
    );
}

#[test]
fn worktree_key_other_subdir_is_main() {
    assert_eq!(
        worktree_key_for_path("/Users/x/projects/github.com/o/r/src/lib"),
        Some("main".to_string())
    );
}

#[test]
fn worktree_key_non_repo_path_is_none() {
    assert_eq!(worktree_key_for_path("/tmp/whatever"), None);
    assert_eq!(worktree_key_for_path("/Users/x/Documents"), None);
}
```

- [ ] **Step 8: Bump health assertion**

In `src-tauri/src/commands/health.rs`: `assert_eq!(h.schema_version, 6);`. Also update any `store.rs` test that hard-codes schema_version 5 (e.g. `schema_version_is_five_after_migration` → rename to `_is_six_` and assert 6; `migrate_is_idempotent` if it checks the number).

- [ ] **Step 9: Run + commit**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -8
```

Expected: 111 + 4 derivation tests = 115 passing (plus the health/version bumps stay green).

```bash
git add src-tauri/migrations/006_session_worktree_key.sql src-tauri/src/store.rs src-tauri/src/commands/health.rs src-tauri/src/commands/sessions.rs
git commit -m "store: migration 006 (sessions.worktree_key) + worktree_key_for_path + setter"
```

---

### Task 2: Reconcile populates worktree_key + list_related_sessions matches on it

**Files:**
- Modify: `src-tauri/src/store.rs`, `src-tauri/src/commands/sessions.rs`

- [ ] **Step 1: `ReconcileSession` gains worktree_key**

In `commands/sessions.rs`, the `ReconcileSession` struct — add:

```rust
    pub worktree_key: Option<String>,
```

- [ ] **Step 2: Both reconcile callers compute it**

In `reconcile_sessions` and `reconcile_one_host`, where each `ReconcileSession` is built (alongside the existing `project_id = find_project_id_for_path(...)` and account resolution), add:

```rust
        let worktree_key = worktree_key_for_path(&sess.path.to_string_lossy());
```

and include `worktree_key` in the `ReconcileSession { ... }` literal. (`sess.path` is the tmux session's cwd — confirm its type; if it's a `PathBuf`/`Path`, use `.to_string_lossy()`; if already `&str`, pass directly.)

- [ ] **Step 3: `upsert_session_in_tx` writes worktree_key**

In `store.rs`, `upsert_session_in_tx` — add `worktree_key: Option<&str>` to its parameters, add the column to the INSERT list + values, and to the `ON CONFLICT DO UPDATE SET` clause:

```rust
// INSERT (… , worktree_key) VALUES (… , ?N)
// ON CONFLICT(host_alias, tmux_name) DO UPDATE SET
//   …,
//   worktree_key = excluded.worktree_key
```

Update `apply_host_reconcile` to pass `s.worktree_key.as_deref()` (from the `ReconcileSession`) into `upsert_session_in_tx`. Extend the `_in_tx`-parallel maintenance comment: note that `worktree_key` is written by the `_in_tx` variant only (reconcile-only); the public `upsert_session` intentionally omits it.

- [ ] **Step 4: `list_related_sessions` matches on worktree_key**

Replace the source lookup + match SQL:

```rust
pub fn list_related_sessions(
    &self,
    session_id: i64,
) -> Result<Vec<SessionRow>, rusqlite::Error> {
    let (proj, key): (Option<i64>, Option<String>) = self.conn.query_row(
        "SELECT project_id, worktree_key FROM sessions WHERE id=?1",
        rusqlite::params![session_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let Some(project_id) = proj else {
        return Ok(Vec::new());
    };
    // A session with a resolved project always has a worktree_key after
    // reconcile ("main" at minimum); a NULL key (legacy/pre-reconcile) matches
    // nothing, which is safe.
    let Some(key) = key else {
        return Ok(Vec::new());
    };
    let mut stmt = self.conn.prepare(
        "SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
                last_activity_at, status, notes, account_uuid, kind, reviews_session_id,
                worktree_key
         FROM sessions
         WHERE project_id=?1
           AND worktree_key=?2
           AND id<>?3
         ORDER BY host_alias ASC, tmux_name ASC",
    )?;
    let rows = stmt.query_map(rusqlite::params![project_id, key, session_id], |row| {
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
        })
    })?;
    rows.collect()
}
```

- [ ] **Step 5: Update existing list_related_sessions tests + add new ones**

The existing tests (`list_related_sessions_returns_siblings_with_same_project_and_worktree`, `list_related_sessions_matches_null_worktree`, `list_related_sessions_excludes_orphans`) build sessions via `upsert_session` (which leaves `worktree_key` NULL). Update them to set the key via `set_worktree_key` after creating each session, and adjust expectations to the new name-based semantics. Replace `list_related_sessions_matches_null_worktree` (no longer meaningful) with name-based cases:

```rust
#[test]
fn related_matches_same_project_and_worktree_key() {
    let store = Store::open_in_memory().expect("store");
    store.upsert_host("local").unwrap();
    let a = store.upsert_session("a", "local", Some(1), None, 1, 1, "running", None).unwrap();
    let b = store.upsert_session("b", "local", Some(1), None, 1, 1, "running", None).unwrap();
    store.set_worktree_key(a, Some("main")).unwrap();
    store.set_worktree_key(b, Some("main")).unwrap();
    let related = store.list_related_sessions(a).unwrap();
    assert_eq!(related.len(), 1);
    assert_eq!(related[0].tmux_name, "b");
}

#[test]
fn related_excludes_different_worktree_key() {
    let store = Store::open_in_memory().expect("store");
    store.upsert_host("local").unwrap();
    let a = store.upsert_session("a", "local", Some(1), None, 1, 1, "running", None).unwrap();
    let b = store.upsert_session("b", "local", Some(1), None, 1, 1, "running", None).unwrap();
    store.set_worktree_key(a, Some("main")).unwrap();
    store.set_worktree_key(b, Some("feat-x")).unwrap();
    assert!(store.list_related_sessions(a).unwrap().is_empty());
}

#[test]
fn related_matches_across_hosts_same_key() {
    let store = Store::open_in_memory().expect("store");
    store.upsert_host("local").unwrap();
    store.upsert_host("mefistos").unwrap();
    let a = store.upsert_session("a", "local", Some(1), None, 1, 1, "running", None).unwrap();
    let b = store.upsert_session("b", "mefistos", Some(1), None, 1, 1, "running", None).unwrap();
    store.set_worktree_key(a, Some("main")).unwrap();
    store.set_worktree_key(b, Some("main")).unwrap();
    let related = store.list_related_sessions(a).unwrap();
    assert_eq!(related.len(), 1);
    assert_eq!(related[0].host_alias, "mefistos");
}

#[test]
fn related_returns_empty_for_null_key() {
    let store = Store::open_in_memory().expect("store");
    store.upsert_host("local").unwrap();
    let a = store.upsert_session("a", "local", Some(1), None, 1, 1, "running", None).unwrap();
    let _b = store.upsert_session("b", "local", Some(1), None, 1, 1, "running", None).unwrap();
    // a has no worktree_key set → matches nothing.
    assert!(store.list_related_sessions(a).unwrap().is_empty());
}
```

Keep `list_related_sessions_excludes_orphans` (project_id NULL → []) — still valid.

Confirm `upsert_session` signature: `(tmux_name, host_alias, project_id, worktree_id, created_at, last_activity_at, status, account_uuid) -> i64`. Match the actual arg order.

- [ ] **Step 6: Run + commit**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -8
```

Expected: 115 + new related tests, existing reconcile/event tests green.

```bash
git add src-tauri/src/store.rs src-tauri/src/commands/sessions.rs
git commit -m "reconcile: populate worktree_key + match related sessions on it"
```

---

## M2 — Frontend

### Task 3: SessionRow.worktree_key + Sidebar/SessionDetails grouping

**Files:**
- Modify: `src/lib/sessions.ts`, `src/lib/Sidebar.svelte`, `src/lib/SessionDetails.svelte`, test files with SessionRow fixtures

- [ ] **Step 1: Extend the TS type**

In `src/lib/sessions.ts`, `SessionRow` interface — add after `reviews_session_id`:

```ts
  worktree_key: string | null;
```

- [ ] **Step 2: Sidebar relatedCountById groups by worktree_key**

In `src/lib/Sidebar.svelte`, the `relatedCountById` `$derived.by`:

```ts
  const relatedCountById = $derived.by(() => {
    const grouped = new Map<string, SessionRow[]>();
    for (const s of $sessions) {
      if (s.project_id == null || s.worktree_key == null) continue;
      const key = `${s.project_id}:${s.worktree_key}`;
      if (!grouped.has(key)) grouped.set(key, []);
      grouped.get(key)!.push(s);
    }
    const out = new Map<number, number>();
    for (const list of grouped.values()) {
      for (const s of list) out.set(s.id, list.length - 1);
    }
    return out;
  });
```

(Was grouping on `${s.project_id}:${s.worktree_id ?? 'null'}`. Sessions with null `worktree_key` are skipped — transient pre-reconcile, no false grouping.)

- [ ] **Step 3: SessionDetails related panel matches on worktree_key**

In `src/lib/SessionDetails.svelte`, the related-sessions `$derived` — change the sibling filter to match `project_id` AND `worktree_key` (both non-null, equal), excluding self:

```ts
  const related = $derived(
    session.project_id == null || session.worktree_key == null
      ? []
      : $sessions.filter(
          (s) =>
            s.id !== session.id &&
            s.project_id === session.project_id &&
            s.worktree_key === session.worktree_key,
        ),
  );
```

(Read the file first — match the actual variable name for the current session and the existing related-derived shape; it previously filtered on `worktree_id`.)

- [ ] **Step 4: Patch SessionRow fixtures**

```bash
grep -rn "reviews_session_id: null" src/ | grep -v node_modules
```

Every SessionRow literal / `sessionFor` helper that has `reviews_session_id` needs `worktree_key` too. Add `worktree_key: 'main'` (or `null` where the test wants an orphan/unkeyed row) to each. For the Sidebar related-badge test and the new grouping behaviour, ensure the related sessions share the same `worktree_key` (e.g. `'main'`) so the badge still appears; the perf-test fixtures can use `worktree_key: 'main'`.

- [ ] **Step 5: Update Sidebar + SessionDetails tests for the new semantics**

- Sidebar related-badge test: the two sibling sessions must share `worktree_key` to show 🔗. Add a counter-case: two same-project sessions with DIFFERENT `worktree_key` show NO badge.
- SessionDetails related test (if present): siblings share `worktree_key`; a different-key session is excluded.

Match the existing test conventions (the `sessionFor` helper, `mockBackend`, render pattern).

- [ ] **Step 6: Run + commit**

```bash
pnpm tsc --noEmit 2>&1 | tail -5
pnpm vitest run 2>&1 | tail -4
```

Expected: tsc clean (pre-existing carry forward), vitest 157 + new cases.

```bash
git add src/lib/sessions.ts src/lib/Sidebar.svelte src/lib/SessionDetails.svelte src/lib/*.test.ts
git commit -m "frontend: group related sessions by worktree_key"
```

---

## M3 — Live verify + push

### Task 4: Live verify + push

- [ ] **Step 1: Final sweep**

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -5
pnpm vitest run 2>&1 | tail -5
```

- [ ] **Step 2: Build + restart**

```bash
pnpm tauri build --bundles app 2>&1 | tail -6
pkill -f "claude-fleet.app/Contents/MacOS/claude-fleet" 2>/dev/null; sleep 1
open -a /Users/martinjanci/projects/github.com/martin-janci/claude-fleet/src-tauri/target/release/bundle/macos/claude-fleet.app
```

- [ ] **Step 3: Verify migration + data**

```bash
DB="/Users/martinjanci/Library/Application Support/sk.rlt.claude-fleet/state.db"
sqlite3 "$DB" "SELECT MAX(version) FROM schema_version;"   # → 6
sqlite3 -header -column "$DB" "SELECT tmux_name, host_alias, project_id, worktree_key FROM sessions;"
```

Expected: `worktree_key` populated (`main` for repo-root sessions) after the app's startup reconcile.

- [ ] **Step 4: Live UI verify**

1. Two local sessions in the same worktree (repo root) → both show 🔗1; SessionDetails Related panel lists the sibling.
2. Start a session in a different worktree (`git worktree add` then a session there, or just a session in `.claude/worktrees/<x>`) → it does NOT link to the `main` sessions (different key).
3. A local + mefistos session in the same repo's `main` → 🔗 link cross-host (if mefistos reachable).

- [ ] **Step 5: Push**

```bash
git fetch origin && git rebase origin/main   # integrate any mefistos commits
cargo test --manifest-path src-tauri/Cargo.toml --lib 2>&1 | tail -3   # re-verify post-rebase
pnpm vitest run 2>&1 | tail -3
git push origin main 2>&1 | tail -3
```

---

## Self-Review (filled in by plan author)

**Spec coverage check:**
- `worktree_key_for_path` derivation (root→main, named worktree, non-repo→None, remote==local) → Task 1 ✓
- Migration 006 + `SessionRow.worktree_key` (Rust) + projections + health bump → Task 1 ✓
- `set_worktree_key` (for tests + symmetry) → Task 1 ✓
- Reconcile computes + stores via `ReconcileSession` + `upsert_session_in_tx` → Task 2 ✓
- `list_related_sessions` matches on worktree_key (NULL → []) → Task 2 ✓
- Frontend `SessionRow.worktree_key` + Sidebar regroup + SessionDetails related filter → Task 3 ✓
- Tests: derivation, related (same/different/cross-host/null), frontend grouping → Tasks 1-3 ✓
- Live verify (same worktree links, different doesn't, cross-host) → Task 4 ✓
- Out of scope (worktree_id removal, remote worktree rows, Sidebar tree) → not present ✓

**Placeholder scan:** all code concrete (migration SQL, regex, query, derivation rules, test bodies). No TBD.

**Type consistency:**
- `worktree_key: Option<String>` (Rust) ↔ `worktree_key: string | null` (TS) — consistent.
- `worktree_key_for_path(&str) -> Option<String>` used in both reconcile callers.
- `set_worktree_key(id, Option<&str>)` consistent between def (Task 1) + test calls (Task 2).
- `upsert_session_in_tx` gains `worktree_key: Option<&str>`; `apply_host_reconcile` passes `s.worktree_key.as_deref()`.
- `list_related_sessions` projection includes `worktree_key: row.get(12)?` — index matches the 13-column SELECT (0-11 existing + 12 new).
- Tasks numbered 1-4; M1=1-2, M2=3, M3=4.
