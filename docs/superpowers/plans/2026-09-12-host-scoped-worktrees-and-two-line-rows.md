# Host-scoped worktree picker and two-line session rows — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The New-session dialog lists only the worktrees that exist on the chosen host (scanned live over SSH and cached), and sidebar session rows become two lines with a global "details" toggle.

**Architecture:** A new `list_host_worktrees` service + Tauri command scans `git worktree list --porcelain` on a remote host, maps entries to host-scoped `worktrees` rows (upsert + prune), and returns them; the dialog re-fetches on host switch and falls back to `main` / "+ new worktree". `SessionRowItem.svelte` splits into line 1 (name + one status chip + actions) and line 2 (host, tmux name / worktree, elapsed, badges, prompt preview), gated by a persisted `showRowDetails` store.

**Tech Stack:** Rust (Tauri 2, rusqlite, tokio, `FakeSsh` test double), Svelte 5 runes, Vitest + Testing Library. Spec: `docs/superpowers/specs/2026-09-12-host-scoped-worktrees-and-two-line-rows-design.md`.

Conventions that apply to every task: quote every interpolated shell value with `crate::shell::quote`; never hold the `Store` mutex across an `.await`; run frontend tools as `npx vitest run <file>` and `npx svelte-check --tsconfig ./tsconfig.json` (the `pnpm` script binaries are not on PATH); Rust from `src-tauri/` with `cargo test <name>`; commit after each task; no attribution lines in commit messages. Any new Tauri command changes `docs/control-api-reference.md` — regenerate it (Task 3) or CI fails.

---

## File map

| File | Change |
|---|---|
| `src-tauri/src/store/projects.rs` | add `delete_host_worktrees_not_in` |
| `src-tauri/src/service/repair.rs` | make `resolve_remote_paths` `pub(crate)` |
| `src-tauri/src/service/worktrees.rs` | add `ListHostWorktreesArgs`, `HostWorktrees`, `rows_from_porcelain`, `list_host_worktrees(_with)` |
| `src-tauri/src/commands/worktrees.rs`, `src-tauri/src/lib.rs` | register `list_host_worktrees` |
| `src-tauri/src/service/sessions/lifecycle.rs` | reject a local worktree row for a remote host (`reject_foreign_worktree`) |
| `docs/control-api-reference.md` | regenerated |
| `src/lib/projects.ts` | `HostWorktrees` type + `listHostWorktrees` wrapper |
| `src/lib/NewSessionDialog.svelte` (+ `.test.ts`) | host-scoped worktree list, states, per-host memory |
| `src/lib/sessions.ts` | `showRowDetails` store |
| `src/lib/SidebarFilters.svelte`, `src/lib/Sidebar.test.ts` | "details" pill |
| `src/lib/session_status.ts` | `rowElapsed`, `rowPrompt` |
| `src/lib/SessionRowItem.svelte`, `src/lib/Sidebar.test.ts` | two-line layout |

---

### Task 1: `Store::delete_host_worktrees_not_in`

**Files:**
- Modify: `src-tauri/src/store/projects.rs` (after `delete_worktree_if_unused`, ~line 640)
- Test: same file, `mod tests` at the bottom

- [ ] **Step 1: Write the failing test**

Append inside the existing `#[cfg(test)] mod tests` of `src-tauri/src/store/projects.rs`:

```rust
    #[test]
    fn delete_host_worktrees_not_in_prunes_only_that_hosts_unlisted_rows() {
        let s = Store::open_in_memory().unwrap();
        let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
        let local = s.upsert_worktree(pid, "feat", "/p/o/r/.claude/worktrees/feat", Some("feat")).unwrap();
        let keep = s.upsert_worktree_on("vps", pid, "main", "/home/u/r", Some("main")).unwrap();
        let gone = s.upsert_worktree_on("vps", pid, "old", "/home/u/r/.claude/worktrees/old", None).unwrap();
        let busy = s.upsert_worktree_on("vps", pid, "busy", "/home/u/r/.claude/worktrees/busy", None).unwrap();
        // An alive session pins `busy` even though the scan did not list it.
        s.upsert_session("vps", "dev-r--busy", Some(pid), Some(busy), 1, 1).unwrap();

        let removed = s
            .delete_host_worktrees_not_in("vps", pid, &["main".to_string()])
            .unwrap();

        assert_eq!(removed, 1);
        assert!(s.get_worktree_row(gone).unwrap().is_none());
        assert!(s.get_worktree_row(keep).unwrap().is_some());
        assert!(s.get_worktree_row(busy).unwrap().is_some(), "row with an alive session is kept");
        assert!(s.get_worktree_row(local).unwrap().is_some(), "local rows are never touched");
    }
```

If `upsert_session` has a different signature in this file's tests, copy the call shape from an existing test in `src-tauri/src/store/projects.rs` (search `upsert_session(`) — the intent is one running session whose `worktree_id` is `busy`.

- [ ] **Step 2: Run it to verify it fails**

Run: `cd src-tauri && cargo test delete_host_worktrees_not_in_prunes -- --nocapture`
Expected: compile error `no method named delete_host_worktrees_not_in`.

- [ ] **Step 3: Implement**

Add to `impl Store` in `src-tauri/src/store/projects.rs`, right after `delete_worktree_if_unused`:

```rust
    /// After a scan of `host_alias`'s checkout of `project_id`: drop that
    /// host's rows the scan did not report, except rows a session still
    /// points at (any status — the FK must stay valid). Local rows are never
    /// touched. Emits `worktree:removed` per dropped row like
    /// [`Self::delete_worktree_if_unused`]. Returns how many went.
    pub fn delete_host_worktrees_not_in(
        &self,
        host_alias: &str,
        project_id: i64,
        keep_names: &[String],
    ) -> Result<usize, rusqlite::Error> {
        if host_alias == crate::service::projects::LOCAL_HOST {
            return Ok(0);
        }
        let ids: Vec<i64> = {
            let mut stmt = self.conn.prepare_cached(
                "SELECT id, name FROM worktrees WHERE host_alias=?1 AND project_id=?2",
            )?;
            let rows = stmt.query_map(rusqlite::params![host_alias, project_id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?;
            rows.filter_map(Result::ok)
                .filter(|(_, name)| !keep_names.iter().any(|k| k == name))
                .map(|(id, _)| id)
                .collect()
        };
        let mut n = 0;
        for id in ids {
            if self.delete_worktree_if_unused(id)? {
                n += 1;
            }
        }
        Ok(n)
    }
```

- [ ] **Step 4: Run the test**

Run: `cd src-tauri && cargo test delete_host_worktrees_not_in_prunes`
Expected: `test result: ok. 1 passed`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/store/projects.rs
git commit -m "feat(store): prune a host's worktree rows a scan no longer reports"
```

---

### Task 2: `service::worktrees::list_host_worktrees`

**Files:**
- Modify: `src-tauri/src/service/repair.rs:2090` (`async fn resolve_remote_paths` → `pub(crate) async fn resolve_remote_paths`)
- Modify: `src-tauri/src/service/worktrees.rs`
- Test: `src-tauri/src/service/worktrees.rs` `mod tests`

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests` in `src-tauri/src/service/worktrees.rs`:

```rust
    use crate::ssh_fake::{FakeSsh, Match, Reply};

    const PORCELAIN: &str = "worktree /home/u/projects/github.com/o/r\nHEAD 1111\nbranch refs/heads/main\n\nworktree /home/u/projects/github.com/o/r/.claude/worktrees/feat\nHEAD 2222\nbranch refs/heads/feature/feat\n\nworktree /home/u/projects/github.com/o/r/.claude/worktrees/det\nHEAD 3333\ndetached\n\nworktree /home/u/projects/github.com/o/r.git\nbare\n\nworktree /home/u/projects/github.com/o/r/.claude/worktrees/stale\nHEAD 4444\nbranch refs/heads/stale\nprunable gitdir file points to non-existent location\n";

    #[test]
    fn rows_from_porcelain_maps_root_to_main_and_skips_bare_and_prunable() {
        let rows = rows_from_porcelain("/home/u/projects/github.com/o/r", PORCELAIN);
        let names: Vec<(&str, Option<&str>)> = rows
            .iter()
            .map(|(n, _, b)| (n.as_str(), b.as_deref()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("main", Some("main")),
                ("feat", Some("feature/feat")),
                ("det", None),
            ]
        );
        assert_eq!(rows[1].1, "/home/u/projects/github.com/o/r/.claude/worktrees/feat");
    }

    #[tokio::test]
    async fn list_host_worktrees_scans_the_host_and_caches_rows() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let pid = {
            let s = store.lock().unwrap();
            let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
            // A stale remote row the scan will not report, and a local row.
            s.upsert_worktree_on("vps", pid, "old", "/home/u/projects/github.com/o/r/.claude/worktrees/old", None).unwrap();
            s.upsert_worktree(pid, "local-only", "/p/o/r/.claude/worktrees/local-only", Some("x")).unwrap();
            pid
        };
        let fake = FakeSsh::new();
        fake.with_home("/home/u")
            .on(Match::script_contains("worktree list --porcelain"), Reply::ok(PORCELAIN));

        let out = list_host_worktrees_with(
            ListHostWorktreesArgs { host_alias: "vps".into(), project_id: pid },
            &store,
            &fake,
        )
        .await
        .unwrap();

        assert!(out.cloned);
        let names: Vec<&str> = out.worktrees.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, vec!["main", "det", "feat"], "main first, then by name");
        assert!(out.worktrees.iter().all(|w| w.host_alias == "vps"));
        assert_eq!(out.worktrees[2].branch.as_deref(), Some("feature/feat"));
        let script = fake.calls_for("vps").last().unwrap().script().unwrap();
        assert!(script.contains("git -C '/home/u/projects/github.com/o/r' worktree list --porcelain"), "{script}");
        // Cached: rows exist with stable ids; the stale row is gone; local untouched.
        let s = store.lock().unwrap();
        let cached = s.list_worktrees_on_host("vps").unwrap();
        assert_eq!(cached.len(), 3);
        assert!(cached.iter().all(|w| w.name != "old"));
        assert_eq!(s.list_worktrees_for_project(pid).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn list_host_worktrees_reports_not_cloned() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let pid = store.lock().unwrap().upsert_project("o", "r", "/p/o/r").unwrap();
        let fake = FakeSsh::new();
        fake.with_home("/home/u")
            .on(Match::script_contains("worktree list --porcelain"), Reply::ok("__NOT_CLONED__\n"));
        let out = list_host_worktrees_with(
            ListHostWorktreesArgs { host_alias: "vps".into(), project_id: pid },
            &store,
            &fake,
        )
        .await
        .unwrap();
        assert!(!out.cloned);
        assert!(out.worktrees.is_empty());
    }

    #[tokio::test]
    async fn list_host_worktrees_local_uses_the_db_without_ssh() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let pid = {
            let s = store.lock().unwrap();
            let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
            s.upsert_worktree(pid, "main", "/p/o/r", Some("main")).unwrap();
            pid
        };
        let fake = FakeSsh::new();
        let out = list_host_worktrees_with(
            ListHostWorktreesArgs { host_alias: "local".into(), project_id: pid },
            &store,
            &fake,
        )
        .await
        .unwrap();
        assert!(out.cloned);
        assert_eq!(out.worktrees.len(), 1);
        assert!(fake.calls().is_empty());
    }

    #[tokio::test]
    async fn list_host_worktrees_git_failure_is_e_git_setup() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let pid = store.lock().unwrap().upsert_project("o", "r", "/p/o/r").unwrap();
        let fake = FakeSsh::new();
        fake.with_home("/home/u")
            .on(Match::script_contains("worktree list --porcelain"), Reply::fail(128, "fatal: not a git repository"));
        let err = list_host_worktrees_with(
            ListHostWorktreesArgs { host_alias: "vps".into(), project_id: pid },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_GIT_SETUP");
        assert!(err.message.contains("not a git repository"));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd src-tauri && cargo test list_host_worktrees`
Expected: compile errors (`rows_from_porcelain`, `list_host_worktrees_with`, `ListHostWorktreesArgs` not found).

- [ ] **Step 3: Implement**

In `src-tauri/src/service/repair.rs` change line 2090 `async fn resolve_remote_paths(` to `pub(crate) async fn resolve_remote_paths(`.

In `src-tauri/src/service/worktrees.rs` add to the `use` block: `use crate::ssh::SshExec;` and `use std::time::Duration;`. Then add after `list_worktrees`:

```rust
#[derive(Deserialize)]
pub struct ListHostWorktreesArgs {
    pub host_alias: String,
    pub project_id: i64,
}

/// The worktrees of one project as they exist on one host.
#[derive(Debug, Clone, Serialize)]
pub struct HostWorktrees {
    pub host_alias: String,
    pub project_id: i64,
    /// `false` when the project root is not a git checkout on the host yet
    /// (`new_session` clones on first use).
    pub cloned: bool,
    /// Host-scoped rows, `main` first, then by name.
    pub worktrees: Vec<WorktreeRow>,
}

/// Printed by the scan script instead of porcelain when `<root>/.git` is
/// missing on the host.
const NOT_CLONED_MARKER: &str = "__NOT_CLONED__";

/// Wall clock for the remote `git worktree list` (a cold ControlMaster plus
/// a login shell; git itself is instant).
const SCAN_WALL_CLOCK: Duration = Duration::from_secs(15);

/// Map `git worktree list --porcelain` of the checkout at `root` to
/// `(name, path, branch)` triples: the root entry is `main`, every other
/// entry is named by its last path component. Bare and prunable entries
/// are skipped (nothing can run in them).
pub fn rows_from_porcelain(root: &str, porcelain: &str) -> Vec<(String, String, Option<String>)> {
    let root = root.trim_end_matches('/');
    crate::service::repair::parse_porcelain(porcelain)
        .into_iter()
        .filter(|w| !w.bare && !w.prunable)
        .filter_map(|w| {
            let path = w.path.trim_end_matches('/').to_string();
            let name = if path == root {
                "main".to_string()
            } else {
                path.rsplit('/').next().filter(|s| !s.is_empty())?.to_string()
            };
            Some((name, path, w.branch))
        })
        .collect()
}

/// Production entry point: the shared SSH client.
pub async fn list_host_worktrees(
    args: ListHostWorktreesArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<HostWorktrees, IpcError> {
    list_host_worktrees_with(args, store, &**ssh).await
}

/// List `project_id`'s worktrees on `host_alias`. `local` answers from the
/// DB (the project scan owns those rows). A remote host is scanned with one
/// `git worktree list --porcelain` over SSH; the result is written back as
/// that host's rows (`upsert_worktree_on` + prune of unlisted rows) so the
/// ids are stable for `new_session`'s `worktree_id`.
pub async fn list_host_worktrees_with(
    args: ListHostWorktreesArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
) -> Result<HostWorktrees, IpcError> {
    let host = args.host_alias.as_str();
    let pid = args.project_id;
    if host == crate::service::projects::LOCAL_HOST {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        let worktrees = s.list_worktrees_for_project(pid).map_err(IpcError::from)?;
        return Ok(HostWorktrees {
            host_alias: host.to_string(),
            project_id: pid,
            cloned: true,
            worktrees: sort_main_first(worktrees),
        });
    }
    // Everything the remote path needs, under one short lock.
    let (owner, repo, base, layout) = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        let (owner, repo) = crate::service::sessions::fetch_owner_repo(&s, pid)?;
        (
            owner,
            repo,
            crate::service::projects::project_base_for(&s, host),
            crate::service::projects::layout(&s),
        )
    };
    let (root, _) = crate::service::repair::resolve_remote_paths(
        ssh, host, &base, layout, &owner, &repo, None,
    )
    .await?;
    let script = format!(
        "if [ -d {root}/.git ]; then git -C {root} worktree list --porcelain; else echo {marker}; fi",
        root = quote(&root),
        marker = NOT_CLONED_MARKER,
    );
    let quoted = quote(&script);
    let out = ssh
        .run(host, &["bash", "-lc", &quoted], SCAN_WALL_CLOCK)
        .await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(IpcError::new(
            "E_GIT_SETUP",
            format!("couldn't list worktrees of {owner}/{repo} on {host}: {stderr}"),
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.trim() == NOT_CLONED_MARKER {
        return Ok(HostWorktrees {
            host_alias: host.to_string(),
            project_id: pid,
            cloned: false,
            worktrees: Vec::new(),
        });
    }
    let found = rows_from_porcelain(&root, &stdout);
    let worktrees = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        let mut rows = Vec::with_capacity(found.len());
        let mut names = Vec::with_capacity(found.len());
        for (name, path, branch) in &found {
            let id = s
                .upsert_worktree_on(host, pid, name, path, branch.as_deref())
                .map_err(IpcError::from)?;
            if let Some(row) = s.get_worktree_row(id).map_err(IpcError::from)? {
                rows.push(row);
            }
            names.push(name.clone());
        }
        s.delete_host_worktrees_not_in(host, pid, &names)
            .map_err(IpcError::from)?;
        rows
    };
    Ok(HostWorktrees {
        host_alias: host.to_string(),
        project_id: pid,
        cloned: true,
        worktrees: sort_main_first(worktrees),
    })
}

fn sort_main_first(mut rows: Vec<WorktreeRow>) -> Vec<WorktreeRow> {
    rows.sort_by(|a, b| {
        (a.name != "main")
            .cmp(&(b.name != "main"))
            .then_with(|| a.name.cmp(&b.name))
    });
    rows
}
```

`fetch_owner_repo` is `pub(crate)` in `service/sessions/paths.rs`; check it is re-exported from `service::sessions` (`grep -n "pub(crate) use paths::" src-tauri/src/service/sessions/mod.rs`). If not, add `fetch_owner_repo` to that re-export list. `parse_porcelain` is already `pub`.

- [ ] **Step 4: Run the tests**

Run: `cd src-tauri && cargo test worktrees::`
Expected: all `list_host_worktrees_*`, `rows_from_porcelain_*` and the pre-existing worktrees tests pass.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/service/worktrees.rs src-tauri/src/service/repair.rs src-tauri/src/service/sessions/mod.rs
git commit -m "feat(worktrees): list a host's worktrees by scanning its checkout"
```

---

### Task 3: Tauri command + reference regen

**Files:**
- Modify: `src-tauri/src/commands/worktrees.rs`
- Modify: `src-tauri/src/lib.rs:218` (handler list)
- Regenerate: `docs/control-api-reference.md`

- [ ] **Step 1: Add the command**

In `src-tauri/src/commands/worktrees.rs` extend the import to `use crate::service::worktrees::{self, DeleteWorktreeArgs, HostWorktrees, ListHostWorktreesArgs, ListWorktreesArgs, WorktreeOccupancy};` and append:

```rust
/// The worktrees of one project as they exist on one host (a remote host
/// is scanned over SSH and cached as its rows). Feeds the New-session
/// dialog's worktree picker.
#[tauri::command]
pub async fn list_host_worktrees(
    args: ListHostWorktreesArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<HostWorktrees, IpcError> {
    worktrees::list_host_worktrees(args, &store, &ssh).await
}
```

In `src-tauri/src/lib.rs`, after `commands::worktrees::list_worktrees,` add `commands::worktrees::list_host_worktrees,`.

- [ ] **Step 2: Regenerate the reference and verify**

Run: `REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current`
Expected: PASS and `git status` shows `docs/control-api-reference.md` modified with a `list_host_worktrees` entry.

Run: `cd src-tauri && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: no warnings, no diff.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/commands/worktrees.rs src-tauri/src/lib.rs docs/control-api-reference.md
git commit -m "feat(commands): list_host_worktrees for the new-session dialog"
```

---

### Task 4: `new_session` refuses a local worktree row for a remote host

**Files:**
- Modify: `src-tauri/src/service/sessions/lifecycle.rs` (the `else` arm of `new_session_inner`, ~line 405-445)
- Modify: `src-tauri/src/service/sessions/paths.rs:310` (`fetch_worktree` also returns the row's host)
- Test: `src-tauri/src/service/sessions/lifecycle.rs` (add a `#[cfg(test)] mod foreign_worktree_tests` at the bottom if none exists)

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod foreign_worktree_tests {
    use super::reject_foreign_worktree;

    #[test]
    fn a_local_row_is_refused_for_a_remote_host() {
        let err = reject_foreign_worktree("mefistos", "local", "nifty-swanson").unwrap_err();
        assert_eq!(err.code, "E_INVALID_ARG");
        assert!(err.message.contains("nifty-swanson"));
        assert!(err.message.contains("mefistos"));
    }

    #[test]
    fn a_row_of_the_same_host_or_a_local_target_passes() {
        assert!(reject_foreign_worktree("mefistos", "mefistos", "w").is_ok());
        assert!(reject_foreign_worktree("local", "local", "w").is_ok());
    }

    #[test]
    fn a_remote_row_for_local_is_refused_too() {
        assert!(reject_foreign_worktree("local", "mefistos", "w").is_err());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd src-tauri && cargo test foreign_worktree_tests`
Expected: compile error `cannot find function reject_foreign_worktree`.

- [ ] **Step 3: Implement**

In `src-tauri/src/service/sessions/paths.rs` change `fetch_worktree` to return the host too:

```rust
/// `(name, branch, host_alias)` of a worktree row.
pub(super) fn fetch_worktree(
    s: &Store,
    worktree_id: i64,
) -> Result<(String, Option<String>, String), IpcError> {
    let mut stmt = s
        .conn_ref()
        .prepare("SELECT name, branch, host_alias FROM worktrees WHERE id=?1")?;
    stmt.query_row(rusqlite::params![worktree_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, String>(2)?,
        ))
    })
    .map_err(IpcError::from)
}
```

Fix every other caller of `fetch_worktree` (`grep -rn "fetch_worktree(" src-tauri/src/service/sessions/`) to destructure the third field (`let (name, branch, _) = …`).

In `src-tauri/src/service/sessions/lifecycle.rs` add near `ensure_remote_project`:

```rust
/// A worktree row names a checkout on ONE host. Using another host's row
/// would make `ensure_remote_project` add a worktree for a branch that may
/// exist only on the originating machine (`fatal: invalid reference`), so
/// refuse it here with an actionable message instead.
pub(super) fn reject_foreign_worktree(
    target_host: &str,
    row_host: &str,
    name: &str,
) -> Result<(), IpcError> {
    if target_host == row_host {
        return Ok(());
    }
    Err(IpcError::new(
        "E_INVALID_ARG",
        format!(
            "worktree {name} is a checkout on {row_host}; pick one that exists on {target_host} or start a new worktree"
        ),
    ))
}
```

In the `else` arm of `new_session_inner`, where `wt` is fetched under the lock:

```rust
                let wt = if let Some(wid) = args.worktree_id {
                    let (name, branch, row_host) = fetch_worktree(&s, wid)?;
                    reject_foreign_worktree(&args.host_alias, &row_host, &name)?;
                    Some((name, branch))
                } else {
                    None
                };
```

- [ ] **Step 4: Run the tests**

Run: `cd src-tauri && cargo test sessions::`
Expected: PASS, including the three new tests.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/service/sessions/lifecycle.rs src-tauri/src/service/sessions/paths.rs
git commit -m "fix(sessions): refuse another host's worktree row in new_session"
```

---

### Task 5: Frontend wrapper `listHostWorktrees`

**Files:**
- Modify: `src/lib/projects.ts` (after `listWorktreeOccupancy`)

- [ ] **Step 1: Add the type and wrapper**

```ts
/** `list_host_worktrees` result: one project's worktrees as they exist on
 *  one host. `cloned: false` means the repo is not checked out there yet. */
export interface HostWorktrees {
  host_alias: string;
  project_id: number;
  cloned: boolean;
  worktrees: WorktreeRow[];
}

/** The worktrees of `projectId` on `hostAlias`. `local` answers from the
 *  DB; a remote host is scanned over SSH (one short call) and cached. */
export async function listHostWorktrees(
  hostAlias: string,
  projectId: number,
): Promise<Result<HostWorktrees>> {
  return invokeCmd<HostWorktrees>('list_host_worktrees', {
    args: { host_alias: hostAlias, project_id: projectId },
  });
}
```

- [ ] **Step 2: Type-check**

Run: `npx svelte-check --tsconfig ./tsconfig.json`
Expected: `0 errors`.

- [ ] **Step 3: Commit**

```bash
git add src/lib/projects.ts
git commit -m "feat(projects): listHostWorktrees wrapper"
```

---

### Task 6: Dialog lists the chosen host's worktrees

**Files:**
- Modify: `src/lib/NewSessionDialog.svelte`
- Test: `src/lib/NewSessionDialog.test.ts`

- [ ] **Step 1: Add a test helper and the failing tests**

Near the top of `src/lib/NewSessionDialog.test.ts`, after `const project = …`, add:

```ts
const remoteMain = { id: 501, project_id: 1, host_alias: 'mefistos', name: 'main', path: '/home/u/projects/github.com/martin-janci/claude-fleet', branch: 'main' };
const remoteFeat = { id: 502, project_id: 1, host_alias: 'mefistos', name: 'feat', path: '/home/u/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/feat', branch: 'feature/feat' };

/** Answer `list_host_worktrees` for mefistos; other commands keep `extra`. */
function mockHostWorktrees(
  reply: { cloned: boolean; worktrees: unknown[] } | Error | (() => Promise<unknown>),
  extra: (cmd: string, args?: unknown) => unknown = () => null,
) {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, args?: unknown) => {
    if (cmd === 'list_host_worktrees') {
      if (reply instanceof Error) throw { code: 'E_SSH', message: reply.message };
      if (typeof reply === 'function') return reply();
      const a = (args as { args: { host_alias: string; project_id: number } }).args;
      return { host_alias: a.host_alias, project_id: a.project_id, ...reply };
    }
    return extra(cmd, args);
  });
}

function worktreeLabels(): string[] {
  return Array.from(document.querySelectorAll('[data-testid="wt-picker"] [role="option"] .label')).map(
    (e) => e.textContent?.trim() ?? '',
  );
}
```

Add a new `describe` block:

```ts
describe('NewSessionDialog host-scoped worktrees', () => {
  async function pickHost(alias: string) {
    const btn = Array.from(document.querySelectorAll('.host-pick')).find(
      (p) => p.textContent?.trim() === alias,
    ) as HTMLButtonElement;
    await fireEvent.click(btn);
    await tick();
  }

  it('local never calls list_host_worktrees and lists the project tree rows', async () => {
    mockHostWorktrees({ cloned: true, worktrees: [] });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    expect(worktreeLabels()).toEqual(['main', '+ new worktree']);
    const calls = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'list_host_worktrees');
    expect(calls).toHaveLength(0);
  });

  it('switching to a remote host scans it once and swaps the list', async () => {
    mockHostWorktrees({ cloned: true, worktrees: [remoteMain, remoteFeat] });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickHost('mefistos');
    await vi.waitFor(() => expect(worktreeLabels()).toEqual(['main', 'feat', '+ new worktree']));
    const calls = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.filter((c) => c[0] === 'list_host_worktrees');
    expect(calls).toHaveLength(1);
    expect((calls[0][1] as any).args).toEqual({ host_alias: 'mefistos', project_id: 1 });
    // The remote main row is selected, not the local one.
    expect(document.querySelector('[data-testid="wt-picker"] [role="option"].active')?.getAttribute('data-key')).toBe('501');
  });

  it('shows a scanning status while the scan is in flight', async () => {
    let resolve!: (v: unknown) => void;
    mockHostWorktrees(() => new Promise((r) => (resolve = r)));
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickHost('mefistos');
    expect(screen.getByTestId('wt-status')).toHaveTextContent('Scanning mefistos');
    expect(worktreeLabels()).toEqual(['+ new worktree']);
    resolve({ host_alias: 'mefistos', project_id: 1, cloned: true, worktrees: [remoteMain] });
    await vi.waitFor(() => expect(screen.queryByTestId('wt-status')).toBeNull());
    expect(worktreeLabels()).toEqual(['main', '+ new worktree']);
  });

  it('a host without the clone offers only + new worktree and says so', async () => {
    mockHostWorktrees({ cloned: false, worktrees: [] });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickHost('mefistos');
    await vi.waitFor(() => expect(screen.getByTestId('wt-status')).toHaveTextContent('Not cloned on mefistos yet'));
    expect(worktreeLabels()).toEqual(['+ new worktree']);
    expect(document.querySelector('[data-testid="wt-picker"] [role="option"].active')?.getAttribute('data-key')).toBe('new');
    expect((screen.getByTestId('new-worktree-name') as HTMLInputElement).value).not.toBe('');
  });

  it('a failed scan shows the error and keeps + new worktree usable', async () => {
    mockHostWorktrees(new Error('ssh: connect to host mefistos: timed out'));
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickHost('mefistos');
    await vi.waitFor(() => expect(screen.getByTestId('wt-status')).toHaveTextContent('timed out'));
    expect(worktreeLabels()).toEqual(['+ new worktree']);
    expect(screen.getByText('Create')).not.toBeDisabled();
  });

  it('a slow earlier scan cannot overwrite a later host', async () => {
    hosts.update((h) => [...h, { alias: 'vps', ssh_alias: 'vps', reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false }]);
    let resolveMef!: (v: unknown) => void;
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd !== 'list_host_worktrees') return null;
      const host = (args as { args: { host_alias: string } }).args.host_alias;
      if (host === 'mefistos') return new Promise((r) => (resolveMef = r));
      return { host_alias: 'vps', project_id: 1, cloned: true, worktrees: [{ ...remoteMain, id: 601, host_alias: 'vps' }] };
    });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickHost('mefistos');
    await pickHost('vps');
    await vi.waitFor(() => expect(worktreeLabels()).toEqual(['main', '+ new worktree']));
    resolveMef({ host_alias: 'mefistos', project_id: 1, cloned: true, worktrees: [remoteMain, remoteFeat] });
    await tick(); await tick();
    expect(worktreeLabels()).toEqual(['main', '+ new worktree']);
    expect(document.querySelector('[data-testid="wt-picker"] [role="option"].active')?.getAttribute('data-key')).toBe('601');
  });

  it('remembers the worktree per host', async () => {
    mockHostWorktrees({ cloned: true, worktrees: [remoteMain, remoteFeat] });
    const spy = vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({ ok: true, value: { id: 1 } as any });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await pickHost('mefistos');
    await vi.waitFor(() => expect(worktreeLabels()).toContain('feat'));
    await fireEvent.click(document.querySelector('[data-testid="wt-picker"] [data-key="502"]')!);
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    const mem = JSON.parse(localStorage.getItem('cf:pref:newsession.project.1')!);
    expect(mem.host).toBe('mefistos');
    expect(mem.worktrees).toEqual({ mefistos: 502 });
    spy.mockRestore();
    // Re-open: mefistos remembers feat, local still defaults to its first row.
    document.body.innerHTML = '';
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await vi.waitFor(() =>
      expect(document.querySelector('[data-testid="wt-picker"] [role="option"].active')?.getAttribute('data-key')).toBe('502'),
    );
    await pickHost('local');
    expect(document.querySelector('[data-testid="wt-picker"] [role="option"].active')?.getAttribute('data-key')).toBe('11');
  });
});
```

Also update the two existing tests that switch to mefistos and rely on a worktree being selected, `'clicking a host pick + Create sends host_alias to new_session'` (line ~85) and `'a second session on the same worktree gets a name-suffixed tmux name instead of a collision'` (line ~450): inside their `mockImplementation`, add

```ts
      if (cmd === 'list_host_worktrees') return { host_alias: 'mefistos', project_id: 1, cloned: true, worktrees: [remoteMain] };
```

and, after clicking the mefistos button, `await vi.waitFor(() => expect(worktreeLabels()).toContain('main'));` before asserting. The two "remote path preview" tests near the top do not need rows (new mode still previews the root); leave them.

- [ ] **Step 2: Run to verify they fail**

Run: `npx vitest run src/lib/NewSessionDialog.test.ts -t 'host-scoped'`
Expected: FAIL (`wt-status` not found, labels include local rows on mefistos, etc.).

- [ ] **Step 3: Implement in `NewSessionDialog.svelte`**

Imports: add `listHostWorktrees` to the `./projects` import: `import { listHostWorktrees, type ProjectTreeRow, type WorktreeRow } from './projects';`.

Replace the `ProjectMemory` block (lines ~47-60) with:

```ts
  interface ProjectMemory {
    host: string;
    /** Legacy (pre host-scoped picker): the local host's choice. */
    worktree?: number | 'new';
    /** Per host: worktree row id or 'new'. */
    worktrees?: Record<string, number | 'new'>;
    kind: 'work' | 'shell';
  }
  const isChoice = (v: unknown): v is number | 'new' => v === 'new' || typeof v === 'number';
  const isMemory = (v: unknown): v is ProjectMemory =>
    typeof v === 'object' &&
    v !== null &&
    typeof (v as ProjectMemory).host === 'string' &&
    ((v as ProjectMemory).worktree === undefined || isChoice((v as ProjectMemory).worktree)) &&
    ((v as ProjectMemory).worktrees === undefined ||
      (typeof (v as ProjectMemory).worktrees === 'object' &&
        Object.values((v as ProjectMemory).worktrees!).every(isChoice))) &&
    ((v as ProjectMemory).kind === 'work' || (v as ProjectMemory).kind === 'shell');
  const memoryKey = `newsession.project.${projectId}`;
  const memory = readPref<ProjectMemory | null>(memoryKey, null, (v): v is ProjectMemory | null => v === null || isMemory(v));
  /** The remembered choice for `host` (legacy flat value counts for local). */
  function rememberedFor(host: string): number | 'new' | undefined {
    return memory?.worktrees?.[host] ?? (host === 'local' ? memory?.worktree : undefined);
  }
```

Replace the `// ── Worktree choice ──` block's `initialWorktree` and the `chosenWorktree` derivation with:

```ts
  // ── Worktree choice ──────────────────────────────────────────────────
  // The rows on offer are the CHOSEN HOST's: local answers from the project
  // tree synchronously; a remote host is scanned over SSH (`hostWorktrees`).
  type HostWorktreesState = {
    status: 'loading' | 'ready' | 'error';
    rows: WorktreeRow[];
    cloned: boolean;
    error?: string;
  };
  let hostWorktrees = $state<HostWorktreesState>(
    untrack(() => ({ status: 'ready', rows: project.worktrees, cloned: true })),
  );
  // A slow scan of the previous host must not land after a newer one.
  let scanSeq = 0;
  $effect(() => {
    const host = chosenHost;
    const localRows = project.worktrees;
    if (host === 'local') {
      scanSeq++;
      hostWorktrees = { status: 'ready', rows: localRows, cloned: true };
      return;
    }
    const seq = ++scanSeq;
    hostWorktrees = { status: 'loading', rows: [], cloned: true };
    void listHostWorktrees(host, projectId).then((r) => {
      if (seq !== scanSeq) return;
      if (!r.ok) {
        hostWorktrees = { status: 'error', rows: [], cloned: true, error: r.error.message };
        return;
      }
      hostWorktrees = {
        status: 'ready',
        rows: r.value?.worktrees ?? [],
        cloned: r.value?.cloned ?? true,
      };
    });
  });

  function initialWorktree(): number | null {
    const remembered = rememberedFor(untrack(() => chosenHost));
    if (remembered === 'new') return null;
    if (typeof remembered === 'number' && project.worktrees.some((w) => w.id === remembered)) {
      return remembered;
    }
    return project.worktrees[0]?.id ?? null;
  }
  let chosenWorktreeId = $state<number | null>(untrack(initialWorktree));
  let inNewMode = $derived(chosenWorktreeId === null);
  let chosenWorktree = $derived(hostWorktrees.rows.find((w) => w.id === chosenWorktreeId) ?? null);

  // When the host's rows arrive (or change), keep the selection valid: the
  // remembered row for that host, else its `main`, else "+ new worktree".
  $effect(() => {
    if (hostWorktrees.status !== 'ready') return;
    const rows = hostWorktrees.rows;
    const current = untrack(() => chosenWorktreeId);
    if (current !== null && rows.some((w) => w.id === current)) return;
    const remembered = rememberedFor(untrack(() => chosenHost));
    const pick =
      (typeof remembered === 'number' && rows.find((w) => w.id === remembered)) ||
      rows.find((w) => w.name === 'main') ||
      (remembered === 'new' ? null : rows[0]) ||
      null;
    if (pick) onPickWorktree(pick.id);
    else if (current !== null || !untrack(() => newWorktreeName)) onPickNew();
  });
```

Note: `onPickWorktree` / `onPickNew` are function declarations further down the script, so they are hoisted and callable here.

Then replace every remaining read of `project.worktrees` below that point with `hostWorktrees.rows`:
- `takenSlugs`: `for (const w of hostWorktrees.rows) set.add(w.name.toLowerCase());`
- `worktreeDir`: `hostWorktrees.rows.some((w) => w.path.includes('/.claude/worktrees/'))` — fall back to `project.worktrees` when the host list is empty: `(hostWorktrees.rows.length ? hostWorktrees.rows : project.worktrees).some(…)`.
- `pathPreview`: for a remote host with a row, use the row's real path: `return chosenHost === 'local' ? wt.path : wt.path || \`${root}/.claude/worktrees/${wt.name}\`;`
- `worktreeItems`: build from `hostWorktrees.rows`; when `status !== 'ready'` or `!cloned`, offer only the `new` item:

```ts
  const worktreeItems: PickerItem[] = $derived([
    ...(hostWorktrees.status === 'ready' && hostWorktrees.cloned ? hostWorktrees.rows : []).map((wt) => ({
      key: String(wt.id),
      label: wt.name,
      description: wt.branch && wt.branch !== wt.name ? wt.branch : undefined,
      meta: $sessions.some((s) => s.worktree_id === wt.id && s.status !== 'ghost') ? 'in use' : undefined,
      testid: 'worktree-row',
    })),
    { key: 'new', label: '+ new worktree', description: 'fresh branch from the base branch', testid: 'new-worktree-chip' },
  ]);
  const worktreeStatus = $derived.by((): string | null => {
    if (hostWorktrees.status === 'loading') return `Scanning ${chosenHost}…`;
    if (hostWorktrees.status === 'error') return `Couldn't list worktrees on ${chosenHost}: ${hostWorktrees.error}`;
    if (!hostWorktrees.cloned) return `Not cloned on ${chosenHost} yet — it is cloned on the first session.`;
    return null;
  });
```

- `onPickWorktree`: `const wt = hostWorktrees.rows.find((w) => w.id === id) ?? null;`
- the friendly-name `$effect` around line 159 that reads `project.worktrees.find(...)`: same replacement.

`remember()` writes per host:

```ts
  function remember() {
    const prev = readPref<ProjectMemory | null>(memoryKey, null, (v): v is ProjectMemory | null => v === null || isMemory(v));
    writePref<ProjectMemory>(memoryKey, {
      host: chosenHost,
      kind: chosenKind,
      worktrees: { ...(prev?.worktrees ?? {}), [chosenHost]: chosenWorktreeId === null ? 'new' : chosenWorktreeId },
    });
  }
```

Template: under `<label for="wt-picker">Worktree</label>` add, before `<PickerList …>`:

```svelte
    {#if worktreeStatus}
      <p class="wt-status" data-testid="wt-status" class:err={hostWorktrees.status === 'error'}>{worktreeStatus}</p>
    {/if}
```

and in `<style>`:

```css
  .wt-status { font-size: 0.72rem; color: var(--fg-muted); margin: 0 0 0.2rem; }
  .wt-status.err { color: var(--danger, #d85a30); }
```

(check the variable name used for error text elsewhere in this file's `<style>` — reuse it instead of `--danger` if it differs).

- [ ] **Step 4: Run the dialog tests and the type check**

Run: `npx vitest run src/lib/NewSessionDialog.test.ts && npx svelte-check --tsconfig ./tsconfig.json`
Expected: all dialog tests pass (new and old), `0 errors`.

- [ ] **Step 5: Commit**

```bash
git add src/lib/NewSessionDialog.svelte src/lib/NewSessionDialog.test.ts
git commit -m "feat(new-session): list the chosen host's worktrees; scan remote hosts"
```

---

### Task 7: `showRowDetails` store and the "details" pill

**Files:**
- Modify: `src/lib/sessions.ts:147` (next to `showBgAgents`)
- Modify: `src/lib/SidebarFilters.svelte` (triage nav)
- Test: `src/lib/Sidebar.test.ts`

- [ ] **Step 1: Write the failing test**

Add to `src/lib/Sidebar.test.ts` (import `showRowDetails` from `./sessions` alongside `showBgAgents`, and reset it in `beforeEach` with `showRowDetails.set(true);`):

```ts
  it('the details pill hides the second row line and persists', async () => {
    mockBackend(fakeProjects, [{ ...sessionFor(1, 'dev-a'), started_at: Math.floor(Date.now() / 1000) - 60 }]);
    render(Sidebar);
    await tick(); await tick();
    expect(screen.getByTestId('sess-details')).toBeInTheDocument();
    const pill = screen.getByTestId('toggle-row-details');
    expect(pill).toHaveAttribute('aria-pressed', 'true');
    await fireEvent.click(pill);
    await tick();
    expect(screen.queryByTestId('sess-details')).toBeNull();
    expect(pill).toHaveAttribute('aria-pressed', 'false');
    expect(JSON.parse(localStorage.getItem('cf:pref:rows.details')!)).toBe(false);
  });
```

- [ ] **Step 2: Run to verify it fails**

Run: `npx vitest run src/lib/Sidebar.test.ts -t 'details pill'`
Expected: FAIL (`sess-details` / `toggle-row-details` not found).

- [ ] **Step 3: Implement the store and the pill**

`src/lib/sessions.ts`, after the `showBgAgents` lines:

```ts
// Sidebar density toggle — when true, session rows show their second
// (details) line: host, tmux name / worktree, elapsed, badges, last prompt.
export const showRowDetails = writable<boolean>(readPref('rows.details', true, isBool));
showRowDetails.subscribe((v) => writePref('rows.details', v));
```

`src/lib/SidebarFilters.svelte`: import `showRowDetails` from `./sessions` and add to the `triage` nav, after the `☑ select` button:

```svelte
    <button
      class="pill"
      class:active={$showRowDetails}
      data-testid="toggle-row-details"
      aria-pressed={$showRowDetails}
      title={$showRowDetails ? 'Hide the details line under each session' : 'Show host, worktree, elapsed and badges under each session'}
      onclick={() => showRowDetails.update((v) => !v)}
    >
      ≡ details
    </button>
```

(The row line itself lands in Task 9; this test passes only after Task 9. Run Task 9 before declaring Task 7's test green — or commit Task 7 with the test marked `it.skip` and un-skip it in Task 9. Prefer the latter.)

- [ ] **Step 4: Type-check and run the sidebar suite**

Run: `npx svelte-check --tsconfig ./tsconfig.json && npx vitest run src/lib/Sidebar.test.ts`
Expected: `0 errors`; suite green with the new test skipped.

- [ ] **Step 5: Commit**

```bash
git add src/lib/sessions.ts src/lib/SidebarFilters.svelte src/lib/Sidebar.test.ts
git commit -m "feat(sidebar): persisted details toggle for session rows"
```

---

### Task 8: `rowElapsed` / `rowPrompt` helpers

**Files:**
- Modify: `src/lib/session_status.ts`
- Test: create `src/lib/session_status.test.ts` if it does not exist (check first)

- [ ] **Step 1: Write the failing test**

```ts
import { describe, it, expect } from 'vitest';
import { rowElapsed, rowPrompt, rowMeta } from './session_status';
import type { SessionRow } from './sessions';

const base = { started_at: null, last_prompt: null, last_turn_at: null } as unknown as SessionRow;

describe('row text helpers', () => {
  it('rowElapsed is empty without a start and formats h/m otherwise', () => {
    expect(rowElapsed(base, 1000)).toBe('');
    expect(rowElapsed({ ...base, started_at: 1000 - 3 * 3600 - 5 * 60 }, 1000)).toBe('3h 5m');
  });
  it('rowPrompt takes the first line, truncated', () => {
    expect(rowPrompt(base)).toBe('');
    expect(rowPrompt({ ...base, last_prompt: 'Implement the triage filter\nsecond line' })).toBe('Implement the triage filter');
  });
  it('rowMeta still joins both with a dot', () => {
    expect(rowMeta({ ...base, started_at: 1000 - 60, last_prompt: 'hi' }, 1000)).toBe('1m · hi');
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npx vitest run src/lib/session_status.test.ts`
Expected: FAIL (`rowElapsed` is not exported).

- [ ] **Step 3: Implement**

Replace `rowMeta` in `src/lib/session_status.ts` with:

```ts
/** Elapsed since the session started ("3h 5m"), or '' before it started. */
export function rowElapsed(sess: SessionRow, nowSec: number): string {
  return sess.started_at !== null ? formatElapsed(sessionStart(sess), nowSec) : '';
}

/** First line of the last prompt, truncated for the row; '' when none. */
export function rowPrompt(sess: SessionRow): string {
  return promptPreview(sess.last_prompt, 48) ?? '';
}

/** Secondary row text: elapsed since start + the last prompt's first line. */
export function rowMeta(sess: SessionRow, nowSec: number): string {
  return [rowElapsed(sess, nowSec), rowPrompt(sess)].filter(Boolean).join(' · ');
}
```

Check `promptPreview`'s return type in `src/lib/attention.ts`; if it returns `string` (never null), drop the `?? ''`.

- [ ] **Step 4: Run the test**

Run: `npx vitest run src/lib/session_status.test.ts`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add src/lib/session_status.ts src/lib/session_status.test.ts
git commit -m "refactor(sidebar): split rowMeta into rowElapsed and rowPrompt"
```

---

### Task 9: Two-line session row

**Files:**
- Modify: `src/lib/SessionRowItem.svelte` (script imports, the non-ghost branch of the markup, CSS)
- Modify: `src/lib/Sidebar.test.ts` (un-skip Task 7's test; adjust `host-badge` and friendly-name tests)

- [ ] **Step 1: Write / adjust the failing tests**

In `src/lib/Sidebar.test.ts`:

1. Un-skip `'the details pill hides the second row line and persists'`.
2. In `'shows host badge before each session name'` (~line 663) rename to `'shows the host in the details line of each session'` and change the assertions to:

```ts
    const badges = screen.queryAllByTestId('host-badge');
    expect(badges).toHaveLength(1);
    expect(badges[0].textContent).toBe('local');
    expect(badges[0].closest('[data-testid="sess-details"]')).not.toBeNull();
```

3. In `'shows the friendly name by default with tmux_name as secondary text'` add:

```ts
    expect(row.querySelector('.sess-line1 .sess-name')).toHaveTextContent('Fix login');
    expect(screen.getByTestId('sess-tmux-name').closest('[data-testid="sess-details"]')).not.toBeNull();
```

4. Add:

```ts
  it('line 1 holds the name and one status chip; line 2 holds host, worktree, elapsed and prompt', async () => {
    const now = Math.floor(Date.now() / 1000);
    const s = {
      ...sessionFor(1, 'dev-martin-janci-claude-fleet--fix-login'),
      worktree_key: 'fix-login',
      claude_status: 'working' as const,
      started_at: now - 3600,
      last_prompt: 'Implement the triage filter',
      context_pct: 62,
    };
    mockBackend(fakeProjects, [s]);
    render(Sidebar);
    await tick(); await tick();
    const row = screen.getByTestId('sess-row');
    const line1 = row.querySelector('.sess-line1')!;
    expect(line1.querySelector('.sess-name')).toHaveTextContent('dev-martin-janci-claude-fleet--fix-login');
    expect(line1.querySelector('[data-testid="claude-chip"]')).toHaveTextContent('working');
    const details = screen.getByTestId('sess-details');
    expect(details.querySelector('[data-testid="host-badge"]')).toHaveTextContent('local');
    expect(details).toHaveTextContent('fix-login');
    expect(details).toHaveTextContent('1h');
    expect(details.querySelector('[data-testid="context-badge"]')).not.toBeNull();
    expect(screen.getByTestId('sess-meta')).toHaveTextContent('Implement the triage filter');
    expect(line1.querySelector('[data-testid="host-badge"]')).toBeNull();
  });
```

The existing `'shows elapsed time, last prompt and a CI badge as secondary row text'` test keeps passing if `sess-meta` now contains only the prompt and the elapsed text sits in its own span inside `sess-details`: change its `expect(meta).toHaveTextContent('3h 5m')` to `expect(screen.getByTestId('sess-details')).toHaveTextContent('3h 5m')`.

- [ ] **Step 2: Run to verify they fail**

Run: `npx vitest run src/lib/Sidebar.test.ts`
Expected: the four touched tests fail (`sess-line1` / `sess-details` missing, host badge text `[local]`).

- [ ] **Step 3: Implement the layout**

`src/lib/SessionRowItem.svelte` script: import `showRowDetails` from `./sessions` and replace `import { rowMeta, timeAgo } from './session_status';` with `import { rowElapsed, rowPrompt, timeAgo } from './session_status';`. Replace `const meta = $derived(rowMeta(sess, nowSec));` with:

```ts
  const elapsed = $derived(rowElapsed(sess, nowSec));
  const prompt = $derived(rowPrompt(sess));
  const primaryIsFriendly = $derived($showFriendlyNames && !!sess.friendly_name);
  const primaryName = $derived(primaryIsFriendly ? sess.friendly_name! : sess.tmux_name);
  // Line 2 names what line 1 does not: the tmux name under a friendly name,
  // else the worktree when it is not already part of the tmux name.
  const secondaryName = $derived.by((): string | null => {
    if (primaryIsFriendly) return sess.tmux_name;
    if (sess.worktree_key && !sess.tmux_name.endsWith(`--${sess.worktree_key}`)) return sess.worktree_key;
    return null;
  });
```

Markup: replace the whole `{:else}` (live row) branch — from `<span class="status-dot status-{sess.status}" …>` through the closing `</div>` of `row-actions` — with:

```svelte
      <div class="sess-lines">
        <div class="sess-line1">
          <span class="status-dot status-{sess.status}" title={sess.status} aria-hidden="true"></span>
          {#if relatedCount > 0}
            <span class="related-badge" data-testid="related-badge" role="img" title="{relatedCount} related session(s)" aria-label="{relatedCount} related sessions">🔗{relatedCount}</span>
          {/if}
          {#if sess.kind === 'review'}
            <span class="review-badge" role="img" title="review session" aria-label="review session">🔍</span>
          {/if}
          {#if sess.kind === 'shell'}
            <span class="shell-badge" title="shell session">▶</span>
          {/if}
          {#if sess.kind === 'bg'}
            <span class="bg-badge" role="img" title="background agent" aria-label="background agent">🤖</span>
          {/if}
          <span class="sess-name" title={primaryIsFriendly ? sess.tmux_name : undefined}>{primaryName}</span>
          {#if sess.stuck_kind}
            <span
              class="claude-chip stuck-chip"
              data-testid="stuck-chip"
              style="background: {STUCK_COLOR}22; color: {STUCK_COLOR}; border-color: {STUCK_COLOR}66;"
              title="Stuck: {stuckKindLabel(sess.stuck_kind)}{sess.current_activity ? ' — ' + sess.current_activity : ''}"
            >⚠ stuck: {stuckKindLabel(sess.stuck_kind)}</span>
          {:else if sess.claude_status}
            <span
              class="claude-chip"
              data-testid="claude-chip"
              style="background: {claudeStatusColor(sess.claude_status)}22; color: {claudeStatusColor(sess.claude_status)}; border-color: {claudeStatusColor(sess.claude_status)}44;"
              title="Claude: {sess.claude_status}{sess.current_activity ? ' — ' + sess.current_activity : ''}"
            >{claudeStatusLabel(sess.claude_status)}</span>
          {/if}
          <div class="row-actions">
            <!-- the existing six buttons, unchanged: peek, restart, edit-label, rename-tmux, recreate-live, kill -->
          </div>
        </div>
        {#if $showRowDetails}
          <div class="sess-details" data-testid="sess-details">
            <span class="host-badge" data-testid="host-badge">{sess.host_alias}</span>
            {#if secondaryName}
              <span class="sess-secondary" data-testid="sess-tmux-name">{secondaryName}</span>
            {/if}
            {#if elapsed}
              <span class="sess-elapsed">{elapsed}</span>
            {/if}
            {#if ctxLevel !== null && sess.context_pct !== null}
              <!-- the existing context meter span, unchanged -->
            {/if}
            {#if sessionUsageTokens(sess) > 0}
              <!-- the existing cost-badge span, unchanged -->
            {/if}
            {#if sess.effort_level}
              <span class="effort-badge" title="Effort: {sess.effort_level}">{sess.effort_level}</span>
            {/if}
            {#if sess.pr_url}
              <!-- the existing PR link + ci-badge, unchanged -->
            {/if}
            {#if prompt}
              <span class="sess-meta" data-testid="sess-meta" title={sess.last_prompt ?? undefined}>{prompt}</span>
            {/if}
          </div>
        {/if}
      </div>
```

Copy the elided blocks verbatim from the current file (the comments mark where they go). Keep the `data-testid="sess-tmux-name"` on the secondary name so the existing friendly-name test holds.

CSS: change `.sess-row` to `align-items: flex-start;` is NOT needed — keep `align-items: center` for the checkbox; add:

```css
  .sess-lines { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 0.1rem; }
  .sess-line1 { display: flex; align-items: center; gap: 0.4rem; min-width: 0; }
  .sess-line1 .sess-name { flex: 1; }
  .sess-details {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    min-width: 0;
    padding-left: 0.85rem;
    font-size: 0.65rem;
    color: var(--fg-muted);
    white-space: nowrap;
    overflow: hidden;
  }
  .sess-details > * { flex-shrink: 0; }
  .sess-details > .sess-meta { flex-shrink: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; }
  .sess-details > * + *::before { content: '·'; margin-right: 0.35rem; color: var(--fg-muted); opacity: 0.6; }
  .sess-details > .host-badge::before { content: none; }
```

Remove the now-unused `.sess-main` rule and the `[`/`]` brackets from the ghost row's host badge too (`<span class="host-badge" data-testid="host-badge">{sess.host_alias}</span>`) so both variants read the same. The `.host-badge`, `.sess-secondary`, `.sess-meta` rules stay.

- [ ] **Step 4: Run the sidebar suite and the type check**

Run: `npx vitest run src/lib/Sidebar.test.ts && npx svelte-check --tsconfig ./tsconfig.json`
Expected: all pass, `0 errors`.

- [ ] **Step 5: Commit**

```bash
git add src/lib/SessionRowItem.svelte src/lib/Sidebar.test.ts
git commit -m "feat(sidebar): two-line session rows with the name on its own line"
```

---

### Task 10: Full CI mirror, version bump, build, install, push

- [ ] **Step 1: CI mirror**

```bash
(cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test)
pnpm install --frozen-lockfile && npx svelte-check --tsconfig ./tsconfig.json && npx vitest run && pnpm run build
```

Expected: every command exits 0. Fix anything red before continuing.

- [ ] **Step 2: Bump to 0.2.13**

Run the same node snippet used for 0.2.12 against `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`; commit as `chore(release): bump version to 0.2.13`.

- [ ] **Step 3: Build and install**

```bash
CARGO_TARGET_DIR=/Volumes/CargoSD/target/claude-fleet pnpm tauri build --bundles app
```

Then quit the app (`osascript -e 'tell application "claude-fleet" to quit'`), move `/Applications/claude-fleet.app` to `.bak`, copy the bundle from `/Volumes/CargoSD/target/claude-fleet/release/bundle/macos/claude-fleet.app`, `xattr -dr com.apple.quarantine`, `open -a`, and verify `CFBundleShortVersionString` is 0.2.13. Commit the refreshed `src-tauri/Cargo.lock`.

- [ ] **Step 4: Push**

`git fetch origin && git rebase origin/main && git push origin HEAD:main` (gh cannot open PRs from this machine). Verify `git rev-list --left-right --count HEAD...origin/main` prints `0 0`.

---

## Self-review

- Spec coverage: A backend (T1–T3), A `new_session` guard (T4), A frontend + memory + states (T5–T6), B layout + friendly-primary + secondary name (T9), B toggle (T7), `rowMeta` split (T8), tests for each. Out-of-scope items untouched.
- Types: `HostWorktrees { host_alias, project_id, cloned, worktrees }` identical in Rust (T2/T3) and TS (T5/T6); `ListHostWorktreesArgs` field names match the wrapper's `args`; `fetch_worktree` returns a 3-tuple everywhere after T4; `showRowDetails` / pref key `rows.details` consistent between T7 and T9; `rowElapsed` / `rowPrompt` used in T9 as defined in T8.
- Placeholders: none; the two "unchanged" markup blocks in T9 name the exact spans to copy from the current file.
