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
    /// Claude model for the new session (`claude --model`): an alias such as
    /// `opus` / `sonnet[1m]` or a full model id. `None` / empty = the host's
    /// default. Rejected for a `"shell"` session.
    #[serde(default)]
    pub model: Option<String>,
    /// Reasoning effort for the new session (one of
    /// [`crate::validate::EFFORT_LEVELS`]). `None` / empty = the host's
    /// default. Rejected for a `"shell"` session.
    #[serde(default)]
    pub effort: Option<String>,
    /// Credential profile for the new session: `claude` runs with
    /// `CLAUDE_CONFIG_DIR=~/.claude-profiles/<profile>` on the host, so it
    /// bills that profile's `/login` (docs/accounts.md). A profile with no
    /// login yet asks for one in the pane. `None` / empty = the host's own
    /// login. Rejected for a `"shell"` session.
    #[serde(default)]
    pub profile: Option<String>,
    /// Which agent runs in the pane (`sessions.agent`, migration 121):
    /// `"claude"` (the default), `"codex"` (OpenAI's Codex CLI), `"agy"`
    /// (Google's Antigravity CLI) or `"shell"`, which is the same as
    /// `kind: "shell"`. `None` / empty = Claude Code, or a shell for
    /// `kind: "shell"`. `"agy"` is refused (`E_UNSUPPORTED`) until its
    /// adapter is validated.
    #[serde(default)]
    pub agent: Option<String>,
    /// Who or what is starting the session (migration 124): set by the
    /// caller in Rust, like `owner_person_id` and for the same reason never
    /// read from the request, so a client cannot label its own session a
    /// mission's. `None` = a person: `finalize_new_session` records
    /// [`SessionOrigin::person`] with `owner_person_id`.
    #[serde(skip_deserializing)]
    pub origin: Option<crate::store::SessionOrigin>,
    /// The person was asked about `accounts.pause_at` (redesign step 4.4)
    /// and chose to start on that login anyway. The MCP / hub `new_session`
    /// refuses a login at or past the line without it
    /// (`E_CONFIRM_REQUIRED`, naming the login with headroom); the
    /// desktop's own dialog asks first and sets it.
    #[serde(default)]
    pub over_limit_ok: bool,
    /// Whose session this is to be (multi-user M1, T5): the `people` row the
    /// new row is owned by, and therefore `private` to. `None` leaves it
    /// `unclaimed` — the safe holding state (spec §4.3), never "everybody's".
    ///
    /// **`#[serde(skip_deserializing)]`, and that is the point.** A hub client
    /// must not be able to name an owner over the wire: whose a session is
    /// follows from the CONNECTION (`mcp::tools::fleet::owner_for`, which maps
    /// a `Caller` to its person) or from the SOURCE row a create path derives
    /// from, never from a field in the request. Every caller that may set it
    /// does so in Rust, after the request has been authenticated;
    /// `HubBackend::new_session` deliberately does not send it, and
    /// `backend::tests_routing` asserts it is absent from the wire.
    ///
    /// It is read ONCE on the way through, after the row exists: the
    /// hard-failing [`Store::claim_if_unclaimed`] in `finalize_new_session`.
    /// There is no second, name-keyed mechanism ahead of the tmux create any
    /// more — see the note on `finalize_new_session` and the long one in
    /// `store::reconcile` for why that shape cannot be made sound.
    #[serde(skip_deserializing)]
    pub owner_person_id: Option<i64>,
    /// An opaque id the caller minted (redesign step 5.13): when set, the
    /// start reports its worktree, tmux and agent steps as `start:progress`
    /// frames carrying this token and nothing else, so the client that
    /// started the session can drive its Pulse sequence from them. Unlike
    /// `call_id` it crosses to a hub: the frames reach a paired desktop on
    /// the hub's `/events` stream. 1–64 of `[A-Za-z0-9_-]`.
    #[serde(default)]
    pub start_token: Option<String>,
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
/// `.claude/worktrees/` (auto-detected, fallback `.worktrees/`), in the
/// directory [`crate::projects::worktree_dir_name`] gives (`feat/imports` →
/// `feat-imports`; the branch keeps its `/`). Idempotent:
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
         dir={dir}\n\
         basebr={basebr}\n\
         {WORKTREE_BASE_SNIPPET}\
         wt=\"$base/$dir\"\n\
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
        dir = quote(&crate::projects::worktree_dir_name(name)),
    )
}

/// Where a new worktree goes under a repo root (the script's cwd): the
/// repo's `.worktrees/` unless it already uses `.claude/worktrees/`. Sets
/// `$base`. Shared by [`worktree_add_script`] and the fork's own
/// `service::rewind::fork_worktree_script`, so a forked checkout lands
/// exactly where a `new_session { new_worktree }` one would.
pub(crate) const WORKTREE_BASE_SNIPPET: &str = "if [ -d .worktrees ]; then base=.worktrees\n\
     elif [ -d .claude/worktrees ]; then base=.claude/worktrees\n\
     else base=.worktrees\n\
     fi\n";

/// The name rule for a NEW worktree (and so its branch): a name git accepts
/// for a branch ([`crate::validate::branch_name`]), never `main` /
/// `master`. `new_session { new_worktree }` and a fork into a new worktree
/// both apply it.
pub(crate) fn validate_new_worktree_name(name: &str) -> Result<(), IpcError> {
    crate::validate::branch_name(name)?;
    if name == "main" || name == "master" {
        return Err(IpcError::new(
            codes::E_INVALID,
            "worktree name must not be 'main' or 'master'",
        ));
    }
    Ok(())
}

pub(super) async fn create_worktree_local(
    root: &str,
    name: &str,
    base: Option<&str>,
) -> Result<String, IpcError> {
    crate::service::hub::ensure_local_allowed(crate::service::projects::LOCAL_HOST)?;
    let script = worktree_add_script(root, name, base);
    let out = crate::proc::command("bash")
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
    worktree_path_from_stdout(&out.stdout)
}

/// The worktree path [`worktree_add_script`] printed: its LAST non-empty
/// line. The script runs under `bash -lc`, so a login banner (`/etc/profile`,
/// `~/.bash_profile`) prints ahead of it; taken whole, the banner became the
/// pane's cwd and the stored `worktrees.path`. Anything but an absolute path
/// is refused.
pub(super) fn worktree_path_from_stdout(stdout: &[u8]) -> Result<String, IpcError> {
    let text = String::from_utf8_lossy(stdout);
    let path = text
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .unwrap_or_default();
    if !path.starts_with('/') {
        return Err(IpcError::new(
            codes::E_GIT_SETUP,
            format!("git worktree add did not report a worktree path: {path:?}"),
        ));
    }
    Ok(path.to_string())
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
        let s = lock(store)?;
        reject_held_conversation(&s, &args.host_alias, id)?;
        reject_foreign_conversation(&s, id, args.owner_person_id)?;
    }
    normalize_launch(&mut args)?;
    {
        let s = lock(store)?;
        reject_lost_session_name(&s, &args.host_alias, &args.name, args.owner_person_id)?;
        reject_adoptable_session_name(&s, &args.host_alias, &args.name)?;
    }

    if let Some(name) = args.new_worktree.as_deref() {
        validate_new_worktree_name(name)?;
    }
    if let Some(t) = args.start_token.as_deref() {
        validate_start_token(t)?;
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

    // The steps are reported from inside; the outcome closes the last one
    // (or marks the one in flight failed) here, on every exit path.
    let progress = StartReporter::new(store, args.start_token.clone());
    let out = new_session_inner(args, store, ssh, token, &progress).await;
    match &out {
        Ok(_) => progress.finish(),
        Err(_) => progress.fail(),
    }
    out
}

/// Refuse a name that belongs to a lost session with a resumable
/// conversation: reconcile's upsert (ON CONFLICT host_alias, tmux_name) would
/// revive that row and `set_claude_session_id` would overwrite its id, losing
/// the conversation `restore_host_sessions` could bring back. Runs before any
/// tmux call and before the work/shell split.
///
/// It also refuses a name held by a lost row belonging to **another person**
/// (multi-user M1, T5), resumable or not. The same `ON CONFLICT DO UPDATE`
/// revives such a row with its `owner_person_id` intact, so without this
/// refusal person B's `new_session` under a name person A once used comes back
/// as a row owned by — and readable only to — A. `as_person` is the person the
/// caller is acting as (`None` for a caller that is nobody); the store's
/// comparison answers fail-closed for it, so a person-less caller is refused
/// by every owned lost row rather than inheriting one.
pub(crate) fn reject_lost_session_name(
    s: &Store,
    host_alias: &str,
    name: &str,
    as_person: Option<i64>,
) -> Result<(), IpcError> {
    if s.lost_session_named_owned_by_other(host_alias, name, as_person)?
        .is_some()
    {
        // Deliberately names neither the row nor the person: the caller may
        // not see either, and "pick another name" is the whole of what they
        // need to act on.
        return Err(IpcError::new(
            codes::E_EXISTS,
            format!(
                "{name} on {host_alias} belongs to another person's lost session; \
                 pick a different name"
            ),
        ));
    }
    // Multi-user M1 (T10): the ROW's id is not in the message. The name is
    // the caller's own argument and the advice is actionable without it —
    // `restore_host_sessions` takes a host, and the ids a caller may see come
    // from `discover_lost_sessions`, which is person-fenced. The owned-row
    // arm above already named neither the row nor the person; this arm
    // reached the same caller (`new_session`, before the work/shell split)
    // and was naming a row id, so the two arms disagreed about the same
    // question.
    if s.lost_resumable_session_named(host_alias, name)?.is_some() {
        return Err(IpcError::new(
            codes::E_EXISTS,
            format!(
                "{name} belongs to a lost session; restore it with \
                 restore_host_sessions or dismiss it first"
            ),
        ));
    }
    Ok(())
}

/// Refuse a name that ALREADY has a `sessions` row on this host, of any
/// status (multi-user M1, T5's review).
///
/// `new_session` inserts no row: it starts tmux, runs a reconcile pass, and
/// then finds "its" row **by name** (`list_sessions_for_host(...).find(|r|
/// r.tmux_name == args.name)`), which is the `row_id`
/// [`finalize_new_session`] claims. That lookup cannot tell the row this
/// create caused from a row that was already there, and a pre-existing row
/// under the same name reliably survives the create:
///
/// * a ghost / lost row is REVIVED by the reconcile upsert's `ON CONFLICT DO
///   UPDATE` (`status='running'`, `lost_at=NULL`) — the same row id, with its
///   `session_events` timeline, conversations, journal entries, work links and
///   tags still attached;
/// * a row still recorded `running` whose tmux session has in fact died takes
///   that same `DO UPDATE` branch without even a revival.
///
/// Either way the create would hand back, as the session it just made, a row
/// it did not make — and `claim_if_unclaimed` would stamp the caller as the
/// owner of somebody's abandoned history. So the create FAILS here instead of
/// adopting it, before any tmux call.
///
/// [`reject_lost_session_name`] runs first and keeps its two more specific
/// messages (another person's lost session, a restorable one). What is left
/// for this one is every other row: a live row, a ghost, an unclaimed lost
/// row with no conversation to restore. It names neither the row nor its
/// owner — the name is the caller's own argument and "pick another" is the
/// whole of what they can act on (T10).
///
/// Nothing legitimate is refused by it. A minted name already avoids every
/// row on the host ([`fill_session_name`] checks `list_sessions_for_host`), a
/// killed session's row is hard-deleted by the kill's own reconcile pass, and
/// a name whose tmux session is genuinely live could only ever have failed in
/// `tmux new-session` with `duplicate session` — later, and less legibly.
pub(crate) fn reject_adoptable_session_name(
    s: &Store,
    host_alias: &str,
    name: &str,
) -> Result<(), IpcError> {
    if s.get_session(name, host_alias)?.is_some() {
        return Err(IpcError::new(
            codes::E_EXISTS,
            format!("{name} on {host_alias} already names a session; pick a different name"),
        ));
    }
    Ok(())
}

/// Refuse to resume a conversation some session on the host already holds
/// (live or lost): two panes on one transcript would interleave, and a lost
/// holder should be brought back by `restore_host_sessions` instead.
pub(crate) fn reject_held_conversation(
    s: &Store,
    host_alias: &str,
    claude_id: &str,
) -> Result<(), IpcError> {
    // Multi-user M1 (T10): the HOLDER's id and tmux name are not in the
    // message. `claude_id` is the caller's own argument, so saying "something
    // on this host already holds it" tells them nothing they did not supply;
    // naming the row told them a private session's id and its tmux name,
    // which in this fleet is a branch or a ticket key — spec §4.3 content,
    // and the one thing this refusal must not disclose. The tool layer's
    // `require_conversation_person` normally refuses a foreign conversation
    // before this runs, but this function is also the desktop path's fence,
    // where there is no `Caller` to ask, and an `unclaimed` holder on a
    // multi-person hub reaches it either way.
    if s.session_with_claude_id(host_alias, claude_id)?.is_some() {
        return Err(IpcError::new(
            codes::E_EXISTS,
            format!(
                "conversation {claude_id} is already held by a session on \
                 {host_alias}; restore it with restore_host_sessions instead"
            ),
        ));
    }
    Ok(())
}

/// Multi-user M1 (T7, with T3's record): refuse to resume a conversation
/// that belonged to somebody else.
///
/// **The attack works precisely when the session row is gone**, which is why
/// a check against live rows cannot close it and why migration 099 records
/// `conversation_owners` durably: A's session is reaped (a kill, a GC
/// sweep, a `move_session`), B passes its `claude_session_id` to
/// `new_session`, and the resumed session replays A's whole transcript into
/// a pane B owns. `reject_held_conversation` above does not see it — there
/// is no live row holding that conversation any more — and nothing else on
/// this path asks whose it was.
///
/// The predicate has the shape every ownership comparison in M1 has, and for
/// the same reason: a conversation nobody is recorded against (`None`) must
/// never match a caller who is nobody (`None`). Written out rather than as
/// `recorded == owner`, so it cannot be read as an identity check.
///
/// A conversation with no recorded owner is allowed through — that is the
/// pre-M1 world and an `unclaimed` row's conversation, and refusing it would
/// make every pre-upgrade transcript unresumable (rule 7: the upgrade widens
/// nothing, and it must not narrow a single-person install into uselessness
/// either).
pub fn reject_foreign_conversation(
    s: &Store,
    claude_id: &str,
    owner: Option<i64>,
) -> Result<(), IpcError> {
    if conversation_owner_allows(s, claude_id, owner)? {
        return Ok(());
    }
    Err(IpcError::new(
        codes::E_FORBIDDEN,
        format!(
            "conversation {claude_id} belongs to another person; a transcript is \
             not resumed into somebody else's session"
        ),
    ))
}

/// The predicate half of [`reject_foreign_conversation`]: may `owner` take
/// over the conversation `claude_id`?
///
/// Written out because two callers need the ANSWER rather than this
/// function's refusal. `work_link { resume }` and `work_link { summarize }`
/// are addressed by a work key and a link id, never by a session
/// (multi-user M1, T7), so they must answer with the LINK's own
/// `E_NOTFOUND` — a link whose conversation is not this caller's reads
/// exactly like a link that does not exist — and not with an `E_FORBIDDEN`
/// that confirms both the link and whose it is.
///
/// Every rule the doc above states holds here, because this is where it is
/// written: a failed read is not a pass, `(None, None)` never matches, and a
/// conversation with no recorded owner is allowed through.
pub fn conversation_owner_allows(
    s: &Store,
    claude_id: &str,
    owner: Option<i64>,
) -> Result<bool, IpcError> {
    // A failed read is NOT a pass: a conversation whose owner cannot be
    // established is one this caller cannot show it owns.
    Ok(match s.conversation_owner(claude_id)? {
        None => true,
        Some(o) => matches!(owner, Some(p) if p == o),
    })
}

/// Blank `model` / `effort` mean the host's default (`None`); anything else
/// must be a valid value, and neither applies to a shell session. `agent`
/// folds into `kind` here ([`normalize_agent`]), so everything after this
/// reads a shell session from `kind` alone.
pub(crate) fn normalize_launch(args: &mut NewSessionArgs) -> Result<(), IpcError> {
    let trim = |v: &mut Option<String>| {
        *v = v
            .take()
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty());
    };
    trim(&mut args.agent);
    normalize_agent(args)?;
    trim(&mut args.model);
    trim(&mut args.effort);
    trim(&mut args.profile);
    if args.kind.as_deref() == Some("shell")
        && (args.model.is_some() || args.effort.is_some() || args.profile.is_some())
    {
        return Err(IpcError::new(
            codes::E_INVALID,
            "model, effort and profile apply to Claude sessions, not shell sessions",
        ));
    }
    if let Some(p) = args.profile.as_deref() {
        match args.agent.as_deref() {
            Some(crate::store::AGENT_CODEX) => {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    "a credential profile applies to Claude Code sessions; Codex has none",
                ))
            }
            Some(crate::store::AGENT_AGY) => {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    "a credential profile applies to Claude Code sessions; agy keeps its own login",
                ))
            }
            _ => {}
        }
        crate::validate::claude_profile(p)?;
    }
    if let Some(m) = args.model.as_deref() {
        crate::validate::claude_model(m)?;
    }
    if let Some(e) = args.effort.as_deref() {
        crate::validate::effort_level(e)?;
    }
    Ok(())
}

/// Settle `agent` against `kind`: `agent: "shell"` makes a shell session
/// (and conflicts with any other kind), `kind: "shell"` with no agent is a
/// shell, and no agent can run in a shell session. A shell row's `agent`
/// follows from `kind` (`Store::set_session_kind`), so an absent agent
/// stays absent here; a Codex or agy row is written by `new_session`.
/// An unknown name is invalid.
fn normalize_agent(args: &mut NewSessionArgs) -> Result<(), IpcError> {
    use crate::store::{AGENTS, AGENT_AGY, AGENT_CLAUDE, AGENT_CODEX, AGENT_SHELL};
    let shell_kind = args.kind.as_deref() == Some("shell");
    match args.agent.as_deref() {
        None => {}
        Some(AGENT_SHELL) => match args.kind.as_deref() {
            None | Some("shell") => args.kind = Some("shell".into()),
            Some(other) => {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!("agent shell runs a plain shell; it cannot start a {other} session"),
                ))
            }
        },
        Some(AGENT_CLAUDE | AGENT_CODEX | AGENT_AGY) if shell_kind => {
            return Err(IpcError::new(
                codes::E_INVALID,
                "a shell session runs no agent; drop agent or kind",
            ))
        }
        Some(AGENT_AGY) => refuse_unvalidated_agent(AGENT_AGY)?,
        Some(AGENT_CLAUDE | AGENT_CODEX) => {}
        Some(_) => {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("agent must be one of {}", AGENTS.join(", ")),
            ))
        }
    }
    Ok(())
}

/// Refuse to launch an agent whose adapter is not validated yet (redesign
/// 12.3): agy's adapter (`agent_adapter::agy`) is provisional — no captured
/// pane fixtures, its SQLite transcripts unread — so the New session picker
/// shows it as coming, and `new_session`, restart, recreate and repair
/// refuse it with `E_UNSUPPORTED` rather than start a pane fleet cannot
/// read. The adapter code stays; lifting this is one line once it is
/// validated against real captures.
pub(crate) fn refuse_unvalidated_agent(agent: &str) -> Result<(), IpcError> {
    if agent == crate::store::AGENT_AGY {
        return Err(IpcError::new(
            codes::E_UNSUPPORTED,
            "agy sessions are not supported yet: fleet cannot read agy's state or \
             transcripts until its adapter is validated; use claude or codex",
        ));
    }
    Ok(())
}

/// The conversation id a new session runs under, and its pane command.
/// Work/review sessions get an app-minted id so a later recreate/restart
/// resumes THIS conversation, not "most recent for the cwd" — or, with
/// `resume_claude_session_id`, the given conversation, launched exactly as
/// `recreate_pane_command` would. A shell session has no id, and neither
/// does a new session of an agent that picks its own (Codex).
pub(crate) fn claude_id_and_pane_cmd(args: &NewSessionArgs) -> (Option<String>, String) {
    let kind = args.kind.as_deref().unwrap_or("work");
    let Some(agent) = crate::agent_adapter::for_session(kind, args.agent.as_deref()) else {
        return (
            None,
            crate::tmux::shell_pane_command(args.start_command.as_deref()),
        );
    };
    // `model` / `effort` are also stored on the row (`store_launch`), so a
    // later recreate / restart / repair / move launches with them again.
    let launch = crate::tmux::ClaudeLaunch {
        model: args.model.clone(),
        effort: args.effort.clone(),
        profile: args.profile.clone(),
    };
    match args.resume_claude_session_id.as_deref() {
        Some(id) => (
            Some(id.to_string()),
            agent.launch_command(
                Some(id).filter(|id| agent.valid_conversation_id(id)),
                &args.name,
                &launch,
            ),
        ),
        None => match agent.mint_conversation_id() {
            Some(id) => {
                let pane = agent.launch_command(Some(&id), &args.name, &launch);
                (Some(id), pane)
            }
            None => (None, agent.start_command(&args.name, &launch)),
        },
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
    progress: &StartReporter<'_>,
) -> Result<SessionRow, IpcError> {
    use crate::events::StartStep;
    progress.advance(StartStep::Worktree);
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
            PathBuf::from(worktree_path_from_stdout(&out.stdout)?)
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

    progress.advance(StartStep::Tmux);
    let tmux = exec_for(&args.host_alias, ssh);
    tmux.new_session(&args.name, &path, &pane_cmd).await?;

    // AFTER the create, like every other create site (the one rule, and
    // `every_path_that_creates_a_tmux_session_forgets_the_kill_first` pins
    // it): the forget is what lets the reconcile below insert a row for a
    // name fleet killed moments ago, and a create that FAILED must not forget
    // a real kill — a pass still carrying the old name would re-insert the
    // dead row. Nothing rides this call any more besides the forget; the
    // owner reservation that used to make this site the inverted one is gone
    // (T5's review — see `store::reconcile`'s ownership note).
    record_tmux_created(store, &args.host_alias, &args.name);

    reconcile_one_host(store, ssh, &args.host_alias).await?;
    progress.advance(StartStep::Agent);
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
    if !is_shell {
        let launch = crate::tmux::ClaudeLaunch {
            model: args.model.clone(),
            effort: args.effort.clone(),
            profile: args.profile.clone(),
        };
        store_start_launch(&s, row.id, &args.name, &launch)?;
    }
    // Not soft: a Codex or agy pane on a row that says Claude would be resumed,
    // read and answered as Claude's.
    if let Some(agent) = args
        .agent
        .as_deref()
        .filter(|a| *a != crate::store::AGENT_CLAUDE && !is_shell)
    {
        s.set_session_agent(row.id, agent)?;
    }
    finalize_new_session(
        &s,
        row.id,
        &args.host_alias,
        &args.name,
        derived_friendly.as_deref(),
        claude_id.as_deref(),
        is_shell,
        args.owner_person_id,
        &args
            .origin
            .clone()
            .unwrap_or_else(|| crate::store::SessionOrigin::person(args.owner_person_id)),
    )
}

/// Point a new session's row at the worktree it was started in: the one
/// named by `worktree_id`, or the one `new_worktree` just created (its row
/// upserted for the session's host, keyed by name, the branch the worktree
/// script made). Reconcile only ever sets `worktree_key`, never this FK, and
/// tidy-up, safe kill and the idle killer inspect a work session's tree only
/// through it: without it a started session could only ever be archived.
/// A system project (`fixed`) has no worktree, and a `main` row (the clone
/// itself, never a `worktree add`) is never linked either — safe kill and
/// discard `git worktree remove` a linked tree, and the repo root is not
/// one; `discover_lost_sessions` hands `new_session` a main row's id for a
/// remote project root. Returns the linked id.
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
        (Some(w), _) => match s.get_worktree_row(w)? {
            Some(row) if row.name == "main" => return Ok(None),
            _ => w,
        },
        (None, Some(name)) => s.upsert_worktree_on(
            &args.host_alias,
            args.project_id,
            &crate::projects::worktree_dir_name(name),
            cwd,
            Some(name),
        )?,
        (None, None) => return Ok(None),
    };
    s.link_session_worktree(row_id, wid)?;
    Ok(Some(wid))
}

/// The writes `new_session` makes after the session exists, then ONE re-read
/// so the returned row is the row as of the last write (the frontend merges
/// it optimistically and orders it by `row_version`). Soft-fails the
/// cosmetic writes (`started_at`, friendly name, claude id) with a warning;
/// the `kind` tag, the OWNERSHIP claim and the final read are hard failures,
/// and so is the origin (migration 124): the origin chip is how a person
/// tells their own start from a mission's or an agent's.
// Nine arguments: the row's identity, the three values to write, the owner
// and the origin. Grouping them into a struct would mean a type whose only purpose is
// this one call, and every one of them is already named at the single call
// site.
#[allow(clippy::too_many_arguments)]
pub(super) fn finalize_new_session(
    s: &Store,
    row_id: i64,
    host_alias: &str,
    name: &str,
    friendly_name: Option<&str>,
    claude_id: Option<&str>,
    is_shell: bool,
    owner: Option<i64>,
    origin: &crate::store::SessionOrigin,
) -> Result<SessionRow, IpcError> {
    // Ownership (multi-user M1, T5), and a HARD failure — alone among the
    // writes here, which are all soft because "the session is live either
    // way". This one is not cosmetic: an unstamped row is `unclaimed`, which
    // means the person who just started the session cannot read their own row
    // (spec §4.3 serves an out-of-scope caller a count and nothing more) and
    // an agent that can prove the pane could claim it for somebody else.
    // Returning such a row as a success would be the privacy failure dressed
    // as one.
    //
    // It is also the ONLY mechanism that stamps an owner (T5's review). The
    // name-keyed intent reconcile used to read before the row existed is
    // deleted: a tmux name is not an identity, and three attempts to guard
    // that mechanism each moved its hole rather than closing it (the long
    // note in `store::reconcile` records all three). So every row the
    // reconcile upsert inserts is `unclaimed`, and this call does the work.
    //
    // **What that trades, written down rather than papered over.** The intent
    // existed to close the window between the reconcile pass inserting the row
    // and this claim: in that window the row is `unclaimed`, so an agent that
    // can prove the pane (spec rule 6) could claim it first. That window is
    // accepted, because it is not comparable to the hole it replaces:
    //
    // * the claimant must PROVE the pane — of a session that is still being
    //   created, in which no agent is yet running and no prompt has been sent;
    // * it is in-process and short: the pass that inserts the row and this
    //   claim are separated only by `list_sessions_for_host`, one worktree
    //   link and one friendly-name derivation, all under the same `Store`
    //   lock this function already holds (the pass that inserted the row had
    //   to release it, and nothing awaits in between);
    // * losing it is loud, not silent — the claim then meets a row owned by
    //   somebody else and `E_FORBIDDEN` fails the create, so the caller never
    //   gets handed a session that is not theirs.
    //
    // Whereas the intent's hole needed no race at all: naming an existing
    // `unclaimed` row, or a live tmux session fleet had no row for yet, was
    // enough to be stamped its owner by the next pass. A smaller, loud,
    // proof-gated window replaces a silent deterministic one.
    s.claim_if_unclaimed(row_id, owner)?;
    s.set_session_origin(row_id, origin)?;
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
/// It says nothing about WHOSE the next row under that name is, and there is
/// no sibling call that does (multi-user M1, T5's review). A create path
/// states its owner once, against a row id, through
/// [`Store::claim_if_unclaimed`] — `new_session` in [`finalize_new_session`],
/// `spawn_review` and `move_session` carrying their SOURCE row's owner. The
/// name-keyed reservation this function used to file alongside the forget is
/// deleted; `store::reconcile`'s ownership note says why a name cannot carry
/// that statement.
///
/// Best-effort and lock-safe: the guard is taken and dropped inside, never
/// held across an await, and a poisoned lock only costs one refused cycle.
pub(crate) fn record_tmux_created(store: &Mutex<Store>, host_alias: &str, tmux_name: &str) {
    if let Ok(s) = store.lock() {
        s.forget_kill(host_alias, tmux_name);
    }
}

/// The hub's own personal owner, for a create path that has no caller to ask
/// (multi-user M1, T5).
///
/// Several service-layer paths build a `NewSessionArgs` far from any request:
/// the operator session, the asset catalog's authoring session, a ticket start,
/// a work resume. None of them carries a `Caller` — the person is known at the
/// MCP / Tauri edge and the signature chain in between is long — and all of
/// them are "fleet, acting for whoever runs this hub". That is
/// `Store::personal_owner_id`, the same person `mcp::tools::fleet::owner_for`
/// maps the master token to.
///
/// `None` on a hub that cannot say who its owner is (T1's fail-closed rule),
/// which leaves the session `unclaimed` rather than guessing. A failed read is
/// logged and treated the same way: this must never be the reason a start
/// fails, and `unclaimed` is recoverable (the row can be claimed) where a
/// wrong owner is not.
///
/// It is **not** a substitute for a caller's own person. On a hub with several
/// people, a start made through one of these paths is the hub owner's; the
/// paths that DO have a caller (`mcp::tools::session_ops`, `dispatch_task`)
/// pass that caller's person instead, and a path that gains a caller later
/// should stop calling this.
pub fn hub_personal_owner(store: &Mutex<Store>) -> Option<i64> {
    let s = store.lock().ok()?;
    match s.personal_owner_id() {
        Ok(owner) => owner,
        Err(e) => {
            tracing::warn!(error = %e.message, "[owner] reading the hub's personal owner failed");
            None
        }
    }
}

pub(crate) fn record_kill(s: &Store, id: i64, claude_session_id: Option<&str>) {
    if let Err(e) = s.insert_session_event(id, "killed", None) {
        tracing::warn!(session_id = id, error = %e, "[event] insert killed failed");
    }
    if let Some(cid) = claude_session_id {
        let _ = s.close_conversation(id, cid, "killed");
    }
    if let Err(e) = s.cancel_forms_of_session(id) {
        tracing::warn!(session_id = id, error = %e, "[forms] cancel on kill failed");
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
        super::reconcile::reconcile_one_host_with(store, deps, &args.host_alias)
            .await
            .map_err(after_the_kill)?;
        return Ok(id);
    }
    let tmux = (deps.exec)(&args.host_alias);
    tmux.kill_session(&args.name).await?;
    // Its shell terminals (step 5.3) go with it. Best-effort: a terminal
    // left behind is no session row, only a stray tmux session.
    super::terminals::close_all_after_kill(&*tmux, &args.name).await;
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
    super::reconcile::reconcile_one_host_with(store, deps, &args.host_alias)
        .await
        .map_err(after_the_kill)?;
    Ok(id)
}

/// The `details` key [`kill_session`] sets on an error that came AFTER the
/// session was killed: the trailing reconcile failed, the kill did not.
pub const KILL_LANDED: &str = "kill_landed";

/// Mark a post-kill failure (review r18 M4): a caller that must know whether
/// the session is gone — `move_session`'s finalise — reads it with
/// [`kill_landed`] instead of taking every `Err` for "still alive".
pub(crate) fn after_the_kill(e: IpcError) -> IpcError {
    let mut details = match e.details {
        Some(serde_json::Value::Object(ref m)) => m.clone(),
        _ => serde_json::Map::new(),
    };
    details.insert(KILL_LANDED.into(), serde_json::Value::Bool(true));
    let details = serde_json::Value::Object(details);
    e.with_details(details)
}

/// Whether a [`kill_session`] error came after the kill itself succeeded.
pub fn kill_landed(e: &IpcError) -> bool {
    e.details
        .as_ref()
        .and_then(|d| d.get(KILL_LANDED))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
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
    let tmux = exec_for(&args.host_alias, ssh);
    rename_session_with(args, store, ssh, &*tmux).await
}

/// [`rename_session`] with its tmux executor as a parameter — the seam a
/// test drives the race below through with a fake.
pub(super) async fn rename_session_with(
    args: RenameSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    tmux: &dyn crate::tmux::TmuxExec,
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
        //
        // The person the guard compares against is the RENAMED row's own owner
        // (multi-user M1, T5): a rename is not a create path, it carries this
        // row over, so "another person" means a lost row that is not this
        // session's owner's. `rename_session_row` hard-DELETES the stale row
        // standing under the new name, which is exactly what must not happen
        // to a colleague's lost session.
        let as_person = s
            .get_session(&args.old_name, &args.host_alias)?
            .and_then(|r| r.owner_person_id);
        reject_lost_session_name(&s, &args.host_alias, &args.new_name, as_person)?;
    }
    tmux.rename_session(&args.old_name, &args.new_name).await?;
    // The session now answers to `new_name`, which may be a name fleet killed
    // a moment ago; the reconcile below must be free to insert it.
    record_tmux_created(store, &args.host_alias, &args.new_name);
    // Carry the row over to the new name BEFORE the reconcile: that pass
    // keys rows on the tmux name, and left alone it would insert a new row
    // and reap this one — its id, participant (inbox, address), timeline
    // and conversations with it.
    let carried = {
        let s = lock(store)?;
        s.rename_session_row(&args.host_alias, &args.old_name, &args.new_name, now_unix())
    };
    if let Err(e) = carried {
        // The store runs the lost-name guard once more, under its own lock:
        // a lost row named `new_name` can land between the check above and
        // here (the tmux rename runs with the lock released), and refusing
        // it is right — but tmux already answers to `new_name`. Put the
        // pane back before reporting, so it and the row still agree.
        if let Err(back) = tmux.rename_session(&args.new_name, &args.old_name).await {
            return Err(IpcError::new(
                &e.code,
                format!(
                    "{}; and tmux could not be renamed back from {} to {}: {}",
                    e.message, args.new_name, args.old_name, back.message
                ),
            ));
        }
        return Err(e);
    }
    // Its shell terminals (step 5.3) follow it to the new name.
    super::terminals::rename_after_rename(tmux, &args.old_name, &args.new_name).await;
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

/// `touch_session_viewed`: the session a person is looking at.
#[derive(Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "TouchSessionViewedParams")]
pub struct TouchSessionViewedArgs {
    /// The session on screen.
    pub session_id: i64,
}

/// Record that a person is looking at the session now (migration 125), so
/// every turn that has ended reads as seen (`done_unread` clears). Answers
/// the row as it now stands; a call that moves nothing still answers it.
pub fn touch_session_viewed(
    args: TouchSessionViewedArgs,
    store: &Mutex<Store>,
) -> Result<SessionRow, IpcError> {
    let s = lock(store)?;
    s.touch_session_viewed(args.session_id, now_unix())?;
    s.get_session_by_id(args.session_id)?.ok_or_else(|| {
        IpcError::new(
            codes::E_NOTFOUND,
            format!("session {} not found", args.session_id),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetSessionTagsArgs {
    pub session_id: i64,
    /// The full list (replaces; empty clears).
    pub tags: Vec<String>,
}

/// Validate a session's tags: at most 16, each 1–32 chars of
/// `[A-Za-z0-9_.:-]`, trimmed and de-duplicated in order. PURE; the MCP
/// `set_session_tags` and the desktop's Label field share it.
pub fn normalize_session_tags(tags: Vec<String>) -> Result<Vec<String>, IpcError> {
    if tags.len() > 16 {
        return Err(IpcError::new(
            codes::E_VALIDATE,
            "at most 16 tags per session",
        ));
    }
    let mut out: Vec<String> = Vec::with_capacity(tags.len());
    for t in tags {
        let t = t.trim().to_string();
        if t.is_empty() || t.chars().count() > 32 {
            return Err(IpcError::new(
                codes::E_VALIDATE,
                format!("tag {t:?} must be 1–32 characters"),
            ));
        }
        if !t
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'))
        {
            return Err(IpcError::new(
                codes::E_VALIDATE,
                format!("tag {t:?} may only contain letters, digits, _ . : -"),
            ));
        }
        if !out.contains(&t) {
            out.push(t);
        }
    }
    Ok(out)
}

/// Replace a session's tags (the desktop's Label, M15 G2.7). Answers the
/// updated row.
pub fn set_session_tags(
    args: SetSessionTagsArgs,
    store: &Mutex<Store>,
) -> Result<SessionRow, IpcError> {
    let tags = normalize_session_tags(args.tags)?;
    let s = lock(store)?;
    s.set_session_tags(args.session_id, &tags)?.ok_or_else(|| {
        IpcError::new(
            codes::E_NOTFOUND,
            format!("session {} not found", args.session_id),
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
    /// Switch the session's credential profile on the way through: a
    /// profile name, or `""` for the host's own login. The conversation is
    /// resumed under it (a running `claude` cannot change its login, so a
    /// switch IS a restart). `None` keeps the stored profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

/// The stored-profile change a [`RestartSessionArgs::profile`] asks for:
/// `None` = keep, `Some(None)` = the host's login, `Some(Some(name))` = that
/// profile. A name is validated here, before anything is written.
pub(crate) fn profile_switch(raw: Option<&str>) -> Result<Option<Option<String>>, IpcError> {
    match raw.map(str::trim) {
        None => Ok(None),
        Some("") => Ok(Some(None)),
        Some(p) => {
            crate::validate::claude_profile(p)?;
            Ok(Some(Some(p.to_string())))
        }
    }
}

pub async fn restart_session(
    args: RestartSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<SessionRow, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    crate::validate::tmux_name_addressable(&args.name)?;
    let switch = profile_switch(args.profile.as_deref())?;
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
    // unless forced ([`restart_guard`], which exempts the operator).
    let (kind, agent, claude_id, session_id, launch, gone_cwd) = {
        let s = lock(store)?;
        restart_guard(&s, &args.host_alias, &args.name, args.force)?;
        let row = s.get_session(&args.name, &args.host_alias)?;
        let gone_cwd = gone_pane_cwd(&s, row.as_ref(), &args.host_alias, &args.name)?;
        match row {
            Some(r) => {
                if switch.is_some()
                    && (r.kind == "shell"
                        || crate::store::has_no_pane(&r.kind)
                        || r.agent != crate::store::AGENT_CLAUDE)
                {
                    return Err(IpcError::new(
                        codes::E_INVALID,
                        "a credential profile applies to Claude sessions only",
                    ));
                }
                // The new profile is launched from memory and stored only
                // once the relaunch succeeded ([`relaunch_recording_login`]):
                // written first, a refused repair or a failed respawn left the
                // row naming a login the pane never ran under.
                let mut launch = stored_launch(&s, r.id)?;
                if let Some(profile) = switch.as_ref() {
                    launch.profile = profile.clone();
                }
                (
                    r.kind,
                    r.agent,
                    r.claude_session_id,
                    Some(r.id),
                    launch,
                    gone_cwd,
                )
            }
            None if switch.is_some() => {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!(
                        "no session {} on {} to switch the profile of",
                        args.name, args.host_alias
                    ),
                ));
            }
            // No row under that name: a restart is about to create a live
            // tmux session fleet has no record of. It lands `unclaimed`
            // (multi-user M1, T5) — `record_tmux_created` below reserves no
            // owner for it. A restart is not a create path and carries no
            // person; attributing the session to whoever pressed the button
            // would be a guess, and `unclaimed` is the defined answer for a
            // row nobody can speak for (spec §4.3).
            None => (
                "work".to_string(),
                crate::store::AGENT_CLAUDE.to_string(),
                None,
                None,
                Default::default(),
                gone_cwd,
            ),
        }
    };
    refuse_unvalidated_agent(&agent)?;
    let pane_cmd: String =
        recreate_pane_command(&kind, &agent, claude_id.as_deref(), &args.name, &launch);
    let tmux = exec_for(&args.host_alias, ssh);
    // A Claude pane relaunched reads the login it runs under afresh.
    let login = session_id
        .filter(|_| agent == crate::store::AGENT_CLAUDE && kind != "shell")
        .map(|id| (id, switch.clone()));
    relaunch_recording_login(store, login, async {
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
            None if crate::store::has_no_pane(&kind) => {
                tmux.restart_session(&args.name, &pane_cmd).await?
            }
            None => {
                restart_unrepaired(&*tmux, &args.name, &pane_cmd, || {
                    resolve_gone_pane_cwd(gone_cwd, &args.host_alias, ssh.as_ref())
                })
                .await?
            }
        }
        Ok(())
    })
    .await?;
    // Any of the branches leaves a live tmux session under this name,
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

/// Run `relaunch` (a restart's repair + respawn), then record the login the
/// relaunched pane runs under on `login`'s row: `Some(profile)` switches the
/// stored profile, and a host-login row drops its account link either way
/// ([`Store::clear_host_login_account`]) so the next reconcile takes the
/// host's current account. On `Err` nothing is written: the pane still runs
/// the old login, and the row must keep saying so.
pub(super) async fn relaunch_recording_login(
    store: &Mutex<Store>,
    login: Option<(i64, Option<Option<String>>)>,
    relaunch: impl std::future::Future<Output = Result<(), IpcError>>,
) -> Result<(), IpcError> {
    relaunch.await?;
    if let Some((id, switch)) = login {
        let s = lock(store)?;
        record_relaunched_login(&s, id, switch.as_ref())?;
    }
    Ok(())
}

/// The store half of [`relaunch_recording_login`], for every path that
/// relaunches a Claude pane (restart, recreate, repair).
pub(crate) fn record_relaunched_login(
    s: &Store,
    session_id: i64,
    switch: Option<&Option<String>>,
) -> Result<(), rusqlite::Error> {
    if let Some(profile) = switch {
        s.set_session_profile(session_id, profile.as_deref())?;
    }
    s.clear_host_login_account(session_id)
}

/// `restart_session`'s controller guard: refuse to restart the registered
/// fleet controller unless forced — except the operator.
///
/// The operator is exempt for the reason it is exempt from
/// `refuse_if_operator`: the agent panel's `lost` button calls
/// `restart_session` without `force`. It is never the controller now
/// (`operator::refuse_operator_as_controller`), but a hub where it once
/// registered itself keeps the record, and there the button answered
/// `E_SELF_TARGET` (2026-10-06, `fleet-operator` on mefistos).
pub(super) fn restart_guard(
    s: &Store,
    host_alias: &str,
    name: &str,
    force: bool,
) -> Result<(), IpcError> {
    if crate::service::operator::is_operator_session(s, host_alias, name) {
        return Ok(());
    }
    guard_not_controller(s.get_controller()?.as_ref(), host_alias, name, force)
}

/// Where [`restart_session`] starts a NEW tmux session for a row repair
/// declines (no project, a system project, no row at all) once its tmux
/// session is gone. Decided under the store lock, resolved after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum GonePaneCwd {
    /// A system project's `base_path`: stored absolute, the cwd verbatim on
    /// any host (`fetch_system_base_path`).
    Fixed(String),
    /// The UX agent's directory, [`crate::service::operator::OPERATOR_DIR`],
    /// for a recorded operator row that lost its project.
    Operator,
    /// The host's home directory: an orphan has no directory of its own.
    Home,
}

/// PURE (given the store): see [`GonePaneCwd`]. A system project wins over
/// the operator check because it is the exact path the operator was born in.
pub(super) fn gone_pane_cwd(
    s: &Store,
    row: Option<&SessionRow>,
    host_alias: &str,
    name: &str,
) -> Result<GonePaneCwd, IpcError> {
    if let Some(pid) = row.and_then(|r| r.project_id) {
        if let Some(fixed) = fetch_system_base_path(s, pid)? {
            return Ok(GonePaneCwd::Fixed(fixed));
        }
    }
    if crate::service::operator::is_operator_session(s, host_alias, name) {
        return Ok(GonePaneCwd::Operator);
    }
    Ok(GonePaneCwd::Home)
}

/// [`GonePaneCwd`] as an absolute path on `host_alias`; a `~` must not reach
/// `tmux new-session -c`, which takes it literally.
pub(super) async fn resolve_gone_pane_cwd(
    cwd: GonePaneCwd,
    host_alias: &str,
    ssh: &dyn SshExec,
) -> Result<String, IpcError> {
    match cwd {
        GonePaneCwd::Fixed(p) => Ok(p),
        GonePaneCwd::Operator => crate::service::operator::operator_dir_on(host_alias, ssh).await,
        GonePaneCwd::Home if host_alias == "local" => {
            crate::service::provision::expand_home_local("~")
        }
        GonePaneCwd::Home => ssh.remote_home(host_alias).await,
    }
}

/// The restart of a row with nothing to repair. `respawn-pane` needs the
/// tmux session to exist, and after a host reboot there is no tmux server at
/// all: the respawn failed `error connecting to /tmp/tmux-1000/default`, and
/// the agent panel's `lost` button with it (2026-10-06, `fleet-operator` on
/// mefistos). A session tmux does not list is created instead, in `cwd()` —
/// resolved only then, since it may cost an SSH round trip. A host whose
/// sessions cannot be listed keeps the plain respawn, which reports that
/// host's own error.
pub(super) async fn restart_unrepaired<F, Fut>(
    tmux: &dyn TmuxExec,
    name: &str,
    pane_cmd: &str,
    cwd: F,
) -> Result<(), IpcError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<String, IpcError>>,
{
    let alive = match tmux.list_sessions().await {
        Ok(live) => live.iter().any(|t| t.name == name),
        Err(_) => true,
    };
    if alive {
        return tmux.restart_session(name, pane_cmd).await;
    }
    let cwd = cwd().await?;
    tmux.new_session(name, std::path::Path::new(&cwd), pane_cmd)
        .await
}

/// The pane command to relaunch when (re)creating a session. `shell` → a bare
/// shell; otherwise resume the session's own Claude id (or `--continue` for a
/// legacy session with no stored id). A stored id is validated before use so a
/// tampered DB value can't inject shell — an invalid id degrades to `None`.
/// `launch` is the session's stored model / effort ([`stored_launch`]).
pub(crate) fn recreate_pane_command(
    kind: &str,
    agent: &str,
    claude_session_id: Option<&str>,
    tmux_name: &str,
    launch: &crate::tmux::ClaudeLaunch,
) -> String {
    let Some(agent) = crate::agent_adapter::for_session(kind, Some(agent)) else {
        return crate::tmux::shell_pane_command(None);
    };
    let id = claude_session_id.filter(|id| agent.valid_conversation_id(id));
    agent.launch_command(id, tmux_name, launch)
}

/// Store `launch` as the session's own (every part, `None` included).
pub(crate) fn store_launch(
    s: &Store,
    session_id: i64,
    launch: &crate::tmux::ClaudeLaunch,
) -> Result<(), rusqlite::Error> {
    s.set_session_launch_model(session_id, launch.model.as_deref())?;
    s.set_session_profile(session_id, launch.profile.as_deref())?;
    s.set_session_effort(session_id, launch.effort.as_deref())
}

/// [`store_launch`] for a session just started. Soft for model and effort:
/// the pane already runs with them, and a failed write only means a later
/// recreate / restart uses the host's defaults. Not soft for a profile
/// (review r05 F8): that later restart would run under the host's own login
/// and silently bill another account, so the start reports the failure.
pub(crate) fn store_start_launch(
    s: &Store,
    session_id: i64,
    name: &str,
    launch: &crate::tmux::ClaudeLaunch,
) -> Result<(), IpcError> {
    match store_launch(s, session_id, launch) {
        Ok(()) => Ok(()),
        Err(e) if launch.profile.is_some() => {
            tracing::warn!(session = %name, error = %e, "[new_session] storing the credential profile failed");
            Err(IpcError::new(
                codes::E_INTERNAL,
                format!(
                    "session {name} started, but its credential profile was not saved; \
                     restart it with the profile chosen again so it keeps that login"
                ),
            ))
        }
        Err(e) => {
            tracing::warn!(session = %name, error = %e, "[new_session] storing the launch options failed");
            Ok(())
        }
    }
}

/// The model / effort / profile a session was started or last switched to, checked
/// again ([`crate::tmux::ClaudeLaunch::checked`]). A failed read is the
/// host's default: a rebuilt pane must not fail over a cosmetic column.
pub(crate) fn stored_launch(
    s: &Store,
    session_id: i64,
) -> Result<crate::tmux::ClaudeLaunch, IpcError> {
    // A read that fails fails the start, like a failed profile write (review
    // r05 F8): falling back to the defaults would relaunch the session under
    // the host's login, not the one it was started with.
    let (model, effort, profile) = s.session_launch(session_id).map_err(|e| {
        IpcError::new(
            codes::E_INTERNAL,
            format!("reading session {session_id}'s launch options failed: {e}"),
        )
    })?;
    Ok(crate::tmux::ClaudeLaunch::checked(model, effort, profile))
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
        refuse_unvalidated_agent(&sess.agent)?;
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
            &sess.agent,
            sess.claude_session_id.as_deref(),
            &sess.tmux_name,
            &stored_launch(&s, sess.id)?,
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
        // The checkout itself is gone on a remote host: clone it back the
        // way a new session there would, then repair again (a worktree row
        // is re-added from its branch on origin). Locally the checkout is
        // the user's own, so the refusal stands.
        Err(e) if e.code == codes::E_REPO_MISSING && sess.host_alias != "local" => {
            reclone_project_root(store, &**ssh, sess.id).await?;
            crate::service::repair::ensure_session_workspace(
                sess.id,
                crate::service::repair::Entry::Recreate,
                store,
                ssh,
            )
            .await?
            .cwd
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
        // The rebuilt `claude` reads the host's login afresh.
        if sess.agent == crate::store::AGENT_CLAUDE && sess.kind != "shell" {
            record_relaunched_login(&s, sess.id, None)?;
        }
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

/// Clone a remote session's missing project checkout back to the root the
/// repair resolved for it ([`ensure_remote_project`] without a worktree).
/// The clone step only runs when `<root>/.git` is absent, and it never
/// replaces a non-empty directory (`rmdir`), so a root that exists but is
/// not a usable checkout fails here rather than being overwritten.
pub(super) async fn reclone_project_root(
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    session_id: i64,
) -> Result<(), IpcError> {
    let (spec, _) = crate::service::repair::spec_for_session_with(store, ssh, session_id).await?;
    let project_id = spec
        .project_id
        .ok_or_else(|| IpcError::new(codes::E_NOREPO, "session has no project to clone"))?;
    let (owner, repo) = {
        let s = lock(store)?;
        fetch_owner_repo(&s, project_id)?
    };
    tracing::info!(
        session_id,
        host = %spec.host_alias,
        root = %spec.project_root,
        "recreate: project checkout missing, cloning {owner}/{repo}"
    );
    ensure_remote_project(
        ssh,
        &spec.host_alias,
        &owner,
        &repo,
        &spec.project_root,
        None,
        CancellationToken::new(),
    )
    .await
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
