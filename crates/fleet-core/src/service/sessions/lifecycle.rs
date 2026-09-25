//! Session lifecycle: create (including worktree setup), kill, rename,
//! friendly name, restart, recreate, and dismissing ghosts.

use super::*;
use crate::ipc_error::codes;
use crate::ipc_error::lock;
use crate::service::repair::{render_git_script_expecting, BranchSource, Step, MIRROR_REFUSED};
use crate::ssh::SshExec;

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
    /// Resume this Claude conversation id instead of minting a fresh one (a
    /// conversation found by `discover_lost_sessions`). Must be a lowercase
    /// UUID not already held by a session on the host; rejected for a
    /// `"shell"` session. The pane starts in `worktree_id` / the project root,
    /// which must be the transcript's cwd for `claude --resume` to find it
    /// (see `LostCandidate::resumable`).
    #[serde(default)]
    pub resume_claude_session_id: Option<String>,
}

/// An existing worktree to make sure is present on a remote host, ahead of
/// `ensure_remote_project`. `path` is the worktree's absolute path ON THE
/// HOST, as recorded by the host scan (`service::worktrees::list_host_worktrees`)
/// or the local scan mirrored into the `worktrees` table — NOT re-derived
/// from `name`, since a checkout can live under `.worktrees/`, under
/// `.claude/worktrees/`, or anywhere else git has it registered. The script
/// checks and (if missing) creates exactly this directory.
pub(super) struct RemoteWorktree<'a> {
    pub name: &'a str,
    pub branch: Option<&'a str>,
    pub path: &'a str,
}

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
        codes::E_INVALID,
        format!(
            "worktree {name} is a checkout on {row_host}; pick one that exists on {target_host} or start a new worktree"
        ),
    ))
}

/// Ensure the remote host has the project cloned at `<project_root>` and,
/// optionally, has a worktree checked out to `<branch>` at its own recorded
/// path (see [`RemoteWorktree`]). Idempotent: if the directory + .git is
/// already there, the clone step is skipped; same for worktree-add.
/// Auto-clones via SSH (`git@github.com:<owner>/<repo>.git`), assuming the
/// remote has SSH github access (the common case for dev machines).
///
/// The `token` parameter allows the caller to cancel the (potentially long-
/// running) `git clone` step. On cancellation the child is killed and
/// `Err(E_CANCELLED)` is returned. Partial clone dirs are NOT cleaned up
/// on cancel — that's a follow-up task.
///
/// Returns Ok(()) on success. Failure surfaces stderr in the IpcError so the
/// user can diagnose (missing SSH key, private-repo auth, etc.) — except a
/// worktree whose branch was never pushed, which gets an actionable
/// `E_GIT_SETUP` ("push it first") instead of git's `invalid reference`.
pub(super) async fn ensure_remote_project(
    ssh: &dyn SshExec,
    host: &str,
    owner: &str,
    repo: &str,
    project_root: &str,
    worktree: Option<&RemoteWorktree<'_>>,
    token: CancellationToken,
) -> Result<(), IpcError> {
    // Validate every component that gets interpolated into a remote path or
    // git command. Shell-quoting (below) stops command injection but NOT
    // `..` path traversal — a repo named `../../.ssh` would still be a valid
    // quoted argument that escapes the projects directory.
    crate::validate::path_component("owner", owner)?;
    crate::validate::path_component("repo", repo)?;
    if let Some(wt) = worktree {
        crate::validate::path_component("worktree name", wt.name)?;
        if let Some(b) = wt.branch {
            crate::validate::git_ref(b)?;
        }
        crate::validate::remote_abs_path("worktree path", wt.path)?;
    }
    let clone_url = format!("git@github.com:{owner}/{repo}.git");
    let script = ensure_remote_project_script(project_root, &clone_url, worktree);
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
        return Err(git_setup_error(
            host,
            owner,
            repo,
            mirrored_branch(worktree),
            &stdout,
            &stderr,
        ));
    }
    Ok(())
}

/// The branch `ensure_remote_project` mirrors for `worktree`: the row's
/// branch, else the worktree name; `None` for no worktree / the main checkout
/// (which is the clone itself, never a `worktree add`).
pub(super) fn mirrored_branch<'a>(worktree: Option<&RemoteWorktree<'a>>) -> Option<&'a str> {
    match worktree {
        Some(wt) if wt.name != "main" => Some(wt.branch.unwrap_or(wt.name)),
        _ => None,
    }
}

/// The bash script `ensure_remote_project` runs (via `bash -lc`):
///   1. clones the repo if `<root>/.git` is missing;
///   2. creates `<root>/.claude/worktrees/<name>` if requested and absent,
///      with the branch resolved at run time by the repair module's
///      [`BranchSource::Mirror`] add — the host's own `refs/heads/<branch>`,
///      else fetched fresh from origin and tracked, else refused with
///      [`MIRROR_REFUSED`] (the branch only exists on the source machine).
///
/// Both steps are guarded so a re-run on an already-set-up host is a no-op.
/// Pure, so tests pin its shape without a host.
pub(super) fn ensure_remote_project_script(
    project_root: &str,
    clone_url: &str,
    worktree: Option<&RemoteWorktree<'_>>,
) -> String {
    let root = quote(project_root);
    let mut script = format!(
        "set -e\n\
         if [ ! -d {root}/.git ]; then \
           mkdir -p \"$(dirname -- {root})\"; \
           tmp=\"$(dirname -- {root})/.fleet-clone-$$\"; rm -rf \"$tmp\"; \
           git clone {url} \"$tmp\" && {{ [ ! -e {root} ] || rmdir {root}; }} && mv \"$tmp\" {root} || {{ rm -rf \"$tmp\"; exit 1; }}; \
         fi\n",
        url = quote(clone_url),
    );
    if let Some(wt) = worktree {
        if let Some(branch) = mirrored_branch(worktree) {
            // The path the host scan recorded, never a derived
            // `.claude/worktrees/<name>`: a checkout under `.worktrees/` or
            // anywhere else git has it registered must be found, not
            // duplicated.
            let wt_abs = wt.path.to_string();
            let add = render_git_script_expecting(
                project_root,
                &[Step::AddWorktree {
                    path: wt_abs.clone(),
                    branch: branch.to_string(),
                    from: BranchSource::Mirror,
                }],
                None,
            );
            script.push_str(&format!(
                "if [ ! -d {abs} ]; then\n{add}fi\n",
                abs = quote(&wt_abs),
            ));
        }
    }
    script
}

/// The `E_GIT_SETUP` for a failed `ensure_remote_project` script. A mirror
/// the script refused because `branch` is on neither the host nor origin
/// says what to do (push it, or start a new worktree there) instead of
/// surfacing git's raw stderr; everything else keeps stderr (stdout when
/// stderr is empty) so the user can diagnose it.
pub(super) fn git_setup_error(
    host: &str,
    owner: &str,
    repo: &str,
    branch: Option<&str>,
    stdout: &str,
    stderr: &str,
) -> IpcError {
    if let Some(b) = branch.filter(|_| stderr.contains(MIRROR_REFUSED)) {
        return IpcError::new(
            codes::E_GIT_SETUP,
            format!(
                "branch {b} is not on origin; push it from the source machine, \
                 or start a new worktree on {host}"
            ),
        );
    }
    IpcError::new(
        codes::E_GIT_SETUP,
        format!(
            "couldn't ensure {owner}/{repo} on {host}: {}",
            if stderr.trim().is_empty() {
                stdout.trim()
            } else {
                stderr.trim()
            }
        ),
    )
}

/// Build a bash script (run via `bash -lc`) that creates a new worktree for a
/// NEW branch `name` off the repo's default branch, under `.worktrees/` or
/// `.claude/worktrees/` (auto-detected, fallback `.worktrees/`). Idempotent:
/// if the worktree dir already exists it's reused. Git's chatter goes to
/// stderr; the ONLY stdout is the absolute PHYSICAL path of the worktree
/// (`pwd -P`, last line), which the caller uses as the tmux cwd. The logical
/// `pwd` echoed a symlinked root's spelling, a second identity for the same
/// checkout next to the physical one tmux and git report.
///
/// When the base IS the worktree's own name and that branch already exists
/// locally (work graph M2: resuming work whose worktree a safe kill removed,
/// with its branch still there), the existing branch is checked out instead
/// of `-b` failing with "a branch named … already exists".
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
         if [ \"$basebr\" = \"$name\" ] && git show-ref --verify --quiet \"refs/heads/$name\"; then\n\
         git worktree add \"$wt\" \"$name\" 1>&2\n\
         else\n\
         git worktree add \"$wt\" -b \"$name\" \"$start\" 1>&2\n\
         fi\n\
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
    crate::service::hub::ensure_local_allowed(crate::service::projects::LOCAL_HOST)?;
    let script = worktree_add_script(root, name, base);
    let out = tokio::process::Command::new("bash")
        .args(["-lc", &script])
        .output()
        .await
        .map_err(|e| IpcError::new(codes::E_GIT_SETUP, format!("bash: {e}")))?;
    if !out.status.success() {
        return Err(IpcError::new(
            codes::E_GIT_SETUP,
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
        let s = lock(store)?;
        args.name = fill_session_name(&s, &args)?;
    }
    crate::validate::tmux_name(&args.name)?;
    if let Some(fname) = args.friendly_name.as_deref() {
        crate::validate::friendly_name(fname)?;
    }
    if let Some(id) = args.resume_claude_session_id.as_deref() {
        crate::validate::claude_session_id(id)?;
        if args.kind.as_deref() == Some("shell") {
            return Err(IpcError::new(
                codes::E_INVALID,
                "resume_claude_session_id applies to Claude sessions, not shell sessions",
            ));
        }
        reject_held_conversation(&*lock(store)?, &args.host_alias, id)?;
    }
    reject_lost_session_name(&*lock(store)?, &args.host_alias, &args.name)?;

    if let Some(name) = args.new_worktree.as_deref() {
        crate::validate::git_ref(name)?;
        if name == "main" || name == "master" {
            return Err(IpcError::new(
                codes::E_INVALID,
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

/// Refuse a name that belongs to a lost session with a resumable
/// conversation: reconcile's upsert (ON CONFLICT host_alias, tmux_name) would
/// revive that row and `set_claude_session_id` would overwrite its id, losing
/// the conversation `restore_host_sessions` could bring back. Runs before any
/// tmux call and before the work/shell split.
pub(crate) fn reject_lost_session_name(
    s: &Store,
    host_alias: &str,
    name: &str,
) -> Result<(), IpcError> {
    match s.lost_resumable_session_named(host_alias, name)? {
        Some(row) => Err(IpcError::new(
            codes::E_EXISTS,
            format!(
                "{name} belongs to a lost session (id {}); restore it with restore_host_sessions or dismiss it first",
                row.id
            ),
        )),
        None => Ok(()),
    }
}

/// Refuse to resume a conversation some session on the host already holds
/// (live or lost): two panes on one transcript would interleave, and a lost
/// holder should be brought back by `restore_host_sessions` instead.
pub(crate) fn reject_held_conversation(
    s: &Store,
    host_alias: &str,
    claude_id: &str,
) -> Result<(), IpcError> {
    match s.session_with_claude_id(host_alias, claude_id)? {
        Some(row) => Err(IpcError::new(
            codes::E_EXISTS,
            format!(
                "conversation {claude_id} already belongs to session {} ({}); restore it with restore_host_sessions instead",
                row.id, row.tmux_name
            ),
        )),
        None => Ok(()),
    }
}

/// The Claude conversation id a new session runs under, and its pane command.
/// Work/review sessions get an app-minted id so a later recreate/restart
/// resumes THIS conversation, not "most recent for the cwd" — or, with
/// `resume_claude_session_id`, the given conversation, launched exactly as
/// `recreate_pane_command` would. A shell session has no id.
pub(crate) fn claude_id_and_pane_cmd(args: &NewSessionArgs) -> (Option<String>, String) {
    if args.kind.as_deref() == Some("shell") {
        return (
            None,
            crate::tmux::shell_pane_command(args.start_command.as_deref()),
        );
    }
    match args.resume_claude_session_id.as_deref() {
        Some(id) => (
            Some(id.to_string()),
            recreate_pane_command("work", Some(id), &args.name),
        ),
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            let pane = crate::tmux::pane_command_for(Some(&id), &args.name);
            (Some(id), pane)
        }
    }
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
        let (name, _, _, _) = fetch_worktree(s, wid)?;
        (name != "main").then_some(name)
    } else {
        None
    };
    // `derive_tmux_name` (`discover.rs`) mints the same `dev-<owner>-<repo>`
    // / `dev-<owner>-<repo>--<worktree>` shape from a worktree key, "main"
    // meaning the repo root — the same convention `wt: None` encodes here.
    // `tmux_safe` is idempotent (it only ever removes `.`/`:`), so applying
    // it again after appending `term` is safe.
    let worktree_key = wt.as_deref().unwrap_or("main");
    let deterministic = tmux_safe(&format!(
        "{}{term}",
        derive_tmux_name(&owner, &repo, worktree_key)
    ));
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

/// The pane cwd for a SYSTEM project (`Some`), or `None` for an ordinary one.
///
/// A system project (the UX agent's operator dir) is not a repository: its
/// `base_path` is the cwd on the host the session is for, resolved there by
/// the service that made it (`operator::ensure_operator` → `resolve_dir`).
/// So there is nothing to clone on a remote host, nothing to repair, and no
/// worktree to make — a worktree request against it is a caller error, not
/// something to guess a path for.
pub(super) fn system_project_cwd(
    s: &Store,
    project_id: i64,
    worktree_id: Option<i64>,
    new_worktree: Option<&str>,
) -> Result<Option<String>, IpcError> {
    let Some(path) = fetch_system_base_path(s, project_id)? else {
        return Ok(None);
    };
    if worktree_id.is_some() || new_worktree.is_some() {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("project {project_id} is a system project without a repository; it has no worktrees"),
        ));
    }
    Ok(Some(path))
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
    // A SYSTEM project is neither: its `base_path` is the cwd on either kind
    // of host, and there is no repository to clone or repair.
    crate::service::hub::ensure_local_allowed(&args.host_alias)?;
    let fixed: Option<String> = {
        let s = lock(store)?;
        system_project_cwd(
            &s,
            args.project_id,
            args.worktree_id,
            args.new_worktree.as_deref(),
        )?
    };
    let path: PathBuf = if let Some(p) = fixed.clone() {
        PathBuf::from(p)
    } else if args.host_alias == "local" {
        if let Some(ref name) = args.new_worktree {
            // NEW WORKTREE: create branch + worktree, return the new dir.
            let base_path = {
                let s = lock(store)?;
                fetch_base_path(&s, args.project_id)?
            };
            PathBuf::from(
                create_worktree_local(&base_path, name, args.base_branch.as_deref()).await?,
            )
        } else {
            let s = lock(store)?;
            if let Some(wid) = args.worktree_id {
                // A worktree row names a checkout on ONE host — refuse a
                // remote host's row here just as the remote arm below refuses
                // a foreign one, so a mismatched `host_alias: "local"` call
                // can't turn into a pane cwd that doesn't exist on this
                // machine.
                let (name, _, row_host, path) = fetch_worktree(&s, wid)?;
                reject_foreign_worktree(&args.host_alias, &row_host, &name)?;
                PathBuf::from(path)
            } else {
                PathBuf::from(fetch_base_path(&s, args.project_id)?)
            }
        }
    } else {
        // Remote path: derive from owner/repo, then ensure-on-remote.
        if let Some(ref name) = args.new_worktree {
            // NEW WORKTREE on remote: ensure clone exists, then create worktree.
            let (owner, repo) = {
                let s = lock(store)?;
                fetch_owner_repo(&s, args.project_id)?
            };
            let home = ssh.remote_home(&args.host_alias).await?;
            let (project_root, _) = {
                let s = lock(store)?;
                remote_project_path_for(&s, &args.host_alias, &home, &owner, &repo, None)
            };
            ensure_remote_project(
                &**ssh,
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
                    codes::E_GIT_SETUP,
                    String::from_utf8_lossy(&out.stderr).trim().to_string(),
                ));
            }
            PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            let (owner, repo, wt_info) = {
                let s = lock(store)?;
                let (owner, repo) = fetch_owner_repo(&s, args.project_id)?;
                let wt = if let Some(wid) = args.worktree_id {
                    // (name, branch, host_alias, path) — the row names a
                    // checkout on ONE host; refuse it before it's used to
                    // derive anything for a different target host.
                    let (name, branch, row_host, path) = fetch_worktree(&s, wid)?;
                    reject_foreign_worktree(&args.host_alias, &row_host, &name)?;
                    Some((name, branch, path))
                } else {
                    None
                };
                (owner, repo, wt)
            };
            let home = ssh.remote_home(&args.host_alias).await?;
            // `remote_project_path_for`'s project root does not depend on the
            // worktree name (only its discarded second return value, the
            // `.claude/worktrees/<name>` cwd guess, does) — pass `None`.
            let (project_root, _) = {
                let s = lock(store)?;
                remote_project_path_for(&s, &args.host_alias, &home, &owner, &repo, None)
            };
            // The pane's cwd: for an existing non-main worktree, the row's
            // own scanned path (it may live under `.worktrees/` or anywhere
            // else git has it registered) — NOT the `.claude/worktrees/<name>`
            // guess `remote_project_path_for` makes. For `main` / no worktree,
            // the project root.
            let cwd = match &wt_info {
                Some((name, _, path)) if name != "main" => path.clone(),
                _ => project_root.clone(),
            };
            let worktree_for_clone = wt_info.as_ref().map(|(name, branch, path)| RemoteWorktree {
                name,
                branch: branch.as_deref(),
                path,
            });
            ensure_remote_project(
                &**ssh,
                &args.host_alias,
                &owner,
                &repo,
                &project_root,
                worktree_for_clone.as_ref(),
                token,
            )
            .await?;
            PathBuf::from(cwd)
        }
    };
    // A "shell" session runs a plain login shell in the pane instead of
    // Claude Code. Any other value (incl. None) is treated as a "work" session.
    let is_shell = args.kind.as_deref() == Some("shell");
    let (claude_id, pane_cmd) = claude_id_and_pane_cmd(&args);

    // Automatic self-repair for an EXISTING worktree row / main checkout: the
    // row may point at a directory that was deleted since it was written.
    // Automatic means create-only (re-add a missing worktree from its existing
    // branch); anything more returns E_REPAIR_REQUIRED before tmux starts.
    // A brand-new worktree was just created by `worktree_add_script`, and a
    // system project has no repository to check.
    let (path, repaired) = if args.new_worktree.is_none() && fixed.is_none() {
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

    // The name is live again — a kill of it moments ago must not make the
    // reconcile below refuse to insert the row this function returns.
    record_tmux_created(store, &args.host_alias, &args.name);
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
    let s = lock(store)?;
    let row = s
        .list_sessions_for_host(&args.host_alias)?
        .into_iter()
        .find(|r| r.tmux_name == args.name)
        .ok_or_else(|| {
            IpcError::new(
                codes::E_INTERNAL,
                format!(
                    "session {} on {} vanished after creation",
                    args.name, args.host_alias
                ),
            )
        })?;

    let worktree_id =
        link_new_session_worktree(&s, row.id, &args, &path.to_string_lossy(), fixed.is_some())?;
    let derived_friendly = derive_friendly_name(&s, &args, worktree_id.or(row.worktree_id))?;
    finalize_new_session(
        &s,
        row.id,
        &args.host_alias,
        &args.name,
        derived_friendly.as_deref(),
        claude_id.as_deref(),
        is_shell,
    )
}

/// Point a new session's row at the worktree it was started in: the one
/// named by `worktree_id`, or the one `new_worktree` just created (its row
/// upserted for the session's host, keyed by name, the branch the worktree
/// script made). Reconcile only ever sets `worktree_key`, never this FK, and
/// tidy-up, safe kill and the idle killer inspect a work session's tree only
/// through it: without it a started session could only ever be archived.
/// A system project (`fixed`) has no worktree. Returns the linked id.
pub(super) fn link_new_session_worktree(
    s: &Store,
    row_id: i64,
    args: &NewSessionArgs,
    cwd: &str,
    fixed: bool,
) -> Result<Option<i64>, IpcError> {
    if fixed {
        return Ok(None);
    }
    let wid = match (args.worktree_id, args.new_worktree.as_deref()) {
        (Some(w), _) => w,
        (None, Some(name)) => {
            s.upsert_worktree_on(&args.host_alias, args.project_id, name, cwd, Some(name))?
        }
        (None, None) => return Ok(None),
    };
    s.link_session_worktree(row_id, wid)?;
    Ok(Some(wid))
}

/// The writes `new_session` makes after the session exists, then ONE re-read
/// so the returned row is the row as of the last write (the frontend merges
/// it optimistically and orders it by `row_version`). Soft-fails the
/// cosmetic writes (`started_at`, friendly name, claude id) with a warning;
/// the `kind` tag and the final read are hard failures.
pub(super) fn finalize_new_session(
    s: &Store,
    row_id: i64,
    host_alias: &str,
    name: &str,
    friendly_name: Option<&str>,
    claude_id: Option<&str>,
    is_shell: bool,
) -> Result<SessionRow, IpcError> {
    // PROD-5: the fleet created this session now.
    if let Err(e) = s.set_started_at(row_id, now_unix()) {
        tracing::warn!(session = %name, error = %e, "[new_session] storing started_at failed");
    }
    if let Some(value) = friendly_name {
        if let Err(e) = s.set_friendly_name(host_alias, name, Some(value)) {
            tracing::warn!(session = %name, error = %e, "[new_session] storing friendly_name failed");
        }
    }
    if is_shell {
        s.set_session_kind(row_id, "shell", None)?;
    } else if let Some(cid) = claude_id {
        // Soft-fail: the session is live; a failed write just means a future
        // recreate falls back to `cl --continue`.
        if let Err(e) = s.set_claude_session_id(row_id, cid) {
            tracing::warn!(session = %name, error = %e, "[new_session] storing claude_session_id failed");
        }
    }
    s.get_session_by_id(row_id)?
        .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "session vanished after creation"))
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
        let (name, branch, _, _) = fetch_worktree(s, wid)?;
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

#[derive(Serialize, Deserialize)]
pub struct KillSessionArgs {
    pub host_alias: String,
    pub name: String,
    /// Override the controller self-target guard.
    #[serde(default)]
    pub force: bool,
}

/// Message for any attempt to stop an `external` row (an interactive Claude
/// session fleet merely observes).
pub(super) const EXTERNAL_STOP_REFUSED: &str =
    "this Claude session runs outside fleet; close it where it runs";

/// Decide what `kill_session` does for a pane-less `bg:<id>` row, given a
/// fresh `claude agents` listing for its host. Pure so every branch is
/// unit-testable without ssh:
///
/// - `external` rows are never stopped by fleet → `E_INVALID_STATE`;
/// - the agent is absent from the listing → `Ok(None)`: already gone, the
///   caller skips `claude stop` and reconcile prunes the row;
/// - the agent is listed with a valid job id → `Ok(Some(job_id))`;
/// - the agent is listed without one → `E_INVALID_STATE`, pointing the user
///   at Remove from list (there is nothing `claude stop` can address).
pub(super) fn bg_stop_target(
    kind: &str,
    agents: &[crate::claude_agents::ClaudeAgentRow],
    claude_session_id: &str,
) -> Result<Option<String>, IpcError> {
    if kind == "external" {
        return Err(IpcError::new(codes::E_INVALID_STATE, EXTERNAL_STOP_REFUSED));
    }
    let Some(agent) = crate::claude_agents::find_by_session_id(agents, claude_session_id) else {
        return Ok(None);
    };
    match &agent.job_id {
        Some(job) => Ok(Some(job.clone())),
        None => Err(IpcError::new(
            codes::E_INVALID_STATE,
            "this background agent has no job id to stop; use Remove from list",
        )),
    }
}

/// What `kill_session` does with a pane-less `bg:<id>` row.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum BgKillAction {
    /// An inactive agent (stored `claude_status = stopped`, its daemon is
    /// gone): skip `claude stop` and remove the row from the list.
    Dismiss,
    /// A live agent: `claude stop <job_id>`.
    Stop(String),
    /// A live row whose agent is no longer listed: already gone.
    Nothing,
}

/// Whether deciding a kill needs a fresh `claude agents` listing: only a live
/// (non-`stopped`) `bg` row does. External rows are refused and inactive
/// agents are dismissed without touching the host.
pub(super) fn bg_kill_needs_listing(kind: &str, claude_status: Option<&str>) -> bool {
    kind == "bg" && claude_status != Some("stopped")
}

/// Decide the kill of a pane-less row from its `kind`, stored
/// `claude_status` and (for live rows) the host's `claude agents` listing.
/// Pure so every branch is unit-testable without ssh:
///
/// - `external` → refused (`E_INVALID_STATE`), whatever its status;
/// - `bg` + `stopped` → [`BgKillAction::Dismiss`] (a dead daemon cannot
///   answer `claude stop`, and the listed agent would be re-imported);
/// - otherwise the [`bg_stop_target`] decision: `Stop(job)` or `Nothing`.
pub(super) fn bg_kill_action(
    kind: &str,
    claude_status: Option<&str>,
    agents: &[crate::claude_agents::ClaudeAgentRow],
    claude_session_id: &str,
) -> Result<BgKillAction, IpcError> {
    if kind == "external" {
        return Err(IpcError::new(codes::E_INVALID_STATE, EXTERNAL_STOP_REFUSED));
    }
    if claude_status == Some("stopped") {
        return Ok(BgKillAction::Dismiss);
    }
    Ok(match bg_stop_target(kind, agents, claude_session_id)? {
        Some(job) => BgKillAction::Stop(job),
        None => BgKillAction::Nothing,
    })
}

/// Record a kill: the `killed` timeline event and the end of the row's
/// current conversation (`end_reason = killed`). Best-effort, like every
/// timeline write; the rows go with the session row (`ON DELETE CASCADE`).
/// Announce to the store that `tmux_name` is alive on `host_alias` again,
/// because fleet has just created (or renamed into) that tmux session. Every
/// path that does so and then leans on its own reconcile to INSERT the row
/// must call this first: a kill of the same name in the same second would
/// otherwise make the reconcile refuse the insert, and the caller would be
/// left with a live tmux session and no row to return.
///
/// Best-effort and lock-safe: the guard is taken and dropped inside, never
/// held across an await, and a poisoned lock only costs one refused cycle.
pub(crate) fn record_tmux_created(store: &Mutex<Store>, host_alias: &str, tmux_name: &str) {
    if let Ok(s) = store.lock() {
        s.forget_kill(host_alias, tmux_name);
    }
}

pub(crate) fn record_kill(s: &Store, id: i64, claude_session_id: Option<&str>) {
    if let Err(e) = s.insert_session_event(id, "killed", None) {
        tracing::warn!(session_id = id, error = %e, "[event] insert killed failed");
    }
    if let Some(cid) = claude_session_id {
        let _ = s.close_conversation(id, cid, "killed");
    }
}

pub async fn kill_session(
    args: KillSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<i64, IpcError> {
    let deps = super::reconcile::ReconcileDeps::real(ssh, super::reconcile::local_host(store));
    kill_session_with(args, store, ssh, &deps).await
}

/// [`kill_session`] with an injectable tmux executor + reconcile deps, so the
/// kill → reconcile sequence is testable against a fake host. The tmux kill
/// and the follow-up single-host reconcile both go through `deps`; the
/// pane-less (`bg:`) branch still talks to the host over `ssh` directly.
pub(super) async fn kill_session_with(
    args: KillSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    deps: &super::reconcile::ReconcileDeps,
) -> Result<i64, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    // Lookup form: synthetic `bg:<uuid>` rows are killable too (via
    // `claude stop`, below) — only real tmux rows go through tmux.
    crate::validate::tmux_name_lookup(&args.name)?;
    {
        let s = lock(store)?;
        crate::service::operator::refuse_if_operator(
            &s,
            &args.host_alias,
            &args.name,
            "kill_session",
        )?;
    }
    // Look up id BEFORE killing so we can return it after. Read the controller
    // under the same lock and refuse to nuke ourselves unless forced.
    let (id, kind, claude_sid, claude_status) = {
        let s = lock(store)?;
        guard_not_controller(
            s.get_controller()?.as_ref(),
            &args.host_alias,
            &args.name,
            args.force,
        )?;
        s.get_session(&args.name, &args.host_alias)?
            .map(|r| (r.id, r.kind, r.claude_session_id, r.claude_status))
            .ok_or_else(|| {
                IpcError::new(
                    codes::E_NOTFOUND,
                    format!("session {} not found", args.name),
                )
            })?
    };
    if args.name.starts_with("bg:") {
        // A pane-less agent row — there is no tmux pane to kill. An
        // `external` row (an interactive session outside fleet) is refused
        // before any ssh / CLI call; an inactive `bg` agent is removed from
        // the list without `claude stop` (its daemon is gone).
        let sid = claude_sid
            .filter(|c| !c.trim().is_empty())
            .unwrap_or_else(|| args.name.trim_start_matches("bg:").to_string());
        let status = claude_status.as_deref();
        let agents = if bg_kill_needs_listing(&kind, status) {
            exec_for(&args.host_alias, ssh)
                .list_claude_agents()
                .await
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        match bg_kill_action(&kind, status, &agents, &sid)? {
            BgKillAction::Dismiss => {
                let s = lock(store)?;
                record_kill(&s, id, Some(&sid));
                // Records the dismissal and deletes the row, so there is
                // nothing left for a reconcile pass to prune.
                s.dismiss_agent(&args.host_alias, &sid, now_unix())?;
                return Ok(id);
            }
            // `claude stop` is idempotent, so a job that exits in between is
            // not an error.
            BgKillAction::Stop(job) => {
                crate::claude_cli::claude_stop(ssh, &args.host_alias, &job).await?;
            }
            // No longer listed: already gone; the reconcile below prunes it.
            BgKillAction::Nothing => {}
        }
        if let Ok(s) = store.lock() {
            record_kill(&s, id, Some(&sid));
        }
        super::reconcile::reconcile_one_host_with(store, deps, &args.host_alias).await?;
        return Ok(id);
    }
    let tmux = (deps.exec)(&args.host_alias);
    tmux.kill_session(&args.name).await?;
    // Task G: record the kill before reconcile reaps the row. Best-effort.
    // Then ghost the row as fleet's OWN kill (`lost_reason='killed'`)
    // before the reconcile below probes the host: tmux exits with its last
    // session, so killing a host's only session makes that probe see no
    // tmux server — a `tmux_server_gone` verdict that would otherwise mark
    // this row a resumable mass loss and keep it for the lost-session TTL.
    // A ghost row is out of the verdict's reach and reaps on the ordinary
    // one-cycle schedule. Best-effort: on failure the reconcile below
    // still ghosts the row, as before.
    if let Ok(s) = store.lock() {
        record_kill(&s, id, claude_sid.as_deref());
        if let Err(e) = s.mark_session_killed(id, now_unix()) {
            tracing::warn!(session_id = id, error = %e, "[kill] marking the killed row failed");
        }
    }
    super::reconcile::reconcile_one_host_with(store, deps, &args.host_alias).await?;
    Ok(id)
}

#[derive(Serialize, Deserialize)]
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
    // The operator's identity IS `(host, tmux name)`, so a rename does not
    // dodge the self-guard once — it destroys it for good. See
    // `service::operator::refuse_if_operator`.
    {
        let s = lock(store)?;
        crate::service::operator::refuse_if_operator(
            &s,
            &args.host_alias,
            &args.old_name,
            "rename_session",
        )?;
        // tmux would accept the new name (a lost session is not in tmux),
        // and the row carry-over below would then dismiss the lost row —
        // its timeline, participant and restore entry. Refuse first.
        reject_lost_session_name(&s, &args.host_alias, &args.new_name)?;
    }
    let tmux = exec_for(&args.host_alias, ssh);
    tmux.rename_session(&args.old_name, &args.new_name).await?;
    // The session now answers to `new_name`, which may be a name fleet killed
    // a moment ago; the reconcile below must be free to insert it.
    record_tmux_created(store, &args.host_alias, &args.new_name);
    // Carry the row over to the new name BEFORE the reconcile: that pass
    // keys rows on the tmux name, and left alone it would insert a new row
    // and reap this one — its id, participant (inbox, address), timeline
    // and conversations with it.
    {
        let s = lock(store)?;
        s.rename_session_row(&args.host_alias, &args.old_name, &args.new_name, now_unix())?;
    }
    reconcile_one_host(store, ssh, &args.host_alias).await?;
    let s = lock(store)?;
    // `new_name` is validated verbatim (no padding), so look it up as-is —
    // consistent with kill_session / restart_session.
    s.get_session(&args.new_name, &args.host_alias)?
        .ok_or_else(|| {
            IpcError::new(
                codes::E_NOTFOUND,
                format!(
                    "renamed session {} on {} did not appear in list",
                    args.new_name, args.host_alias
                ),
            )
        })
}

#[derive(Serialize, Deserialize)]
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
    let s = lock(store)?;
    s.set_friendly_name(&args.host_alias, &args.tmux_name, value)?
        .ok_or_else(|| {
            IpcError::new(
                codes::E_NOTFOUND,
                format!(
                    "session {} not found on {}",
                    args.tmux_name, args.host_alias
                ),
            )
        })
}

#[derive(Serialize, Deserialize)]
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
    // EXEMPT from `service::operator::refuse_if_operator`, deliberately, and
    // alone among the session-addressed operations. This IS the agent
    // panel's `lost` recovery — `restartOperator()` in `src/lib/operator.ts`
    // calls straight through here — so a guard would make the agent refuse
    // the one button that brings it back. It is also not destructive: the
    // row, the transcript and the conversation all survive a restart, which
    // is what separates it from kill / move / rename / recreate. If this
    // ever does need guarding, give the operator path a bypass FIRST.
    // Pinned by `service::operator::tests::
    // restart_session_is_deliberately_exempt_from_the_guard`.
    //
    // Respawn the pane with the command matching the session's kind so a
    // restarted shell session comes back as a shell, not a Claude pane. Read
    // the controller under the same lock and refuse to restart ourselves
    // unless forced.
    let (kind, claude_id, session_id) = {
        let s = lock(store)?;
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
    let pane_cmd: String = recreate_pane_command(&kind, claude_id.as_deref(), &args.name);
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
    // Any of the three branches leaves a live tmux session under this name,
    // and the create branch may even have rebuilt it from nothing.
    record_tmux_created(store, &args.host_alias, &args.name);
    reconcile_one_host(store, ssh, &args.host_alias).await?;
    let s = lock(store)?;
    s.get_session(&args.name, &args.host_alias)?.ok_or_else(|| {
        IpcError::new(
            codes::E_NOTFOUND,
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
pub(crate) fn recreate_pane_command(
    kind: &str,
    claude_session_id: Option<&str>,
    tmux_name: &str,
) -> String {
    if kind == "shell" {
        return crate::tmux::shell_pane_command(None);
    }
    let id = claude_session_id.filter(|id| crate::validate::claude_session_id(id).is_ok());
    crate::tmux::pane_command_for(id, tmux_name)
}

#[derive(Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "RecreateSessionParams")]
pub struct RecreateSessionArgs {
    /// Fleet session id.
    pub session_id: i64,
    /// Recreate even the fleet controller.
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
        let s = lock(store)?;
        let sess = s
            .get_session_by_id(args.session_id)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, "session not found"))?;
        // Refuse to nuke-and-rebuild ourselves unless forced.
        guard_not_controller(
            s.get_controller()?.as_ref(),
            &sess.host_alias,
            &sess.tmux_name,
            args.force,
        )?;
        // Addressed by `session_id` rather than `(host, name)`, and
        // `confirm: false` — which is exactly the shape the self-guard
        // exists for: without this the agent can end its own conversation
        // with no dialog in the way. Resolved from the row that was just
        // read, so the identifier used to reach the session does not matter.
        crate::service::operator::refuse_if_operator(
            &s,
            &sess.host_alias,
            &sess.tmux_name,
            "recreate_session",
        )?;
        let host = s
            .get_host_row(&sess.host_alias)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, "host not found"))?;
        if !host.reachable {
            return Err(IpcError::new(
                codes::E_HOST_OFFLINE,
                format!("host {} is not reachable", host.alias),
            ));
        }
        let cwd_src = cwd_source_for_session(&s, &sess)?;
        let pane_cmd = recreate_pane_command(
            &sess.kind,
            sess.claude_session_id.as_deref(),
            &sess.tmux_name,
        );
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
    // `restore_session` below keeps the row, so nothing here needs an INSERT
    // — but every `tmux new-session` this service runs ends with the name
    // alive, and saying so uniformly is what keeps the invariant checkable.
    record_tmux_created(store, &sess.host_alias, &sess.tmux_name);

    // Mark the row live again and return it.
    let row = {
        let s = lock(store)?;
        let row = s
            .restore_session(sess.id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "session vanished after restore"))?;
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

#[derive(Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "SessionIdParams")]
pub struct DismissGhostSessionArgs {
    /// Fleet session id.
    pub session_id: i64,
}

pub fn dismiss_ghost_session(
    args: DismissGhostSessionArgs,
    store: &Mutex<Store>,
) -> Result<(), IpcError> {
    let s = lock(store)?;
    let sess = s
        .get_session_by_id(args.session_id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, "session not found"))?;
    if sess.status != "ghost" {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!(
                "session {} is not a ghost (status={})",
                sess.id, sess.status
            ),
        ));
    }
    s.delete_session(sess.id)?;
    Ok(())
}
