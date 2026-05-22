use crate::cancel::{CancelGuard, CancellationRegistry};
use crate::ipc_error::IpcError;
use crate::shell::quote as shq;
use crate::ssh::SshClient;
use crate::store::{HostReconcile, HostRow, ProjectRow, ReconcileSession, SessionRow, Store};
use crate::tmux::{LocalTmux, RemoteTmux, TmuxExec};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
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

/// Apply one host's probe result to the store. Extracted from the reconcile
/// loop so a per-host write failure can be isolated (logged) without `?`
/// aborting the whole multi-host reconcile. The write itself goes through the
/// transactional `Store::apply_host_reconcile` (one fsync, emit-after-commit).
fn reconcile_write_one_host(
    s: &mut Store,
    host: &HostRow,
    res: &Result<Vec<crate::tmux::TmuxSession>, IpcError>,
    projects: &[ProjectRow],
) -> Result<(), IpcError> {
    match res {
        Ok(live) => {
            let mut keep: Vec<String> = Vec::with_capacity(live.len());
            let mut sessions: Vec<ReconcileSession> = Vec::with_capacity(live.len());
            for sess in live {
                keep.push(sess.name.clone());
                let project_id = find_project_id_for_path(projects, &host.alias, &sess.path);
                // Preservation invariant: if the session already has an
                // account_uuid in the DB, keep it; only capture the host's
                // current account for newly-discovered sessions.
                let account_uuid = s
                    .get_session_account(&host.alias, &sess.name)?
                    .or_else(|| host.account_uuid.clone());
                let worktree_key = worktree_key_for_path(&sess.path.to_string_lossy());
                sessions.push(ReconcileSession {
                    tmux_name: &sess.name,
                    project_id,
                    created_at: sess.created,
                    last_activity_at: sess.last_activity,
                    account_uuid,
                    worktree_key,
                });
            }
            s.apply_host_reconcile(HostReconcile {
                alias: &host.alias,
                reachable: true,
                claude_version: host.claude_version.as_deref(),
                tmux_version: host.tmux_version.as_deref(),
                last_pinged_at: now_unix(),
                sessions: &sessions,
                keep: &keep,
            })?;
        }
        Err(_e) => {
            // Mark host unreachable; surface last-known sessions so the UI
            // can render them dimmed/red. We KEEP them (no delete).
            s.apply_host_reconcile(HostReconcile {
                alias: &host.alias,
                reachable: false,
                claude_version: host.claude_version.as_deref(),
                tmux_version: host.tmux_version.as_deref(),
                last_pinged_at: now_unix(),
                sessions: &[],
                keep: &[],
            })?;
        }
    }
    Ok(())
}

async fn reconcile_sessions(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<Vec<SessionRow>, IpcError> {
    // 1. Snapshot under lock (brief). Ensure local host exists first.
    let hosts = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.upsert_host("local")?;
        s.list_hosts()?
    };

    // 2. Fan out probes (off-lock) via JoinSet for parallel execution.
    //    Hidden hosts are skipped here — their last-known sessions are still
    //    surfaced by the final `list_all_sessions` read, without probing.
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

    // 3. Apply all writes in a single short lock window. Each host's
    //    write-burst goes through `Store::apply_host_reconcile`, which wraps
    //    update_host_probe + upserts + touches + delete-not-in in ONE
    //    transaction (one fsync) and emits events only AFTER it commits — so a
    //    mid-burst error rolls everything back and emits nothing for that host.
    {
        let mut s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;

        // The project list is identical for every host — fetch it once here
        // rather than re-querying inside `find_project_id_for_path` per session.
        let projects = s.list_projects()?;

        for (host, res) in &probed {
            // Per-host isolation: one host's DB write failure (e.g. an FK
            // violation on a stale account_uuid) must NOT abort reconcile for
            // every other host. apply_host_reconcile is transactional, so a
            // failed host rolls back cleanly; we log it and carry on.
            if let Err(e) = reconcile_write_one_host(&mut s, host, res, &projects) {
                eprintln!(
                    "reconcile: write for host {} failed: {}",
                    host.alias, e.message
                );
            }
        }
    }

    // 4. Read the final session set in one query (covers active + hidden
    //    hosts) instead of N per-host reads.
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.list_all_sessions().map_err(IpcError::from)
}

async fn reconcile_one_host(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    alias: &str,
) -> Result<(), IpcError> {
    // 1. Snapshot the host under lock (brief).
    let host = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.list_hosts()?
            .into_iter()
            .find(|h| h.alias == alias)
            .ok_or_else(|| IpcError::new("E_NOTFOUND", format!("host {alias} not found")))?
    };

    // 2. Probe off-lock.
    let tmux = exec_for(&host.alias, ssh);
    let result = tmux.list_sessions().await;

    // 3. Apply writes under one brief lock, via the SAME per-host write path
    //    as the multi-host reconcile (single transaction + emit-after-commit).
    let mut s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let projects = s.list_projects()?;
    reconcile_write_one_host(&mut s, &host, &result, &projects)
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

/// Match a session's cwd to a known project id. `projects` is passed in by the
/// caller (fetched once per reconcile) rather than queried per session.
fn find_project_id_for_path(
    projects: &[ProjectRow],
    host_alias: &str,
    path: &std::path::Path,
) -> Option<i64> {
    let path_str = path.to_string_lossy();
    if host_alias == "local" {
        // Local paths: prefix match (handles worktrees nested under repos).
        return projects
            .iter()
            .filter(|p| path_str.starts_with(&p.base_path))
            .max_by_key(|p| p.base_path.len())
            .map(|p| p.id);
    }
    // Remote paths: match by owner+repo extracted from the conventional
    // `.../projects/github.com/<owner>/<repo>/...` layout. Falls through
    // to `None` (orphan) if the path doesn't follow the convention.
    let (owner, repo) = extract_owner_repo(&path_str)?;
    projects
        .iter()
        .find(|p| p.owner == owner && p.repo == repo)
        .map(|p| p.id)
}

pub async fn list_sessions(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<Vec<SessionRow>, IpcError> {
    reconcile_sessions(store, ssh).await
}

#[derive(Deserialize)]
pub struct RelatedSessionsArgs {
    pub session_id: i64,
}

pub fn related_sessions(
    args: RelatedSessionsArgs,
    store: &Mutex<Store>,
) -> Result<Vec<SessionRow>, IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.list_related_sessions(args.session_id)
        .map_err(IpcError::from)
}

#[derive(Deserialize)]
pub struct NewSessionArgs {
    pub host_alias: String,
    pub project_id: i64,
    pub worktree_id: Option<i64>,
    pub name: String,
    pub call_id: Option<u64>,
    pub new_worktree: Option<String>,
    /// Session kind: `"work"` (default) runs Claude Code in the pane;
    /// `"shell"` runs a plain interactive login shell.
    pub kind: Option<String>,
    /// Optional command run once on start for a `"shell"` session, before
    /// the pane drops to an interactive shell. Ignored for `"work"`.
    pub start_command: Option<String>,
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
    // Validate every component that gets interpolated into a remote path or
    // git command. Shell-quoting (below) stops command injection but NOT
    // `..` path traversal — a repo named `../../.ssh` would still be a valid
    // quoted argument that escapes the projects directory.
    crate::validate::path_component("owner", owner)?;
    crate::validate::path_component("repo", repo)?;
    if let Some((wt_name, branch)) = worktree {
        crate::validate::path_component("worktree name", wt_name)?;
        if let Some(b) = branch {
            crate::validate::git_ref(b)?;
        }
    }
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
    // Single-quote the WHOLE script so it crosses the ssh argv-join as one
    // word. ssh concatenates the trailing args with spaces and the remote
    // LOGIN shell (often zsh) re-tokenizes them — without quoting, the
    // `if ...; then ...; fi` splits at `;` and orphans `then` ("zsh: parse
    // error near then"). `shq` escapes the inner single-quotes from the path
    // interpolation above.
    let quoted = shq(&script);
    let out = ssh
        .run_cancellable(
            host,
            &["bash", "-lc", &quoted],
            std::time::Duration::from_secs(120),
            token,
        )
        .await?;
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

/// Build a bash script (run via `bash -lc`) that creates a new worktree for a
/// NEW branch `name` off the repo's default branch, under `.worktrees/` or
/// `.claude/worktrees/` (auto-detected, fallback `.worktrees/`). Idempotent:
/// if the worktree dir already exists it's reused. Git's chatter goes to
/// stderr; the ONLY stdout is the absolute path of the worktree (last line),
/// which the caller uses as the tmux cwd.
fn worktree_add_script(root: &str, name: &str) -> String {
    format!(
        "set -e\n\
         cd {root}\n\
         name={name}\n\
         if [ -d .worktrees ]; then base=.worktrees\n\
         elif [ -d .claude/worktrees ]; then base=.claude/worktrees\n\
         else base=.worktrees\n\
         fi\n\
         wt=\"$base/$name\"\n\
         if [ ! -e \"$wt\" ]; then\n\
         def=\"$(git symbolic-ref --short refs/remotes/origin/HEAD 2>/dev/null | sed 's#^origin/##')\"\n\
         [ -z \"$def\" ] && def=\"$(git rev-parse --abbrev-ref HEAD 2>/dev/null)\"\n\
         git worktree add \"$wt\" -b \"$name\" \"$def\" 1>&2\n\
         fi\n\
         ( cd \"$wt\" && pwd )\n",
        root = shq(root),
        name = shq(name),
    )
}

async fn create_worktree_local(root: &str, name: &str) -> Result<String, IpcError> {
    let script = worktree_add_script(root, name);
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
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub async fn new_session(
    args: NewSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    reg: &Arc<CancellationRegistry>,
) -> Result<SessionRow, IpcError> {
    // Reject hostile input before it reaches ssh / tmux / git.
    crate::validate::host_alias(&args.host_alias)?;
    crate::validate::tmux_name(&args.name)?;

    if let Some(name) = args.new_worktree.as_deref() {
        crate::validate::git_ref(name)?;
        if name == "main" || name == "master" {
            return Err(IpcError::new(
                "E_INVALID",
                "worktree name must not be 'main' or 'master'",
            ));
        }
    }

    // Mint / bind a cancellation token for the duration of this command.
    // If a call_id was provided by the frontend, bind under that id so the
    // frontend can cancel via cancel_command(call_id). Otherwise use an
    // anonymous id (internal callers, tests, local sessions).
    let (cancel_id, token) = match args.call_id {
        Some(id) => {
            let token = CancellationToken::new();
            reg.bind(id, token.clone());
            (id, token)
        }
        None => reg.register_anonymous(),
    };
    // RAII guard releases the registry slot on every exit path — including a
    // panic inside new_session_inner, which a manual unregister would miss.
    let _guard = CancelGuard::new(Arc::clone(reg), cancel_id);

    new_session_inner(args, store, ssh, token).await
}

async fn new_session_inner(
    args: NewSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    token: CancellationToken,
) -> Result<SessionRow, IpcError> {
    // Resolve the cwd that tmux will spawn the pane in. For LOCAL the path
    // comes straight from the DB (it was discovered by scanning ~/projects).
    // For REMOTE we can't use the local path — it doesn't exist on the other
    // machine — so we translate to `~/projects/github.com/<owner>/<repo>`
    // (matching proj-clean's convention) and auto-clone if missing.
    let path: PathBuf = if args.host_alias == "local" {
        if let Some(ref name) = args.new_worktree {
            // NEW WORKTREE: create branch + worktree, return the new dir.
            let base_path = {
                let s = store
                    .lock()
                    .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
                let mut stmt = s
                    .conn_ref()
                    .prepare("SELECT base_path FROM projects WHERE id=?1")?;
                let row: String =
                    stmt.query_row(rusqlite::params![args.project_id], |r| r.get(0))?;
                row
            };
            PathBuf::from(create_worktree_local(&base_path, name).await?)
        } else {
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
                let row: String =
                    stmt.query_row(rusqlite::params![args.project_id], |r| r.get(0))?;
                PathBuf::from(row)
            }
        }
    } else {
        // Remote path: derive from owner/repo, then ensure-on-remote.
        if let Some(ref name) = args.new_worktree {
            // NEW WORKTREE on remote: ensure clone exists, then create worktree.
            let (owner, repo) = {
                let s = store
                    .lock()
                    .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
                fetch_owner_repo(&s, args.project_id)?
            };
            let home = ssh.remote_home(&args.host_alias).await?;
            let (project_root, _) = remote_project_path(&home, &owner, &repo, None);
            ensure_remote_project(
                ssh,
                &args.host_alias,
                &owner,
                &repo,
                &project_root,
                None,
                token.clone(),
            )
            .await?;
            let script = worktree_add_script(&project_root, name);
            // Quote the whole script so it survives the ssh argv-join +
            // remote login-shell re-tokenization (see ensure_remote_project).
            let quoted = shq(&script);
            let out = ssh
                .run_cancellable(
                    &args.host_alias,
                    &["bash", "-lc", &quoted],
                    std::time::Duration::from_secs(60),
                    token,
                )
                .await?;
            if !out.status.success() {
                return Err(IpcError::new(
                    "E_GIT_SETUP",
                    String::from_utf8_lossy(&out.stderr).trim().to_string(),
                ));
            }
            PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
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
            let home = ssh.remote_home(&args.host_alias).await?;
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
            )
            .await?;
            PathBuf::from(cwd)
        }
    };
    // A "shell" session runs a plain login shell in the pane instead of
    // Claude Code. Any other value (incl. None) is treated as a "work" session.
    let is_shell = args.kind.as_deref() == Some("shell");
    let pane_cmd: String = if is_shell {
        crate::tmux::shell_pane_command(args.start_command.as_deref())
    } else {
        crate::tmux::pane_command().to_string()
    };

    let tmux = exec_for(&args.host_alias, ssh);
    tmux.new_session(&args.name, &path, &pane_cmd).await?;

    reconcile_one_host(store, ssh, &args.host_alias).await?;
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let row = s
        .list_sessions_for_host(&args.host_alias)?
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
        })?;
    // Reconcile inserts every session as kind="work"; tag shell sessions
    // afterwards. The session upsert preserves `kind` on re-reconcile.
    if is_shell {
        s.set_session_kind(row.id, "shell", None)?;
        return s
            .get_session(&args.name, &args.host_alias)?
            .ok_or_else(|| IpcError::new("E_INTERNAL", "session vanished after kind tag"));
    }
    Ok(row)
}

#[derive(Deserialize)]
pub struct KillSessionArgs {
    pub host_alias: String,
    pub name: String,
}

pub async fn kill_session(
    args: KillSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<i64, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    crate::validate::tmux_name(&args.name)?;
    // Look up id BEFORE killing so we can return it after.
    let id = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.get_session(&args.name, &args.host_alias)?
            .map(|r| r.id)
            .ok_or_else(|| {
                IpcError::new("E_NOTFOUND", format!("session {} not found", args.name))
            })?
    };
    let tmux = exec_for(&args.host_alias, ssh);
    tmux.kill_session(&args.name).await?;
    reconcile_one_host(store, ssh, &args.host_alias).await?;
    Ok(id)
}

#[derive(Deserialize)]
pub struct RenameSessionArgs {
    pub host_alias: String,
    pub old_name: String,
    pub new_name: String,
}

pub async fn rename_session(
    args: RenameSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<SessionRow, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    crate::validate::tmux_name(&args.old_name)?;
    crate::validate::tmux_name(&args.new_name)?;
    let tmux = exec_for(&args.host_alias, ssh);
    tmux.rename_session(&args.old_name, &args.new_name).await?;
    reconcile_one_host(store, ssh, &args.host_alias).await?;
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    // `new_name` is validated verbatim (no padding), so look it up as-is —
    // consistent with kill_session / restart_session.
    s.get_session(&args.new_name, &args.host_alias)?
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

pub async fn restart_session(
    args: RestartSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<SessionRow, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    crate::validate::tmux_name(&args.name)?;
    // Respawn the pane with the command matching the session's kind so a
    // restarted shell session comes back as a shell, not a Claude pane.
    let is_shell = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.get_session(&args.name, &args.host_alias)?
            .map(|r| r.kind == "shell")
            .unwrap_or(false)
    };
    // Restart respawns a bare shell — the original start command isn't
    // persisted, and a restart is a recovery action, not a re-run.
    let pane_cmd: String = if is_shell {
        crate::tmux::shell_pane_command(None)
    } else {
        crate::tmux::pane_command().to_string()
    };
    let tmux = exec_for(&args.host_alias, ssh);
    tmux.restart_session(&args.name, &pane_cmd).await?;
    reconcile_one_host(store, ssh, &args.host_alias).await?;
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.get_session(&args.name, &args.host_alias)?.ok_or_else(|| {
        IpcError::new(
            "E_NOTFOUND",
            format!(
                "restarted session {} on {} did not appear in list",
                args.name, args.host_alias
            ),
        )
    })
}

/// Build the two tmux invocations that together send a prompt to a session:
///   1. send-keys -t <name> -l <prompt>   (literal, no key-name translation)
///   2. send-keys -t <name> Enter         (real Enter to submit)
pub fn build_send_commands(tmux_name: &str, prompt: &str) -> Vec<String> {
    vec![
        format!("tmux send-keys -t {} -l {}", shq(tmux_name), shq(prompt)),
        format!("tmux send-keys -t {} Enter", shq(tmux_name)),
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
    crate::validate::host_alias(host_alias)?;
    crate::validate::tmux_name(tmux_name)?;
    // Both send-keys commands in ONE shell invocation joined with `&&` (so a
    // failed literal-text send doesn't still fire Enter) — one round-trip
    // instead of two.
    let script = build_send_commands(tmux_name, prompt).join(" && ");
    let out = if host_alias == "local" {
        tokio::process::Command::new("bash")
            .args(["-c", &script])
            .output()
            .await
            .map_err(|e| IpcError::new("E_TMUX", format!("spawn bash: {e}")))?
    } else {
        ssh.run(
            host_alias,
            &["bash", "-lc", &shq(&script)],
            std::time::Duration::from_secs(10),
        )
        .await?
    };
    if !out.status.success() {
        return Err(IpcError::new(
            "E_TMUX",
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

pub async fn send_prompt(args: SendPromptArgs, ssh: &Arc<SshClient>) -> Result<(), IpcError> {
    send_prompt_inner(ssh, &args.host_alias, &args.tmux_name, &args.prompt).await
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

pub async fn spawn_review(
    args: SpawnReviewArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<crate::store::SessionRow, IpcError> {
    // 1. Snapshot source + resolve cwd under a brief lock.
    let (source, cwd) = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        let source = s
            .get_session_by_id(args.source_session_id)?
            .ok_or_else(|| IpcError::new("E_NOTFOUND", "source session not found"))?;
        let cwd = resolve_review_cwd(&s, &source)?;
        (source, cwd)
    };

    // 2. Spawn the review tmux session (off-lock).
    //    A review runs Claude Code — same pane command as any "work" session.
    let short = format!("{:x}", now_unix() & 0xfffff);
    let review_name = format!("{}--review-{}", source.tmux_name, short);
    let tmux = exec_for(&source.host_alias, ssh);
    tmux.new_session(
        &review_name,
        std::path::Path::new(&cwd),
        crate::tmux::pane_command(),
    )
    .await?;

    // 3. Register via per-host reconcile.
    reconcile_one_host(store, ssh, &source.host_alias).await?;

    // 4. Tag as review + capture id.
    let review_id = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        let row = s
            .list_sessions_for_host(&source.host_alias)?
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
    if let Err(e) = send_prompt_inner(ssh, &source.host_alias, &review_name, &args.prompt).await {
        eprintln!("spawn_review: seeding prompt to {review_name} failed (session is live, seed manually): {e:?}");
    }

    // 6. Return the tagged review row.
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.get_session_by_id(review_id)?
        .ok_or_else(|| IpcError::new("E_INTERNAL", "review row missing after tag"))
}

#[derive(Deserialize)]
pub struct RecreateSessionArgs {
    pub session_id: i64,
}

pub async fn recreate_session(
    args: RecreateSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<SessionRow, IpcError> {
    // Load session and validate it is a ghost.
    let (sess, host) = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        let sess = s
            .get_session_by_id(args.session_id)?
            .ok_or_else(|| IpcError::new("E_NOTFOUND", "session not found"))?;
        if sess.status != "ghost" {
            return Err(IpcError::new(
                "E_INVALID_STATE",
                format!("session {} is not a ghost (status={})", sess.id, sess.status),
            ));
        }
        let host = s
            .get_host_row(&sess.host_alias)?
            .ok_or_else(|| IpcError::new("E_NOTFOUND", "host not found"))?;
        if !host.reachable {
            return Err(IpcError::new(
                "E_HOST_OFFLINE",
                format!("host {} is not reachable", host.alias),
            ));
        }
        (sess, host)
    };

    // Create the bare tmux session on the host.
    let tmux = exec_for(&host.alias, ssh);
    tmux.bare_new_session(&sess.tmux_name).await?;

    // Restore the DB row and return it.
    let row = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?
        .restore_session(sess.id)?
        .ok_or_else(|| IpcError::new("E_INTERNAL", "session vanished after restore"))?;
    Ok(row)
}

#[derive(Deserialize)]
pub struct DismissGhostSessionArgs {
    pub session_id: i64,
}

pub fn dismiss_ghost_session(
    args: DismissGhostSessionArgs,
    store: &Mutex<Store>,
) -> Result<(), IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let sess = s
        .get_session_by_id(args.session_id)?
        .ok_or_else(|| IpcError::new("E_NOTFOUND", "session not found"))?;
    if sess.status != "ghost" {
        return Err(IpcError::new(
            "E_INVALID_STATE",
            format!("session {} is not a ghost (status={})", sess.id, sess.status),
        ));
    }
    s.delete_session(sess.id)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn extracts_owner_repo_from_macos_path() {
        let r = extract_owner_repo(
            "/Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/x",
        );
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
        assert_eq!(
            root,
            "/home/mjanci/projects/github.com/martin-janci/claude-fleet"
        );
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
            uuid: "u1".into(),
            email: None,
            display_name: None,
            organization_name: None,
            organization_uuid: None,
            seat_tier: None,
            last_seen_at: None,
        })
        .unwrap();
        // First reconcile captures host's account
        s.upsert_session("dev-a", "h", None, None, 1, 100, "running", Some("u1"))
            .unwrap();
        // Host re-auths into a different account
        s.upsert_account(&AccountRow {
            uuid: "u2".into(),
            email: None,
            display_name: None,
            organization_name: None,
            organization_uuid: None,
            seat_tier: None,
            last_seen_at: None,
        })
        .unwrap();
        // Second reconcile: caller reads existing account before upsert
        let preserved = s.get_session_account("h", "dev-a").unwrap();
        s.upsert_session(
            "dev-a",
            "h",
            None,
            None,
            1,
            200,
            "running",
            preserved.as_deref(), // u1
        )
        .unwrap();
        // Verify session kept the ORIGINAL account
        assert_eq!(
            s.get_session_account("h", "dev-a").unwrap().as_deref(),
            Some("u1")
        );
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
        use crate::tmux::TmuxSession;
        use async_trait::async_trait;
        use std::time::Duration;

        struct SleepyTmux {
            sleep_ms: u64,
        }

        #[async_trait]
        impl TmuxExec for SleepyTmux {
            async fn list_sessions(&self) -> Result<Vec<TmuxSession>, IpcError> {
                tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
                Ok(Vec::new())
            }
            async fn new_session(
                &self,
                _name: &str,
                _cwd: &std::path::Path,
                _pane_cmd: &str,
            ) -> Result<(), IpcError> {
                Ok(())
            }
            async fn kill_session(&self, _name: &str) -> Result<(), IpcError> {
                Ok(())
            }
            async fn rename_session(&self, _old: &str, _new: &str) -> Result<(), IpcError> {
                Ok(())
            }
            async fn restart_session(&self, _name: &str, _pane_cmd: &str) -> Result<(), IpcError> {
                Ok(())
            }
            async fn capture_pane(&self, _name: &str) -> Result<String, IpcError> {
                Ok(String::new())
            }
            async fn bare_new_session(&self, _name: &str) -> Result<(), IpcError> {
                Ok(())
            }
        }

        // Spawn 3 tasks with sleeps 50ms, 500ms, 50ms.
        // Sequential sum ≈ 600ms; parallel max ≈ 500ms.
        let mut set = tokio::task::JoinSet::new();
        let start = std::time::Instant::now();
        for ms in [50u64, 500, 50] {
            set.spawn(async move { SleepyTmux { sleep_ms: ms }.list_sessions().await });
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
            s.upsert_session("alpha-s", "alpha", None, None, 1, 1, "running", None)
                .unwrap();
            s.upsert_session("beta-s", "beta", None, None, 1, 1, "running", None)
                .unwrap();
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
            uuid: "u1".into(),
            email: None,
            display_name: None,
            organization_name: None,
            organization_uuid: None,
            seat_tier: None,
            last_seen_at: None,
        })
        .unwrap();
        // Brand new session — no existing row
        assert!(s.get_session_account("h", "dev-new").unwrap().is_none());
        let preserved = s.get_session_account("h", "dev-new").unwrap();
        let account = preserved.or(Some("u1".to_string()));
        s.upsert_session(
            "dev-new",
            "h",
            None,
            None,
            1,
            100,
            "running",
            account.as_deref(),
        )
        .unwrap();
        assert_eq!(
            s.get_session_account("h", "dev-new").unwrap().as_deref(),
            Some("u1")
        );
    }

    #[tokio::test]
    async fn wait_for_repl_ready_returns_once_prompt_appears() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc as StdArc;

        struct FakeTmux {
            calls: StdArc<AtomicU32>,
        }
        #[async_trait::async_trait]
        impl TmuxExec for FakeTmux {
            async fn list_sessions(&self) -> Result<Vec<crate::tmux::TmuxSession>, IpcError> {
                Ok(vec![])
            }
            async fn new_session(
                &self,
                _: &str,
                _: &std::path::Path,
                _: &str,
            ) -> Result<(), IpcError> {
                Ok(())
            }
            async fn kill_session(&self, _: &str) -> Result<(), IpcError> {
                Ok(())
            }
            async fn rename_session(&self, _: &str, _: &str) -> Result<(), IpcError> {
                Ok(())
            }
            async fn restart_session(&self, _: &str, _: &str) -> Result<(), IpcError> {
                Ok(())
            }
            async fn capture_pane(&self, _: &str) -> Result<String, IpcError> {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                // Not ready for the first 2 polls, then the prompt appears.
                if n < 2 {
                    Ok("starting…".into())
                } else {
                    Ok("│ > ".into())
                }
            }
            async fn bare_new_session(&self, _: &str) -> Result<(), IpcError> {
                Ok(())
            }
        }

        let calls = StdArc::new(AtomicU32::new(0));
        let tmux = FakeTmux {
            calls: calls.clone(),
        };
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
        let wid = store
            .upsert_worktree(pid, "main", "/base/r/main", None)
            .unwrap();
        // Session with worktree → worktree path wins.
        let s1 = store
            .upsert_session("s1", "alpha", Some(pid), Some(wid), 1, 1, "running", None)
            .unwrap();
        let row1 = store.get_session_by_id(s1).unwrap().unwrap();
        assert_eq!(resolve_review_cwd(&store, &row1).unwrap(), "/base/r/main");
        // Session with project but no worktree → project base.
        let s2 = store
            .upsert_session("s2", "alpha", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        let row2 = store.get_session_by_id(s2).unwrap().unwrap();
        assert_eq!(resolve_review_cwd(&store, &row2).unwrap(), "/base/r");
        // Session with neither → error.
        let s3 = store
            .upsert_session("s3", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let row3 = store.get_session_by_id(s3).unwrap().unwrap();
        assert!(resolve_review_cwd(&store, &row3).is_err());
    }

    #[test]
    fn worktree_key_root_is_main_local_and_remote() {
        assert_eq!(
            worktree_key_for_path(
                "/Users/martinjanci/projects/github.com/martin-janci/claude-fleet"
            ),
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
            worktree_key_for_path(
                "/home/mjanci/projects/github.com/o/r/.claude/worktrees/feat-auth/src"
            ),
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

    #[test]
    fn worktree_key_empty_worktree_name_falls_back_to_main() {
        // A trailing `.claude/worktrees/` with no name segment must not yield
        // Some("") — it degrades to the safe "main" fallback.
        assert_eq!(
            worktree_key_for_path("/Users/x/projects/github.com/o/r/.claude/worktrees/"),
            Some("main".to_string())
        );
    }

    // ── worktree_add_script unit tests ────────────────────────────────────────

    #[test]
    fn worktree_add_script_contains_expected_fragments() {
        let script = worktree_add_script("/repo/root", "feat-x");
        assert!(script.contains("cd '/repo/root'"), "cd root: {script}");
        assert!(
            script.contains("name='feat-x'"),
            "name assignment: {script}"
        );
        assert!(
            script.contains("git worktree add"),
            "worktree add: {script}"
        );
        assert!(script.contains(" -b "), "branch flag: {script}");
        assert!(script.contains(".worktrees"), ".worktrees dir: {script}");
        assert!(
            script.contains(".claude/worktrees"),
            ".claude/worktrees dir: {script}"
        );
        assert!(
            script.contains("refs/remotes/origin/HEAD"),
            "default branch detection: {script}"
        );
    }

    // ── create_worktree_local integration test ────────────────────────────────

    #[tokio::test]
    async fn create_worktree_local_creates_and_is_idempotent() {
        use std::process::Command;

        // Create a unique temp dir for the bare repo
        let base = std::env::temp_dir().join(format!(
            "cf-wt-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&base).expect("create base");

        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).expect("create repo");
        let repo_str = repo.to_str().unwrap();

        // git init
        let status = Command::new("git")
            .args(["init", repo_str])
            .status()
            .expect("git init");
        assert!(status.success());

        // configure user.email and user.name so commit works
        Command::new("git")
            .args(["-C", repo_str, "config", "user.email", "test@test.com"])
            .status()
            .expect("git config email");
        Command::new("git")
            .args(["-C", repo_str, "config", "user.name", "Test"])
            .status()
            .expect("git config name");

        // write a file and commit
        let file = repo.join("README.md");
        std::fs::write(&file, "hello").expect("write file");
        Command::new("git")
            .args(["-C", repo_str, "add", "."])
            .status()
            .expect("git add");
        Command::new("git")
            .args(["-C", repo_str, "commit", "-m", "init"])
            .status()
            .expect("git commit");

        // call create_worktree_local
        let result = create_worktree_local(repo_str, "feat-x").await;
        assert!(result.is_ok(), "first call failed: {:?}", result);
        let wt_path = result.unwrap();
        assert!(
            wt_path.ends_with("/.worktrees/feat-x"),
            "path should end with /.worktrees/feat-x, got: {wt_path}"
        );
        assert!(
            std::path::Path::new(&wt_path).is_dir(),
            "worktree dir should exist: {wt_path}"
        );

        // second call — idempotent
        let result2 = create_worktree_local(repo_str, "feat-x").await;
        assert!(
            result2.is_ok(),
            "second (idempotent) call failed: {:?}",
            result2
        );
        assert_eq!(
            result2.unwrap(),
            wt_path,
            "idempotent call must return same path"
        );

        // cleanup
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn remote_script_must_be_quoted_to_survive_login_shell_retokenization() {
        // Regression for "zsh: parse error near `then`" on remote session
        // creation. ssh concatenates the trailing argv with spaces and the
        // remote LOGIN shell re-tokenizes the result, so `bash -lc <script>`
        // with an UNQUOTED `if ...; then ...; fi` splits at `;` and orphans
        // `then`. We reproduce that re-tokenization locally with `sh -c`.
        use std::process::Command;
        let script = "if true; then echo OK; fi";

        // RAW (the bug): the re-login shell mis-parses the orphaned `then`.
        let raw = Command::new("sh")
            .args(["-c", &format!("bash -lc {script}")])
            .output()
            .expect("sh");
        assert!(
            !raw.status.success(),
            "unquoted if/then must fail at the re-tokenizing login shell"
        );

        // QUOTED (the fix): crosses as one word, bash runs the whole script.
        let quoted = Command::new("sh")
            .args(["-c", &format!("bash -lc {}", shq(script))])
            .output()
            .expect("sh");
        assert!(
            quoted.status.success(),
            "shq'd script must run cleanly: {}",
            String::from_utf8_lossy(&quoted.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&quoted.stdout).trim(), "OK");
    }
}

#[cfg(test)]
mod ghost_tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn recreate_session_rejects_non_ghost() {
        let store = std::sync::Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
                .unwrap();
        }
        let id = store
            .lock()
            .unwrap()
            .get_session("dev", "local")
            .unwrap()
            .unwrap()
            .id;
        let args = RecreateSessionArgs { session_id: id };
        let ssh = std::sync::Arc::new(crate::ssh::SshClient::new());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(recreate_session(args, &store, &ssh));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, "E_INVALID_STATE");
    }

    #[test]
    fn dismiss_ghost_rejects_non_ghost() {
        let store = std::sync::Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
                .unwrap();
        }
        let id = store
            .lock()
            .unwrap()
            .get_session("dev", "local")
            .unwrap()
            .unwrap()
            .id;
        let args = DismissGhostSessionArgs { session_id: id };
        let result = dismiss_ghost_session(args, &store);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, "E_INVALID_STATE");
    }

    #[test]
    fn dismiss_ghost_deletes_ghost_session() {
        let store = std::sync::Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
                .unwrap();
        }
        let id = store
            .lock()
            .unwrap()
            .get_session("dev", "local")
            .unwrap()
            .unwrap()
            .id;
        // Manually ghost it
        store
            .lock()
            .unwrap()
            .conn_ref()
            .execute(
                "UPDATE sessions SET status='ghost', lost_at=999 WHERE id=?1",
                rusqlite::params![id],
            )
            .unwrap();
        let args = DismissGhostSessionArgs { session_id: id };
        let result = dismiss_ghost_session(args, &store);
        assert!(result.is_ok());
        assert!(store
            .lock()
            .unwrap()
            .get_session_by_id(id)
            .unwrap()
            .is_none());
    }
}
