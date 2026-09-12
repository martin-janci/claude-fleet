//! Standalone worktree management: list worktrees with their "occupied by an
//! alive Claude session" status, and delete a worktree off the remote host +
//! drop the DB row. Refuses to delete an occupied worktree unless `force` is
//! passed.

use crate::ipc_error::IpcError;
use crate::shell::quote;
use crate::ssh::{SshClient, SshExec};
use crate::store::{Store, WorktreeRow};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One worktree row with its alive-session occupants attached. `occupants`
/// is empty when the worktree is free to delete.
#[derive(Debug, Clone, Serialize)]
pub struct WorktreeOccupancy {
    pub worktree: WorktreeRow,
    pub occupants: Vec<WorktreeOccupant>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorktreeOccupant {
    pub host_alias: String,
    pub tmux_name: String,
}

#[derive(Deserialize)]
pub struct ListWorktreesArgs {
    /// Filter to one project; omit for all projects across the fleet.
    pub project_id: Option<i64>,
}

pub fn list_worktrees(
    args: ListWorktreesArgs,
    store: &Mutex<Store>,
) -> Result<Vec<WorktreeOccupancy>, IpcError> {
    let s = store.lock().map_err(|_| IpcError::lock())?;
    let projects = s.list_projects().map_err(IpcError::from)?;
    let mut out = Vec::new();
    for proj in projects {
        if let Some(pid) = args.project_id {
            if proj.id != pid {
                continue;
            }
        }
        let worktrees = s
            .list_worktrees_for_project(proj.id)
            .map_err(IpcError::from)?;
        for wt in worktrees {
            let occupants = s
                .alive_sessions_for_worktree(wt.id)
                .map_err(IpcError::from)?
                .into_iter()
                .map(|(host_alias, tmux_name)| WorktreeOccupant {
                    host_alias,
                    tmux_name,
                })
                .collect();
            out.push(WorktreeOccupancy {
                worktree: wt,
                occupants,
            });
        }
    }
    Ok(out)
}

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
                path.rsplit('/')
                    .next()
                    .filter(|s| !s.is_empty())?
                    .to_string()
            };
            Some((name, path, w.branch))
        })
        .collect()
}

/// Production entry point: the shared SSH client. Not yet wired to a Tauri
/// command / MCP tool (follow-up task); exercised directly via
/// `list_host_worktrees_with` in tests until then.
#[allow(dead_code)]
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
    let (root, _) =
        crate::service::repair::resolve_remote_paths(ssh, host, &base, layout, &owner, &repo, None)
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

#[derive(Deserialize)]
pub struct DeleteWorktreeArgs {
    pub worktree_id: i64,
    /// Bypass the alive-session occupant guard. The git-level dirty/conflict
    /// check still applies. Off by default.
    #[serde(default)]
    pub force: bool,
}

/// Delete a worktree on the remote host (via `git worktree remove`) and drop
/// the DB row. Refuses if any alive session is currently attached to the
/// worktree unless `force == true`.
///
/// Errors with `E_WORKTREE_BUSY` (occupied), `E_NOTFOUND` (no such row),
/// or `E_GIT` (git command failed — typically dirty tree or untracked files).
pub async fn delete_worktree(
    args: DeleteWorktreeArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    // Resolve everything we need under one lock; the SSH call below runs
    // off-lock.
    let (worktree_path, project_base_path, host_alias) = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        let wt = s
            .get_worktree_row(args.worktree_id)
            .map_err(IpcError::from)?
            .ok_or_else(|| {
                IpcError::new(
                    "E_NOTFOUND",
                    format!("worktree {} not found", args.worktree_id),
                )
            })?;
        // A remote host's row names a checkout on that host, reported by its
        // EnterWorktree hook; the `git -C <local project base>` below would
        // aim at the wrong filesystem. Its ExitWorktree hook removes it.
        if wt.host_alias != crate::service::projects::LOCAL_HOST {
            return Err(IpcError::new(
                "E_INVALID",
                format!(
                    "worktree {} is a checkout on host {}; remove it there (ExitWorktree)",
                    wt.id, wt.host_alias
                ),
            ));
        }
        if !args.force {
            let occupants = s
                .alive_sessions_for_worktree(wt.id)
                .map_err(IpcError::from)?;
            if !occupants.is_empty() {
                let who = occupants
                    .iter()
                    .map(|(h, n)| format!("{h}/{n}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(IpcError::new(
                    "E_WORKTREE_BUSY",
                    format!("worktree is in use by session(s): {who}"),
                ));
            }
        }
        let proj_base = s
            .project_base_path(wt.project_id)
            .map_err(IpcError::from)?
            .ok_or_else(|| {
                IpcError::new(
                    "E_NOTFOUND",
                    format!("project {} for worktree has no base path", wt.project_id),
                )
            })?;
        // Pick a host to run `git worktree remove` on. Prefer any session's
        // host (alive or not) so we hit the box where the worktree lives;
        // fall back to "local" when no session row remembers it.
        let host = s
            .alive_sessions_for_worktree(wt.id)
            .map_err(IpcError::from)?
            .first()
            .map(|(h, _)| h.clone())
            .unwrap_or_else(|| "local".to_string());
        (wt.path, proj_base, host)
    };

    // Run `git worktree remove`. No --force: respect uncommitted work. The
    // user can pass `force=true` for the occupant check, but the git-level
    // dirty guard is intentional — we never want to silently lose work.
    let cmd = format!(
        "git -C {} worktree remove {}",
        quote(&project_base_path),
        quote(&worktree_path)
    );
    let out = if host_alias == "local" {
        tokio::process::Command::new("bash")
            .args(["-lc", &cmd])
            .output()
            .await
            .map_err(|e| IpcError::new("E_SHELL", format!("spawn bash: {e}")))?
    } else {
        ssh.run(
            &host_alias,
            &["bash", "-lc", &quote(&cmd)],
            std::time::Duration::from_secs(30),
        )
        .await?
    };
    if !out.status.success() {
        return Err(IpcError::new(
            "E_GIT",
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }

    // Resolve the fingerprint keys before taking the lock: canonicalizing a
    // local path touches the filesystem (it can hang on a dead NFS mount).
    let fp_keys = Store::fingerprint_keys_of_worktree(store, args.worktree_id);
    let s = store.lock().map_err(|_| IpcError::lock())?;
    s.delete_worktree(args.worktree_id, &fp_keys)
        .map_err(IpcError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A remote host's row (from its EnterWorktree hook) is never removed
    /// through the local project base: refused before any git runs, row kept.
    #[tokio::test]
    async fn delete_worktree_refuses_a_remote_hosts_row() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let id = {
            let s = store.lock().unwrap();
            let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
            s.upsert_worktree_on("vps", pid, "feat", "/home/u/r/.worktrees/feat", None)
                .unwrap()
        };
        let ssh = Arc::new(SshClient::new());
        let err = delete_worktree(
            DeleteWorktreeArgs {
                worktree_id: id,
                force: true,
            },
            &store,
            &ssh,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_INVALID");
        assert!(store
            .lock()
            .unwrap()
            .get_worktree_row(id)
            .unwrap()
            .is_some());
    }
    use crate::store::Store;

    #[test]
    fn list_worktrees_reports_no_occupants_for_empty_db() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let out = list_worktrees(ListWorktreesArgs { project_id: None }, &store).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn list_worktrees_flags_alive_session_occupant() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let pid;
        let wid;
        {
            let s = store.lock().unwrap();
            s.upsert_host("alpha").unwrap();
            pid = s.upsert_project("o", "r", "/p").unwrap();
            wid = s
                .upsert_worktree(pid, "feat", "/p/.worktrees/feat", None)
                .unwrap();
            let sid = s
                .upsert_session("sess", "alpha", Some(pid), Some(wid), 0, 0, "running", None)
                .unwrap();
            // sanity
            assert!(sid > 0);
        }
        let out = list_worktrees(ListWorktreesArgs { project_id: None }, &store).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].worktree.id, wid);
        assert_eq!(out[0].occupants.len(), 1);
        assert_eq!(out[0].occupants[0].tmux_name, "sess");
    }

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
        assert_eq!(
            rows[1].1,
            "/home/u/projects/github.com/o/r/.claude/worktrees/feat"
        );
    }

    #[tokio::test]
    async fn list_host_worktrees_scans_the_host_and_caches_rows() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let pid = {
            let s = store.lock().unwrap();
            let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
            // A stale remote row the scan will not report, and a local row.
            s.upsert_worktree_on(
                "vps",
                pid,
                "old",
                "/home/u/projects/github.com/o/r/.claude/worktrees/old",
                None,
            )
            .unwrap();
            s.upsert_worktree(
                pid,
                "local-only",
                "/p/o/r/.claude/worktrees/local-only",
                Some("x"),
            )
            .unwrap();
            pid
        };
        let fake = FakeSsh::new();
        fake.with_home("/home/u").on(
            Match::script_contains("worktree list --porcelain"),
            Reply::ok(PORCELAIN),
        );

        let out = list_host_worktrees_with(
            ListHostWorktreesArgs {
                host_alias: "vps".into(),
                project_id: pid,
            },
            &store,
            &fake,
        )
        .await
        .unwrap();

        assert!(out.cloned);
        let names: Vec<&str> = out.worktrees.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["main", "det", "feat"],
            "main first, then by name"
        );
        assert!(out.worktrees.iter().all(|w| w.host_alias == "vps"));
        assert_eq!(out.worktrees[2].branch.as_deref(), Some("feature/feat"));
        let script = fake.calls_for("vps").last().unwrap().script().unwrap();
        assert!(
            script.contains("git -C '/home/u/projects/github.com/o/r' worktree list --porcelain"),
            "{script}"
        );
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
        let pid = store
            .lock()
            .unwrap()
            .upsert_project("o", "r", "/p/o/r")
            .unwrap();
        let fake = FakeSsh::new();
        fake.with_home("/home/u").on(
            Match::script_contains("worktree list --porcelain"),
            Reply::ok("__NOT_CLONED__\n"),
        );
        let out = list_host_worktrees_with(
            ListHostWorktreesArgs {
                host_alias: "vps".into(),
                project_id: pid,
            },
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
            s.upsert_worktree(pid, "main", "/p/o/r", Some("main"))
                .unwrap();
            pid
        };
        let fake = FakeSsh::new();
        let out = list_host_worktrees_with(
            ListHostWorktreesArgs {
                host_alias: "local".into(),
                project_id: pid,
            },
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
        let pid = store
            .lock()
            .unwrap()
            .upsert_project("o", "r", "/p/o/r")
            .unwrap();
        let fake = FakeSsh::new();
        fake.with_home("/home/u").on(
            Match::script_contains("worktree list --porcelain"),
            Reply::fail(128, "fatal: not a git repository"),
        );
        let err = list_host_worktrees_with(
            ListHostWorktreesArgs {
                host_alias: "vps".into(),
                project_id: pid,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_GIT_SETUP");
        assert!(err.message.contains("not a git repository"));
    }
}
