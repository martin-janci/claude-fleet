# Resume the exact Claude session — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the app mint a stable Claude session UUID per work/review session, store it, and launch/relaunch `claude` with `--session-id`/`--resume` so recreate/restart return to the *exact* conversation that pane had (fixing the wrong-session bug when several Claude sessions share a worktree).

**Architecture:** Add a nullable `claude_session_id` column (migration 009) on `sessions`; a launch-command builder `pane_command_for(Option<&str>)` that creates-or-resumes by id; the service layer generates+stores the id on creation and passes it on recreate/restart. Legacy sessions (no id) fall back to today's `cl --continue`.

**Tech Stack:** Rust (Tauri 2 service + rusqlite store, `cargo test`), Svelte/TS (one interface field), `uuid` crate (v4).

> **Build/test:** backend from `src-tauri/` (`cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`); frontend from repo root (`pnpm check`, `pnpm exec vitest run`). On this branch the frontend suite is fully green (no `localStorage` env issue).

> **Reference spec:** `docs/specs/2026-05-22-resume-claude-session-design.md`
> **Working dir:** `/Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.worktrees/resume-claude-session` (branch `resume-claude-session`).

---

## File structure

New:
- `src-tauri/migrations/009_session_claude_id.sql`

Modified:
- `src-tauri/src/store.rs` — `SessionRow.claude_session_id`; migration gate; 4 row mappers; `set_claude_session_id`.
- `src-tauri/src/validate.rs` — `claude_session_id` validator.
- `src-tauri/src/tmux.rs` — `pane_command_for(Option<&str>)` (replaces `pane_command()`).
- `src-tauri/src/service/sessions.rs` — generate/store id in `new_session_inner` + `spawn_review`; id-aware `recreate_pane_command` + `restart_session`.
- `src-tauri/Cargo.toml` — add `uuid` (v4) as a direct dependency.
- `src/lib/sessions.ts` — `claude_session_id` field on `SessionRow`.

---

## Phase A — Storage

### Task 1: Migration 009 + `claude_session_id` column, mappers, setter

**Files:**
- Create: `src-tauri/migrations/009_session_claude_id.sql`
- Modify: `src-tauri/src/store.rs`

- [ ] **Step 1: Write the migration file**

Create `src-tauri/migrations/009_session_claude_id.sql` (mirrors 008's format):

```sql
ALTER TABLE sessions ADD COLUMN claude_session_id TEXT;
INSERT OR IGNORE INTO schema_version (version) VALUES (9);
```

- [ ] **Step 2: Register the migration in `migrate()`**

In `src-tauri/src/store.rs`, in `fn migrate`, after the `if v < 8 { … }` block (around line 189) and before `Ok(())`, add:

```rust
        if v < 9 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../migrations/009_session_claude_id.sql"))?;
            tx.commit()?;
        }
```

- [ ] **Step 3: Add the field to `SessionRow`**

In `src-tauri/src/store.rs`, in `pub struct SessionRow` (line ~33), add after `pub lost_at: Option<i64>,`:

```rust
    pub claude_session_id: Option<String>,
```

- [ ] **Step 4: Add the column to all four SessionRow mappers**

There are four functions that `SELECT … FROM sessions` and build a `SessionRow`. In EACH, (a) append `, claude_session_id` to the column list right after `worktree_key, lost_at`, and (b) add `claude_session_id: row.get(14)?,` right after `lost_at: row.get(13)?,`. The four sites:

1. `list_sessions_for_host` (line ~736)
2. `list_all_sessions` (line ~775)
3. `fn fetch_session` (line ~1282)
4. `fn fetch_session_by_id` (line ~1317)

Example — `fetch_session_by_id` becomes:

```rust
    let mut stmt = conn.prepare_cached(
        "SELECT id, tmux_name, host_alias, project_id, worktree_id, created_at,
                last_activity_at, status, notes, account_uuid, kind, reviews_session_id,
                worktree_key, lost_at, claude_session_id
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
            claude_session_id: row.get(14)?,
        })
    })?;
```

Apply the identical two edits (SELECT `+ , claude_session_id`; struct `+ claude_session_id: row.get(14)?,`) to the other three. After this, run `cargo build` once to confirm no remaining `SessionRow { … }` literal is missing the field (the compiler lists every site that needs it — fix any the grep missed, e.g. a test builder).

- [ ] **Step 5: Add `set_claude_session_id` + write the failing tests**

Add this method on `impl Store` (near `set_session_kind`, ~line 849):

```rust
    /// Record the Claude Code session id minted for a session. Reconcile's
    /// `upsert_session` never writes this column, so the value survives
    /// reconciliation.
    pub fn set_claude_session_id(&self, id: i64, uuid: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE sessions SET claude_session_id=?1 WHERE id=?2",
            rusqlite::params![uuid, id],
        )?;
        Ok(())
    }
```

Add tests to `#[cfg(test)] mod tests` in `store.rs`:

```rust
    #[test]
    fn claude_session_id_round_trips_and_defaults_none() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let id = s.get_session("dev", "local").unwrap().unwrap().id;
        // Fresh session has no claude id.
        assert_eq!(
            s.get_session_by_id(id).unwrap().unwrap().claude_session_id,
            None
        );
        // Set + read back.
        s.set_claude_session_id(id, "550e8400-e29b-41d4-a716-446655440000")
            .unwrap();
        assert_eq!(
            s.get_session_by_id(id).unwrap().unwrap().claude_session_id.as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
    }

    #[test]
    fn upsert_session_preserves_claude_session_id() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let id = s.get_session("dev", "local").unwrap().unwrap().id;
        s.set_claude_session_id(id, "550e8400-e29b-41d4-a716-446655440000")
            .unwrap();
        // A reconcile-style re-upsert for the same (host, name) must NOT clobber it.
        s.upsert_session("dev", "local", None, None, 1, 2, "running", None)
            .unwrap();
        assert_eq!(
            s.get_session_by_id(id).unwrap().unwrap().claude_session_id.as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
    }
```

- [ ] **Step 6: Run + verify the upsert-preserve assumption**

Run: `cd src-tauri && cargo test claude_session_id upsert_session_preserves_claude_session_id`
Expected: PASS. If `upsert_session_preserves_claude_session_id` FAILS, it means `upsert_session`'s `ON CONFLICT DO UPDATE` sets columns that reset `claude_session_id` — inspect `upsert_session` (line ~663) and `upsert_session_in_tx` (line ~1001) and confirm neither lists `claude_session_id` in its UPDATE SET clause (they shouldn't, since the column is new and not added to those statements). The new column must remain absent from both upsert statements. Re-run until green.

- [ ] **Step 7: Full backend tests + clippy/fmt + commit**

Run: `cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: all clean (the migration applies in `open_in_memory`; all existing session tests still pass with the new column).

```bash
git add src-tauri/migrations/009_session_claude_id.sql src-tauri/src/store.rs
git commit -m "feat(store): claude_session_id column + setter (migration 009)"
```

---

## Phase B — Validator + launch-command builder

### Task 2: `validate::claude_session_id`

**Files:**
- Modify: `src-tauri/src/validate.rs`

- [ ] **Step 1: Write the failing test**

Add to `#[cfg(test)] mod tests` in `validate.rs`:

```rust
    #[test]
    fn claude_session_id_accepts_uuid_rejects_junk() {
        assert!(claude_session_id("550e8400-e29b-41d4-a716-446655440000").is_ok());
        assert!(claude_session_id("550E8400-E29B-41D4-A716-446655440000").is_err()); // uppercase
        assert!(claude_session_id("").is_err());
        assert!(claude_session_id("not-a-uuid").is_err());
        assert!(claude_session_id("550e8400e29b41d4a716446655440000").is_err()); // no hyphens
        assert!(claude_session_id("550e8400-e29b-41d4-a716-44665544000g").is_err()); // non-hex
        assert!(claude_session_id("'; rm -rf / #").is_err());
        assert!(claude_session_id(&"a".repeat(36)).is_err()); // right length, wrong shape
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd src-tauri && cargo test claude_session_id`
Expected: FAIL — function not found.

- [ ] **Step 3: Implement the validator**

Add to `validate.rs` (after `commit_hash`):

```rust
/// Validate a Claude Code session id (a canonical lowercase UUID,
/// `8-4-4-4-12` hex). The app generates these as UUIDv4 and interpolates them
/// into the pane launch command, so this guards a tampered DB value from
/// injecting shell. Anything not matching the exact shape is rejected.
pub fn claude_session_id(value: &str) -> Result<(), IpcError> {
    let groups = [8usize, 4, 4, 4, 12];
    let parts: Vec<&str> = value.split('-').collect();
    let shape_ok = parts.len() == groups.len()
        && parts.iter().zip(groups).all(|(p, n)| {
            p.len() == n && p.chars().all(|c| c.is_ascii_digit() || matches!(c, 'a'..='f'))
        });
    if shape_ok {
        Ok(())
    } else {
        Err(IpcError::new(
            "E_INVALID",
            "claude session id must be a lowercase UUID",
        ))
    }
}
```

> Note: this is a `pub fn` in the private `validate` module with no production caller until Task 4/5. Run clippy after this task; if `-D warnings` flags it as `dead_code`, add a temporary `#[allow(dead_code)]` with a `// wired up in the recreate/restart launch path` comment, and REMOVE it in Task 5 when the caller lands. (Same pattern used elsewhere in this codebase.)

- [ ] **Step 4: Run to verify it passes + clippy/fmt**

Run: `cd src-tauri && cargo test claude_session_id && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS + clean.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/validate.rs
git commit -m "feat(validate): claude_session_id UUID validator"
```

### Task 3: `tmux::pane_command_for`

**Files:**
- Modify: `src-tauri/src/tmux.rs`

- [ ] **Step 1: Write the failing test**

Add to `#[cfg(test)] mod tests` in `tmux.rs`:

```rust
    #[test]
    fn pane_command_for_resumes_or_creates_with_id() {
        let id = "550e8400-e29b-41d4-a716-446655440000";
        let cmd = pane_command_for(Some(id));
        assert!(cmd.contains(&format!("cl --resume '{id}'")), "got: {cmd}");
        assert!(cmd.contains(&format!("cl --session-id '{id}'")), "got: {cmd}");
        assert!(cmd.contains("|| cl;"), "bare fallback missing: {cmd}");
        assert!(cmd.contains("exec ${SHELL"), "got: {cmd}");
    }

    #[test]
    fn pane_command_for_none_uses_continue() {
        let cmd = pane_command_for(None);
        assert!(cmd.contains("cl --continue || cl;"), "got: {cmd}");
        assert!(!cmd.contains("--session-id"), "got: {cmd}");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd src-tauri && cargo test pane_command_for`
Expected: FAIL — function not found.

- [ ] **Step 3: Implement `pane_command_for`** (keep the existing `pane_command()` for now — its callers are switched in Tasks 4–5)

Add to `tmux.rs` next to `pane_command`:

```rust
/// The pane command for a Claude ("work"/"review") session. With a known
/// session id: resume it, else create it under that id, else a bare `cl` — an
/// idempotent create-or-resume. Without an id (legacy rows): today's
/// most-recent-for-cwd behavior. The id is single-quoted; callers validate it
/// with `validate::claude_session_id` before passing it in.
pub fn pane_command_for(claude_session_id: Option<&str>) -> String {
    let tail = "exec ${SHELL:-/bin/zsh} -l";
    match claude_session_id {
        Some(id) => {
            format!("cl --resume '{id}' 2>/dev/null || cl --session-id '{id}' || cl; {tail}")
        }
        None => format!("cl --continue || cl; {tail}"),
    }
}
```

- [ ] **Step 4: Run to verify it passes + clippy/fmt**

Run: `cd src-tauri && cargo test pane_command_for && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS + clean (`pane_command()` still has callers, so no dead_code).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/tmux.rs
git commit -m "feat(tmux): pane_command_for builds create-or-resume launch command"
```

---

## Phase C — Service wiring

### Task 4: Generate + store the id in `new_session_inner` and `spawn_review`

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/src/service/sessions.rs`

- [ ] **Step 1: Add the `uuid` dependency**

`uuid` is currently only a transitive dep. In `src-tauri/Cargo.toml`, under `[dependencies]`, add:

```toml
uuid = { version = "1", features = ["v4"] }
```

Run `cd src-tauri && cargo build` to confirm it resolves (it's already in the lockfile transitively).

- [ ] **Step 2: Use a generated id for work sessions in `new_session_inner`**

In `src-tauri/src/service/sessions.rs`, the tail of `new_session_inner` currently builds `pane_cmd` then launches and returns `row`. Change the work-session branch to mint an id, launch with it, and store it after the row is found.

Replace the pane-command block:

```rust
    let is_shell = args.kind.as_deref() == Some("shell");
    let pane_cmd: String = if is_shell {
        crate::tmux::shell_pane_command(args.start_command.as_deref())
    } else {
        crate::tmux::pane_command().to_string()
    };
```

with:

```rust
    let is_shell = args.kind.as_deref() == Some("shell");
    // Work/review sessions get an app-minted Claude session id so a later
    // recreate/restart resumes THIS conversation, not "most recent for the cwd".
    let claude_id: Option<String> = if is_shell {
        None
    } else {
        Some(uuid::Uuid::new_v4().to_string())
    };
    let pane_cmd: String = if is_shell {
        crate::tmux::shell_pane_command(args.start_command.as_deref())
    } else {
        crate::tmux::pane_command_for(claude_id.as_deref())
    };
```

Then, after the `row` is found and before the `if is_shell { … }` tag block, persist the id for work sessions. Change the existing tail so it reads:

```rust
    // Reconcile inserts every session as kind="work"; tag shell sessions
    // afterwards. The session upsert preserves `kind` on re-reconcile.
    if is_shell {
        s.set_session_kind(row.id, "shell", None)?;
        return s
            .get_session(&args.name, &args.host_alias)?
            .ok_or_else(|| IpcError::new("E_INTERNAL", "session vanished after kind tag"));
    }
    // Persist the minted Claude session id. Soft-fail: the session is live; a
    // failed write just means a future recreate falls back to `cl --continue`.
    let mut row = row;
    if let Some(ref cid) = claude_id {
        if let Err(e) = s.set_claude_session_id(row.id, cid) {
            eprintln!("new_session: storing claude_session_id for {} failed: {e:?}", args.name);
        } else {
            row.claude_session_id = Some(cid.clone());
        }
    }
    Ok(row)
```

(The `let mut row = row;` shadows the existing `let row = …` binding so we can patch the returned value; keep the earlier `let row = s.list_sessions_for_host(...)…?;` as-is.)

- [ ] **Step 3: Mint + store an id in `spawn_review`**

In `spawn_review`, it currently launches with `crate::tmux::pane_command()` and later tags the review row via `set_session_kind`. Change the launch to use a minted id and store it after tagging.

Where it builds the review session:

```rust
    let tmux = exec_for(&source.host_alias, ssh);
    tmux.new_session(
        &review_name,
        std::path::Path::new(&cwd),
        crate::tmux::pane_command(),
    )
    .await?;
```

becomes:

```rust
    let claude_id = uuid::Uuid::new_v4().to_string();
    let tmux = exec_for(&source.host_alias, ssh);
    tmux.new_session(
        &review_name,
        std::path::Path::new(&cwd),
        &crate::tmux::pane_command_for(Some(&claude_id)),
    )
    .await?;
```

Then in step 4 of `spawn_review` (the block that finds the row and calls `set_session_kind(row.id, "review", Some(source.id))`), after `set_session_kind`, add:

```rust
        s.set_session_kind(row.id, "review", Some(source.id))?;
        let _ = s.set_claude_session_id(row.id, &claude_id); // soft-fail
        row.id
```

(Adjust to the existing binding — the block already captures `row.id`; just add the `set_claude_session_id` line before `row.id`. If `row` is not `mut`/needed beyond `.id`, no patch to the returned row is required since `spawn_review` re-fetches via `get_session_by_id(review_id)` at the end — but that re-fetch will now include the stored id automatically.)

- [ ] **Step 4: Build + clippy/fmt**

Run: `cd src-tauri && cargo build && cargo test commands::sessions service::sessions 2>/dev/null; cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: builds clean (`pane_command()` is still used by `recreate_pane_command` + `restart_session`, so no dead_code yet).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/src/service/sessions.rs
git commit -m "feat(sessions): mint + store a Claude session id for new work/review sessions"
```

### Task 5: id-aware `recreate_session` + `restart_session`; retire `pane_command()`

**Files:**
- Modify: `src-tauri/src/service/sessions.rs`
- Modify: `src-tauri/src/tmux.rs`

- [ ] **Step 1: Update the `recreate_pane_command` test**

In `#[cfg(test)] mod tests` in `service/sessions.rs`, the existing `recreate_pane_command_matches_kind` test calls `recreate_pane_command("shell")` etc. (one arg). Replace it with the new two-arg signature:

```rust
    #[test]
    fn recreate_pane_command_matches_kind_and_id() {
        let id = "550e8400-e29b-41d4-a716-446655440000";
        // shell ignores the id and returns the bare shell command.
        assert_eq!(
            recreate_pane_command("shell", Some(id)),
            crate::tmux::shell_pane_command(None)
        );
        // work + id → resume-or-create.
        assert_eq!(
            recreate_pane_command("work", Some(id)),
            crate::tmux::pane_command_for(Some(id))
        );
        // work, no id → legacy continue.
        assert_eq!(
            recreate_pane_command("work", None),
            crate::tmux::pane_command_for(None)
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd src-tauri && cargo test recreate_pane_command`
Expected: FAIL — signature mismatch (still one arg).

- [ ] **Step 3: Make `recreate_pane_command` id-aware**

Replace the existing helper:

```rust
fn recreate_pane_command(kind: &str) -> String {
    if kind == "shell" {
        crate::tmux::shell_pane_command(None)
    } else {
        crate::tmux::pane_command().to_string()
    }
}
```

with:

```rust
/// The pane command to relaunch when (re)creating a session. `shell` → a bare
/// shell; otherwise resume the session's own Claude id (or `--continue` for a
/// legacy session with no stored id). A stored id is validated before use so a
/// tampered DB value can't inject shell — an invalid id degrades to `None`.
fn recreate_pane_command(kind: &str, claude_session_id: Option<&str>) -> String {
    if kind == "shell" {
        return crate::tmux::shell_pane_command(None);
    }
    let id = claude_session_id.filter(|id| crate::validate::claude_session_id(id).is_ok());
    crate::tmux::pane_command_for(id)
}
```

- [ ] **Step 4: Pass the stored id from `recreate_session`**

In `recreate_session`, the line that computes the pane command is currently:

```rust
        let pane_cmd = recreate_pane_command(&sess.kind);
```

Change it to:

```rust
        let pane_cmd = recreate_pane_command(&sess.kind, sess.claude_session_id.as_deref());
```

- [ ] **Step 5: Make `restart_session` id-aware**

In `restart_session`, it currently reads only `is_shell` from the row and builds:

```rust
    let pane_cmd: String = if is_shell {
        crate::tmux::shell_pane_command(None)
    } else {
        crate::tmux::pane_command().to_string()
    };
```

Replace the lookup + pane_cmd so it reads both `kind` and `claude_session_id` under the lock and uses the shared helper. Change the block that fetches `is_shell`:

```rust
    let is_shell = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.get_session(&args.name, &args.host_alias)?
            .map(|r| r.kind == "shell")
            .unwrap_or(false)
    };
    let pane_cmd: String = if is_shell {
        crate::tmux::shell_pane_command(None)
    } else {
        crate::tmux::pane_command().to_string()
    };
```

with:

```rust
    let (kind, claude_id) = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        match s.get_session(&args.name, &args.host_alias)? {
            Some(r) => (r.kind, r.claude_session_id),
            None => ("work".to_string(), None),
        }
    };
    let pane_cmd: String = recreate_pane_command(&kind, claude_id.as_deref());
```

- [ ] **Step 6: Retire the now-unused `pane_command()`**

After Steps 3–5, nothing calls `crate::tmux::pane_command()` anymore. Remove the `pane_command()` function from `tmux.rs` (and, if Task 2 added a temporary `#[allow(dead_code)]` to `validate::claude_session_id`, REMOVE that attribute now — `recreate_pane_command` is its production caller). Confirm with `grep -rn "pane_command()" src-tauri/src` → no results.

> If any `tmux.rs` unit test referenced `pane_command()` (e.g. `pane_command_falls_back_to_shell_after_claude_exits`), update it to assert against `pane_command_for(None)` instead, preserving the intent.

- [ ] **Step 7: Run all backend tests + clippy/fmt**

Run: `cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: all PASS/clean. `grep -rn "crate::tmux::pane_command()\|fn pane_command(" src-tauri/src` returns nothing.

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/service/sessions.rs src-tauri/src/tmux.rs
git commit -m "feat(sessions): recreate/restart resume the session's own Claude id"
```

---

## Phase D — Frontend + verification

### Task 6: Frontend field + full verification

**Files:**
- Modify: `src/lib/sessions.ts`

- [ ] **Step 1: Add the field to the TS `SessionRow`**

In `src/lib/sessions.ts`, in `export interface SessionRow`, add after `lost_at: number | null;`:

```ts
  claude_session_id: string | null;
```

- [ ] **Step 2: Type-check**

Run (repo root): `pnpm check`
Expected: 0 errors (the backend now serializes the extra field; the interface matches).

- [ ] **Step 3: Full suites**

Run: `cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Run (repo root): `pnpm exec vitest run`
Expected: backend all green; frontend all green.

- [ ] **Step 4: Manual smoke (run the app)**

`pnpm tauri dev`. Then:
- Create a work session; in its pane run `claude` does start (via `--session-id`). Do a tiny exchange.
- Create a SECOND work session in the SAME repo/worktree; do a different exchange.
- Recreate the FIRST session (Sidebar `♻` or SessionDetails) → confirm it resumes the FIRST conversation (not the second). Recreate the second → confirm it resumes the second.
- Restart a session → confirm it resumes its own conversation too.
- (Legacy check) For a session that existed before this change (no stored id), recreate → it still comes up via `--continue` (no crash).

- [ ] **Step 5: Commit**

```bash
git add src/lib/sessions.ts
git commit -m "feat(sessions): carry claude_session_id on the frontend SessionRow"
```

---

## Self-review notes (resolved)

- **Spec coverage:** migration + column + setter + upsert-preserve (Task 1); validator (Task 2); `pane_command_for` (Task 3); generate/store in new_session + spawn_review (Task 4); id-aware recreate + restart, retire `pane_command()` (Task 5); frontend field + verification + manual multi-session smoke (Task 6). Covered.
- **The four mappers:** Task 1 Step 4 enumerates all four `SessionRow` build sites and relies on `cargo build` to surface any missed literal (e.g. a test helper) — the compiler errors on a missing struct field, so nothing is silently skipped.
- **Clean-commit ordering:** `pane_command_for` is added in Task 3 while `pane_command()` keeps its callers; Task 4 switches new/review; Task 5 switches recreate/restart and only THEN removes `pane_command()` — so every commit is clippy-clean. The `validate::claude_session_id` dead-code window (Task 2 → Task 5) is handled with a temporary `#[allow(dead_code)]`.
- **Type/name consistency:** `claude_session_id` (column / `SessionRow` field / TS field), `set_claude_session_id`, `pane_command_for(Option<&str>)`, `recreate_pane_command(&str, Option<&str>)`, `validate::claude_session_id` are used consistently across tasks.
