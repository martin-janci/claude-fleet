//! Session lifecycle: create (including worktree setup), kill, rename,
//! friendly name, restart, recreate, and dismissing ghosts.

use super::*;

#[derive(Deserialize)]
pub struct NewSessionArgs {
    pub host_alias: String,
    pub project_id: i64,
    pub worktree_id: Option<i64>,
    pub name: String,
    pub call_id: Option<u64>,
    pub new_worktree: Option<String>,
    /// Branch to fork a new worktree from. `None` / empty = the repo's default
    /// branch. Only consulted when `new_worktree` is set. Resolution falls back
    /// to the default branch if the named branch isn't found (see
    /// `worktree_add_script`).
    pub base_branch: Option<String>,
    /// Session kind: `"work"` (default) runs Claude Code in the pane;
    /// `"shell"` runs a plain interactive login shell.
    pub kind: Option<String>,
    /// Optional command run once on start for a `"shell"` session, before
    /// the pane drops to an interactive shell. Ignored for `"work"`.
    pub start_command: Option<String>,
    /// Optional user-supplied sidebar label. Empty / missing -> derive
    /// deterministically from the branch via `humanize::humanize_branch` so
    /// the sidebar never shows the raw `dev-<owner>-<repo>--…` slug. The
    /// in-session agent can refine it later via the `set_friendly_name`
    /// MCP tool.
    pub friendly_name: Option<String>,
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
pub(super) async fn ensure_remote_project(
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
        root = quote(project_root),
        url = quote(&clone_url),
    ));
    if let Some((wt_name, branch)) = worktree {
        if wt_name != "main" {
            let wt_rel = format!(".claude/worktrees/{wt_name}");
            let wt_abs = format!("{project_root}/{wt_rel}");
            let branch = branch.unwrap_or(wt_name);
            script.push_str(&format!(
                " && if [ ! -d {abs} ]; then cd {root} && git worktree add {rel} {br}; fi",
                abs = quote(&wt_abs),
                root = quote(project_root),
                rel = quote(&wt_rel),
                br = quote(branch),
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
    // error near then"). `quote` escapes the inner single-quotes from the path
    // interpolation above.
    let quoted = quote(&script);
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
/// stderr; the ONLY stdout is the absolute PHYSICAL path of the worktree
/// (`pwd -P`, last line), which the caller uses as the tmux cwd. The logical
/// `pwd` echoed a symlinked root's spelling, a second identity for the same
/// checkout next to the physical one tmux and git report.
pub(super) fn worktree_add_script(root: &str, name: &str, base: Option<&str>) -> String {
    // Requested base branch, shell-quoted; empty string when unset (= default
    // branch). The shell var is `basebr` to avoid colliding with `base`, which
    // already names the worktree *directory* (.worktrees vs .claude/worktrees).
    let basebr = base
        .map(|b| b.trim())
        .filter(|b| !b.is_empty())
        .map(quote)
        .unwrap_or_else(|| "''".to_string());
    format!(
        "set -e\n\
         cd {root}\n\
         name={name}\n\
         basebr={basebr}\n\
         if [ -d .worktrees ]; then base=.worktrees\n\
         elif [ -d .claude/worktrees ]; then base=.claude/worktrees\n\
         else base=.worktrees\n\
         fi\n\
         wt=\"$base/$name\"\n\
         if [ ! -e \"$wt\" ]; then\n\
         def=\"$(git symbolic-ref --short refs/remotes/origin/HEAD 2>/dev/null | sed 's#^origin/##')\"\n\
         [ -z \"$def\" ] && def=\"$(git rev-parse --abbrev-ref HEAD 2>/dev/null)\"\n\
         if [ -n \"$basebr\" ] && git show-ref --verify --quiet \"refs/heads/$basebr\"; then start=\"$basebr\"\n\
         elif [ -n \"$basebr\" ] && git show-ref --verify --quiet \"refs/remotes/origin/$basebr\"; then start=\"origin/$basebr\"\n\
         else start=\"$def\"\n\
         fi\n\
         git worktree add \"$wt\" -b \"$name\" \"$start\" 1>&2\n\
         fi\n\
         ( cd \"$wt\" && pwd -P )\n",
        root = quote(root),
        name = quote(name),
    )
}

pub(super) async fn create_worktree_local(
    root: &str,
    name: &str,
    base: Option<&str>,
) -> Result<String, IpcError> {
    let script = worktree_add_script(root, name, base);
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
    mut args: NewSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    reg: &Arc<CancellationRegistry>,
) -> Result<SessionRow, IpcError> {
    // Reject hostile input before it reaches ssh / tmux / git.
    crate::validate::host_alias(&args.host_alias)?;
    // An empty name means "pick one for me" (the dialog's dice, an MCP
    // caller that doesn't care): mint it here so every caller shares the
    // same convention and the same collision policy.
    if args.name.trim().is_empty() {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        args.name = fill_session_name(&s, &args)?;
    }
    crate::validate::tmux_name(&args.name)?;
    if let Some(fname) = args.friendly_name.as_deref() {
        crate::validate::friendly_name(fname)?;
    }

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

/// Mint a tmux name for a `new_session` call that left `name` empty.
///
/// The deterministic `dev-<owner>-<repo>[--<worktree>][-term]` is used when no
/// session of that name exists on the host (the dialog's own convention).
/// When it is taken — a second session on the same worktree — a memorable
/// `<adjective>-<noun>` pair from `names::generate_name` is appended instead,
/// avoiding every slug already in use on the project (see
/// `project_taken_slugs`), so the result is unique and reads like
/// `dev-owner-repo--blue-sirius` rather than `…--main-2`. `.` and `:` are
/// mapped to `-` so the result always passes `validate::tmux_name`.
pub(crate) fn fill_session_name(s: &Store, args: &NewSessionArgs) -> Result<String, IpcError> {
    use crate::service::names::{generate_name_default, tmux_safe};
    let (owner, repo) = fetch_owner_repo(s, args.project_id)?;
    let base = format!("dev-{owner}-{repo}");
    let term = if args.kind.as_deref() == Some("shell") {
        "-term"
    } else {
        ""
    };
    let wt: Option<String> = if let Some(n) = args
        .new_worktree
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        Some(n.to_string())
    } else if let Some(wid) = args.worktree_id {
        let (name, _) = fetch_worktree(s, wid)?;
        (name != "main").then_some(name)
    } else {
        None
    };
    let deterministic = tmux_safe(&match &wt {
        Some(w) => format!("{base}--{w}{term}"),
        None => format!("{base}{term}"),
    });
    let on_host = s.list_sessions_for_host(&args.host_alias)?;
    if !on_host.iter().any(|r| r.tmux_name == deterministic) {
        return Ok(deterministic);
    }
    let taken = project_taken_slugs(s, args.project_id, &owner, &repo)?;
    let pair = generate_name_default(&taken);
    Ok(tmux_safe(&match &wt {
        Some(w) => format!("{base}--{w}--{pair}{term}"),
        None => format!("{base}--{pair}{term}"),
    }))
}

/// Every slug already in use on a project — worktree names, the suffix of
/// each session's tmux name, and each slugified friendly name — i.e. the set
/// a freshly generated pair must avoid. Mirrors `takenSlugs` in
/// `NewSessionDialog.svelte`.
pub(super) fn project_taken_slugs(
    s: &Store,
    project_id: i64,
    owner: &str,
    repo: &str,
) -> Result<std::collections::HashSet<String>, IpcError> {
    use crate::service::names::{slugify, tmux_name_suffix};
    let mut taken: std::collections::HashSet<String> = s
        .list_worktrees_for_project(project_id)?
        .into_iter()
        .map(|w| w.name.to_lowercase())
        .collect();
    for r in s.list_all_sessions()? {
        if r.project_id != Some(project_id) {
            continue;
        }
        if let Some(suffix) = tmux_name_suffix(&r.tmux_name, owner, repo) {
            taken.insert(suffix.to_lowercase());
        }
        if let Some(slug) = r.friendly_name.as_deref().map(slugify) {
            if !slug.is_empty() {
                taken.insert(slug);
            }
        }
    }
    Ok(taken)
}

pub(super) async fn new_session_inner(
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
            PathBuf::from(
                create_worktree_local(&base_path, name, args.base_branch.as_deref()).await?,
            )
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
            let (project_root, _) = {
                let s = store.lock().map_err(|_| IpcError::lock())?;
                remote_project_path_for(&s, &args.host_alias, &home, &owner, &repo, None)
            };
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
            let script = worktree_add_script(&project_root, name, args.base_branch.as_deref());
            // Quote the whole script so it survives the ssh argv-join +
            // remote login-shell re-tokenization (see ensure_remote_project).
            let quoted = quote(&script);
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
            let (project_root, cwd) = {
                let s = store.lock().map_err(|_| IpcError::lock())?;
                remote_project_path_for(&s, &args.host_alias, &home, &owner, &repo, wt_name_str)
            };
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

    // Automatic self-repair for an EXISTING worktree row / main checkout: the
    // row may point at a directory that was deleted since it was written.
    // Automatic means create-only (re-add a missing worktree from its existing
    // branch); anything more returns E_REPAIR_REQUIRED before tmux starts.
    // A brand-new worktree was just created by `worktree_add_script`.
    let (path, repaired) = if args.new_worktree.is_none() {
        let rep = crate::service::repair::ensure_for_new_session(
            store,
            ssh,
            crate::service::repair::NewSessionWorkspace {
                host_alias: &args.host_alias,
                project_id: args.project_id,
                worktree_id: args.worktree_id,
                tmux_name: &args.name,
                pane_cmd: &pane_cmd,
                cwd: &path.to_string_lossy(),
                base_branch: args.base_branch.as_deref(),
            },
        )
        .await?;
        (
            PathBuf::from(rep.cwd.clone()),
            Some(rep).filter(|r| !r.actions.is_empty()),
        )
    } else {
        (path, None)
    };

    let tmux = exec_for(&args.host_alias, ssh);
    tmux.new_session(&args.name, &path, &pane_cmd).await?;

    reconcile_one_host(store, ssh, &args.host_alias).await?;
    if let Some(rep) = &repaired {
        // Same detail as every other workspace_repaired event (branch_source
        // included), attached now that reconcile created the row.
        record_session_event(
            store,
            &args.host_alias,
            &args.name,
            crate::service::repair::EVENT_REPAIRED,
            Some(crate::service::repair::event_detail(rep)),
        );
    }
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

    // PROD-5: the fleet created this session now. Soft-fail (cosmetic).
    if let Err(e) = s.set_started_at(row.id, now_unix()) {
        tracing::warn!(
            session = %args.name,
            error = %e,
            "[new_session] storing started_at failed"
        );
    }

    // Deterministic friendly name: trust an explicit user value, otherwise
    // derive from the branch so the sidebar never shows the raw slug. Soft-
    // fail like the claude_session_id write below — a missing label is
    // cosmetic, the session is live.
    let derived_friendly = derive_friendly_name(&s, &args, row.worktree_id)?;
    if let Some(ref value) = derived_friendly {
        if let Err(e) = s.set_friendly_name(&args.host_alias, &args.name, Some(value)) {
            tracing::warn!(
                session = %args.name,
                error = %e,
                "[new_session] storing friendly_name failed"
            );
        }
    }

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
            tracing::warn!(
                session = %args.name,
                error = %e,
                "[new_session] storing claude_session_id failed"
            );
        } else {
            row.claude_session_id = Some(cid.clone());
        }
    }
    if derived_friendly.is_some() {
        // The set_friendly_name call above emitted a session_updated row
        // event already; we refresh in-memory so the value returned to the
        // caller matches what the sidebar will display.
        if let Some(refreshed) = s.get_session(&args.name, &args.host_alias)? {
            row.friendly_name = refreshed.friendly_name;
        }
    }
    Ok(row)
}

/// Resolve the friendly name to persist for a freshly created session:
/// trim the user's explicit value, or derive a humanised label from the
/// branch when none was supplied. Returns `None` only when both inputs
/// resolve to empty — in practice this is rare (the humaniser falls back
/// to the repo name), but we treat it as "leave it NULL, let the agent
/// fill it in later" rather than overwriting with an empty string.
pub(super) fn derive_friendly_name(
    s: &Store,
    args: &NewSessionArgs,
    row_worktree_id: Option<i64>,
) -> Result<Option<String>, IpcError> {
    if let Some(supplied) = args.friendly_name.as_deref() {
        let trimmed = supplied.trim();
        if !trimmed.is_empty() {
            return Ok(Some(trimmed.to_string()));
        }
    }
    let (owner, repo) = fetch_owner_repo(s, args.project_id)?;
    // Pick the most specific branch source available. `new_worktree` is the
    // branch the dialog just created; otherwise the worktree row stored
    // either a real `branch` ref or its display `name`; falling back to the
    // tmux name handles attach-to-bare-main and other oddities so the
    // humaniser always has something to chew on.
    let branch = if let Some(name) = args.new_worktree.as_deref() {
        name.to_string()
    } else if let Some(wid) = row_worktree_id.or(args.worktree_id) {
        let (name, branch) = fetch_worktree(s, wid)?;
        branch.unwrap_or(name)
    } else {
        args.name.clone()
    };
    let derived = crate::humanize::humanize_branch(&branch, &owner, &repo);
    if derived.is_empty() {
        Ok(None)
    } else {
        Ok(Some(derived))
    }
}

#[derive(Deserialize)]
pub struct KillSessionArgs {
    pub host_alias: String,
    pub name: String,
    /// Override the controller self-target guard.
    #[serde(default)]
    pub force: bool,
}

/// Claude session id to `claude stop` for a synthetic `bg:<uuid>` row: the
/// row's stored `claude_session_id` when present, else the uuid embedded in
/// the tmux_name itself. Pure so the fallback order is unit-testable.
pub(super) fn bg_claude_session_id(tmux_name: &str, row_claude_id: Option<&str>) -> String {
    match row_claude_id {
        Some(id) if !id.trim().is_empty() => id.to_string(),
        _ => tmux_name.trim_start_matches("bg:").to_string(),
    }
}

pub async fn kill_session(
    args: KillSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<i64, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    // Lookup form: synthetic `bg:<uuid>` rows are killable too (via
    // `claude stop`, below) — only real tmux rows go through tmux.
    crate::validate::tmux_name_lookup(&args.name)?;
    // Look up id BEFORE killing so we can return it after. Read the controller
    // under the same lock and refuse to nuke ourselves unless forced.
    let (id, claude_sid) = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        guard_not_controller(
            s.get_controller()?.as_ref(),
            &args.host_alias,
            &args.name,
            args.force,
        )?;
        s.get_session(&args.name, &args.host_alias)?
            .map(|r| (r.id, r.claude_session_id))
            .ok_or_else(|| {
                IpcError::new("E_NOTFOUND", format!("session {} not found", args.name))
            })?
    };
    if args.name.starts_with("bg:") {
        // Background (`claude --bg`) agent — there is no tmux pane to kill.
        // `claude stop` is idempotent (an already-dead job is not an error),
        // so this also clears a stale row whose process died un-noticed: the
        // reconcile below sees the agent gone and prunes the row.
        let sid = bg_claude_session_id(&args.name, claude_sid.as_deref());
        crate::claude_cli::claude_stop(ssh, &args.host_alias, &sid).await?;
        if let Ok(s) = store.lock() {
            if let Err(e) = s.insert_session_event(id, "killed", None) {
                tracing::warn!(session_id = id, error = %e, "[event] insert killed failed");
            }
        }
        reconcile_one_host(store, ssh, &args.host_alias).await?;
        return Ok(id);
    }
    let tmux = exec_for(&args.host_alias, ssh);
    tmux.kill_session(&args.name).await?;
    // Task G: record the kill before reconcile reaps the row. Best-effort.
    if let Ok(s) = store.lock() {
        if let Err(e) = s.insert_session_event(id, "killed", None) {
            tracing::warn!(session_id = id, error = %e, "[event] insert killed failed");
        }
    }
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
    crate::validate::tmux_name_addressable(&args.old_name)?;
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
pub struct SetFriendlyNameArgs {
    pub host_alias: String,
    pub tmux_name: String,
    /// Empty / whitespace-only value clears the label.
    pub friendly_name: String,
}

/// Set (or clear, on empty/whitespace) the session's display label. The agent
/// running inside a tmux session calls this via MCP after picking up a task.
pub fn set_session_friendly_name(
    args: SetFriendlyNameArgs,
    store: &Mutex<Store>,
) -> Result<SessionRow, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    // Lookup-mode validator: must accept the synthetic `bg:<uuid>` form used
    // by background-agent rows, which the create-mode `tmux_name` validator
    // rejects (tmux forbids `:` when creating a session, but we're only
    // addressing an existing row here, never spawning tmux).
    crate::validate::tmux_name_lookup(&args.tmux_name)?;
    crate::validate::friendly_name(&args.friendly_name)?;
    let trimmed = args.friendly_name.trim();
    let value = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    };
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.set_friendly_name(&args.host_alias, &args.tmux_name, value)?
        .ok_or_else(|| {
            IpcError::new(
                "E_NOTFOUND",
                format!(
                    "session {} not found on {}",
                    args.tmux_name, args.host_alias
                ),
            )
        })
}

#[derive(Deserialize)]
pub struct RestartSessionArgs {
    pub host_alias: String,
    pub name: String,
    /// Override the controller self-target guard.
    #[serde(default)]
    pub force: bool,
}

pub async fn restart_session(
    args: RestartSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<SessionRow, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    crate::validate::tmux_name_addressable(&args.name)?;
    // Respawn the pane with the command matching the session's kind so a
    // restarted shell session comes back as a shell, not a Claude pane. Read
    // the controller under the same lock and refuse to restart ourselves
    // unless forced.
    let (kind, claude_id, session_id) = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        guard_not_controller(
            s.get_controller()?.as_ref(),
            &args.host_alias,
            &args.name,
            args.force,
        )?;
        match s.get_session(&args.name, &args.host_alias)? {
            Some(r) => (r.kind, r.claude_session_id, Some(r.id)),
            None => ("work".to_string(), None, None),
        }
    };
    let pane_cmd: String = recreate_pane_command(&kind, claude_id.as_deref());
    let tmux = exec_for(&args.host_alias, ssh);
    // Automatic self-repair (create-only) before the pane is respawned, then
    // respawn INTO the verified directory (a pane whose cwd was deleted keeps
    // the dead inode until respawned with an explicit `-c`). A dead tmux
    // session is created instead of failing with "can't find session". A
    // workspace that needs more returns E_REPAIR_REQUIRED. Sessions with
    // nothing to repair (orphans, bg rows) keep the plain respawn.
    let repaired = match session_id {
        Some(id) => match crate::service::repair::ensure_session_workspace(
            id,
            crate::service::repair::Entry::Restart,
            store,
            ssh,
        )
        .await
        {
            Ok(rep) => Some(rep),
            Err(e) if e.code == codes::E_NOREPO || e.code == codes::E_BG_SESSION => None,
            Err(e) => return Err(e),
        },
        None => None,
    };
    match repaired {
        Some(rep) if !rep.tmux_alive => {
            tmux.new_session(&args.name, std::path::Path::new(&rep.cwd), &pane_cmd)
                .await?
        }
        Some(rep) => {
            tmux.respawn_pane_in(&args.name, std::path::Path::new(&rep.cwd), &pane_cmd)
                .await?
        }
        None => tmux.restart_session(&args.name, &pane_cmd).await?,
    }
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

/// Poll the tmux pane until `cl`'s REPL prompt appears, up to ~6s. Returns
/// when ready, or after the timeout (best-effort — a missed prompt just means
/// the user presses Enter / re-sends manually; spawn_review already soft-fails
/// the seed). `cl`'s prompt box draws a border (│) and a `>` prompt; we look
/// for either as a readiness signal.
pub(super) async fn wait_for_repl_ready(tmux: &dyn TmuxExec, name: &str) {
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        if let Ok(pane) = tmux.capture_pane(name).await {
            if pane.contains('>') || pane.contains('│') {
                return;
            }
        }
    }
}

/// The pane command to relaunch when (re)creating a session. `shell` → a bare
/// shell; otherwise resume the session's own Claude id (or `--continue` for a
/// legacy session with no stored id). A stored id is validated before use so a
/// tampered DB value can't inject shell — an invalid id degrades to `None`.
pub(crate) fn recreate_pane_command(kind: &str, claude_session_id: Option<&str>) -> String {
    if kind == "shell" {
        return crate::tmux::shell_pane_command(None);
    }
    let id = claude_session_id.filter(|id| crate::validate::claude_session_id(id).is_ok());
    crate::tmux::pane_command_for(id)
}

#[derive(Deserialize)]
pub struct RecreateSessionArgs {
    pub session_id: i64,
    /// Override the controller self-target guard.
    #[serde(default)]
    pub force: bool,
}

pub async fn recreate_session(
    args: RecreateSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<SessionRow, IpcError> {
    // Snapshot the session, gate on host reachability, and capture cwd-resolution
    // inputs — all under one brief lock, before any tmux/ssh call. For remote
    // hosts the cwd is finalized off-lock (needs `ssh.remote_home`), because the
    // local DB path is meaningless on the other machine.
    let (sess, cwd_src, pane_cmd) = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        let sess = s
            .get_session_by_id(args.session_id)?
            .ok_or_else(|| IpcError::new("E_NOTFOUND", "session not found"))?;
        // Refuse to nuke-and-rebuild ourselves unless forced.
        guard_not_controller(
            s.get_controller()?.as_ref(),
            &sess.host_alias,
            &sess.tmux_name,
            args.force,
        )?;
        let host = s
            .get_host_row(&sess.host_alias)?
            .ok_or_else(|| IpcError::new("E_NOTFOUND", "host not found"))?;
        if !host.reachable {
            return Err(IpcError::new(
                "E_HOST_OFFLINE",
                format!("host {} is not reachable", host.alias),
            ));
        }
        let cwd_src = cwd_source_for_session(&s, &sess)?;
        let pane_cmd = recreate_pane_command(&sess.kind, sess.claude_session_id.as_deref());
        (sess, cwd_src, pane_cmd)
    };
    // Automatic self-repair (create-only): re-add a deleted worktree from its
    // existing branch and use the verified path; anything more returns
    // E_REPAIR_REQUIRED. Orphans (no project) keep the plain resolution.
    let cwd = match crate::service::repair::ensure_session_workspace(
        sess.id,
        crate::service::repair::Entry::Recreate,
        store,
        ssh,
    )
    .await
    {
        Ok(rep) => rep.cwd,
        Err(e) if e.code == codes::E_NOREPO => {
            resolve_cwd_source(cwd_src, &sess.host_alias, ssh).await?
        }
        Err(e) => return Err(e),
    };

    let tmux = exec_for(&sess.host_alias, ssh);
    // Tear down any live session first (frees the old process tree / wedged
    // session). A ghost has no live session, so tolerate "no such session":
    // we ignore the kill result and rely on new_session below to fail loudly
    // if the old session unexpectedly survived (it would report a duplicate).
    let _ = tmux.kill_session(&sess.tmux_name).await;
    // Rebuild fresh in the worktree with the kind-appropriate command — the
    // same primitive new_session() uses.
    tmux.new_session(&sess.tmux_name, std::path::Path::new(&cwd), &pane_cmd)
        .await?;

    // Mark the row live again and return it.
    let row = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        let row = s
            .restore_session(sess.id)?
            .ok_or_else(|| IpcError::new("E_INTERNAL", "session vanished after restore"))?;
        // Task G: record the recreate on the (preserved) row. Best-effort.
        if let Err(e) = s.insert_session_event(sess.id, "recreated", None) {
            tracing::warn!(
                session_id = sess.id,
                error = %e,
                "[event] insert recreated failed"
            );
        }
        row
    };
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
            format!(
                "session {} is not a ghost (status={})",
                sess.id, sess.status
            ),
        ));
    }
    s.delete_session(sess.id)?;
    Ok(())
}
