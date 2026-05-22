# Robust Worktree Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make claude-fleet detect git worktrees whose directory was deleted/corrupted, mark them "missing" then auto-prune (ghosting their sessions), and offer a UI "Recreate" action — without breaking on the worktree `.git`-as-file case.

**Architecture:** Detection rides the existing local `git worktree list --porcelain` scan (parse the `prunable` marker) plus a disk-existence check. A nullable `missing_since` column on `worktrees` drives a two-phase lifecycle that mirrors the proven ghost-session flow: first refresh marks missing, a later refresh auto-prunes (`git worktree prune` + delete row + ghost sessions). A `recreate_worktree` command rebuilds from the stored path/branch. The frontend surfaces a "missing" badge + Recreate button.

**Tech Stack:** Rust, Tauri 2, rusqlite (SQLite), tokio; Svelte 5 + TypeScript; Vitest + cargo test.

---

## File Structure

- `src-tauri/migrations/009_worktree_missing.sql` — **new.** Adds `missing_since`.
- `src-tauri/src/store.rs` — **modify.** `WorktreeRow.missing_since`; SELECTs; `upsert_worktree` clears the flag; new `get_worktree_by_name`, `mark_worktree_missing`, `prune_missing_worktree`, `worktree_recreate_info`.
- `src-tauri/src/projects.rs` — **modify.** `DiscoveredWorktree.is_prunable` + parse the `prunable` line.
- `src-tauri/src/service/projects.rs` — **modify.** Two-phase reconcile in `refresh_projects`; off-lock `git worktree prune`.
- `src-tauri/src/service/sessions.rs` — **modify.** `recreate_worktree` + `worktree_recreate_script`.
- `src-tauri/src/commands/projects.rs` — **modify.** `recreate_worktree` command.
- `src-tauri/src/lib.rs` — **modify.** Register the command.
- `src/lib/projects.ts` — **modify.** `WorktreeRow.missing_since`; `recreateWorktree` wrapper.
- `src/lib/ProjectDetails.svelte` — **modify.** Missing badge + Recreate button.
- `src/lib/NewSessionDialog.svelte` — **modify.** Skip missing as default; mark + Recreate.

All work happens in the worktree at `.worktrees/worktree-robustness` (branch `worktree-robustness`). Run cargo commands from `.worktrees/worktree-robustness/src-tauri`, pnpm from `.worktrees/worktree-robustness`.

---

## Task 1: Migration + `missing_since` on the worktree row

**Files:**
- Create: `src-tauri/migrations/009_worktree_missing.sql`
- Modify: `src-tauri/src/store.rs` (`WorktreeRow` ~line 24; `get_worktree` ~253; `list_worktrees_for_project` ~398; `list_projects_joined` SELECT ~320; `upsert_worktree` ~375)

- [ ] **Step 1: Write the migration**

Create `src-tauri/migrations/009_worktree_missing.sql`:

```sql
-- A worktree whose working directory has gone missing on disk is marked here
-- with the unix time it was first observed missing (NULL = present). Drives the
-- two-phase mark→auto-prune lifecycle, mirroring sessions.lost_at.
ALTER TABLE worktrees ADD COLUMN missing_since INTEGER;
```

- [ ] **Step 2: Add the field to `WorktreeRow`**

In `src-tauri/src/store.rs`, the struct (~line 24) becomes:

```rust
#[derive(Debug, Clone, serde::Serialize)]
pub struct WorktreeRow {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
    pub missing_since: Option<i64>,
}
```

- [ ] **Step 3: Update the three reads to select `missing_since`**

`get_worktree` (~line 253):

```rust
    fn get_worktree(&self, id: i64) -> Result<Option<WorktreeRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, project_id, name, path, branch, missing_since FROM worktrees WHERE id=?1",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![id], |row| {
            Ok(WorktreeRow {
                id: row.get(0)?,
                project_id: row.get(1)?,
                name: row.get(2)?,
                path: row.get(3)?,
                branch: row.get(4)?,
                missing_since: row.get(5)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }
```

`list_worktrees_for_project` (~line 398):

```rust
    pub fn list_worktrees_for_project(
        &self,
        project_id: i64,
    ) -> Result<Vec<WorktreeRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, project_id, name, path, branch, missing_since FROM worktrees WHERE project_id=?1 ORDER BY name",
        )?;
        let rows = stmt.query_map(rusqlite::params![project_id], |row| {
            Ok(WorktreeRow {
                id: row.get(0)?,
                project_id: row.get(1)?,
                name: row.get(2)?,
                path: row.get(3)?,
                branch: row.get(4)?,
                missing_since: row.get(5)?,
            })
        })?;
        rows.collect()
    }
```

`list_projects_joined` — extend the SELECT (~line 320) to add `w.missing_since`, the tuple closure, and the `WorktreeRow` build (~line 363):

```rust
        let mut stmt = self.conn.prepare_cached(
            "SELECT p.id, p.owner, p.repo, p.base_path, p.last_session_at,
                    w.id, w.project_id, w.name, w.path, w.branch, w.missing_since
             FROM projects p
             LEFT JOIN worktrees w ON w.project_id = p.id
             ORDER BY
               CASE WHEN p.last_session_at IS NULL THEN 1 ELSE 0 END,
               p.last_session_at DESC,
               p.id,
               w.id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<i64>>(10)?,
            ))
        })?;
```

Then update the destructure + push (~line 348-371). The tuple now has 11 fields:

```rust
        let mut out: Vec<crate::service::projects::ProjectTreeRow> = Vec::new();
        let mut last_pid: Option<i64> = None;
        for r in rows {
            let (pid, owner, repo, base, last, wid, _wpid, wname, wpath, wbranch, wmissing) = r?;
            if last_pid != Some(pid) {
                out.push(crate::service::projects::ProjectTreeRow {
                    project: ProjectRow {
                        id: pid,
                        owner,
                        repo,
                        base_path: base,
                        last_session_at: last,
                    },
                    worktrees: Vec::new(),
                });
                last_pid = Some(pid);
            }
            if let (Some(wid), Some(wname), Some(wpath)) = (wid, wname, wpath) {
                out.last_mut().unwrap().worktrees.push(WorktreeRow {
                    id: wid,
                    project_id: pid,
                    name: wname,
                    path: wpath,
                    branch: wbranch,
                    missing_since: wmissing,
                });
            }
        }
        Ok(out)
```

- [ ] **Step 4: `upsert_worktree` clears `missing_since` when a worktree is present again**

Replace the `INSERT … ON CONFLICT` in `upsert_worktree` (~line 383) so a re-seen worktree clears the flag:

```rust
        self.conn.execute(
            "INSERT INTO worktrees (project_id, name, path, branch) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(project_id, name) DO UPDATE SET
               path=excluded.path, branch=excluded.branch, missing_since=NULL",
            rusqlite::params![project_id, name, path, branch],
        )?;
```

- [ ] **Step 5: Write a round-trip test**

Add to the `#[cfg(test)] mod tests` in `store.rs` (near `worktrees_upsert_list_and_prune`, ~line 1447). Use the existing in-test `Store` constructor pattern from that test (copy how it builds the store + a project):

```rust
    #[test]
    fn worktree_missing_since_defaults_null_and_roundtrips() {
        let store = Store::open_in_memory().unwrap();
        let pid = store.upsert_project("o", "r", "/tmp/r").unwrap();
        let id = store.upsert_worktree(pid, "feat", "/tmp/r/.worktrees/feat", Some("feat")).unwrap();
        let wt = store.get_worktree(id).unwrap().unwrap();
        assert_eq!(wt.missing_since, None);
    }
```

(If the existing tests use a different constructor than `Store::open_in_memory`, match theirs — read `worktrees_upsert_list_and_prune` first and reuse its setup verbatim.)

- [ ] **Step 6: Run tests**

Run: `cd src-tauri && cargo test --lib store::tests::worktree_missing_since_defaults_null_and_roundtrips`
Expected: PASS (migration applies, column round-trips).

- [ ] **Step 7: Commit**

```bash
git add src-tauri/migrations/009_worktree_missing.sql src-tauri/src/store.rs
git commit -m "feat(store): worktrees.missing_since column + clear-on-present"
```

---

## Task 2: Parse the `prunable` marker in the worktree scan

**Files:**
- Modify: `src-tauri/src/projects.rs` (`DiscoveredWorktree` ~line 13; `parse_worktree_porcelain` ~78; `make_worktree` ~99; tests ~110)

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `src-tauri/src/projects.rs`:

```rust
    #[test]
    fn parse_marks_prunable_worktree() {
        let main = Path::new("/repo");
        let input = "worktree /repo\nHEAD aaaa\nbranch refs/heads/main\n\n\
                     worktree /repo/.worktrees/gone\nHEAD bbbb\nbranch refs/heads/gone\n\
                     prunable gitdir file points to non-existent location\n\n";
        let wts = parse_worktree_porcelain(input, main);
        assert_eq!(wts.len(), 2);
        assert_eq!(wts[0].name, "main");
        assert!(!wts[0].is_prunable);
        assert_eq!(wts[1].name, "gone");
        assert!(wts[1].is_prunable);
        assert_eq!(wts[1].branch.as_deref(), Some("gone"));
    }
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cd src-tauri && cargo test --lib projects::tests::parse_marks_prunable_worktree`
Expected: FAIL — `is_prunable` field does not exist.

- [ ] **Step 3: Add the field**

In `src-tauri/src/projects.rs`, extend the struct (~line 13):

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoveredWorktree {
    pub name: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    /// True when `git worktree list` flagged this entry `prunable` — its
    /// working tree is gone or unusable.
    pub is_prunable: bool,
}
```

- [ ] **Step 4: Parse the `prunable` line and thread it through**

Rewrite `parse_worktree_porcelain` and `make_worktree` (~line 78-108):

```rust
fn parse_worktree_porcelain(input: &str, main_path: &Path) -> Vec<DiscoveredWorktree> {
    let mut out = Vec::new();
    let mut cur_path: Option<PathBuf> = None;
    let mut cur_branch: Option<String> = None;
    let mut cur_prunable = false;
    for line in input.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(path) = cur_path.take() {
                out.push(make_worktree(path, cur_branch.take(), cur_prunable, main_path));
            }
            cur_path = Some(PathBuf::from(rest));
            cur_branch = None;
            cur_prunable = false;
        } else if let Some(rest) = line.strip_prefix("branch ") {
            cur_branch = Some(rest.trim_start_matches("refs/heads/").to_string());
        } else if line == "prunable" || line.starts_with("prunable ") {
            cur_prunable = true;
        }
    }
    if let Some(path) = cur_path {
        out.push(make_worktree(path, cur_branch, cur_prunable, main_path));
    }
    out
}

fn make_worktree(
    path: PathBuf,
    branch: Option<String>,
    is_prunable: bool,
    main_path: &Path,
) -> DiscoveredWorktree {
    let name = if path == main_path {
        "main".to_string()
    } else {
        path.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".to_string())
    };
    DiscoveredWorktree {
        name,
        path,
        branch,
        is_prunable,
    }
}
```

- [ ] **Step 5: Fix the existing parse test's expectations**

The existing test (~line 154-183) builds `DiscoveredWorktree` literals or compares fields. If it constructs the struct directly, add `is_prunable: false` to those literals. Run the whole module to find any compile breakage:

Run: `cd src-tauri && cargo test --lib projects::tests`
Expected: all parse tests PASS (including the new one).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/projects.rs
git commit -m "feat(projects): parse 'prunable' marker from git worktree list"
```

---

## Task 3: Store methods — mark missing, prune (ghost sessions), recreate-info

**Files:**
- Modify: `src-tauri/src/store.rs` (add methods near `upsert_worktree`/`worktree_path`)

- [ ] **Step 1: Write the failing test**

Add to `store.rs` tests:

```rust
    #[test]
    fn mark_and_prune_missing_worktree_ghosts_sessions() {
        let store = Store::open_in_memory().unwrap();
        let pid = store.upsert_project("o", "r", "/tmp/r").unwrap();
        let wid = store.upsert_worktree(pid, "feat", "/tmp/r/.worktrees/feat", Some("feat")).unwrap();
        // A live session attached to this worktree.
        let sid = store
            .upsert_session("sess-feat", "local", Some(pid), Some(wid))
            .unwrap();

        // get_worktree_by_name finds it; not yet missing.
        let found = store.get_worktree_by_name(pid, "feat").unwrap().unwrap();
        assert_eq!(found.id, wid);
        assert_eq!(found.missing_since, None);

        // Phase 1: mark missing.
        store.mark_worktree_missing(pid, "feat", 1234).unwrap();
        assert_eq!(store.get_worktree(wid).unwrap().unwrap().missing_since, Some(1234));
        // mark again is a no-op (does not overwrite the original timestamp).
        store.mark_worktree_missing(pid, "feat", 9999).unwrap();
        assert_eq!(store.get_worktree(wid).unwrap().unwrap().missing_since, Some(1234));

        // Phase 2: prune — row gone, session ghosted.
        store.prune_missing_worktree(wid, 5678).unwrap();
        assert!(store.get_worktree(wid).unwrap().is_none());
        let sess = store.get_session_by_id(sid).unwrap().unwrap();
        assert_eq!(sess.status, "ghost");
        assert_eq!(sess.lost_at, Some(5678));
    }
```

Note: match the real `upsert_session` signature — read it first (`store.rs` ~line 668). If it differs (extra args like `kind`), adapt the call. The assertions on `status`/`lost_at` are the point.

- [ ] **Step 2: Run it to confirm it fails**

Run: `cd src-tauri && cargo test --lib store::tests::mark_and_prune_missing_worktree_ghosts_sessions`
Expected: FAIL — methods don't exist.

- [ ] **Step 3: Implement the three methods**

Add to the `impl Store` block (after `upsert_worktree`, ~line 396):

```rust
    /// Look a worktree up by its `(project_id, name)` key — used by the refresh
    /// reconcile to read the current `missing_since` before deciding phase.
    pub fn get_worktree_by_name(
        &self,
        project_id: i64,
        name: &str,
    ) -> Result<Option<WorktreeRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, project_id, name, path, branch, missing_since
             FROM worktrees WHERE project_id=?1 AND name=?2",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![project_id, name], |row| {
            Ok(WorktreeRow {
                id: row.get(0)?,
                project_id: row.get(1)?,
                name: row.get(2)?,
                path: row.get(3)?,
                branch: row.get(4)?,
                missing_since: row.get(5)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// Phase 1: stamp `missing_since` only if not already set (so the original
    /// observation time is preserved across refresh cycles). Emits the updated
    /// row so the UI can show the "missing" badge.
    pub fn mark_worktree_missing(
        &self,
        project_id: i64,
        name: &str,
        now: i64,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE worktrees SET missing_since=?3
             WHERE project_id=?1 AND name=?2 AND missing_since IS NULL",
            rusqlite::params![project_id, name, now],
        )?;
        if let Some(row) = self.get_worktree_by_name(project_id, name)? {
            self.bus.worktree_updated(&row);
        }
        Ok(())
    }

    /// Phase 2: the worktree is still gone — ghost its live sessions
    /// (status='ghost', lost_at=now) and delete the worktree row. Caller runs
    /// `git worktree prune` off-lock to clear git's own registration.
    pub fn prune_missing_worktree(&self, id: i64, now: i64) -> Result<(), rusqlite::Error> {
        // Collect live sessions on this worktree, ghost them, emit updates.
        let session_ids: Vec<i64> = {
            let mut stmt = self.conn.prepare_cached(
                "SELECT id FROM sessions WHERE worktree_id=?1 AND status!='ghost'",
            )?;
            let ids = stmt
                .query_map(rusqlite::params![id], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        };
        for sid in &session_ids {
            self.conn.execute(
                "UPDATE sessions SET status='ghost', lost_at=?2 WHERE id=?1",
                rusqlite::params![sid, now],
            )?;
            if let Some(row) = self.get_session_by_id(*sid)? {
                self.bus.session_updated(&row);
            }
        }
        self.conn
            .execute("DELETE FROM worktrees WHERE id=?1", rusqlite::params![id])?;
        Ok(())
    }

    /// Resolve a worktree id to `(project base_path, name, branch, project_id)`
    /// for the Recreate action.
    pub fn worktree_recreate_info(
        &self,
        id: i64,
    ) -> Result<Option<(String, String, Option<String>, i64)>, rusqlite::Error> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT p.base_path, w.name, w.branch, w.project_id
             FROM worktrees w JOIN projects p ON p.id = w.project_id
             WHERE w.id=?1",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }
```

- [ ] **Step 4: Run the test**

Run: `cd src-tauri && cargo test --lib store::tests::mark_and_prune_missing_worktree_ghosts_sessions`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/store.rs
git commit -m "feat(store): mark/prune missing worktree + ghost its sessions"
```

---

## Task 4: Two-phase reconcile in `refresh_projects`

**Files:**
- Modify: `src-tauri/src/service/projects.rs` (the write phase ~line 68-90; add a `now_unix` helper + prune step)

- [ ] **Step 1: Write the failing integration test**

Add a `#[cfg(test)] mod tests` to `src-tauri/src/service/projects.rs` (or extend an existing one). This builds a real git repo + worktree, deletes the worktree dir, and runs the two-phase transition through the store directly (the per-worktree decision logic). Because `refresh_projects` reads a real filesystem base, drive it via the `CLAUDE_FLEET_PROJECTS_BASE` env override:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let ok = Command::new("git").arg("-C").arg(dir).args(args).status().unwrap().success();
        assert!(ok, "git {args:?} failed");
    }

    #[tokio::test]
    async fn missing_worktree_is_marked_then_pruned() {
        let base = TempDir::new().unwrap();
        // ~/projects/github.com/<owner>/<repo>
        let repo = base.path().join("owner").join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "--allow-empty", "-q", "-m", "init"]);
        // create a worktree, then delete its directory out-of-band
        let wt = repo.join(".worktrees").join("feat");
        git(&repo, &["worktree", "add", wt.to_str().unwrap(), "-b", "feat"]);
        std::fs::remove_dir_all(&wt).unwrap();

        std::env::set_var("CLAUDE_FLEET_PROJECTS_BASE", base.path());
        let store = std::sync::Mutex::new(crate::store::Store::open_in_memory().unwrap());

        // Refresh #1: worktree is prunable → marked missing (row kept).
        refresh_projects(&store).await.unwrap();
        let tree = list_projects(&store).unwrap();
        let feat = tree[0].worktrees.iter().find(|w| w.name == "feat").expect("feat present after refresh 1");
        assert!(feat.missing_since.is_some());

        // Refresh #2: still missing → pruned (row gone, git registration cleared).
        refresh_projects(&store).await.unwrap();
        let tree = list_projects(&store).unwrap();
        assert!(tree[0].worktrees.iter().all(|w| w.name != "feat"));

        std::env::remove_var("CLAUDE_FLEET_PROJECTS_BASE");
    }
}
```

Note: `git worktree list` reports a removed-dir worktree as `prunable` until `git worktree prune` runs, so it stays listed for refresh #1 (marked) and #2 reads `missing_since` set → prunes. If your local git needs `--porcelain` to emit `prunable` only after a `git worktree list` GC, the disk-existence fallback (`!wt.path.exists()`) still classifies it missing — the test covers both.

- [ ] **Step 2: Run it to confirm it fails**

Run: `cd src-tauri && cargo test --lib service::projects::tests::missing_worktree_is_marked_then_pruned`
Expected: FAIL — worktrees still appear (no two-phase logic yet) / `missing_since` never set.

- [ ] **Step 3: Add a `now_unix` helper**

At the top of `src-tauri/src/service/projects.rs` (after the imports), add:

```rust
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
```

- [ ] **Step 4: Rewrite the write phase with the two-phase reconcile**

Replace the write block (step 4 in `refresh_projects`, ~line 68-90) and add an off-lock prune step after it:

```rust
    // 4. Apply all writes under a single brief lock. Worktrees whose dir is gone
    //    (git `prunable`, or path missing on disk) are marked `missing_since`
    //    on first sight and auto-pruned on a later cycle — mirroring the
    //    ghost-session lifecycle. The main checkout is never treated as missing.
    let now = now_unix();
    let mut prune_repos: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        for (dp, worktrees) in &upserts {
            let project_id =
                s.upsert_project(&dp.owner, &dp.repo, &dp.base_path.to_string_lossy())?;
            let mut keep_names = Vec::with_capacity(worktrees.len());
            for wt in worktrees {
                let missing = wt.name != "main" && (wt.is_prunable || !wt.path.exists());
                // Read the prior state BEFORE upsert (upsert clears missing_since).
                let prior_missing = s
                    .get_worktree_by_name(project_id, &wt.name)?
                    .and_then(|r| r.missing_since)
                    .is_some();
                if !missing {
                    // Present (or main): upsert refreshes path/branch and clears the flag.
                    s.upsert_worktree(
                        project_id,
                        &wt.name,
                        &wt.path.to_string_lossy(),
                        wt.branch.as_deref(),
                    )?;
                    keep_names.push(wt.name.clone());
                } else if !prior_missing {
                    // Phase 1: ensure the row exists, then stamp missing_since.
                    s.upsert_worktree(
                        project_id,
                        &wt.name,
                        &wt.path.to_string_lossy(),
                        wt.branch.as_deref(),
                    )?;
                    s.mark_worktree_missing(project_id, &wt.name, now)?;
                    keep_names.push(wt.name.clone());
                } else {
                    // Phase 2: still missing — prune row + ghost sessions; clear
                    // git's registration off-lock below. Not added to keep_names.
                    if let Some(row) = s.get_worktree_by_name(project_id, &wt.name)? {
                        s.prune_missing_worktree(row.id, now)?;
                        prune_repos.insert(dp.base_path.clone());
                    }
                }
            }
            s.delete_worktrees_not_in(project_id, &keep_names)?;
        }
    }

    // 4b. Clear git's stale worktree registrations off-lock (so subsequent
    //     `git -C <gone-worktree>` calls stop erroring). Best-effort.
    for repo in &prune_repos {
        let _ = tokio::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["worktree", "prune"])
            .output()
            .await;
    }
```

- [ ] **Step 5: Run the test**

Run: `cd src-tauri && cargo test --lib service::projects::tests::missing_worktree_is_marked_then_pruned`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/service/projects.rs
git commit -m "feat(projects): two-phase missing-worktree reconcile + auto-prune"
```

---

## Task 5: `recreate_worktree` command

**Files:**
- Modify: `src-tauri/src/service/sessions.rs` (add `worktree_recreate_script` + `recreate_worktree`)
- Modify: `src-tauri/src/commands/projects.rs` (command wrapper)
- Modify: `src-tauri/src/lib.rs` (register)

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `src-tauri/src/service/sessions.rs` (near `create_worktree_local_creates_and_is_idempotent`):

```rust
    #[tokio::test]
    async fn recreate_worktree_rebuilds_dir_and_clears_missing() {
        use std::process::Command;
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            assert!(Command::new("git").arg("-C").arg(&repo).args(args).status().unwrap().success());
        };
        git(&["init", "-q"]);
        git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "--allow-empty", "-q", "-m", "init"]);
        let wt = repo.join(".worktrees").join("feat");
        git(&["worktree", "add", wt.to_str().unwrap(), "-b", "feat"]);
        std::fs::remove_dir_all(&wt).unwrap();
        // clear git's registration so add can recreate cleanly
        git(&["worktree", "prune"]);

        let store = std::sync::Mutex::new(crate::store::Store::open_in_memory().unwrap());
        let (pid, wid) = {
            let s = store.lock().unwrap();
            let pid = s.upsert_project("o", "repo", repo.to_str().unwrap()).unwrap();
            let wid = s.upsert_worktree(pid, "feat", wt.to_str().unwrap(), Some("feat")).unwrap();
            s.mark_worktree_missing(pid, "feat", 111).unwrap();
            (pid, wid)
        };
        let _ = pid;

        let ssh = std::sync::Arc::new(crate::ssh::SshClient::new());
        let row = recreate_worktree(RecreateWorktreeArgs { worktree_id: wid }, &store, &ssh)
            .await
            .unwrap();
        assert!(wt.exists(), "worktree dir recreated");
        assert_eq!(row.missing_since, None);
    }
```

(Match the real `SshClient::new()` constructor; if it differs, read `ssh.rs`. The local branch never touches SSH.)

- [ ] **Step 2: Run it to confirm it fails**

Run: `cd src-tauri && cargo test --lib service::sessions::tests::recreate_worktree_rebuilds_dir_and_clears_missing`
Expected: FAIL — `recreate_worktree` / `RecreateWorktreeArgs` don't exist.

- [ ] **Step 3: Add a recreate-aware worktree script**

In `src-tauri/src/service/sessions.rs`, near `worktree_add_script` (~line 445), add a script that attaches the existing branch if present, else creates it. (Plain `worktree_add_script` uses `-b`, which fails when the branch still exists after the dir was deleted.)

```rust
/// Recreate a worktree at `<base>/<name>` for `branch`. Unlike
/// `worktree_add_script`, this attaches an EXISTING branch (the branch usually
/// survives when only the worktree directory was deleted); it falls back to
/// creating the branch if it's gone too.
fn worktree_recreate_script(root: &str, name: &str, branch: &str) -> String {
    format!(
        "set -e\n\
         cd {root}\n\
         name={name}\n\
         branch={branch}\n\
         if [ -d .worktrees ]; then base=.worktrees\n\
         elif [ -d .claude/worktrees ]; then base=.claude/worktrees\n\
         else base=.worktrees\n\
         fi\n\
         wt=\"$base/$name\"\n\
         git worktree prune 1>&2 || true\n\
         if [ ! -e \"$wt\" ]; then\n\
         if git show-ref --verify --quiet \"refs/heads/$branch\"; then\n\
         git worktree add \"$wt\" \"$branch\" 1>&2\n\
         else\n\
         def=\"$(git symbolic-ref --short refs/remotes/origin/HEAD 2>/dev/null | sed 's#^origin/##')\"\n\
         [ -z \"$def\" ] && def=\"$(git rev-parse --abbrev-ref HEAD 2>/dev/null)\"\n\
         git worktree add \"$wt\" -b \"$branch\" \"$def\" 1>&2\n\
         fi\n\
         fi\n\
         ( cd \"$wt\" && pwd )\n",
        root = shq(root),
        name = shq(name),
        branch = shq(branch),
    )
}
```

- [ ] **Step 4: Add the service function**

Add to `src-tauri/src/service/sessions.rs` (near `recreate_session`, ~line 955):

```rust
#[derive(Deserialize)]
pub struct RecreateWorktreeArgs {
    pub worktree_id: i64,
}

/// Recreate a missing worktree on disk from its stored path/branch, then clear
/// its `missing_since` flag. Local-only (worktree discovery is local).
pub async fn recreate_worktree(
    args: RecreateWorktreeArgs,
    store: &Mutex<Store>,
    _ssh: &Arc<SshClient>,
) -> Result<WorktreeRow, IpcError> {
    let (base_path, name, branch, project_id) = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.worktree_recreate_info(args.worktree_id)?
            .ok_or_else(|| IpcError::new("E_NOTFOUND", "worktree not found"))?
    };
    crate::validate::path_component(&name)?;
    let branch_name = branch.clone().unwrap_or_else(|| name.clone());
    crate::validate::git_ref(&branch_name)?;

    let script = worktree_recreate_script(&base_path, &name, &branch_name);
    let out = tokio::process::Command::new("bash")
        .args(["-lc", &script])
        .output()
        .await
        .map_err(|e| IpcError::new("E_GIT_SETUP", format!("bash: {e}")))?;
    if !out.status.success() {
        return Err(IpcError::new(
            "E_GIT_SETUP",
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();

    let row = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        // upsert clears missing_since and emits the updated row.
        s.upsert_worktree(project_id, &name, &path, branch.as_deref())?;
        s.get_worktree(args.worktree_id)?
            .ok_or_else(|| IpcError::new("E_INTERNAL", "worktree vanished after recreate"))?
    };
    Ok(row)
}
```

Confirm `WorktreeRow` is imported in `sessions.rs` (add `use crate::store::WorktreeRow;` if not already present).

- [ ] **Step 5: Run the test**

Run: `cd src-tauri && cargo test --lib service::sessions::tests::recreate_worktree_rebuilds_dir_and_clears_missing`
Expected: PASS.

- [ ] **Step 6: Add the Tauri command**

In `src-tauri/src/commands/projects.rs`, append:

```rust
#[tauri::command]
pub async fn recreate_worktree(
    args: crate::service::sessions::RecreateWorktreeArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<crate::ssh::SshClient>>,
) -> Result<crate::store::WorktreeRow, IpcError> {
    crate::service::sessions::recreate_worktree(args, &store, &ssh).await
}
```

- [ ] **Step 7: Register it**

In `src-tauri/src/lib.rs`, add to the `tauri::generate_handler![ … ]` list near the other `commands::projects::*` entries:

```rust
            commands::projects::recreate_worktree,
```

- [ ] **Step 8: Verify build + clippy**

Run: `cd src-tauri && cargo build && cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add src-tauri/src/service/sessions.rs src-tauri/src/commands/projects.rs src-tauri/src/lib.rs
git commit -m "feat(projects): recreate_worktree command"
```

---

## Task 6: Frontend — `missing_since` on the worktree row + recreate wrapper

**Files:**
- Modify: `src/lib/projects.ts` (`WorktreeRow` ~line 12; add `recreateWorktree`)
- Test: `src/lib/projects.test.ts`

- [ ] **Step 1: Write the failing test**

Add to `src/lib/projects.test.ts`:

```ts
import { mergeWorktree, projects } from './projects';
import { get } from 'svelte/store';

it('mergeWorktree carries missing_since onto the worktree', () => {
  projects.set([
    { project: { id: 1, owner: 'o', repo: 'r', base_path: '/r', last_session_at: null },
      worktrees: [{ id: 9, project_id: 1, name: 'feat', path: '/r/.worktrees/feat', branch: 'feat', missing_since: null }] },
  ]);
  mergeWorktree({ id: 9, project_id: 1, name: 'feat', path: '/r/.worktrees/feat', branch: 'feat', missing_since: 1234 });
  const wt = get(projects)[0].worktrees.find((w) => w.id === 9)!;
  expect(wt.missing_since).toBe(1234);
});
```

(If `projects.test.ts` already imports these, reuse the existing imports rather than duplicating.)

- [ ] **Step 2: Run it to confirm it fails**

Run: `pnpm test -- src/lib/projects.test.ts`
Expected: FAIL — `missing_since` not assignable to `WorktreeRow` (type error) / property missing.

- [ ] **Step 3: Add the field + wrapper**

In `src/lib/projects.ts`, extend the interface (~line 12):

```ts
export interface WorktreeRow {
  id: number;
  project_id: number;
  name: string;
  path: string;
  branch: string | null;
  /** Unix seconds since the worktree dir was first seen missing; null = present. */
  missing_since: number | null;
}
```

Add a command wrapper (near `refreshProjects`):

```ts
export function recreateWorktree(worktreeId: number): Promise<Result<WorktreeRow>> {
  return invokeCmd<WorktreeRow>('recreate_worktree', { args: { worktree_id: worktreeId } });
}
```

(`invokeCmd` and `Result` are already imported in this file — confirm and reuse.)

- [ ] **Step 4: Run the test**

Run: `pnpm test -- src/lib/projects.test.ts`
Expected: PASS.

- [ ] **Step 5: Type-check**

Run: `pnpm check`
Expected: no new errors in `projects.ts`.

- [ ] **Step 6: Commit**

```bash
git add src/lib/projects.ts src/lib/projects.test.ts
git commit -m "feat(projects-ui): WorktreeRow.missing_since + recreateWorktree wrapper"
```

---

## Task 7: ProjectDetails — missing badge + Recreate button

**Files:**
- Modify: `src/lib/ProjectDetails.svelte`

- [ ] **Step 1: Read the current worktree list rendering**

Open `src/lib/ProjectDetails.svelte` and find the `{#each ... worktrees ...}` block that renders each worktree (name + branch). Note the variable name used for the loop item (assume `wt`).

- [ ] **Step 2: Add the badge + button to each worktree row**

Inside the per-worktree markup, after the name/branch, add (adjust the loop var name to match):

```svelte
{#if wt.missing_since}
  <span class="wt-missing" data-testid="worktree-missing">missing</span>
  <button
    class="wt-recreate"
    data-testid="worktree-recreate"
    onclick={() => void recreateWorktree(wt.id)}
  >
    Recreate
  </button>
{/if}
```

Add the import at the top of `<script>`:

```ts
import { recreateWorktree } from './projects';
```

- [ ] **Step 3: Add styles**

In the `<style>` block:

```css
.wt-missing {
  margin-left: 0.4rem;
  font-size: 0.7rem;
  color: #e0a020;
  border: 1px solid #e0a020;
  border-radius: 3px;
  padding: 0 0.3rem;
}
.wt-recreate {
  margin-left: 0.4rem;
  font-size: 0.7rem;
  padding: 0.05rem 0.4rem;
  border: 1px solid var(--border);
  background: transparent;
  color: var(--fg-muted);
  border-radius: 3px;
  cursor: pointer;
}
.wt-recreate:hover { color: var(--fg); border-color: var(--accent); }
```

- [ ] **Step 4: Type-check**

Run: `pnpm check`
Expected: no new errors in `ProjectDetails.svelte`.

- [ ] **Step 5: Manual check**

Run `pnpm tauri dev`, delete a worktree dir on disk (`rm -rf <repo>/.worktrees/<name>`), refresh projects; the worktree shows a "missing" badge + Recreate button; clicking Recreate rebuilds it and the badge clears on the next refresh.

- [ ] **Step 6: Commit**

```bash
git add src/lib/ProjectDetails.svelte
git commit -m "feat(projects-ui): missing badge + Recreate button in ProjectDetails"
```

---

## Task 8: NewSessionDialog — don't default to a missing worktree

**Files:**
- Modify: `src/lib/NewSessionDialog.svelte`
- Test: `src/lib/NewSessionDialog.test.ts`

- [ ] **Step 1: Write the failing test**

Add to `src/lib/NewSessionDialog.test.ts` (reuse the file's existing render/setup helpers — read them first and mirror the existing test that checks the default worktree selection):

```ts
it('does not default-select a missing worktree', async () => {
  // Render the dialog for a project whose first worktree is missing and the
  // second is present; the default selection should be the present one.
  // (Mirror the existing "defaults to last-host pref" test's render harness.)
  // Assert the selected worktree id is the present worktree, not the missing one.
});
```

Replace the comment body with concrete assertions using the file's existing harness (props shape, how selection is read). The behavioral requirement: given `worktrees: [{id:1,...,missing_since:111},{id:2,...,missing_since:null}]`, the initial selected worktree id is `2`.

- [ ] **Step 2: Run it to confirm it fails**

Run: `pnpm test -- src/lib/NewSessionDialog.test.ts`
Expected: FAIL — defaults to the first (missing) worktree.

- [ ] **Step 3: Fix the default selection**

In `src/lib/NewSessionDialog.svelte`, find the default-selection line (~line 51, `untrack(() => project.worktrees[0]?.id ?? null)`). Change it to prefer the first **present** worktree, falling back to the first if all are missing:

```ts
untrack(() => {
  const present = project.worktrees.find((w) => !w.missing_since);
  return (present ?? project.worktrees[0])?.id ?? null;
})
```

- [ ] **Step 4: Mark missing worktree chips + inline Recreate**

In the worktree chip `{#each}`, render a missing chip distinctly and offer Recreate (adjust the loop var to match the file):

```svelte
{#if wt.missing_since}
  <span class="chip-missing" data-testid="nsd-worktree-missing">missing</span>
  <button type="button" data-testid="nsd-worktree-recreate" onclick={() => void recreateWorktree(wt.id)}>Recreate</button>
{/if}
```

Add `import { recreateWorktree } from './projects';` to the `<script>` (if `WorktreeRow`/projects items are already imported, add to that import).

- [ ] **Step 5: Run tests + type-check**

Run: `pnpm test -- src/lib/NewSessionDialog.test.ts && pnpm check`
Expected: tests PASS; no new type errors.

- [ ] **Step 6: Commit**

```bash
git add src/lib/NewSessionDialog.svelte src/lib/NewSessionDialog.test.ts
git commit -m "feat(projects-ui): skip missing worktree as default + inline Recreate"
```

---

## Task 9: Final verification

- [ ] **Step 1: Backend gates**

Run: `cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: all tests pass (including the new store/projects/sessions tests); no clippy warnings; formatting clean. If `cargo fmt --check` reports drift in files this plan didn't touch, run `cargo fmt` and commit it separately as `chore(fmt)`.

- [ ] **Step 2: Frontend gates**

Run (from the worktree root): `pnpm test && pnpm check && pnpm run build`
Expected: new clipboard/projects/NewSessionDialog tests pass; `pnpm check` clean; build succeeds. (On Node 22+, the localStorage shim + tauri mocks in `vitest.setup.ts` already handle the environment.)

- [ ] **Step 3: Commit any fmt fixup**

```bash
git add -A && git commit -m "chore: fmt" || true
```

---

## Self-Review notes

- **Spec coverage:** §1 Detection → Task 2 (parse prunable) + Task 4 (disk-existence + main excluded). §2 two-phase data model → Tasks 1, 3, 4. §3 Recreate → Tasks 5, 7, 8. §4 `.git`/edge cases → relies on git (Task 2/4), grace period (Task 4 phase split), reappear-clears (Task 1 upsert + Task 4 present branch), recreate-after-prune (Task 5 script). §5 UI → Tasks 6, 7, 8. §6 Testing → Tasks 1-6, 8. Out-of-scope items (manual prune button, friendly error rewrite, remote detection, symlink canonicalization) intentionally omitted.
- **Type/name consistency:** `missing_since: Option<i64>` (Rust) / `missing_since: number | null` (TS) used everywhere; `is_prunable` (Task 2) consumed in Task 4; `get_worktree_by_name`/`mark_worktree_missing`/`prune_missing_worktree`/`worktree_recreate_info` (Task 3) called in Tasks 4/5; `RecreateWorktreeArgs { worktree_id }` (Task 5) matches the `recreate_worktree` invoke in Task 6 and the command in Task 5; `recreateWorktree` (Task 6) used in Tasks 7/8.
- **No placeholders:** the NewSessionDialog test (Task 8 Step 1) is the one spot requiring the implementer to fill assertions against the file's existing harness — flagged explicitly with the exact behavioral requirement (default selects id `2`, the present worktree).
```
