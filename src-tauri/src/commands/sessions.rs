use crate::cancel::CancellationRegistry;
use crate::ipc_error::IpcError;
use crate::ssh::SshClient;
use crate::store::{HostRow, SessionRow, Store};
use crate::tmux::{LocalTmux, RemoteTmux, TmuxExec};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::State;
use tokio_util::sync::CancellationToken;

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

async fn reconcile_sessions(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<Vec<SessionRow>, IpcError> {
    // 1. Snapshot under lock (brief). Ensure local host exists first.
    let hosts = {
        let s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.upsert_host("local")?;
        s.list_hosts()?
    };

    // 2. Fan out probes (off-lock) via JoinSet for parallel execution.
    //    Hidden hosts are skipped here — their last-known sessions are
    //    fetched in the write phase without probing.
    //    Each task receives owned data so it satisfies 'static + Send.
    //
    //    TODO(iter4a-M4): `JoinSet::drop` aborts the futures but does NOT
    //    kill the spawned ssh/tmux child processes; on early return/panic
    //    those orphan. Will be addressed by the CancellationToken plumbing
    //    in M4 (Tasks 16–18).
    let mut set = tokio::task::JoinSet::new();
    for host in hosts.into_iter().filter(|h| !h.hidden) {
        let ssh_arc = Arc::clone(ssh);
        set.spawn(async move {
            let tmux = exec_for(&host.alias, &ssh_arc);
            let result = tmux.list_sessions().await;
            (host, result)
        });
    }

    // Collect per-host probe results. Join errors (task panics) are logged
    // and skipped — they don't abort the rest of reconcile.
    let mut probed: Vec<(HostRow, Result<Vec<crate::tmux::TmuxSession>, IpcError>)> = Vec::new();
    while let Some(join) = set.join_next().await {
        match join {
            Ok((host, res)) => probed.push((host, res)),
            Err(e) => eprintln!("reconcile join error: {e}"),
        }
    }

    // 3. Apply all writes in a single short lock window.
    //
    //    TODO(iter4a-M3): wrap the body of this block in
    //    `s.with_transaction(|tx| { … _in_tx variants … })` so the whole
    //    reconcile commits with one fsync instead of one per upsert. The
    //    `_in_tx` Store variants are added when M3 wires the EventBus into
    //    Store mutations (Task 8/9), so the transaction wrap lands cleanly
    //    at the same time.
    let mut all_rows: Vec<SessionRow> = Vec::new();
    {
        let s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;

        for (host, res) in &probed {
            match res {
                Ok(live) => {
                    let _ = s.update_host_probe(
                        &host.alias,
                        true,
                        host.claude_version.as_deref(),
                        host.tmux_version.as_deref(),
                        now_unix(),
                    );
                    let mut keep: Vec<String> = Vec::with_capacity(live.len());
                    for sess in live {
                        keep.push(sess.name.clone());
                        let project_id = find_project_id_for_path(&s, &host.alias, &sess.path);
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
                Err(_e) => {
                    // Mark host unreachable; surface last-known sessions so the
                    // UI can render them dimmed/red. We KEEP them (no delete).
                    let _ = s.update_host_probe(
                        &host.alias,
                        false,
                        host.claude_version.as_deref(),
                        host.tmux_version.as_deref(),
                        now_unix(),
                    );
                    all_rows.extend(s.list_sessions_for_host(&host.alias)?);
                }
            }
        }

        // Hidden hosts: don't probe but DO surface their last-known sessions.
        let all_hosts = s.list_hosts()?;
        for host in all_hosts.iter().filter(|h| h.hidden) {
            all_rows.extend(s.list_sessions_for_host(&host.alias)?);
        }
    }
    Ok(all_rows)
}

async fn reconcile_one_host(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    alias: &str,
) -> Result<(), IpcError> {
    // 1. Snapshot the host under lock (brief).
    let host = {
        let s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.list_hosts()?
            .into_iter()
            .find(|h| h.alias == alias)
            .ok_or_else(|| IpcError::new("E_NOTFOUND", format!("host {alias} not found")))?
    };

    // 2. Probe off-lock.
    let tmux = exec_for(&host.alias, ssh);
    let result = tmux.list_sessions().await;

    // 3. Apply writes under one brief lock.
    let s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    match result {
        Ok(live) => {
            let _ = s.update_host_probe(
                &host.alias,
                true,
                host.claude_version.as_deref(),
                host.tmux_version.as_deref(),
                now_unix(),
            );
            let mut keep: Vec<String> = Vec::with_capacity(live.len());
            for sess in &live {
                keep.push(sess.name.clone());
                let project_id = find_project_id_for_path(&s, &host.alias, &sess.path);
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
        }
        Err(_e) => {
            let _ = s.update_host_probe(
                &host.alias,
                false,
                host.claude_version.as_deref(),
                host.tmux_version.as_deref(),
                now_unix(),
            );
        }
    }
    Ok(())
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
pub async fn list_sessions(
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<Vec<SessionRow>, IpcError> {
    reconcile_sessions(&store, &ssh).await
}

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

#[derive(Deserialize)]
pub struct NewSessionArgs {
    pub host_alias: String,
    pub project_id: i64,
    pub worktree_id: Option<i64>,
    pub name: String,
    pub call_id: Option<u64>,
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
async fn remote_home(ssh: &Arc<SshClient>, host: &str) -> Result<String, IpcError> {
    let out = ssh.run(
        host,
        &["printenv", "HOME"],
        std::time::Duration::from_secs(5),
    ).await?;
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
/// The `token` parameter allows the caller to cancel the (potentially long-
/// running) `git clone` step. On cancellation the child is killed and
/// `Err(E_CANCELLED)` is returned. Partial clone dirs are NOT cleaned up
/// on cancel — that's a follow-up task.
///
/// Returns Ok(()) on success. Failure surfaces stderr in the IpcError so the
/// user can diagnose (missing SSH key, private-repo auth, etc.).
async fn ensure_remote_project(
    ssh: &Arc<SshClient>,
    host: &str,
    owner: &str,
    repo: &str,
    project_root: &str,
    worktree: Option<(&str, Option<&str>)>, // (name, branch)
    token: CancellationToken,
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
    let out = ssh.run_cancellable(
        host,
        &["bash", "-lc", &script],
        std::time::Duration::from_secs(120),
        token,
    ).await?;
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
pub async fn new_session(
    args: NewSessionArgs,
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
) -> Result<SessionRow, IpcError> {
    // Mint / bind a cancellation token for the duration of this command.
    // If a call_id was provided by the frontend, bind under that id so the
    // frontend can cancel via cancel_command(call_id). Otherwise use an
    // anonymous id (internal callers, tests, local sessions).
    let (cancel_id, token) = match args.call_id {
        Some(id) => {
            let token = CancellationToken::new();
            reg.bind(id, token.clone());
            (Some(id), token)
        }
        None => {
            let (id, token) = reg.register_anonymous();
            (Some(id), token)
        }
    };

    let result = new_session_inner(args, &store, &ssh, token).await;

    if let Some(id) = cancel_id {
        reg.unregister(id);
    }

    result
}

async fn new_session_inner(
    args: NewSessionArgs,
    store: &State<'_, Mutex<Store>>,
    ssh: &State<'_, Arc<SshClient>>,
    token: CancellationToken,
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
        let home = remote_home(ssh, &args.host_alias).await?;
        let wt_name_str = wt_info.as_ref().map(|(name, _)| name.as_str());
        let (project_root, cwd) = remote_project_path(&home, &owner, &repo, wt_name_str);
        let worktree_for_clone = wt_info
            .as_ref()
            .map(|(name, branch)| (name.as_str(), branch.as_deref()));
        ensure_remote_project(
            ssh,
            &args.host_alias,
            &owner,
            &repo,
            &project_root,
            worktree_for_clone,
            token,
        ).await?;
        PathBuf::from(cwd)
    };
    let tmux = exec_for(&args.host_alias, ssh);
    tmux.new_session(&args.name, &path).await?;

    reconcile_one_host(store, ssh, &args.host_alias).await?;
    let s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.list_sessions_for_host(&args.host_alias)?
        .into_iter()
        .find(|r| r.tmux_name == args.name)
        .ok_or_else(|| {
            IpcError::new(
                "E_INTERNAL",
                format!(
                    "session {} on {} vanished after creation",
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
pub async fn kill_session(
    args: KillSessionArgs,
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<i64, IpcError> {
    // Look up id BEFORE killing so we can return it after.
    let id = {
        let s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.list_sessions_for_host(&args.host_alias)?
            .into_iter()
            .find(|r| r.tmux_name == args.name)
            .map(|r| r.id)
            .ok_or_else(|| IpcError::new("E_NOTFOUND", format!("session {} not found", args.name)))?
    };
    let tmux = exec_for(&args.host_alias, &ssh);
    tmux.kill_session(&args.name).await?;
    reconcile_one_host(&store, &ssh, &args.host_alias).await?;
    Ok(id)
}

#[derive(Deserialize)]
pub struct RenameSessionArgs {
    pub host_alias: String,
    pub old_name: String,
    pub new_name: String,
}

#[tauri::command]
pub async fn rename_session(
    args: RenameSessionArgs,
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    let tmux = exec_for(&args.host_alias, &ssh);
    tmux.rename_session(&args.old_name, &args.new_name).await?;
    reconcile_one_host(&store, &ssh, &args.host_alias).await?;
    let s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.list_sessions_for_host(&args.host_alias)?
        .into_iter()
        .find(|r| r.tmux_name == args.new_name.trim())
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
pub async fn restart_session(
    args: RestartSessionArgs,
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<SessionRow, IpcError> {
    let tmux = exec_for(&args.host_alias, &ssh);
    tmux.restart_session(&args.name).await?;
    reconcile_one_host(&store, &ssh, &args.host_alias).await?;
    let s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.list_sessions_for_host(&args.host_alias)?
        .into_iter()
        .find(|r| r.tmux_name == args.name)
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

async fn send_prompt_inner(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    tmux_name: &str,
    prompt: &str,
) -> Result<(), IpcError> {
    let cmds = build_send_commands(tmux_name, prompt);
    if host_alias == "local" {
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
                host_alias,
                &["bash", "-lc", &quoted],
                std::time::Duration::from_secs(10),
            ).await?;
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

#[tauri::command]
pub async fn send_prompt(
    args: SendPromptArgs,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<(), IpcError> {
    send_prompt_inner(&ssh, &args.host_alias, &args.tmux_name, &args.prompt).await
}

/// Resolve the cwd a review should open in. Order: source's worktree path
/// (by worktree_id) → source's project base_path → error.
fn resolve_review_cwd(s: &Store, source: &crate::store::SessionRow) -> Result<String, IpcError> {
    if let Some(wt_id) = source.worktree_id {
        if let Some(path) = s.worktree_path(wt_id)? {
            return Ok(path);
        }
    }
    if let Some(pid) = source.project_id {
        if let Some(base) = s.project_base_path(pid)? {
            return Ok(base);
        }
    }
    Err(IpcError::new(
        "E_INVALID",
        "cannot determine a worktree path to review for this session",
    ))
}

/// Poll the tmux pane until `cl`'s REPL prompt appears, up to ~6s. Returns
/// when ready, or after the timeout (best-effort — a missed prompt just means
/// the user presses Enter / re-sends manually; spawn_review already soft-fails
/// the seed). `cl`'s prompt box draws a border (│) and a `>` prompt; we look
/// for either as a readiness signal.
async fn wait_for_repl_ready(tmux: &dyn TmuxExec, name: &str) {
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        if let Ok(pane) = tmux.capture_pane(name).await {
            if pane.contains('>') || pane.contains('│') {
                return;
            }
        }
    }
}

#[derive(Deserialize)]
pub struct SpawnReviewArgs {
    pub source_session_id: i64,
    pub prompt: String,
    // Reserved for future cancellation wiring. The frontend's
    // invokeCmdAbortable injects a call_id; v1 spawn_review doesn't register a
    // CancellationToken under it (the spawn is short — tmux create + reconcile
    // + ~1.5s seed delay), so an abort is currently a no-op on the backend.
    #[allow(dead_code)]
    pub call_id: Option<u64>,
}

#[tauri::command]
pub async fn spawn_review(
    args: SpawnReviewArgs,
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<crate::store::SessionRow, IpcError> {
    // 1. Snapshot source + resolve cwd under a brief lock.
    let (source, cwd) = {
        let s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        let source = s.get_session_by_id(args.source_session_id)?
            .ok_or_else(|| IpcError::new("E_NOTFOUND", "source session not found"))?;
        let cwd = resolve_review_cwd(&s, &source)?;
        (source, cwd)
    };

    // 2. Spawn the review tmux session (off-lock).
    //    new_session runs pane_command() internally — same as any other session.
    let short = format!("{:x}", now_unix() & 0xfffff);
    let review_name = format!("{}--review-{}", source.tmux_name, short);
    let tmux = exec_for(&source.host_alias, &ssh);
    tmux.new_session(&review_name, std::path::Path::new(&cwd)).await?;

    // 3. Register via per-host reconcile.
    reconcile_one_host(&store, &ssh, &source.host_alias).await?;

    // 4. Tag as review + capture id.
    let review_id = {
        let s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        let row = s.list_sessions_for_host(&source.host_alias)?
            .into_iter()
            .find(|r| r.tmux_name == review_name)
            .ok_or_else(|| IpcError::new("E_INTERNAL", "review session vanished after spawn"))?;
        s.set_session_kind(row.id, "review", Some(source.id))?;
        row.id
    };

    // 5. Seed the prompt. Wait until cl's TUI is ready before send-keys lands.
    wait_for_repl_ready(tmux.as_ref(), &review_name).await;
    // Soft-fail: the review session is already spawned, registered, and tagged.
    // If seeding the prompt fails (e.g. cl wasn't ready yet), DON'T discard the
    // session — return it anyway so the user can type the review prompt manually
    // in the terminal. Log the failure for diagnostics.
    if let Err(e) = send_prompt_inner(&ssh, &source.host_alias, &review_name, &args.prompt).await {
        eprintln!("spawn_review: seeding prompt to {review_name} failed (session is live, seed manually): {e:?}");
    }

    // 6. Return the tagged review row.
    let s = store.lock().map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.get_session_by_id(review_id)?
        .ok_or_else(|| IpcError::new("E_INTERNAL", "review row missing after tag"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

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

    #[tokio::test]
    async fn parallel_reconcile_does_not_serialise_on_slow_host() {
        use std::time::Duration;
        use async_trait::async_trait;
        use crate::tmux::TmuxSession;

        struct SleepyTmux { sleep_ms: u64 }

        #[async_trait]
        impl TmuxExec for SleepyTmux {
            async fn list_sessions(&self) -> Result<Vec<TmuxSession>, IpcError> {
                tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
                Ok(Vec::new())
            }
            async fn new_session(&self, _name: &str, _cwd: &std::path::Path) -> Result<(), IpcError> {
                Ok(())
            }
            async fn kill_session(&self, _name: &str) -> Result<(), IpcError> { Ok(()) }
            async fn rename_session(&self, _old: &str, _new: &str) -> Result<(), IpcError> { Ok(()) }
            async fn restart_session(&self, _name: &str) -> Result<(), IpcError> { Ok(()) }
            async fn capture_pane(&self, _name: &str) -> Result<String, IpcError> { Ok(String::new()) }
        }

        // Spawn 3 tasks with sleeps 50ms, 500ms, 50ms.
        // Sequential sum ≈ 600ms; parallel max ≈ 500ms.
        let mut set = tokio::task::JoinSet::new();
        let start = std::time::Instant::now();
        for ms in [50u64, 500, 50] {
            set.spawn(async move {
                SleepyTmux { sleep_ms: ms }.list_sessions().await
            });
        }
        while set.join_next().await.is_some() {}
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(700),
            "parallel reconcile took {elapsed:?}, expected ≈max not sum",
        );
    }

    #[tokio::test]
    async fn reconcile_one_host_does_not_touch_other_hosts() {
        // Exercises the Store-level invariant: a write burst targeting host
        // 'alpha' must leave host 'beta's session rows untouched.
        let store = Mutex::new(Store::open_in_memory().expect("store"));
        {
            let s = store.lock().unwrap();
            s.upsert_host("alpha").unwrap();
            s.upsert_host("beta").unwrap();
            s.upsert_session("alpha-s", "alpha", None, None, 1, 1, "running", None).unwrap();
            s.upsert_session("beta-s", "beta", None, None, 1, 1, "running", None).unwrap();
        }
        // Simulate "alpha was probed and has zero sessions" — directly call the
        // delete helper that reconcile_one_host uses internally.
        {
            let s = store.lock().unwrap();
            s.delete_sessions_not_in("alpha", &[]).unwrap();
        }
        let s = store.lock().unwrap();
        let alpha = s.list_sessions_for_host("alpha").unwrap();
        let beta = s.list_sessions_for_host("beta").unwrap();
        assert!(alpha.is_empty(), "alpha cleared");
        assert_eq!(beta.len(), 1, "beta untouched");
        assert_eq!(beta[0].tmux_name, "beta-s");
    }

    #[test]
    fn upsert_session_captures_new_account_for_fresh_row() {
        use crate::store::AccountRow;
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

    #[tokio::test]
    async fn wait_for_repl_ready_returns_once_prompt_appears() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc as StdArc;

        struct FakeTmux { calls: StdArc<AtomicU32> }
        #[async_trait::async_trait]
        impl TmuxExec for FakeTmux {
            async fn list_sessions(&self) -> Result<Vec<crate::tmux::TmuxSession>, IpcError> { Ok(vec![]) }
            async fn new_session(&self, _: &str, _: &std::path::Path) -> Result<(), IpcError> { Ok(()) }
            async fn kill_session(&self, _: &str) -> Result<(), IpcError> { Ok(()) }
            async fn rename_session(&self, _: &str, _: &str) -> Result<(), IpcError> { Ok(()) }
            async fn restart_session(&self, _: &str) -> Result<(), IpcError> { Ok(()) }
            async fn capture_pane(&self, _: &str) -> Result<String, IpcError> {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                // Not ready for the first 2 polls, then the prompt appears.
                if n < 2 { Ok("starting…".into()) } else { Ok("│ > ".into()) }
            }
        }

        let calls = StdArc::new(AtomicU32::new(0));
        let tmux = FakeTmux { calls: calls.clone() };
        let start = std::time::Instant::now();
        wait_for_repl_ready(&tmux, "x").await;
        // Returned after ~3 polls (~600ms), well under the 6s cap.
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        assert!(calls.load(Ordering::SeqCst) >= 3);
    }

    #[test]
    fn resolve_review_cwd_prefers_worktree_then_project_then_errors() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("alpha").unwrap();
        // Project with a base_path, and a worktree under it.
        let pid = store.upsert_project("o", "r", "/base/r").unwrap();
        let wid = store.upsert_worktree(pid, "main", "/base/r/main", None).unwrap();
        // Session with worktree → worktree path wins.
        let s1 = store.upsert_session("s1", "alpha", Some(pid), Some(wid), 1, 1, "running", None).unwrap();
        let row1 = store.get_session_by_id(s1).unwrap().unwrap();
        assert_eq!(resolve_review_cwd(&store, &row1).unwrap(), "/base/r/main");
        // Session with project but no worktree → project base.
        let s2 = store.upsert_session("s2", "alpha", Some(pid), None, 1, 1, "running", None).unwrap();
        let row2 = store.get_session_by_id(s2).unwrap().unwrap();
        assert_eq!(resolve_review_cwd(&store, &row2).unwrap(), "/base/r");
        // Session with neither → error.
        let s3 = store.upsert_session("s3", "alpha", None, None, 1, 1, "running", None).unwrap();
        let row3 = store.get_session_by_id(s3).unwrap().unwrap();
        assert!(resolve_review_cwd(&store, &row3).is_err());
    }
}
