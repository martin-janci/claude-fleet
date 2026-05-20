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
        if host.hidden {
            continue;
        }
        let tmux = exec_for(&host.alias, ssh);
        let live = match tmux.list_sessions() {
            Ok(v) => v,
            Err(_e) => {
                // Mark host unreachable but don't fail the whole reconcile;
                // other hosts can still list their sessions.
                let _ = s.update_host_probe(
                    &host.alias,
                    false,
                    host.claude_version.as_deref(),
                    host.tmux_version.as_deref(),
                    now_unix(),
                );
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
            s.upsert_session(
                &sess.name,
                &host.alias,
                project_id,
                None,
                sess.created,
                sess.last_activity,
                "running",
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

#[tauri::command]
pub fn new_session(
    args: NewSessionArgs,
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    let path: PathBuf = {
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
}
