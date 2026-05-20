use crate::ipc_error::IpcError;
use crate::ssh::SshClient;
use crate::store::{SessionRow, Store};
use crate::tmux::{LocalTmux, RemoteTmux, TmuxExec};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::State;

fn exec_for(host: &str, ssh: &Arc<SshClient>) -> Box<dyn TmuxExec> {
    if host == "local" {
        Box::new(LocalTmux)
    } else {
        Box::new(RemoteTmux {
            client: Arc::clone(ssh),
            host: host.to_string(),
        })
    }
}

fn reconcile_sessions(
    s: &Store,
    ssh: &Arc<SshClient>,
) -> Result<Vec<SessionRow>, IpcError> {
    // Ensure the local host always exists (it's the bootstrap default).
    s.upsert_host("local")?;
    let hosts = s.list_hosts()?;
    let mut all_rows: Vec<SessionRow> = Vec::new();

    for host in hosts {
        // Hidden hosts: don't probe (network noise), but DO surface their
        // last-known sessions so they appear in the "Other sessions" group
        // — matches the spec's "hidden host's sessions are still listed"
        // semantic. We don't update reachable here.
        if host.hidden {
            all_rows.extend(s.list_sessions_for_host(&host.alias)?);
            continue;
        }
        let tmux = exec_for(&host.alias, ssh);
        let live = match tmux.list_sessions() {
            Ok(v) => v,
            Err(_e) => {
                // Mark host unreachable but don't fail the whole reconcile;
                // other hosts can still list their sessions. We KEEP the
                // last-known sessions in the DB (no delete_sessions_not_in)
                // and surface them so the UI can render them dimmed/red —
                // a transient network drop shouldn't make the user lose
                // sight of their sessions.
                let _ = s.update_host_probe(
                    &host.alias,
                    false,
                    host.claude_version.as_deref(),
                    host.tmux_version.as_deref(),
                    now_unix(),
                );
                all_rows.extend(s.list_sessions_for_host(&host.alias)?);
                continue;
            }
        };
        // Successful list = reachable. Bump the timestamp.
        let _ = s.update_host_probe(
            &host.alias,
            true,
            host.claude_version.as_deref(),
            host.tmux_version.as_deref(),
            now_unix(),
        );
        let mut keep = Vec::with_capacity(live.len());
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
        s.delete_sessions_not_in(&host.alias, &keep)?;
        all_rows.extend(s.list_sessions_for_host(&host.alias)?);
    }
    Ok(all_rows)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Extract `(owner, repo)` from a path that follows the conventional
/// `.../projects/github.com/<owner>/<repo>/...` layout (the same layout
/// `proj-clean` enforces on disk). Remote hosts often store repos under
/// a different prefix (e.g. `/home/mjanci/...` instead of `/Users/...`),
/// but the GitHub portion is stable — so we match into the repo cell
/// regardless of where the path starts.
fn extract_owner_repo(path: &str) -> Option<(String, String)> {
    static RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"/projects/github\.com/([^/]+)/([^/]+)").expect("static regex")
    });
    let caps = RE.captures(path)?;
    Some((
        caps.get(1)?.as_str().to_string(),
        caps.get(2)?.as_str().to_string(),
    ))
}

fn find_project_id_for_path(
    s: &Store,
    host_alias: &str,
    path: &std::path::Path,
) -> Option<i64> {
    let path_str = path.to_string_lossy();
    let projects = s.list_projects().ok()?;
    if host_alias == "local" {
        // Local paths: existing prefix match (handles worktrees nested under repos).
        return projects
            .into_iter()
            .filter(|p| path_str.starts_with(&p.base_path))
            .max_by_key(|p| p.base_path.len())
            .map(|p| p.id);
    }
    // Remote paths: match by owner+repo extracted from the conventional
    // `.../projects/github.com/<owner>/<repo>/...` layout. Falls through
    // to `None` (orphan) if the path doesn't follow the convention.
    let (owner, repo) = extract_owner_repo(&path_str)?;
    projects
        .into_iter()
        .find(|p| p.owner == owner && p.repo == repo)
        .map(|p| p.id)
}

#[tauri::command]
pub fn list_sessions(
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<SessionRow>, IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    reconcile_sessions(&s, &ssh)
}

#[derive(Deserialize)]
pub struct NewSessionArgs {
    pub host_alias: String,
    pub project_id: i64,
    pub worktree_id: Option<i64>,
    pub name: String,
}

/// Look up `(owner, repo)` for a given project id.
fn fetch_owner_repo(s: &Store, project_id: i64) -> Result<(String, String), IpcError> {
    let mut stmt = s
        .conn_ref()
        .prepare("SELECT owner, repo FROM projects WHERE id=?1")?;
    stmt.query_row(rusqlite::params![project_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })
    .map_err(IpcError::from)
}

/// Look up `(name, branch)` for a worktree id. `branch` may be NULL in the DB.
fn fetch_worktree(s: &Store, worktree_id: i64) -> Result<(String, Option<String>), IpcError> {
    let mut stmt = s
        .conn_ref()
        .prepare("SELECT name, branch FROM worktrees WHERE id=?1")?;
    stmt.query_row(rusqlite::params![worktree_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })
    .map_err(IpcError::from)
}

/// Resolve `$HOME` on a remote host. Each new_session call hits this once;
/// for the iter-1 MVP we don't cache (sub-50ms over an established control
/// master). If the SSH session is unreachable, this propagates an error
/// rather than guessing — calling `new_session` for an unreachable host
/// should fail loudly.
fn remote_home(ssh: &Arc<SshClient>, host: &str) -> Result<String, IpcError> {
    let out = ssh.run(
        host,
        &["printenv", "HOME"],
        std::time::Duration::from_secs(5),
    )?;
    if !out.status.success() {
        return Err(IpcError::new(
            "E_SSH",
            format!(
                "couldn't read $HOME on {host}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }
    let home = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if home.is_empty() {
        return Err(IpcError::new(
            "E_SSH",
            format!("remote $HOME on {host} is empty"),
        ));
    }
    Ok(home)
}

/// Build the absolute path on the remote host where a project (and optional
/// worktree) should live. Mirrors the local convention `proj-clean` enforces:
/// `~/projects/github.com/<owner>/<repo>` for the project root and
/// `~/projects/github.com/<owner>/<repo>/.claude/worktrees/<wt>` for non-main
/// worktrees. Returns just the project root if `wt_name` is None or "main".
fn remote_project_path(
    home: &str,
    owner: &str,
    repo: &str,
    wt_name: Option<&str>,
) -> (String, String) {
    let project_root = format!("{home}/projects/github.com/{owner}/{repo}");
    let cwd = match wt_name {
        Some(name) if name != "main" => {
            format!("{project_root}/.claude/worktrees/{name}")
        }
        _ => project_root.clone(),
    };
    (project_root, cwd)
}

/// Conservative single-quote shell escape (duplicated from `tmux::shell_quote`
/// to keep this module self-contained for the iter-1 MVP; consolidating into
/// a shared util is a planned iter-2 cleanup).
fn shq(s: &str) -> String {
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

/// Ensure the remote host has the project cloned at `<project_root>` and,
/// optionally, has a worktree at `<project_root>/.claude/worktrees/<wt>`
/// checked out to `<branch>`. Idempotent: if the directory + .git is already
/// there, the clone step is skipped; same for worktree-add. Auto-clones via
/// SSH (`git@github.com:<owner>/<repo>.git`), assuming the remote has SSH
/// github access (the common case for dev machines).
///
/// Returns Ok(()) on success. Failure surfaces stderr in the IpcError so the
/// user can diagnose (missing SSH key, private-repo auth, etc.).
fn ensure_remote_project(
    ssh: &Arc<SshClient>,
    host: &str,
    owner: &str,
    repo: &str,
    project_root: &str,
    worktree: Option<(&str, Option<&str>)>, // (name, branch)
) -> Result<(), IpcError> {
    let clone_url = format!("git@github.com:{owner}/{repo}.git");
    // Build a single bash script that:
    //   1. clones the repo if .git is missing
    //   2. creates the worktree if requested and not yet present
    // Both steps are guarded so a re-run on an already-set-up host is a no-op.
    let mut script = String::new();
    script.push_str(&format!(
        "if [ ! -d {root}/.git ]; then mkdir -p $(dirname {root}) && git clone {url} {root}; fi",
        root = shq(project_root),
        url = shq(&clone_url),
    ));
    if let Some((wt_name, branch)) = worktree {
        if wt_name != "main" {
            let wt_rel = format!(".claude/worktrees/{wt_name}");
            let wt_abs = format!("{project_root}/{wt_rel}");
            let branch = branch.unwrap_or(wt_name);
            script.push_str(&format!(
                " && if [ ! -d {abs} ]; then cd {root} && git worktree add {rel} {br}; fi",
                abs = shq(&wt_abs),
                root = shq(project_root),
                rel = shq(&wt_rel),
                br = shq(branch),
            ));
        }
    }
    // Wrap in bash -lc so $PATH (git on Homebrew/Linuxbrew) is sourced. Use
    // the same single-quote-the-whole-script trick as RemoteTmux::remote_bash
    // to avoid the ssh argv-joining bug.
    let out = ssh.run(
        host,
        &["bash", "-lc", &script],
        std::time::Duration::from_secs(120),
    )?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        return Err(IpcError::new(
            "E_GIT_SETUP",
            format!(
                "couldn't ensure {owner}/{repo} on {host}: {}",
                if stderr.trim().is_empty() {
                    stdout.trim().to_string()
                } else {
                    stderr.trim().to_string()
                }
            ),
        ));
    }
    Ok(())
}

#[tauri::command]
pub fn new_session(
    args: NewSessionArgs,
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    // Resolve the cwd that tmux will spawn the pane in. For LOCAL the path
    // comes straight from the DB (it was discovered by scanning ~/projects).
    // For REMOTE we can't use the local path — it doesn't exist on the other
    // machine — so we translate to `~/projects/github.com/<owner>/<repo>`
    // (matching proj-clean's convention) and auto-clone if missing.
    let path: PathBuf = if args.host_alias == "local" {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        if let Some(wid) = args.worktree_id {
            let mut stmt = s
                .conn_ref()
                .prepare("SELECT path FROM worktrees WHERE id=?1")?;
            let row: String = stmt.query_row(rusqlite::params![wid], |r| r.get(0))?;
            PathBuf::from(row)
        } else {
            let mut stmt = s
                .conn_ref()
                .prepare("SELECT base_path FROM projects WHERE id=?1")?;
            let row: String = stmt.query_row(rusqlite::params![args.project_id], |r| r.get(0))?;
            PathBuf::from(row)
        }
    } else {
        // Remote path: derive from owner/repo, then ensure-on-remote.
        let (owner, repo, wt_info) = {
            let s = store
                .lock()
                .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
            let (owner, repo) = fetch_owner_repo(&s, args.project_id)?;
            let wt = if let Some(wid) = args.worktree_id {
                Some(fetch_worktree(&s, wid)?)
            } else {
                None
            };
            (owner, repo, wt)
        };
        let home = remote_home(&ssh, &args.host_alias)?;
        let wt_name_str = wt_info.as_ref().map(|(name, _)| name.as_str());
        let (project_root, cwd) = remote_project_path(&home, &owner, &repo, wt_name_str);
        let worktree_for_clone = wt_info
            .as_ref()
            .map(|(name, branch)| (name.as_str(), branch.as_deref()));
        ensure_remote_project(
            &ssh,
            &args.host_alias,
            &owner,
            &repo,
            &project_root,
            worktree_for_clone,
        )?;
        PathBuf::from(cwd)
    };
    let tmux = exec_for(&args.host_alias, &ssh);
    tmux.new_session(&args.name, &path)?;

    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let rows = reconcile_sessions(&s, &ssh)?;
    rows.into_iter()
        .find(|r| r.tmux_name == args.name && r.host_alias == args.host_alias)
        .ok_or_else(|| {
            IpcError::new(
                "E_NOTFOUND",
                format!(
                    "session {} on {} did not appear in list",
                    args.name, args.host_alias
                ),
            )
        })
}

#[derive(Deserialize)]
pub struct KillSessionArgs {
    pub host_alias: String,
    pub name: String,
}

#[tauri::command]
pub fn kill_session(
    args: KillSessionArgs,
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    let tmux = exec_for(&args.host_alias, &ssh);
    tmux.kill_session(&args.name)?;
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    reconcile_sessions(&s, &ssh)?;
    Ok(())
}

#[derive(Deserialize)]
pub struct RenameSessionArgs {
    pub host_alias: String,
    pub old_name: String,
    pub new_name: String,
}

#[tauri::command]
pub fn rename_session(
    args: RenameSessionArgs,
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    let tmux = exec_for(&args.host_alias, &ssh);
    tmux.rename_session(&args.old_name, &args.new_name)?;
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let rows = reconcile_sessions(&s, &ssh)?;
    rows.into_iter()
        .find(|r| r.tmux_name == args.new_name.trim() && r.host_alias == args.host_alias)
        .ok_or_else(|| {
            IpcError::new(
                "E_NOTFOUND",
                format!(
                    "renamed session {} on {} did not appear in list",
                    args.new_name, args.host_alias
                ),
            )
        })
}

#[derive(Deserialize)]
pub struct RestartSessionArgs {
    pub host_alias: String,
    pub name: String,
}

#[tauri::command]
pub fn restart_session(
    args: RestartSessionArgs,
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    let tmux = exec_for(&args.host_alias, &ssh);
    tmux.restart_session(&args.name)?;
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let rows = reconcile_sessions(&s, &ssh)?;
    rows.into_iter()
        .find(|r| r.tmux_name == args.name && r.host_alias == args.host_alias)
        .ok_or_else(|| {
            IpcError::new(
                "E_NOTFOUND",
                format!(
                    "restarted session {} on {} did not appear in list",
                    args.name, args.host_alias
                ),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_owner_repo_from_macos_path() {
        let r = extract_owner_repo("/Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/x");
        assert_eq!(r, Some(("martin-janci".into(), "claude-fleet".into())));
    }

    #[test]
    fn extracts_owner_repo_from_linux_path() {
        let r = extract_owner_repo("/home/mjanci/projects/github.com/martin-janci/sales-twins-app");
        assert_eq!(r, Some(("martin-janci".into(), "sales-twins-app".into())));
    }

    #[test]
    fn extracts_owner_repo_when_followed_by_subdir() {
        let r = extract_owner_repo("/anywhere/projects/github.com/papayapos/pos-frontend/src/lib");
        assert_eq!(r, Some(("papayapos".into(), "pos-frontend".into())));
    }

    #[test]
    fn returns_none_when_not_github_com_layout() {
        assert_eq!(extract_owner_repo("/tmp/random/repo"), None);
        assert_eq!(extract_owner_repo("/home/x/projects/gitlab.com/a/b"), None);
    }

    #[test]
    fn remote_project_path_returns_project_root_for_main_or_no_worktree() {
        let (root, cwd) = remote_project_path("/home/mjanci", "martin-janci", "claude-fleet", None);
        assert_eq!(root, "/home/mjanci/projects/github.com/martin-janci/claude-fleet");
        assert_eq!(cwd, root);

        let (root, cwd) =
            remote_project_path("/home/mjanci", "papayapos", "pos-frontend", Some("main"));
        assert_eq!(cwd, root);
    }

    #[test]
    fn remote_project_path_uses_worktree_subdir_for_non_main() {
        let (root, cwd) = remote_project_path(
            "/home/mjanci",
            "martin-janci",
            "sales-twins-app",
            Some("feature-x"),
        );
        assert_eq!(
            root,
            "/home/mjanci/projects/github.com/martin-janci/sales-twins-app"
        );
        assert_eq!(
            cwd,
            "/home/mjanci/projects/github.com/martin-janci/sales-twins-app/.claude/worktrees/feature-x"
        );
    }

    #[test]
    fn shq_wraps_basic_strings() {
        assert_eq!(shq("foo"), "'foo'");
        assert_eq!(shq("/home/mjanci"), "'/home/mjanci'");
    }

    #[test]
    fn shq_escapes_embedded_single_quotes() {
        assert_eq!(shq("don't"), "'don'\\''t'");
    }

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
}
