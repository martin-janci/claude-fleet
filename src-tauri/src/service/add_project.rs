//! Adding a project fleet does not know yet: clone a GitHub repo, adopt an
//! existing checkout, or create a new one. The New-session flow's entry
//! point for "this repo is not on disk yet".
//!
//! This module implements the `clone` (Task 2), `folder` (Task 3) and `new`
//! (Task 4) sources of `docs/superpowers/plans/2026-09-12-add-project.md`,
//! plus the read-only `list_github_repos` (also Task 4).

use crate::ipc_error::{codes, IpcError};
use crate::projects::Layout;
use crate::repo_url::{clone_url_for, parse_repo_url};
use crate::service::projects::ProjectTreeRow;
use crate::shell::quote;
use crate::ssh::{SshClient, SshExec};
use crate::store::Store;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// ssh `ConnectTimeout` for a clone: a typo'd or unreachable host must fail
/// fast, independent of how long the clone itself is allowed to run. Kept
/// short and passed separately from [`CLONE_WALL_CLOCK`] via
/// `SshExec::run_bounded` — `SshExec::run`'s single `timeout` argument is
/// BOTH the connect timeout AND (×3) the wall clock, which would otherwise
/// force a choice between a slow-to-fail connect and a too-short clone.
const CLONE_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Wall clock for a clone: big repos over a slow link. The frontend can
/// cancel sooner once `add_project_with` itself races a `CancellationToken`
/// derived from `call_id` (Task 5 — see the field's doc comment).
const CLONE_WALL_CLOCK: Duration = Duration::from_secs(600);

/// Written to stderr by [`clone_script`]'s "already a checkout" guard and
/// checked by [`git_error`] alongside exit code 3, so an unrelated command
/// that happens to exit 3 is never misread as "already cloned".
const ALREADY_CLONED_MARKER: &str = "__add_project_clone_dest_exists__";

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AddProjectSource {
    Clone {
        url: String,
    },
    Folder {
        path: String,
    },
    New {
        owner: String,
        repo: String,
        #[serde(default)]
        create_remote: bool,
        #[serde(default)]
        confirm: Option<String>,
    },
}

#[derive(Deserialize)]
pub struct AddProjectArgs {
    pub host_alias: String,
    pub source: AddProjectSource,
    /// Set by `invokeCmdAbortable` so the dialog's Cancel can abort a clone.
    /// Registering `call_id` in the cancellation registry at the command
    /// layer is not enough by itself: cancelling actually happens inside
    /// THIS module, so Task 5 must have `add_project_with` accept the
    /// resulting `CancellationToken` and race it directly — via
    /// `SshExec::run_cancellable` for the remote branch and a
    /// `tokio::select!` added to `run_local_script` for the local one.
    /// Unused here.
    #[serde(default)]
    #[allow(dead_code)] // Task 5 wires this in.
    pub call_id: Option<String>,
}

/// Production entry point.
#[allow(dead_code)] // Task 5 (the `add_project` Tauri command) wires this in.
pub async fn add_project(
    args: AddProjectArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<ProjectTreeRow, IpcError> {
    add_project_with(args, store, &**ssh).await
}

pub async fn add_project_with(
    args: AddProjectArgs,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
) -> Result<ProjectTreeRow, IpcError> {
    // This becomes a Tauri-reachable entry point in Task 5, so the alias
    // must be rejected before it ever reaches an ssh argv — same guard
    // every other caller-supplied-alias service applies first (e.g.
    // `service::transcript::fetch_transcript`, `service::hosts::add_host`).
    crate::validate::host_alias(&args.host_alias)?;
    match &args.source {
        AddProjectSource::Clone { url } => clone_source(&args, url, store, ssh).await,
        AddProjectSource::Folder { path } => folder_source(&args, path, store).await,
        AddProjectSource::New {
            owner,
            repo,
            create_remote,
            confirm,
        } => {
            new_source(
                &args,
                owner,
                repo,
                *create_remote,
                confirm.as_deref(),
                store,
                ssh,
            )
            .await
        }
    }
}

async fn clone_source(
    args: &AddProjectArgs,
    url: &str,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
) -> Result<ProjectTreeRow, IpcError> {
    let (owner, repo) = parse_repo_url(url).ok_or_else(|| {
        IpcError::new(
            codes::E_INVALID,
            format!("{url:?} is not a GitHub repository (try owner/repo or its URL)"),
        )
    })?;
    refuse_existing_project(store, &owner, &repo)?;
    // TOCTOU, accepted: the lock above is dropped here and re-taken inside
    // `register`. Two concurrent adds of `Owner/Repo` and `owner/repo` could
    // both pass that case-insensitive guard and then both succeed in
    // `register`, because the `projects(owner, repo)` unique index is
    // SQLite's default BINARY collation (case-sensitive), not `NOCASE`. This
    // is a single-user desktop app — a transaction to close the window isn't
    // worth it, but the next reader should know the guard isn't atomic.
    let (host_root, local_root, layout) = roots(store, &args.host_alias)?;
    let local_base = layout.project_dir(&local_root, &owner, &repo);
    let clone_url = clone_url_for(&owner, &repo);

    let (out, dest) = if args.host_alias == crate::service::projects::LOCAL_HOST {
        let out =
            run_local_script(&clone_script(&local_base, &clone_url), CLONE_WALL_CLOCK).await?;
        (out, local_base.clone())
    } else {
        let home = ssh.remote_home(&args.host_alias).await?;
        let root = crate::service::projects::expand_home(&host_root, &home);
        let dest = layout.project_dir(&root, &owner, &repo);
        let script = clone_script(&dest, &clone_url);
        let out = ssh
            .run_bounded(
                &args.host_alias,
                &["bash", "-lc", &quote(&script)],
                CLONE_CONNECT_TIMEOUT,
                CLONE_WALL_CLOCK,
            )
            .await?;
        (out, dest)
    };
    if !out.status.success() {
        return Err(git_error(&args.host_alias, &owner, &repo, &dest, &out));
    }
    register(store, &owner, &repo, &local_base, false)
}

/// Clone `url` into `dest` unless `dest` is already a checkout. Every value
/// quoted; `set -e` so a failed `mkdir` does not reach `git clone`. Exit
/// code 3 plus [`ALREADY_CLONED_MARKER`] on stderr is the script's own
/// "already a checkout" guard, mapped to `E_EXISTS` by [`git_error`] —
/// nothing here ever removes `dest`.
pub(crate) fn clone_script(dest: &str, clone_url: &str) -> String {
    let d = quote(dest);
    format!(
        "set -e\n\
         if git -C {d} rev-parse --git-dir >/dev/null 2>&1; then echo {marker} >&2; exit 3; fi\n\
         mkdir -p \"$(dirname -- {d})\"\n\
         git clone {url} {d}\n",
        url = quote(clone_url),
        marker = quote(ALREADY_CLONED_MARKER),
    )
}

/// TTL for a `create_remote` confirmation token minted by [`ConfirmTokens`]:
/// long enough to survive the user's own confirm dialog, short enough that a
/// stale token from an abandoned dialog can't be replayed much later for a
/// confirmation the user never actually saw.
const CONFIRM_TOKEN_TTL: Duration = Duration::from_secs(5 * 60);

struct ConfirmEntry {
    host: String,
    owner: String,
    repo: String,
    expires_at: Instant,
}

/// Process-wide registry of outstanding `create_remote` confirmation
/// tokens — the `add_project` equivalent of `mcp::guard::PendingConfirms`,
/// minus the desktop round-trip: a plain `confirm == "owner/repo"` string is
/// satisfiable by a frontend that just echoes the fields it already sends,
/// so `create_remote` is gated on a backend-minted, single-use, TTL-bound
/// token instead, bound to the exact `(host, owner, repo)` it was minted
/// for.
///
/// SECURITY NOTE for a future caller: this trusts that whoever presents a
/// token is the same caller `mint` handed it to — appropriate for a Tauri
/// command reachable only from the app's own webview, where there is no
/// separate untrusted party to convince. **`add_project`/`new_source` must
/// never be exposed as an MCP tool (or any other externally-reachable
/// surface) without ALSO going through `mcp::guard::PendingConfirms`'s
/// desktop-approval `confirm_gate` — a token from this registry alone is not
/// suffient authorization from a caller outside the app's own webview.**
#[derive(Default)]
struct ConfirmTokens {
    entries: Mutex<HashMap<String, ConfirmEntry>>,
}

impl ConfirmTokens {
    /// Mint a fresh token for `(host, owner, repo)`, valid until `now +
    /// `[`CONFIRM_TOKEN_TTL`]. `now` is a parameter (not `Instant::now()`
    /// internally) purely so a test can simulate expiry without sleeping.
    fn mint(&self, host: &str, owner: &str, repo: &str, now: Instant) -> String {
        let token = crate::mcp::generate_token();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|_, e| e.expires_at > now);
        entries.insert(
            token.clone(),
            ConfirmEntry {
                host: host.to_string(),
                owner: owner.to_string(),
                repo: repo.to_string(),
                expires_at: now + CONFIRM_TOKEN_TTL,
            },
        );
        token
    }

    /// `true` (and single-use consumed) only when `token` is known, names
    /// exactly `(host, owner, repo)`, and has not expired as of `now`. An
    /// unknown, expired, or mismatched token is left in place UNLESS it is
    /// expired (pruned either way) — a mismatch does not burn the token, so
    /// a caller that raced two different confirmations can still use the
    /// right one.
    fn consume(&self, token: &str, host: &str, owner: &str, repo: &str, now: Instant) -> bool {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(e) = entries.get(token) else {
            return false;
        };
        let valid = e.expires_at > now && e.host == host && e.owner == owner && e.repo == repo;
        if valid || e.expires_at <= now {
            entries.remove(token);
        }
        valid
    }
}

fn confirm_tokens() -> &'static ConfirmTokens {
    static TOKENS: std::sync::OnceLock<ConfirmTokens> = std::sync::OnceLock::new();
    TOKENS.get_or_init(Default::default)
}

/// Create a brand-new project fleet does not know about yet: `owner`/`repo`
/// validated the same way [`parse_repo_url`] would (`repo_url::is_component`,
/// NOT `validate::path_component` — the rules a new project name must follow
/// match what clone accepts, not the generic path-component rule), refuse an
/// existing project, then run [`new_project_script`] locally or over SSH,
/// and `register` the result.
///
/// Order of checks, each one before anything that follows it:
///
/// 1. the names are valid;
/// 2. (tests only) the local host with `create_remote` is refused outright —
///    see the comment at that check;
/// 3. an existing project is refused ([`existing_project_or_resume`]) —
///    EXCEPT a `create_remote` retry of the very project a previous attempt
///    registered at this same path, which is allowed through so the script
///    can resume the GitHub half;
/// 4. `create_remote` is gated on [`ConfirmTokens`] — see that type's doc
///    comment for the token shape and why a plain string isn't enough.
///
/// The existing-project check runs BEFORE the token is consumed, so a call
/// that is refused anyway does not burn the user's confirmation. The token
/// IS consumed before steps that can still fail transiently — resolving the
/// remote `$HOME` over SSH and the SSH connect for the script itself — so a
/// failure there (an unreachable host, a timeout) means the user has to
/// confirm again on the next attempt: a token is single-use and is never
/// restored after a later failure.
///
/// A failure inside the script itself is mapped by [`new_project_error`],
/// NOT the clone path's `git_error`: unlike a clone (where any failure means
/// `dest` is safe to consider untouched), a `new` with `create_remote` can
/// leave a REAL local repository behind even though the GitHub half failed.
/// Such a repository is registered anyway when it has a commit
/// ([`local_repo_is_usable`]), and a retry resumes it (see
/// [`new_project_script`]).
///
/// `new` creates a NEW project; a retry may only resume a repository `new`
/// itself created. The proof is the [`NEW_PROJECT_TAG_KEY`] git config key,
/// written into the repository right after `git init` by a `create_remote`
/// run and nowhere else. Both resume paths of [`new_project_script`] — the
/// no-`origin` one (run `gh repo create` + push) and the "origin already
/// set" one (push only) — require that key to name `owner/repo`
/// (case-insensitively); a repository without it (a local-only repo, a
/// clone, a scanned checkout, a repo `new` made for another name) is
/// refused as already existing before any `gh`, push or `ls-remote`, even
/// when a fleet row sits at this path.
///
/// The push-only retry exists solely to finish a creation `new` started that
/// failed at the push stage — never a general "push my branch" command. So
/// when the script reports a tagged `dest` with an `origin` and a commit:
///
/// - `origin` is not exactly `owner/repo` on GitHub
///   ([`origin_is_github_repo`]) → refused, nothing pushed;
/// - the remote could not be checked ([`remote_branch_check_script`] exit
///   other than 0 or 2) → error, nothing pushed;
/// - the remote already has `main` (exit 0) → the creation already
///   completed (e.g. the push landed but its SSH session died): register the
///   project (an upsert, so an existing row is simply returned) and succeed,
///   nothing pushed;
/// - only a remote that definitely lacks `main` (exit 2) gets the push.
///
/// No fleet row is required for this: after a timeout, or a failed
/// `register` following full success, on a REMOTE host there is no row, and
/// `refresh_projects` only scans the local root, so requiring one would
/// leave such a project stuck.
async fn new_source(
    args: &AddProjectArgs,
    owner: &str,
    repo: &str,
    create_remote: bool,
    confirm: Option<&str>,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
) -> Result<ProjectTreeRow, IpcError> {
    if !crate::repo_url::is_component(owner, 39) || !crate::repo_url::is_component(repo, 100) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("{owner}/{repo} is not a valid GitHub owner/repo name"),
        ));
    }
    // The real `gh` CLI already ran once from a test during development (it
    // only failed to publish a repository because that environment had no
    // auth). On the local host this function runs the script through the
    // production `bash -lc` with the real `PATH` — where an authenticated
    // `gh` lives, and on macOS it can read its token from the keychain — so
    // a test that combined `host_alias: "local"`, `create_remote: true` and
    // a minted token would publish a real GitHub repository. Structurally
    // impossible under `cfg(test)`: exercise `create_remote` through
    // `FakeSsh` on a remote host, or run the script via the tests'
    // `run_hermetic` helper with a stub `gh`.
    #[cfg(test)]
    if create_remote && args.host_alias == crate::service::projects::LOCAL_HOST {
        return Err(IpcError::new(
            codes::E_FORBIDDEN,
            "refusing `new` with create_remote on the local host under test: it would run \
             the real gh CLI",
        ));
    }
    // Same TOCTOU note as `clone_source`: the lock taken inside is dropped
    // and re-taken inside `register`.
    let (host_root, local_root, layout) = roots(store, &args.host_alias)?;
    let local_base = layout.project_dir(&local_root, owner, repo);
    let resume_as = existing_project_or_resume(store, owner, repo, &local_base, create_remote)?;
    // Gate BEFORE any ssh/local command runs: a repo the user is about to
    // publish to GitHub must never reach the host without a token this
    // process itself minted for exactly this (host, owner, repo).
    if create_remote {
        let now = Instant::now();
        let authorized = confirm
            .is_some_and(|t| confirm_tokens().consume(t, &args.host_alias, owner, repo, now));
        if !authorized {
            let token = confirm_tokens().mint(&args.host_alias, owner, repo, now);
            return Err(IpcError::new(
                codes::E_CONFIRM_REQUIRED,
                format!(
                    "creating {owner}/{repo} on GitHub needs confirmation; retry with the returned token"
                ),
            )
            .with_details(serde_json::json!({ "confirm": token })));
        }
    }
    // A resumed retry re-registers the row under the casing it already has,
    // so the upsert hits that row instead of adding a case-variant twin (the
    // `projects(owner, repo)` unique index is case-sensitive).
    let (reg_owner, reg_repo) = resume_as
        .as_ref()
        .map_or((owner, repo), |(o, r)| (o.as_str(), r.as_str()));

    let host = args.host_alias.as_str();
    let dest = if host == crate::service::projects::LOCAL_HOST {
        local_base.clone()
    } else {
        let home = ssh.remote_home(host).await?;
        let root = crate::service::projects::expand_home(&host_root, &home);
        layout.project_dir(&root, owner, repo)
    };
    let script = new_project_script(&dest, owner, repo, create_remote);
    let mut out = run_new_step(host, ssh, &script, CLONE_WALL_CLOCK)
        .await
        .map_err(|e| note_unknown_github_state_on_timeout(e, create_remote, owner, repo))?;

    if create_remote && origin_already_set(&out) {
        // The script only reaches this state for a repository carrying
        // `new`'s own tag for exactly this owner/repo (see this function's
        // doc comment); a previous attempt got as far as `gh repo create`
        // (which set `origin`) but the push may not have landed.
        let Some(url) = origin_url_from(&out).filter(|u| origin_is_github_repo(u, owner, repo))
        else {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!(
                    "{dest} on {host} already has an origin remote that is not \
                     github.com/{owner}/{repo}; refusing to push to it. Inspect it with \
                     `git -C {dest} remote -v`."
                ),
            ));
        };
        let check = run_new_step(
            host,
            ssh,
            &remote_branch_check_script(&dest),
            REMOTE_BRANCH_CHECK_WALL_CLOCK,
        )
        .await
        .map_err(|e| remote_check_failed(owner, repo, &e.message))?;
        match check.status.code() {
            Some(2) => {}
            Some(0) => {
                // The creation already completed; nothing left to push.
                return register(store, reg_owner, reg_repo, &local_base, false)
                    .map_err(|e| describe_register_failure_after_gh_success(owner, repo, e));
            }
            _ => {
                let detail = user_facing_stderr(&String::from_utf8_lossy(&check.stderr));
                let detail = if detail.is_empty() {
                    format!("exit {:?}", check.status.code())
                } else {
                    detail
                };
                return Err(remote_check_failed(owner, repo, &detail));
            }
        }
        out = run_new_step(host, ssh, &push_only_script(&dest, &url), CLONE_WALL_CLOCK)
            .await
            .map_err(|e| note_unknown_github_state_on_timeout(e, create_remote, owner, repo))?;
    }

    if !out.status.success() {
        let err = new_project_error(host, owner, repo, &dest, &out);
        // `last_stage` is only a precondition here, never the evidence: it
        // excludes the script's own refusals (exit 3/4/5, a changed origin),
        // which print no stage marker and may be looking at a directory this
        // flow never created. Whether the repository is actually usable is
        // decided by probing it for a real HEAD.
        let reached_own_steps = last_stage(&String::from_utf8_lossy(&out.stderr)).is_some();
        if create_remote && reached_own_steps && local_repo_is_usable(host, ssh, &dest).await {
            // The local checkout is real and has a commit even though the
            // GitHub half of `new` didn't finish — register it anyway so the
            // user is not left with an orphan, unlisted directory; a retry
            // is let through `existing_project_or_resume` and resumes it.
            // Best-effort: if THIS also fails, the original error above is
            // still the one returned.
            let _ = register(store, reg_owner, reg_repo, &local_base, false);
        }
        return Err(err);
    }
    let row = register(store, reg_owner, reg_repo, &local_base, false);
    if create_remote {
        return row.map_err(|e| describe_register_failure_after_gh_success(owner, repo, e));
    }
    row
}

/// Wall clock for [`remote_branch_check_script`]: one read-only network
/// round trip to GitHub, plus the SSH round trip on a remote host.
const REMOTE_BRANCH_CHECK_WALL_CLOCK: Duration = Duration::from_secs(60);

/// The push-only retry could not tell whether `main` already exists on the
/// GitHub repository, so it pushed nothing.
fn remote_check_failed(owner: &str, repo: &str, detail: &str) -> IpcError {
    IpcError::new(
        codes::E_GH,
        format!(
            "couldn't check whether {owner}/{repo} on GitHub already has its main branch: \
             {detail}. Nothing was pushed; retry once GitHub is reachable."
        ),
    )
}

/// `new`'s refusal for a `dest` that already holds a repository `new` must
/// not touch (the script's exit-3 guard). "It should appear after a
/// refresh" is only true on the local host: `refresh_projects` scans the
/// local projects root and nothing else.
fn already_exists_error(host: &str, owner: &str, repo: &str, dest: &str) -> IpcError {
    let msg = if host == crate::service::projects::LOCAL_HOST {
        format!("{owner}/{repo} already exists at {dest}; it should appear after a refresh")
    } else {
        format!("{owner}/{repo} already exists at {dest} on {host}")
    };
    IpcError::new(codes::E_EXISTS, msg)
}

/// Wall clock for [`local_repo_is_usable`]'s HEAD probe: one local git call,
/// plus the SSH round trip on a remote host.
const HEAD_PROBE_WALL_CLOCK: Duration = Duration::from_secs(30);

/// Run one `new`-flow script on `host`: locally via [`run_local_script`], or
/// over SSH via `bash -lc`. Same contract as both: the raw `Output` for any
/// exit status, `Err` only for a spawn/connect failure or the wall clock.
async fn run_new_step(
    host: &str,
    ssh: &dyn SshExec,
    script: &str,
    wall_clock: Duration,
) -> Result<std::process::Output, IpcError> {
    if host == crate::service::projects::LOCAL_HOST {
        run_local_script(script, wall_clock).await
    } else {
        ssh.run_bounded(
            host,
            &["bash", "-lc", &quote(script)],
            CLONE_CONNECT_TIMEOUT,
            wall_clock,
        )
        .await
    }
}

/// `dest` on `host` is a git repository with a real HEAD commit — the thing
/// a later `git worktree add` needs to fork from. Probed directly rather
/// than inferred from which stage markers a failed run printed: a marker
/// only says a step STARTED, not that the commit exists. Any probe failure
/// (including not reaching the host) reads as "not usable".
async fn local_repo_is_usable(host: &str, ssh: &dyn SshExec, dest: &str) -> bool {
    let probe = format!(
        "git -C {} rev-parse --verify -q HEAD >/dev/null",
        quote(dest)
    );
    matches!(
        run_new_step(host, ssh, &probe, HEAD_PROBE_WALL_CLOCK).await,
        Ok(out) if out.status.success()
    )
}

/// `register`'s error after a FULLY successful `create_remote` script (init,
/// commit, `gh repo create`, and the push all succeeded): the GitHub
/// repository now genuinely exists even though the local bookkeeping step
/// failed, so the message must say so — otherwise the user has no way to
/// tell "the GitHub create itself failed" (handled by [`new_project_error`])
/// apart from "GitHub is fine, only the local database write failed".
fn describe_register_failure_after_gh_success(owner: &str, repo: &str, e: IpcError) -> IpcError {
    IpcError::new(
        &e.code,
        format!(
            "the GitHub repository {owner}/{repo} was created, but registering the project \
             locally failed: {}",
            e.message
        ),
    )
}

/// Wraps a genuine timeout (`E_TIMEOUT` / `E_SSH_TIMEOUT`) from running
/// [`new_project_script`] with `create_remote` set: a timeout means the
/// process was killed before it finished, and — unlike a normal failure
/// exit — there is no captured output to read a stage marker from, so
/// whether `gh` already created (or even pushed to) the GitHub repository
/// before the deadline hit is genuinely unknown. Every other error passes
/// through unchanged.
fn note_unknown_github_state_on_timeout(
    e: IpcError,
    create_remote: bool,
    owner: &str,
    repo: &str,
) -> IpcError {
    if create_remote && (e.code == codes::E_TIMEOUT || e.code == codes::E_SSH_TIMEOUT) {
        return IpcError::new(
            &e.code,
            format!(
                "{} — creating {owner}/{repo} on GitHub may or may not have completed before \
                 the timeout; the GitHub state is unknown. Check GitHub before retrying.",
                e.message
            ),
        );
    }
    e
}

/// Fallback committer identity for [`new_project_script`]'s initial commit —
/// used ONLY when `create_remote` is OFF and `git config` resolves neither
/// `user.name` nor `user.email` from any scope (local/global/system), i.e. a
/// bare host with no identity configured at all. A user's own configured
/// identity always wins, and a PARTIAL identity (only one of the two set)
/// only has the missing field substituted — never both. This exists purely
/// so the initial commit does not fail outright with git's "Please tell me
/// who you are". When `create_remote` IS set, this is never used: see
/// [`new_project_script`]'s doc comment for why a placeholder author must
/// never reach a real GitHub repository.
const FALLBACK_GIT_NAME: &str = "claude-fleet";
const FALLBACK_GIT_EMAIL: &str = "claude-fleet@localhost";

/// Emitted to stderr immediately BEFORE each step [`new_project_script`] is
/// about to attempt (never after), so a failure's LAST marker in stderr says
/// which step was actually in progress — [`new_project_error`] reads it back
/// to say what was left behind, instead of the clone path's generic
/// "couldn't clone" message, which would be wrong (and in the `gh` case,
/// actively misleading about whether GitHub state changed) for this script.
/// Internal only: [`user_facing_stderr`] keeps them out of every message.
const STAGE_INIT: &str = "__stage=init__";
const STAGE_COMMIT: &str = "__stage=commit__";
const STAGE_GH_CREATE: &str = "__stage=gh_create__";
const STAGE_GH_PUSH: &str = "__stage=gh_push__";
const STAGE_MARKERS: [&str; 4] = [STAGE_INIT, STAGE_COMMIT, STAGE_GH_CREATE, STAGE_GH_PUSH];

/// Written to stderr, alongside exit code 4, when `dest` already exists but
/// is not a git checkout at all — distinct from [`ALREADY_CLONED_MARKER`]
/// (exit 3), which means `dest` is already a *finished* checkout.
const NOT_A_GIT_REPO_MARKER: &str = "__add_project_dest_not_a_git_repo__";

/// Written to stderr, alongside exit code 5, when `create_remote` is set and
/// `git config` does not resolve both `user.name` and `user.email` — see
/// [`new_project_script`]'s doc comment for why this refuses outright
/// instead of substituting the placeholder identity here.
const NO_GIT_IDENTITY_MARKER: &str = "__add_project_no_git_identity__";

/// Frame `dest`'s effective push URL for `origin` on stderr, alongside exit
/// code 6, when `create_remote` is set and `dest` already has an `origin`
/// and a commit — the "created on GitHub but the push failed" state. Read
/// back by [`origin_url_from`]; `new_source` then verifies it before running
/// [`push_only_script`].
const ORIGIN_URL_BEGIN_MARKER: &str = "__add_project_origin_url_begin__";
const ORIGIN_URL_END_MARKER: &str = "__add_project_origin_url_end__";

/// Git config key (`git config --local`) that marks a repository as one a
/// `create_remote` run of [`new_project_script`] created, holding
/// `owner/repo`. Written right after `git init` and nowhere else; required
/// by both resume paths — see `new_source`'s doc comment.
const NEW_PROJECT_TAG_KEY: &str = "claude-fleet.new-project";

/// Written to stderr, alongside exit code 7, when `git` is not on the host's
/// `PATH` — checked before anything is created.
const NO_GIT_MARKER: &str = "__add_project_no_git__";

/// Written to stderr, alongside exit code 3, by [`push_only_script`] when
/// `origin` no longer names exactly the URL `new_source` verified.
const ORIGIN_CHANGED_MARKER: &str = "__add_project_origin_changed__";

/// Create a brand-new project at `dest`, or safely RESUME a previous
/// attempt that stopped part-way:
///
/// - `git` is not installed → [`NO_GIT_MARKER`] + exit 7 before anything
///   else, so a host without git is left untouched (an empty `dest` would
///   otherwise turn every retry into exit 4).
/// - `dest` is an existing git repo and `create_remote` is set → it must
///   carry [`NEW_PROJECT_TAG_KEY`] = `owner/repo` (case-insensitively), or
///   it is refused via [`ALREADY_CLONED_MARKER`] + exit 3 before either
///   resume path below runs: `new` never publishes a repository it did not
///   create.
/// - `dest` does not exist → with `create_remote`, first require a git
///   identity ([`NO_GIT_IDENTITY_MARKER`] + exit 5) BEFORE `mkdir`, so a
///   refusal leaves nothing behind at all. Then `mkdir -p` and `git init`,
///   with `create_remote` immediately followed by writing
///   [`NEW_PROJECT_TAG_KEY`]. Deliberately NOT `git init -b main`: that flag
///   needs git ≥2.28, which is newer than some still-common distros ship;
///   `git init && git symbolic-ref HEAD refs/heads/main` names the initial
///   branch `main` on every git that still has an unborn-HEAD `init`. Then
///   an empty initial commit so a later `git worktree add` has a branch to
///   fork from.
/// - `dest` is a git repo WITHOUT an `origin` remote → a prior attempt got
///   through `init` but not through the `gh` stage. Skip `mkdir`/`init`;
///   make the initial commit only if there is no HEAD yet (a prior commit
///   failed, e.g. `commit.gpgsign` with no TTY); then run the `gh` stage
///   (with `create_remote` off there is nothing more to do: exit 0).
/// - `dest` is a git repo WITH an `origin`, a HEAD, and `create_remote` is
///   set → `gh repo create` already ran but the push did not finish. Print
///   the push URL framed by [`ORIGIN_URL_BEGIN_MARKER`]/
///   [`ORIGIN_URL_END_MARKER`] and exit 6 WITHOUT pushing: the caller
///   verifies the URL and runs [`push_only_script`].
/// - `dest` is a git repo WITH an `origin` otherwise → refuse via
///   [`ALREADY_CLONED_MARKER`] + exit 3, same convention as [`clone_script`]
///   (mapped to `E_EXISTS` by [`new_project_error`]).
/// - `dest` exists and is NOT a git repo at all → refuse via
///   [`NOT_A_GIT_REPO_MARKER`] + exit 4 (a name collision with something
///   else, not a resumable prior attempt).
///
/// The commit step never lets a placeholder author reach a real GitHub
/// repository: with `create_remote` set, a missing identity refuses outright
/// (exit 5) instead of substituting [`FALLBACK_GIT_NAME`]/
/// [`FALLBACK_GIT_EMAIL`] — those are used only when `create_remote` is off,
/// and then only per missing field. The identity check that precedes
/// `mkdir` runs from `/` so no surrounding repository's local config can
/// satisfy it; a user whose identity comes only from an `includeIf
/// "gitdir:…"` block is therefore asked for a global one.
///
/// `create_remote` appends two SEPARATE steps — `gh repo create … --remote
/// origin` (no `--push`) then a plain `git push -u origin main` — rather
/// than one combined `--push` invocation, specifically so a failure's last
/// stage marker can tell "the GitHub repository was never created" (failed
/// during [`STAGE_GH_CREATE`]) apart from "it was created but the push
/// failed, so it may already exist" (failed during [`STAGE_GH_PUSH`]).
///
/// `GIT_TERMINAL_PROMPT=0` is exported so a push can never block on a
/// credential prompt when the app was launched from a terminal.
fn new_project_script(dest: &str, owner: &str, repo: &str, create_remote: bool) -> String {
    let d = quote(dest);

    let commit_block = if create_remote {
        format!(
            "if git config user.email >/dev/null 2>&1 && git config user.name >/dev/null 2>&1; then\n\
             git commit --allow-empty -m 'Initial commit'\n\
             else\n\
             echo {marker} >&2\n\
             exit 5\n\
             fi\n",
            marker = quote(NO_GIT_IDENTITY_MARKER),
        )
    } else {
        format!(
            "args=()\n\
             if ! git config user.name >/dev/null 2>&1; then args+=(-c {name_kv}); fi\n\
             if ! git config user.email >/dev/null 2>&1; then args+=(-c {email_kv}); fi\n\
             git \"${{args[@]}}\" commit --allow-empty -m 'Initial commit'\n",
            name_kv = quote(&format!("user.name={FALLBACK_GIT_NAME}")),
            email_kv = quote(&format!("user.email={FALLBACK_GIT_EMAIL}")),
        )
    };

    let identity_precheck = if create_remote {
        format!(
            "if ! {{ git -C / config user.email && git -C / config user.name; }} >/dev/null 2>&1; then\n\
             echo {marker} >&2\n\
             exit 5\n\
             fi\n",
            marker = quote(NO_GIT_IDENTITY_MARKER),
        )
    } else {
        String::new()
    };

    let origin_resume = if create_remote {
        format!(
            "if git rev-parse --verify -q HEAD >/dev/null; then\n\
             echo {begin} >&2\n\
             git remote get-url --push origin >&2\n\
             echo {end} >&2\n\
             exit 6\n\
             fi\n",
            begin = quote(ORIGIN_URL_BEGIN_MARKER),
            end = quote(ORIGIN_URL_END_MARKER),
        )
    } else {
        String::new()
    };

    let slug = format!("{owner}/{repo}");
    let (tag_guard, tag_write) = if create_remote {
        (
            format!(
                "tag=$(git config --local --get {key} 2>/dev/null | tr '[:upper:]' '[:lower:]')\n\
                 if [ \"$tag\" != {expected} ]; then\n\
                 echo {exists_marker} >&2\n\
                 exit 3\n\
                 fi\n",
                key = quote(NEW_PROJECT_TAG_KEY),
                expected = quote(&slug.to_ascii_lowercase()),
                exists_marker = quote(ALREADY_CLONED_MARKER),
            ),
            format!(
                "git config --local {key} {slug}\n",
                key = quote(NEW_PROJECT_TAG_KEY),
                slug = quote(&slug),
            ),
        )
    } else {
        (String::new(), String::new())
    };

    let stage_init = quote(STAGE_INIT);
    let stage_commit = quote(STAGE_COMMIT);
    let mut s = format!(
        "set -e\n\
         export GIT_TERMINAL_PROMPT=0\n\
         if ! command -v git >/dev/null 2>&1; then\n\
         echo {nogit_marker} >&2\n\
         exit 7\n\
         fi\n\
         if [ -e {d} ]; then\n\
         if git -C {d} rev-parse --git-dir >/dev/null 2>&1; then\n\
         cd {d}\n\
         {tag_guard}\
         if git remote get-url origin >/dev/null 2>&1; then\n\
         {origin_resume}\
         echo {exists_marker} >&2\n\
         exit 3\n\
         fi\n\
         if ! git rev-parse --verify -q HEAD >/dev/null; then\n\
         echo {stage_commit} >&2\n\
         {commit_block}\
         fi\n\
         else\n\
         echo {notgit_marker} >&2\n\
         exit 4\n\
         fi\n\
         else\n\
         {identity_precheck}\
         echo {stage_init} >&2\n\
         mkdir -p {d}\n\
         cd {d}\n\
         git init\n\
         {tag_write}\
         git symbolic-ref HEAD refs/heads/main\n\
         echo {stage_commit} >&2\n\
         {commit_block}\
         fi\n",
        exists_marker = quote(ALREADY_CLONED_MARKER),
        notgit_marker = quote(NOT_A_GIT_REPO_MARKER),
        nogit_marker = quote(NO_GIT_MARKER),
    );
    if create_remote {
        s.push_str(&format!(
            "echo {stage_create} >&2\n\
             gh repo create {slug} --private --source . --remote origin\n\
             echo {stage_push} >&2\n\
             git push -u origin main\n",
            stage_create = quote(STAGE_GH_CREATE),
            stage_push = quote(STAGE_GH_PUSH),
            slug = quote(&slug),
        ));
    }
    s
}

/// Only the push, for a `dest` whose `origin` `new_source` has already
/// verified to be `origin_url` (see [`origin_is_github_repo`]). Re-checks
/// that the effective push URL is still exactly that string immediately
/// before pushing — refusing via [`ORIGIN_CHANGED_MARKER`] + exit 3 if it
/// changed in between — so nothing is ever pushed to an unverified remote.
fn push_only_script(dest: &str, origin_url: &str) -> String {
    format!(
        "set -e\n\
         export GIT_TERMINAL_PROMPT=0\n\
         cd {d}\n\
         if [ \"$(git remote get-url --push origin)\" != {url} ]; then\n\
         echo {changed} >&2\n\
         exit 3\n\
         fi\n\
         echo {stage_push} >&2\n\
         git push -u origin main\n",
        d = quote(dest),
        url = quote(origin_url),
        changed = quote(ORIGIN_CHANGED_MARKER),
        stage_push = quote(STAGE_GH_PUSH),
    )
}

/// Read-only: does `origin` (already verified by [`origin_is_github_repo`])
/// have `refs/heads/main`? `git ls-remote --exit-code` exits 0 when it does,
/// 2 when the remote answered but has no such ref, and anything else when
/// the remote could not be asked — `new_source` pushes only on 2.
fn remote_branch_check_script(dest: &str) -> String {
    format!(
        "export GIT_TERMINAL_PROMPT=0\n\
         git -C {d} ls-remote --exit-code origin refs/heads/main\n",
        d = quote(dest),
    )
}

/// The script stopped at its "already has an origin and a commit" branch
/// (exit 6 with the URL frame) instead of doing anything.
fn origin_already_set(out: &std::process::Output) -> bool {
    out.status.code() == Some(6)
        && String::from_utf8_lossy(&out.stderr).contains(ORIGIN_URL_END_MARKER)
}

/// The push URL [`new_project_script`] framed on stderr — `None` when the
/// frame is missing, empty, or holds more than one line (several push URLs
/// configured: never a single verifiable destination).
fn origin_url_from(out: &std::process::Output) -> Option<String> {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let start = stderr.find(ORIGIN_URL_BEGIN_MARKER)? + ORIGIN_URL_BEGIN_MARKER.len();
    let len = stderr[start..].find(ORIGIN_URL_END_MARKER)?;
    let url = stderr[start..start + len].trim();
    (!url.is_empty() && !url.contains('\n')).then(|| url.to_string())
}

/// `url` names exactly `owner/repo` on GitHub, compared case-insensitively
/// like [`existing_project_or_resume`]. Parsed with [`parse_repo_url`], but
/// only for an explicit `github.com` URL: that parser also accepts a bare
/// `owner/repo`, which as a git remote is a LOCAL relative path, not GitHub.
fn origin_is_github_repo(url: &str, owner: &str, repo: &str) -> bool {
    let url = url.trim();
    let on_github = [
        "git@github.com:",
        "ssh://git@github.com/",
        "https://github.com/",
        "http://github.com/",
    ]
    .iter()
    .any(|prefix| url.starts_with(prefix));
    on_github
        && parse_repo_url(url)
            .is_some_and(|(o, r)| o.eq_ignore_ascii_case(owner) && r.eq_ignore_ascii_case(repo))
}

/// The LAST stage marker [`new_project_script`] managed to print before
/// failing, in stage order (later stages checked last so they win when
/// present) — `None` when the failure happened before any stage marker at
/// all (the existence/identity/origin guards, or a spawn-level problem).
fn last_stage(stderr: &str) -> Option<&'static str> {
    let mut found = None;
    for marker in STAGE_MARKERS {
        if stderr.contains(marker) {
            found = Some(marker);
        }
    }
    found
}

/// The part of a failed run's stderr a user should see: only what came
/// after the LAST stage marker (earlier output belongs to steps that
/// succeeded — `git init`'s `hint:` block, for one), with any `hint:` lines
/// and every internal marker removed. Mirrors how [`git_error`] keeps
/// [`ALREADY_CLONED_MARKER`] out of its message.
fn user_facing_stderr(stderr: &str) -> String {
    let tail_start = STAGE_MARKERS
        .iter()
        .filter_map(|m| stderr.rfind(m).map(|i| i + m.len()))
        .max()
        .unwrap_or(0);
    let mut tail = stderr[tail_start..].to_string();
    for marker in STAGE_MARKERS.iter().chain(&[
        ALREADY_CLONED_MARKER,
        NOT_A_GIT_REPO_MARKER,
        NO_GIT_IDENTITY_MARKER,
        NO_GIT_MARKER,
        ORIGIN_URL_BEGIN_MARKER,
        ORIGIN_URL_END_MARKER,
        ORIGIN_CHANGED_MARKER,
    ]) {
        tail = tail.replace(marker, "");
    }
    tail.lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with("hint:"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// [`new_project_script`]'s (or [`push_only_script`]'s) failure at `dest`
/// on `host`, mapped to an `IpcError`. Deliberately NOT [`git_error`]: a
/// clone failure always means `dest` is safe to consider untouched (nothing
/// to resume, nothing kept), but a `new` failure can leave a REAL, usable
/// local repository behind — this reads [`last_stage`] to say exactly what
/// state `dest` is actually in, rather than reusing `git_error`'s "couldn't
/// clone" wording (wrong here) or its blanket `E_EXISTS`-on-exit-3
/// convention (shared only for the truly-already-done case, which this
/// script signals the same way). The raw stderr never reaches the message:
/// only [`user_facing_stderr`] does.
fn new_project_error(
    host: &str,
    owner: &str,
    repo: &str,
    dest: &str,
    out: &std::process::Output,
) -> IpcError {
    let stderr_raw = String::from_utf8_lossy(&out.stderr).to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let detail = || -> String {
        let s = user_facing_stderr(&stderr_raw);
        if !s.is_empty() {
            s
        } else if !stdout.is_empty() {
            stdout.clone()
        } else {
            "(no output)".to_string()
        }
    };

    if out.status.code() == Some(3) && stderr_raw.contains(ORIGIN_CHANGED_MARKER) {
        return IpcError::new(
            codes::E_EXISTS,
            format!(
                "the origin remote of {dest} on {host} changed before the push; nothing was \
                 pushed. Check `git -C {dest} remote -v`."
            ),
        );
    }
    if out.status.code() == Some(3) && stderr_raw.contains(ALREADY_CLONED_MARKER) {
        return already_exists_error(host, owner, repo, dest);
    }
    if out.status.code() == Some(4) && stderr_raw.contains(NOT_A_GIT_REPO_MARKER) {
        return IpcError::new(
            codes::E_EXISTS,
            format!("{dest} already exists and is not a git repository"),
        );
    }
    if out.status.code() == Some(7) && stderr_raw.contains(NO_GIT_MARKER) {
        return IpcError::new(
            codes::E_GIT_SETUP,
            format!(
                "git is not installed on {host}; install git there and retry to create \
                 {owner}/{repo}. Nothing was created."
            ),
        );
    }
    if out.status.code() == Some(5) && stderr_raw.contains(NO_GIT_IDENTITY_MARKER) {
        return IpcError::new(
            codes::E_INVALID,
            format!(
                "creating {owner}/{repo} on GitHub needs a real git identity on {host}: run \
                 `git config --global user.name \"…\"` and `git config --global user.email \
                 \"…\"` there, then retry"
            ),
        );
    }

    match last_stage(&stderr_raw) {
        Some(STAGE_GH_PUSH) => IpcError::new(
            codes::E_GH,
            format!(
                "created {dest} on {host}, and the GitHub repository {owner}/{repo} may already \
                 exist, but pushing to it failed: {}. The local repository is kept; check \
                 GitHub and retry if needed.",
                detail()
            ),
        ),
        Some(STAGE_GH_CREATE) if out.status.code() == Some(127) => IpcError::new(
            codes::E_GH,
            format!(
                "created {dest} on {host}, but gh is not installed there; install the GitHub \
                 CLI and retry to create {owner}/{repo} on GitHub. The local repository is kept."
            ),
        ),
        Some(STAGE_GH_CREATE) => IpcError::new(
            codes::E_GH,
            format!(
                "created {dest} on {host}, but `gh repo create` failed: {}. The local \
                 repository is kept; retry to create it on GitHub.",
                detail()
            ),
        ),
        _ => IpcError::new(
            codes::E_GIT_SETUP,
            format!("couldn't set up {owner}/{repo} on {host}: {}", detail()),
        ),
    }
}

/// Wall clock for `gh repo list` ONLY — `new_source`'s `create_remote` runs
/// under [`CLONE_WALL_CLOCK`] instead, since `gh repo create`/`git push` are
/// one part of a script that also does local git work and deserves the same
/// generous bound as a clone, not this browse-mode-sized one. A plain read
/// API call, but generous enough for a slow link without leaving the
/// Add-project dialog's browse mode spinning forever on a wedged `gh`.
const GH_WALL_CLOCK: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GithubRepo {
    pub name_with_owner: String,
    pub description: Option<String>,
    pub is_private: bool,
    pub updated_at: Option<String>,
}

/// `gh repo list --json`'s camelCase wire shape. Kept private and separate
/// from [`GithubRepo`] (whose fields are snake_case for the TS side) rather
/// than annotating one struct with two different renames.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhRepoWire {
    name_with_owner: String,
    description: Option<String>,
    is_private: bool,
    updated_at: Option<String>,
}

/// Production entry point for the Add-project dialog's browse mode.
#[allow(dead_code)] // Task 5 (the `list_github_repos` Tauri command) wires this in.
pub async fn list_github_repos(
    host_alias: &str,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<Vec<GithubRepo>, IpcError> {
    list_github_repos_with(host_alias, store, &**ssh).await
}

/// The repositories `gh` can see on `host_alias` — read-only, nothing is
/// registered or written. `store` is threaded through for symmetry with
/// every other `_with` function in this module (and in case a future host
/// lookup needs it); today only `host_alias` itself is consulted.
pub async fn list_github_repos_with(
    host_alias: &str,
    _store: &Mutex<Store>,
    ssh: &dyn SshExec,
) -> Result<Vec<GithubRepo>, IpcError> {
    crate::validate::host_alias(host_alias)?;
    const CMD: &str =
        "gh repo list --limit 200 --json nameWithOwner,description,isPrivate,updatedAt";
    let out = if host_alias == crate::service::projects::LOCAL_HOST {
        run_local_script(CMD, GH_WALL_CLOCK).await?
    } else {
        ssh.run_bounded(
            host_alias,
            &["bash", "-lc", &quote(CMD)],
            CLONE_CONNECT_TIMEOUT,
            GH_WALL_CLOCK,
        )
        .await?
    };
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        // Exit 127 is the shell's own "command not found" — `gh` is simply
        // missing from this host's PATH, which needs a different remedy
        // (install it) than every other `gh` failure (auth, network, a bad
        // flag…), which is reported with gh's own stderr verbatim so "run gh
        // auth login" reaches the user unchanged. Keyed on the exit code
        // ALONE, not stderr text: a noisy login profile (e.g. a shell
        // printing "brew: command not found" on every login) could put that
        // phrase in stderr alongside a real, unrelated `gh` failure.
        if out.status.code() == Some(127) {
            return Err(IpcError::new(
                codes::E_GH,
                format!("gh is not installed on {host_alias}; install the GitHub CLI there"),
            ));
        }
        return Err(IpcError::new(
            codes::E_GH,
            if stderr.is_empty() {
                format!("gh repo list failed on {host_alias}")
            } else {
                stderr
            },
        ));
    }
    let repos: Vec<GhRepoWire> = serde_json::from_slice(&out.stdout).map_err(|_| {
        let cut = out.stdout.len().min(200);
        let snippet = String::from_utf8_lossy(&out.stdout[..cut]);
        IpcError::new(
            codes::E_GH,
            format!("couldn't parse gh's repo list on {host_alias}: {snippet}"),
        )
    })?;
    Ok(repos
        .into_iter()
        .map(|w| GithubRepo {
            name_with_owner: w.name_with_owner,
            description: w.description,
            is_private: w.is_private,
            updated_at: w.updated_at,
        })
        .collect())
}

/// Wall clock for each local `git` read `adopt` runs. The user reaches this
/// path through a native folder picker, so `path` could plausibly sit on a
/// hung network mount — a few seconds is generous for a `rev-parse` or
/// `worktree list` against a real checkout, and keeps a stuck probe from
/// pinning a blocking-pool thread forever.
const ADOPT_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Adopt an existing checkout at `path` as a fleet project: register it
/// WHERE IT IS (`base_path` is its canonical top level) — nothing is moved,
/// copied, or written under `path`. Local-only: a checkout that lives on a
/// remote host must be cloned there instead, since fleet has no way to
/// register a path it cannot resolve without SSH-ing in for every read.
async fn folder_source(
    args: &AddProjectArgs,
    path: &str,
    store: &Mutex<Store>,
) -> Result<ProjectTreeRow, IpcError> {
    if args.host_alias != crate::service::projects::LOCAL_HOST {
        return Err(IpcError::new(
            codes::E_INVALID,
            "adopting a folder works on the local host only; clone it on the remote host instead",
        ));
    }
    crate::validate::remote_abs_path("folder", path)?;
    // `std::fs` and `git` are blocking; keep them off the async worker. A
    // `Handle` travels into the blocking closure so `git_out` can bound each
    // probe with `tokio::time::timeout` (see `ADOPT_PROBE_TIMEOUT`) without
    // itself being async — `spawn_blocking`'s closure is a plain `FnOnce`.
    let p = path.to_string();
    let handle = tokio::runtime::Handle::current();
    let (base_path, owner, repo) = tokio::task::spawn_blocking(move || adopt(&handle, &p))
        .await
        .map_err(|e| IpcError::new(codes::E_IO, format!("folder probe failed: {e}")))??;
    refuse_existing_project(store, &owner, &repo)?;
    refuse_existing_base_path(store, &base_path)?;
    register(store, &owner, &repo, &base_path, true)
}

/// The checkout at `path`: its canonical top level (resolved through a
/// linked worktree to its MAIN checkout, if that's what `path` names — see
/// [`resolve_main_checkout`]), and the `(owner, repo)` its `origin` names —
/// falling back to `("local", <sanitized basename of the top level>)` when
/// there is no origin, or `origin` is not a GitHub URL.
fn adopt(
    handle: &tokio::runtime::Handle,
    path: &str,
) -> Result<(String, String, String), IpcError> {
    let top = match git_out(handle, path, &["rev-parse", "--show-toplevel"]) {
        Ok(s) if !s.is_empty() => s,
        Ok(_) => {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("{path} is not a git checkout"),
            ))
        }
        // Surface git's own stderr: a nonexistent path, a permission error,
        // and git's `safe.directory` "dubious ownership" refusal all fail
        // here, and each says something different — collapsing them into one
        // generic "is not a git checkout" made them undiagnosable.
        Err(detail) => {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("{path} is not a git checkout: {detail}"),
            ))
        }
    };
    let top = resolve_main_checkout(handle, &top)?;
    let origin = git_out(handle, &top, &["remote", "get-url", "origin"]).unwrap_or_default();
    let (owner, repo) = parse_repo_url(&origin).unwrap_or_else(|| {
        let name = std::path::Path::new(&top)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".to_string());
        ("local".to_string(), sanitize_basename(&name))
    });
    Ok((top, owner, repo))
}

/// When `top` is a linked worktree of a NON-bare repo, resolve and return its
/// MAIN checkout's path instead; otherwise (not a linked worktree, or a
/// linked worktree of a BARE repo) return `top` unchanged.
///
/// A linked worktree's `git rev-parse --git-dir` points into the main
/// checkout's `.git/worktrees/<name>`, while `--git-common-dir` stays the
/// main checkout's own `.git` — the two differ only there, so comparing them
/// (raw, no canonicalization needed to tell "different" from "same") detects
/// the shape. `refresh_projects` already has a dedicated self-heal for a
/// project row registered at a linked-worktree path ("a linked worktree
/// scanned as a repo" in `service::projects`) — it treats that as a defect
/// state to correct, so `adopt` must not manufacture it in the first place:
/// adopting `/somewhere/wt-feature` must register the main checkout, not the
/// worktree.
///
/// A BARE repo's worktrees are the exception: `--git-dir`/`--git-common-dir`
/// differ there too, but the bare repo itself has no working tree — it is
/// the same "not a checkout" state `adopt` already refuses outright when
/// pointed at it directly. Preferring it here would silently register that
/// unusable path instead. There is no better checkout to prefer than the one
/// the user actually picked, so a bare repo's worktree keeps `top`.
fn resolve_main_checkout(handle: &tokio::runtime::Handle, top: &str) -> Result<String, IpcError> {
    let git_dir = git_out(handle, top, &["rev-parse", "--git-dir"]).unwrap_or_default();
    let common_dir = git_out(handle, top, &["rev-parse", "--git-common-dir"]).unwrap_or_default();
    if git_dir.is_empty() || common_dir.is_empty() || git_dir == common_dir {
        return Ok(top.to_string());
    }
    let list = git_out(handle, top, &["worktree", "list", "--porcelain"]).map_err(|detail| {
        IpcError::new(
            codes::E_INVALID,
            format!(
                "{top} is a linked worktree, but its main checkout could not be resolved: {detail}"
            ),
        )
    })?;
    // Porcelain entries are separated by a blank line; git's own ordering
    // guarantee is that the FIRST block is the main worktree (the identical
    // assumption `crate::projects::list_worktrees` makes). Scoping both the
    // path and the `bare` check to that first block — rather than scanning
    // every line — means a `bare` marker belonging to some other entry can
    // never be mistaken for the first block's own.
    let first_block = list.split("\n\n").next().unwrap_or("");
    let mut first_block_lines = first_block.lines();
    let main = first_block_lines
        .next()
        .and_then(|l| l.strip_prefix("worktree "))
        .ok_or_else(|| {
            IpcError::new(
                codes::E_INTERNAL,
                format!("{top}: `git worktree list` reported no worktrees"),
            )
        })?;
    if first_block_lines.any(|l| l == "bare") {
        return Ok(top.to_string());
    }
    Ok(crate::projects::path_identity::canonical_str(main))
}

/// Sanitize a raw directory basename for the `("local", <basename>)`
/// fallback: `repo_url::parse_repo_url`'s documented invariant is that a
/// parsed pair is always safe to interpolate into a path, but a filesystem
/// basename never passed through that check, so `/Users/m/my repo` or
/// `/Users/m/-rf` would otherwise store a pair violating it — later
/// interpolated into remote paths by `worktree_prune`, `move_session` and
/// `repair`. Chosen over refusing outright: adopting a folder should not fail
/// just because its directory name is unconventional when there is a safe,
/// obvious rendering of it. Runs of characters `repo_url::is_component`
/// would reject (including a leading `-`, which a command-line parser would
/// read as an option) collapse to a single `-`, then are trimmed from the
/// ends; a name that sanitizes to nothing — or still fails the check, e.g.
/// the reserved `.git` marker — falls back to `"repo"`.
fn sanitize_basename(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.') {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let candidate = out.trim_matches('-');
    if candidate.is_empty() || !crate::repo_url::is_component(candidate, 100) {
        "repo".to_string()
    } else {
        candidate.to_string()
    }
}

/// `git -C <dir> <args>` trimmed stdout on success, bounded by
/// [`ADOPT_PROBE_TIMEOUT`]. `Err` carries a diagnostic — git's stderr when
/// non-empty, else a description of what went wrong (a spawn failure, a
/// timeout) — instead of the caller collapsing every failure mode into one
/// generic message. Runs `git` via `tokio::process` and awaits it through
/// `handle.block_on`: this function itself runs on a `spawn_blocking` thread
/// (never a runtime worker), so blocking there to await a bounded future is
/// the documented safe bridge, and it is what lets a hung git process be
/// killed on timeout (`kill_on_drop`) instead of leaking a thread forever.
fn git_out(handle: &tokio::runtime::Handle, dir: &str, args: &[&str]) -> Result<String, String> {
    handle.block_on(async {
        let run = tokio::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .kill_on_drop(true)
            .output();
        match tokio::time::timeout(ADOPT_PROBE_TIMEOUT, run).await {
            Ok(Ok(out)) if out.status.success() => {
                Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
            }
            Ok(Ok(out)) => {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                Err(if stderr.is_empty() {
                    format!("git exited with {}", out.status)
                } else {
                    stderr
                })
            }
            Ok(Err(e)) => Err(format!("couldn't run git: {e}")),
            Err(_) => Err(format!(
                "timed out after {}s (is the folder on an unresponsive network mount?)",
                ADOPT_PROBE_TIMEOUT.as_secs()
            )),
        }
    })
}

/// `(owner, repo)` already names a fleet project. GitHub and the default
/// macOS filesystem both treat owner/repo case-insensitively, so `Owner/Repo`
/// and `owner/repo` would otherwise become two rows pointing at one checkout.
/// `parse_repo_url` deliberately preserves the typed case (the sidebar shows
/// the owner as typed), so the de-duplication belongs here. Takes and drops
/// the lock; no `.await` while held.
fn refuse_existing_project(store: &Mutex<Store>, owner: &str, repo: &str) -> Result<(), IpcError> {
    let s = store.lock().map_err(|_| IpcError::lock())?;
    let exists = s
        .list_projects()?
        .into_iter()
        .any(|p| p.owner.eq_ignore_ascii_case(owner) && p.repo.eq_ignore_ascii_case(repo));
    if exists {
        return Err(IpcError::new(
            codes::E_EXISTS,
            format!("{owner}/{repo} is already a fleet project"),
        ));
    }
    Ok(())
}

/// `new`'s existing-project check: [`refuse_existing_project`]'s rule, with
/// one exception. When `create_remote` is set and a row for the same
/// `owner/repo` (case-insensitively) is already registered at exactly
/// `local_base` — the path this call would register — it MAY be the row a
/// previous `create_remote` attempt left behind when its GitHub half failed,
/// so the call is let through and `Some((owner, repo))` returns the row's
/// own casing for the re-registration (`register` is an upsert). A row alone
/// is no proof `new` created the repository (a scanned checkout or a clone
/// has one too): the script's [`NEW_PROJECT_TAG_KEY`] guard decides, and
/// refuses anything untagged before any `gh`, push or `ls-remote`. Every other existing-project
/// case refuses exactly like [`refuse_existing_project`]. Takes and drops
/// the lock; no `.await` while held.
fn existing_project_or_resume(
    store: &Mutex<Store>,
    owner: &str,
    repo: &str,
    local_base: &str,
    create_remote: bool,
) -> Result<Option<(String, String)>, IpcError> {
    let s = store.lock().map_err(|_| IpcError::lock())?;
    let same_name: Vec<_> = s
        .list_projects()?
        .into_iter()
        .filter(|p| p.owner.eq_ignore_ascii_case(owner) && p.repo.eq_ignore_ascii_case(repo))
        .collect();
    if same_name.is_empty() {
        return Ok(None);
    }
    if create_remote {
        if let Some(p) = same_name.iter().find(|p| p.base_path == local_base) {
            return Ok(Some((p.owner.clone(), p.repo.clone())));
        }
    }
    Err(IpcError::new(
        codes::E_EXISTS,
        format!("{owner}/{repo} is already a fleet project"),
    ))
}

/// `base_path` already names another project's row. Nothing about adopting a
/// folder stops two different `(owner, repo)` pairs from resolving to the
/// same directory — e.g. adopt with no `origin` (falls back to
/// `local/<basename>`), add an `origin` afterwards, then adopt the same
/// folder again — so the path itself needs its own de-duplication alongside
/// [`refuse_existing_project`]'s name check. Takes and drops the lock; no
/// `.await` while held.
fn refuse_existing_base_path(store: &Mutex<Store>, base_path: &str) -> Result<(), IpcError> {
    let s = store.lock().map_err(|_| IpcError::lock())?;
    let exists = s
        .list_projects()?
        .into_iter()
        .any(|p| p.base_path == base_path);
    if exists {
        return Err(IpcError::new(
            codes::E_EXISTS,
            format!("{base_path} is already registered as a fleet project"),
        ));
    }
    Ok(())
}

/// `(host_root, local_root, layout)` resolved under one lock. `host_root` is
/// `host`'s unexpanded projects root (may start with `~/`, expanded against
/// that host's `$HOME` by the caller); `local_root` is the absolute LOCAL
/// projects root — the registered `base_path` always derives from it, even
/// when the clone lands on a remote host.
fn roots(store: &Mutex<Store>, host: &str) -> Result<(String, String, Layout), IpcError> {
    let s = store.lock().map_err(|_| IpcError::lock())?;
    Ok((
        crate::service::projects::project_base_for(&s, host),
        crate::service::projects::local_projects_root(&s)
            .to_string_lossy()
            .into_owned(),
        crate::service::projects::layout(&s),
    ))
}

/// Run `script` locally via `bash -lc`, bounded by `wall_clock`. Returns the
/// raw `Output` for ANY exit status, same contract as `SshExec::run` —
/// mapping a failure to an `IpcError` is the caller's job (`git_error`).
/// `Err` is reserved for a spawn failure or the wall clock elapsing.
async fn run_local_script(
    script: &str,
    wall_clock: Duration,
) -> Result<std::process::Output, IpcError> {
    let child = tokio::process::Command::new("bash")
        .arg("-lc")
        .arg(script)
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(wall_clock, child).await {
        Ok(res) => res.map_err(|e| IpcError::new(codes::E_SHELL, format!("spawn bash: {e}"))),
        Err(_) => Err(IpcError::new(
            codes::E_TIMEOUT,
            format!("local script exceeded {}s", wall_clock.as_secs()),
        )),
    }
}

/// The clone script's failure at `dest` on `host`, mapped to an `IpcError`.
/// Exit code 3 together with [`ALREADY_CLONED_MARKER`] on stderr (both, so
/// an unrelated command that happens to exit 3 is never misread) is the
/// script's own "already a checkout" guard, reported as `E_EXISTS` with the
/// path so the caller knows where to look. Anything else is `E_GIT_SETUP`
/// naming `owner/repo` and `host`, with stderr (falling back to stdout, and
/// `(no stderr)` when both are empty) — matching the sibling clone error at
/// `service::sessions::lifecycle::git_setup_error`. Shared by the local and
/// remote clone paths — the script text is identical either way, only the
/// transport differs.
fn git_error(
    host: &str,
    owner: &str,
    repo: &str,
    dest: &str,
    out: &std::process::Output,
) -> IpcError {
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if out.status.code() == Some(3) && stderr.contains(ALREADY_CLONED_MARKER) {
        return IpcError::new(
            codes::E_EXISTS,
            format!("already cloned at {dest}; it should appear after a refresh"),
        );
    }
    // The marker is an internal implementation detail of `clone_script`'s own
    // "already a checkout" guard (handled above for its real exit-3 case) —
    // it must never reach a user-facing `E_GIT_SETUP` message, even if some
    // other command happened to echo it. Stripped BEFORE the stderr → stdout
    // → "(no stderr)" fallback ladder below, so a stderr containing only the
    // marker still falls through to stdout instead of the ladder picking
    // "stderr is non-empty", stripping it down to nothing, and reporting a
    // hardcoded "(no stderr)" even though stdout had real diagnostic text.
    let stderr = stderr.replace(ALREADY_CLONED_MARKER, "").trim().to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        "(no stderr)".to_string()
    };
    IpcError::new(
        codes::E_GIT_SETUP,
        format!("couldn't clone {owner}/{repo} on {host}: {detail}"),
    )
}

/// Register `owner`/`repo` at `base_path` (always the LOCAL path the project
/// would occupy) and read the row back as a `ProjectTreeRow` — the same
/// shape `service::projects::list_projects` builds. `adopted` marks a row
/// from the `folder` source (`ProjectRow::adopted`, migration 027) so
/// `refresh_projects`'s stale-rows sweep never deletes it for living outside
/// the scanned root, which is the adopt feature's entire point.
/// `upsert_project`/`upsert_adopted_project` already emit `project:updated`;
/// worktrees are empty until the next scan/refresh.
fn register(
    store: &Mutex<Store>,
    owner: &str,
    repo: &str,
    base_path: &str,
    adopted: bool,
) -> Result<ProjectTreeRow, IpcError> {
    let s = store.lock().map_err(|_| IpcError::lock())?;
    if adopted {
        s.upsert_adopted_project(owner, repo, base_path)?;
    } else {
        s.upsert_project(owner, repo, base_path)?;
    }
    s.list_projects_joined()?
        .into_iter()
        .find(|r| r.project.owner == owner && r.project.repo == repo)
        .ok_or_else(|| {
            IpcError::new(
                codes::E_INTERNAL,
                format!("{owner}/{repo} vanished immediately after being registered"),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh_fake::{FakeSsh, Match, Reply};
    use std::path::Path;

    fn store_with_no_projects() -> Mutex<Store> {
        Mutex::new(Store::open_in_memory().unwrap())
    }

    // ── local-script PATH/HOME stubbing (for a stubbed `gh`, never a real
    // one) — used only by tests that drive `add_project_with`'s LOCAL
    // branch end-to-end use a stub `gh` on `PATH` via `run_hermetic` below,
    // which builds its OWN `Command` (never `run_local_script`) so there is
    // no need to hook production code for it. ──

    /// Write an executable `gh` stub into `dir` whose body is exactly
    /// `body` (a bash script fragment — no shebang/chmod needed from the
    /// caller). Never anything that could pass for the real CLI: the point
    /// is that no test here ever spawns the actual `gh`.
    fn write_stub_gh(dir: &Path, body: &str) {
        let path = dir.join("gh");
        std::fs::write(&path, format!("#!/bin/bash\n{body}")).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }

    /// Point `local_projects_root`/`project_base_for` at `root` for `store`,
    /// so a `local`-host test can control exactly where `new`/`clone` would
    /// write without touching the real `~/projects`.
    fn set_local_projects_root(store: &Mutex<Store>, root: &Path) {
        let s = store.lock().unwrap();
        s.set_setting(
            crate::service::settings::PROJECTS_BASE_PATH,
            &serde_json::json!({ "local": root.to_string_lossy() }).to_string(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn clone_on_a_remote_host_runs_git_clone_and_registers_the_row() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        fake.with_home("/home/u")
            .on(Match::script_contains("git clone"), Reply::ok(""));

        let row = add_project_with(
            AddProjectArgs {
                host_alias: "vps".into(),
                source: AddProjectSource::Clone {
                    url: "https://github.com/o/r".into(),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap();

        assert_eq!(
            (row.project.owner.as_str(), row.project.repo.as_str()),
            ("o", "r")
        );
        assert!(
            row.worktrees.is_empty(),
            "a fresh clone has no scanned worktrees yet"
        );
        let script = fake.calls_for("vps").last().unwrap().script().unwrap();
        assert!(
            script.contains("git clone 'git@github.com:o/r.git' '/home/u/projects/github.com/o/r'"),
            "{script}"
        );
        // The row's base_path is the LOCAL path it would occupy, not the host's.
        let s = store.lock().unwrap();
        let p = s.list_projects().unwrap();
        assert_eq!(p.len(), 1);
        assert!(
            !p[0].base_path.starts_with("/home/u/"),
            "{}",
            p[0].base_path
        );
    }

    #[tokio::test]
    async fn a_bad_url_is_refused_before_any_ssh() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "vps".into(),
                source: AddProjectSource::Clone {
                    url: "https://gitlab.com/o/r".into(),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(
            fake.calls().is_empty(),
            "nothing should run for an unparseable url"
        );
    }

    #[tokio::test]
    async fn a_project_that_already_exists_is_refused() {
        let store = store_with_no_projects();
        store
            .lock()
            .unwrap()
            .upsert_project("o", "r", "/p/o/r")
            .unwrap();
        let fake = FakeSsh::new();
        fake.with_home("/home/u");
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "vps".into(),
                source: AddProjectSource::Clone { url: "o/r".into() },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_EXISTS");
        assert!(err.message.contains("o/r"));
    }

    #[tokio::test]
    async fn an_existing_project_is_refused_case_insensitively() {
        // GitHub and the default macOS filesystem both treat owner/repo
        // case-insensitively, so `Owner/Repo` must collide with `owner/repo`
        // even though the parser preserves typed case.
        let store = store_with_no_projects();
        store
            .lock()
            .unwrap()
            .upsert_project("Owner", "Repo", "/p/Owner/Repo")
            .unwrap();
        let fake = FakeSsh::new();
        fake.with_home("/home/u");
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "vps".into(),
                source: AddProjectSource::Clone {
                    url: "owner/repo".into(),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_EXISTS);
        assert!(fake.calls().is_empty(), "refused before touching ssh");
    }

    #[tokio::test]
    async fn a_failing_clone_leaves_no_project_row() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        fake.with_home("/home/u").on(
            Match::script_contains("git clone"),
            Reply::fail(128, "fatal: repository not found"),
        );
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "vps".into(),
                source: AddProjectSource::Clone { url: "o/r".into() },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_GIT_SETUP");
        assert!(err.message.contains("repository not found"));
        assert!(store.lock().unwrap().list_projects().unwrap().is_empty());
    }

    #[tokio::test]
    async fn local_clone_runs_git_and_a_second_attempt_reports_e_exists() {
        // `clone_url_for` always targets github.com, so exercising the
        // `local` branch of `clone_source` end-to-end would need network
        // access. This drives `run_local_script` + `clone_script` directly
        // — the pair the local branch calls — against a real local bare
        // repo, proving the local execution path (no `FakeSsh` involved)
        // actually runs git and maps a repeat clone to `E_EXISTS`.
        let src = tempfile::tempdir().unwrap();
        let ok = std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(src.path())
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git init --bare failed");
        let dest_parent = tempfile::tempdir().unwrap();
        let dest = dest_parent.path().join("checkout");
        let clone_url = format!("file://{}", src.path().display());
        let dest_str = dest.to_str().unwrap();
        let script = clone_script(dest_str, &clone_url);

        let out = run_local_script(&script, Duration::from_secs(10))
            .await
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        assert!(dest.join(".git").is_dir());

        let out2 = run_local_script(&script, Duration::from_secs(10))
            .await
            .unwrap();
        assert!(!out2.status.success());
        let err = git_error("local", "o", "r", dest_str, &out2);
        assert_eq!(err.code, codes::E_EXISTS);
        assert!(err.message.contains(dest_str), "{}", err.message);
    }

    #[tokio::test]
    async fn the_clone_script_quotes_an_owner_with_a_space_and_a_quote() {
        // parse_repo_url rejects those characters, so drive the script
        // builder directly to prove the quoting.
        let script = clone_script("/ro ot/o'x/r", "git@github.com:o'x/r.git");
        assert!(
            script.contains(&crate::shell::quote("/ro ot/o'x/r")),
            "{script}"
        );
        assert!(
            script.contains(&crate::shell::quote("git@github.com:o'x/r.git")),
            "{script}"
        );
        assert!(script.contains("mkdir -p \"$(dirname --"), "{script}");
    }

    #[tokio::test]
    async fn an_invalid_host_alias_is_refused_before_any_ssh() {
        // Same guard, same shape as `a_bad_url_is_refused_before_any_ssh`,
        // but for the alias itself: `add_project_with` is a Tauri-reachable
        // entry point (Task 5), so a hostile alias (e.g. one starting with
        // `-`, which `ssh` would parse as an option) must never reach an ssh
        // argv, regardless of which source is requested.
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "-oProxyCommand=evil".into(),
                source: AddProjectSource::Clone { url: "o/r".into() },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(
            fake.calls().is_empty(),
            "nothing should run for an invalid host alias"
        );
    }

    #[test]
    fn git_setup_exit_code_3_without_the_marker_is_not_mistaken_for_e_exists() {
        // Exit code 3 alone is not enough — only code 3 TOGETHER WITH
        // `ALREADY_CLONED_MARKER` on stderr is `clone_script`'s own guard.
        // Some other command that happens to exit 3 for an unrelated reason
        // must still surface as E_GIT_SETUP, which is exactly the case the
        // marker hardening exists to cover.
        let out = std::process::Output {
            status: std::os::unix::process::ExitStatusExt::from_raw(3 << 8),
            stdout: Vec::new(),
            stderr: b"some other failure".to_vec(),
        };
        let err = git_error("vps", "o", "r", "/p/o/r", &out);
        assert_eq!(err.code, codes::E_GIT_SETUP);
        assert!(
            err.message.contains("some other failure"),
            "{}",
            err.message
        );
    }

    #[test]
    fn git_error_falls_back_to_stdout_when_stderr_is_only_the_marker() {
        // A stderr containing NOTHING but the marker (exit code not 3, so it
        // isn't `clone_script`'s own "already a checkout" guard) must still
        // fall through to stdout, not collapse to a hardcoded "(no stderr)"
        // that throws away real diagnostic text — and the marker itself must
        // never reach the user-facing message.
        let out = std::process::Output {
            status: std::os::unix::process::ExitStatusExt::from_raw(1 << 8),
            stdout: b"real diagnostic from stdout".to_vec(),
            stderr: ALREADY_CLONED_MARKER.as_bytes().to_vec(),
        };
        let err = git_error("vps", "o", "r", "/p/o/r", &out);
        assert_eq!(err.code, codes::E_GIT_SETUP);
        assert!(
            err.message.contains("real diagnostic from stdout"),
            "{}",
            err.message
        );
        assert!(
            !err.message.contains(ALREADY_CLONED_MARKER),
            "the marker must never reach a user-facing message: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn folder_is_refused_for_a_remote_host() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "vps".into(),
                source: AddProjectSource::Folder {
                    path: "/srv/r".into(),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(err.message.contains("local"));
        assert!(fake.calls().is_empty());
    }

    #[tokio::test]
    async fn folder_adopts_a_real_checkout_and_reads_its_origin() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("my-repo");
        std::fs::create_dir_all(&path).unwrap();
        for args in [
            vec!["init", "-b", "main"],
            vec!["remote", "add", "origin", "git@github.com:acme/widget.git"],
        ] {
            let ok = std::process::Command::new("git")
                .args(&args)
                .current_dir(&path)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        }
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        let row = add_project_with(
            AddProjectArgs {
                host_alias: "local".into(),
                source: AddProjectSource::Folder {
                    path: path.to_string_lossy().into(),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap();
        assert_eq!(
            (row.project.owner.as_str(), row.project.repo.as_str()),
            ("acme", "widget")
        );
        // Registered where it is — nothing moved.
        assert_eq!(
            row.project.base_path,
            path.canonicalize().unwrap().to_string_lossy()
        );
        assert!(
            row.project.adopted,
            "a folder-sourced row must be flagged adopted"
        );
    }

    #[tokio::test]
    async fn folder_without_an_origin_falls_back_to_the_directory_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("loose-repo");
        std::fs::create_dir_all(&path).unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&path)
            .output()
            .unwrap()
            .status
            .success());
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        let row = add_project_with(
            AddProjectArgs {
                host_alias: "local".into(),
                source: AddProjectSource::Folder {
                    path: path.to_string_lossy().into(),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap();
        assert_eq!(row.project.repo, "loose-repo");
        assert_eq!(row.project.owner, "local");
    }

    #[tokio::test]
    async fn folder_that_is_not_a_checkout_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "local".into(),
                source: AddProjectSource::Folder {
                    path: dir.path().to_string_lossy().into(),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(err.message.contains("git"));
    }

    #[tokio::test]
    async fn folder_that_already_exists_as_a_project_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dup-repo");
        std::fs::create_dir_all(&path).unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&path)
            .output()
            .unwrap()
            .status
            .success());
        let store = store_with_no_projects();
        store
            .lock()
            .unwrap()
            .upsert_project("local", "dup-repo", "/somewhere/else")
            .unwrap();
        let fake = FakeSsh::new();
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "local".into(),
                source: AddProjectSource::Folder {
                    path: path.to_string_lossy().into(),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_EXISTS);
    }

    fn git_ok(dir: &std::path::Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap()
            .status
            .success()
    }

    #[tokio::test]
    async fn folder_adopting_a_linked_worktree_registers_the_main_checkout() {
        use crate::projects::test_git::{init_repo, run};
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("widget");
        if !init_repo(&main) {
            return; // no git on this box
        }
        assert!(run(
            &main,
            &["remote", "add", "origin", "git@github.com:acme/widget.git"]
        ));
        let wt = dir.path().join("wt-feature");
        assert!(run(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                wt.to_str().unwrap(),
                "-b",
                "feature"
            ]
        ));

        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        // Adopt the LINKED WORKTREE's path, not the main checkout's.
        let row = add_project_with(
            AddProjectArgs {
                host_alias: "local".into(),
                source: AddProjectSource::Folder {
                    path: wt.to_string_lossy().into(),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap();
        assert_eq!(
            (row.project.owner.as_str(), row.project.repo.as_str()),
            ("acme", "widget")
        );
        assert_eq!(
            row.project.base_path,
            crate::projects::path_identity::canonical(&main).to_string_lossy(),
            "the MAIN checkout is registered, not the linked worktree"
        );
    }

    #[tokio::test]
    async fn folder_adopting_a_bare_repos_worktree_registers_the_worktree_not_the_bare_dir() {
        // A bare repo's worktrees also fail the git-dir/git-common-dir
        // comparison (same shape as a linked worktree), but the bare repo
        // itself has no working tree — it is the same "not a checkout" state
        // `folder_pointing_at_a_bare_repo_is_refused` refuses when adopted
        // directly. `resolve_main_checkout` must not "resolve" into it.
        use crate::projects::test_git::run;
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("widget.git");
        if !std::process::Command::new("git")
            .args(["init", "-q", "--bare"])
            .arg(&bare)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return; // no git on this box
        }
        assert!(git_ok(
            &bare,
            &["remote", "add", "origin", "git@github.com:acme/widget.git"]
        ));

        // Seed the bare repo with one commit on `main` from a throwaway
        // clone, so `worktree add` has a branch to check out.
        let seed = dir.path().join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        assert!(run(&seed, &["init", "-q", "-b", "main"]));
        assert!(run(&seed, &["commit", "--allow-empty", "-q", "-m", "init"]));
        assert!(run(
            &seed,
            &["remote", "add", "origin", bare.to_str().unwrap()]
        ));
        assert!(run(&seed, &["push", "-q", "origin", "main"]));

        let wt = dir.path().join("wt-main");
        assert!(git_ok(
            &bare,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "main"]
        ));

        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        let row = add_project_with(
            AddProjectArgs {
                host_alias: "local".into(),
                source: AddProjectSource::Folder {
                    path: wt.to_string_lossy().into(),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap();
        assert_eq!(
            (row.project.owner.as_str(), row.project.repo.as_str()),
            ("acme", "widget")
        );
        assert_eq!(
            row.project.base_path,
            wt.canonicalize().unwrap().to_string_lossy(),
            "the worktree the user picked is registered, not the bare repo"
        );
    }

    #[tokio::test]
    async fn folder_from_a_subdirectory_registers_the_top_level() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repo");
        let sub = path.join("src").join("nested");
        std::fs::create_dir_all(&sub).unwrap();
        assert!(git_ok(&path, &["init", "-q", "-b", "main"]));
        assert!(git_ok(
            &path,
            &["remote", "add", "origin", "git@github.com:acme/repo.git"]
        ));
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        let row = add_project_with(
            AddProjectArgs {
                host_alias: "local".into(),
                source: AddProjectSource::Folder {
                    path: sub.to_string_lossy().into(),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap();
        assert_eq!(
            row.project.base_path,
            path.canonicalize().unwrap().to_string_lossy()
        );
    }

    #[tokio::test]
    async fn folder_with_a_non_github_origin_falls_back_to_the_directory_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gl-repo");
        std::fs::create_dir_all(&path).unwrap();
        assert!(git_ok(&path, &["init", "-q", "-b", "main"]));
        assert!(git_ok(
            &path,
            &["remote", "add", "origin", "git@gitlab.com:acme/repo.git"]
        ));
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        let row = add_project_with(
            AddProjectArgs {
                host_alias: "local".into(),
                source: AddProjectSource::Folder {
                    path: path.to_string_lossy().into(),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap();
        assert_eq!(row.project.owner, "local");
        assert_eq!(row.project.repo, "gl-repo");
    }

    #[tokio::test]
    async fn folder_sanitizes_a_basename_with_spaces_instead_of_storing_it_raw() {
        // `parse_repo_url`'s invariant is that a parsed pair is always safe
        // to interpolate into a path — a raw filesystem basename never went
        // through that check, so a name like "my repo" must be sanitized,
        // not stored verbatim.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("my repo");
        std::fs::create_dir_all(&path).unwrap();
        assert!(git_ok(&path, &["init", "-q", "-b", "main"]));
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        let row = add_project_with(
            AddProjectArgs {
                host_alias: "local".into(),
                source: AddProjectSource::Folder {
                    path: path.to_string_lossy().into(),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap();
        assert_eq!(row.project.owner, "local");
        assert_eq!(row.project.repo, "my-repo");
    }

    #[tokio::test]
    async fn folder_with_a_relative_path_is_refused_before_the_probe() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "local".into(),
                source: AddProjectSource::Folder {
                    path: "relative/repo".into(),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
    }

    #[tokio::test]
    async fn folder_pointing_at_a_bare_repo_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bare.git");
        assert!(std::process::Command::new("git")
            .args(["init", "-q", "--bare"])
            .arg(&path)
            .output()
            .unwrap()
            .status
            .success());
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "local".into(),
                source: AddProjectSource::Folder {
                    path: path.to_string_lossy().into(),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
    }

    #[tokio::test]
    async fn folder_refuses_a_path_already_registered_by_another_project() {
        // Today two rows could share one path: adopt with no origin (falls
        // back to local/<basename>), add an origin afterwards, adopt again.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared-repo");
        std::fs::create_dir_all(&path).unwrap();
        assert!(git_ok(&path, &["init", "-q", "-b", "main"]));
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        add_project_with(
            AddProjectArgs {
                host_alias: "local".into(),
                source: AddProjectSource::Folder {
                    path: path.to_string_lossy().into(),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap();
        assert!(git_ok(
            &path,
            &[
                "remote",
                "add",
                "origin",
                "git@github.com:acme/shared-repo.git"
            ]
        ));
        // Same path, now with an origin — a different (owner, repo) pair, so
        // `refuse_existing_project`'s name check would not catch this alone.
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "local".into(),
                source: AddProjectSource::Folder {
                    path: path.to_string_lossy().into(),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_EXISTS);
    }

    #[tokio::test]
    async fn folder_adoption_writes_nothing_to_the_checkout() {
        fn snapshot(
            dir: &std::path::Path,
        ) -> Vec<(std::path::PathBuf, u64, std::time::SystemTime)> {
            fn walk(
                dir: &std::path::Path,
                out: &mut Vec<(std::path::PathBuf, u64, std::time::SystemTime)>,
            ) {
                for entry in std::fs::read_dir(dir).unwrap() {
                    let entry = entry.unwrap();
                    let meta = entry.metadata().unwrap();
                    if meta.is_dir() {
                        walk(&entry.path(), out);
                    } else {
                        out.push((entry.path(), meta.len(), meta.modified().unwrap()));
                    }
                }
            }
            let mut out = Vec::new();
            walk(dir, &mut out);
            out.sort();
            out
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("untouched");
        std::fs::create_dir_all(&path).unwrap();
        assert!(git_ok(&path, &["init", "-q", "-b", "main"]));
        assert!(git_ok(
            &path,
            &[
                "remote",
                "add",
                "origin",
                "git@github.com:acme/untouched.git"
            ]
        ));
        let before = snapshot(&path);

        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        add_project_with(
            AddProjectArgs {
                host_alias: "local".into(),
                source: AddProjectSource::Folder {
                    path: path.to_string_lossy().into(),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap();

        let after = snapshot(&path);
        assert_eq!(before, after, "adopting a folder must never write into it");
    }

    #[tokio::test]
    async fn new_creates_a_repo_with_an_initial_commit() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        // `Match::Any` must be the OLDER rule: FakeSsh's "most recently added
        // rule wins" means an `Any` reply added AFTER `with_home` would also
        // catch the `printenv HOME` call `remote_home` makes and blank it
        // out, so it goes first here and `with_home` narrows it back after.
        fake.on(Match::Any, Reply::ok("")).with_home("/home/u");
        let row = add_project_with(
            AddProjectArgs {
                host_alias: "vps".into(),
                source: AddProjectSource::New {
                    owner: "acme".into(),
                    repo: "widget".into(),
                    create_remote: false,
                    confirm: None,
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap();
        assert_eq!(row.project.repo, "widget");
        let script = fake.calls_for("vps").last().unwrap().script().unwrap();
        // NOT `git init -b main`: that flag needs git >=2.28 (see
        // `new_project_script`'s doc comment).
        assert!(script.contains("git init"), "{script}");
        assert!(
            script.contains("git symbolic-ref HEAD refs/heads/main"),
            "{script}"
        );
        assert!(script.contains("commit --allow-empty"), "{script}");
        assert!(!script.contains("gh repo create"), "{script}");
    }

    /// Pull the minted confirmation token out of an `E_CONFIRM_REQUIRED`
    /// error's `details` — the exact shape the frontend dialog reads.
    fn confirm_token_from(err: &IpcError) -> String {
        err.details
            .as_ref()
            .and_then(|d| d.get("confirm"))
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("no confirm token in details: {err:?}"))
            .to_string()
    }

    #[tokio::test]
    async fn create_remote_without_the_confirmation_is_refused() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "vps".into(),
                source: AddProjectSource::New {
                    owner: "acme".into(),
                    repo: "widget".into(),
                    create_remote: true,
                    confirm: None,
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_CONFIRM_REQUIRED);
        assert!(!confirm_token_from(&err).is_empty());
        assert!(
            fake.calls().is_empty(),
            "the confirm gate must run before any ssh/local command"
        );
    }

    #[tokio::test]
    async fn create_remote_with_the_confirmation_runs_gh_repo_create() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        // See the comment in `new_creates_a_repo_with_an_initial_commit`:
        // `Match::Any` must be added before `with_home`, not after.
        fake.on(Match::Any, Reply::ok("")).with_home("/home/u");
        let new_args = || AddProjectArgs {
            host_alias: "vps".into(),
            source: AddProjectSource::New {
                owner: "acme".into(),
                repo: "widget".into(),
                create_remote: true,
                confirm: None,
            },
            call_id: None,
        };
        // Step 1: no token yet -> mint one (the real two-call flow the
        // frontend dialog will drive).
        let mint_err = add_project_with(new_args(), &store, &fake)
            .await
            .unwrap_err();
        let token = confirm_token_from(&mint_err);

        // Step 2: same request, now WITH the token.
        let mut args = new_args();
        if let AddProjectSource::New { confirm, .. } = &mut args.source {
            *confirm = Some(token);
        }
        add_project_with(args, &store, &fake).await.unwrap();

        let script = fake.calls_for("vps").last().unwrap().script().unwrap();
        assert!(
            script.contains("gh repo create 'acme/widget' --private"),
            "{script}"
        );
        // (`git remote get-url --push` elsewhere in the script is a read.)
        let gh_line = script
            .lines()
            .find(|l| l.starts_with("gh repo create"))
            .unwrap();
        assert!(
            !gh_line.contains("--push"),
            "create and push must be SEPARATE steps: {script}"
        );
        assert!(script.contains("git push -u origin main"), "{script}");
    }

    #[tokio::test]
    async fn create_remote_confirmation_for_a_different_repo_is_refused() {
        // A token minted for one repo must not authorize a DIFFERENT one —
        // e.g. a stale token left over from switching the owner/repo fields.
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        let token = confirm_tokens().mint("vps", "acme", "OTHER", Instant::now());
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "vps".into(),
                source: AddProjectSource::New {
                    owner: "acme".into(),
                    repo: "widget".into(),
                    create_remote: true,
                    confirm: Some(token),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_CONFIRM_REQUIRED);
        assert!(fake.calls().is_empty());
    }

    #[tokio::test]
    async fn create_remote_confirmation_token_is_single_use() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        fake.on(Match::Any, Reply::ok("")).with_home("/home/u");
        let token = confirm_tokens().mint("vps", "acme", "widget", Instant::now());
        let new_args = |confirm: Option<String>| AddProjectArgs {
            host_alias: "vps".into(),
            source: AddProjectSource::New {
                owner: "acme".into(),
                repo: "widget".into(),
                create_remote: true,
                confirm,
            },
            call_id: None,
        };
        add_project_with(new_args(Some(token.clone())), &store, &fake)
            .await
            .unwrap();
        // Drive the token check directly: through `add_project_with` a
        // replay would be let through `existing_project_or_resume` (same
        // row, same path) and then simply mint a new token, which says
        // nothing about whether the OLD one is still accepted.
        let now = Instant::now();
        assert!(
            !confirm_tokens().consume(&token, "vps", "acme", "widget", now),
            "a consumed token must not be usable again"
        );
    }

    #[test]
    fn confirm_token_expires_after_its_ttl() {
        let t0 = Instant::now();
        let token = confirm_tokens().mint("vps", "acme", "ttl-test", t0);
        assert!(
            !confirm_tokens().consume(
                &token,
                "vps",
                "acme",
                "ttl-test",
                t0 + CONFIRM_TOKEN_TTL + Duration::from_secs(1)
            ),
            "a token must not validate once its TTL has elapsed"
        );
        // Freshly minted, it validates right up to (but not past) the TTL.
        let token2 = confirm_tokens().mint("vps", "acme", "ttl-test-2", t0);
        assert!(confirm_tokens().consume(
            &token2,
            "vps",
            "acme",
            "ttl-test-2",
            t0 + CONFIRM_TOKEN_TTL - Duration::from_secs(1)
        ));
    }

    #[tokio::test]
    async fn new_refuses_an_existing_project_before_any_command() {
        let store = store_with_no_projects();
        store
            .lock()
            .unwrap()
            .upsert_project("acme", "widget", "/p/acme/widget")
            .unwrap();
        let fake = FakeSsh::new();
        fake.with_home("/home/u").on(Match::Any, Reply::ok(""));
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "vps".into(),
                source: AddProjectSource::New {
                    owner: "acme".into(),
                    repo: "widget".into(),
                    create_remote: false,
                    confirm: None,
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_EXISTS);
        assert!(
            fake.calls().is_empty(),
            "refused before touching ssh, same as clone/folder"
        );
    }

    #[tokio::test]
    async fn new_rejects_an_invalid_owner_or_repo_name() {
        let cases: Vec<(String, String)> = vec![
            ("-rf".to_string(), "widget".to_string()),
            ("acme".to_string(), "-rf".to_string()),
            ("a".repeat(40), "widget".to_string()), // owner: over the 39-char cap
            ("acme".to_string(), "a".repeat(101)),  // repo: over the 100-char cap
            (".git".to_string(), "widget".to_string()),
            ("acme".to_string(), ".git".to_string()),
        ];
        for (owner, repo) in cases {
            let store = store_with_no_projects();
            let fake = FakeSsh::new();
            let err = add_project_with(
                AddProjectArgs {
                    host_alias: "vps".into(),
                    source: AddProjectSource::New {
                        owner: owner.clone(),
                        repo: repo.clone(),
                        create_remote: false,
                        confirm: None,
                    },
                    call_id: None,
                },
                &store,
                &fake,
            )
            .await
            .unwrap_err();
            assert_eq!(err.code, codes::E_INVALID, "{owner}/{repo}");
            assert!(
                fake.calls().is_empty(),
                "an invalid name must never reach ssh: {owner}/{repo}"
            );
        }
    }

    #[tokio::test]
    async fn new_on_local_runs_a_real_git_init_and_commit() {
        // Exercise the LOCAL branch (no FakeSsh involved) against a real
        // temporary directory, proving `new_project_script` actually
        // produces a checkout with a commit `git worktree add` can fork
        // from.
        let projects_root = tempfile::tempdir().unwrap();
        let store = store_with_no_projects();
        set_local_projects_root(&store, projects_root.path());
        let fake = FakeSsh::new();
        let row = add_project_with(
            AddProjectArgs {
                host_alias: "local".into(),
                source: AddProjectSource::New {
                    owner: "acme".into(),
                    repo: "newrepo".into(),
                    create_remote: false,
                    confirm: None,
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap();
        assert_eq!(row.project.repo, "newrepo");
        let head = std::process::Command::new("git")
            .args(["-C", &row.project.base_path, "log", "-1", "--format=%s"])
            .output()
            .unwrap();
        assert!(head.status.success(), "{head:?}");
        assert_eq!(
            String::from_utf8_lossy(&head.stdout).trim(),
            "Initial commit"
        );
    }

    #[tokio::test]
    async fn new_refuses_when_the_destination_exists_and_is_not_a_git_repository() {
        let projects_root = tempfile::tempdir().unwrap();
        let store = store_with_no_projects();
        set_local_projects_root(&store, projects_root.path());
        // Pre-create a plain, non-git directory at exactly the path `new`
        // would use.
        let dest = projects_root.path().join("acme").join("widget");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("some-file"), "not a git repo").unwrap();
        let fake = FakeSsh::new();
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "local".into(),
                source: AddProjectSource::New {
                    owner: "acme".into(),
                    repo: "widget".into(),
                    create_remote: false,
                    confirm: None,
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_EXISTS);
        assert!(
            err.message.contains("not a git repository"),
            "{}",
            err.message
        );
        assert!(!err.message.contains("already cloned"), "{}", err.message);
    }

    #[test]
    fn new_project_script_only_substitutes_a_missing_identity_field_when_create_remote_is_off() {
        // The script always embeds BOTH branches — the choice of which one
        // actually runs happens at shell-execution time, not at script-build
        // time — so this asserts the conditional structure itself: EACH
        // field is checked (and substituted) independently, so a real email
        // survives even when only the name is missing.
        let script = new_project_script("/root/acme/widget", "acme", "widget", false);
        assert!(
            script.contains("if ! git config user.name >/dev/null 2>&1"),
            "{script}"
        );
        assert!(
            script.contains("if ! git config user.email >/dev/null 2>&1"),
            "{script}"
        );
        assert!(
            script.contains(&format!("user.name={FALLBACK_GIT_NAME}")),
            "{script}"
        );
        assert!(
            script.contains(&format!("user.email={FALLBACK_GIT_EMAIL}")),
            "{script}"
        );
        assert!(
            !script.contains(NO_GIT_IDENTITY_MARKER),
            "the refuse-outright marker belongs only to the create_remote script: {script}"
        );
    }

    #[test]
    fn new_project_script_with_create_remote_never_embeds_the_placeholder_identity() {
        // With `create_remote` on, a placeholder author must never even be
        // MENTIONED in the script that will commit to a real GitHub repo —
        // a missing identity refuses outright instead.
        let script = new_project_script("/root/acme/widget", "acme", "widget", true);
        // (Matched as `user.*=` values: the bare name `claude-fleet` also
        // prefixes the script's own `claude-fleet.new-project` tag key.)
        assert!(
            !script.contains(&format!("user.name={FALLBACK_GIT_NAME}"))
                && !script.contains(FALLBACK_GIT_EMAIL),
            "{script}"
        );
        assert!(script.contains(NO_GIT_IDENTITY_MARKER), "{script}");
        assert!(script.contains("exit 5"), "{script}");
    }

    /// Run `script` with `bash -c` (never `-lc`, so `/etc/profile` is never
    /// sourced) under a FULLY CLEARED environment, plus exactly the
    /// variables a hermetic git-identity resolution needs: `PATH`
    /// (`extra_path` first, so a test's own stubbed `gh` can be picked up,
    /// then an always-present guard directory — see below — then just
    /// enough of the real system to find `bash`/`git`), `HOME`,
    /// `GIT_CONFIG_NOSYSTEM=1`, a fresh EMPTY `XDG_CONFIG_HOME` (git also
    /// reads `$XDG_CONFIG_HOME/git/config` as a global fallback — a real one
    /// on the test host would otherwise leak an identity in), and
    /// `GIT_CONFIG_GLOBAL` pointed at exactly `global_gitconfig` (`/dev/null`
    /// for "no identity at all") so `~/.gitconfig` itself is irrelevant.
    /// `.env_clear()` also drops any inherited `GIT_DIR`/`GIT_WORK_TREE`/
    /// `GIT_INDEX_FILE` — e.g. from a git hook running `cargo test` — which
    /// would otherwise aim `git init` at the OUTER repository instead of
    /// `dest`.
    ///
    /// # The `gh` guard
    ///
    /// The code under test (`new_project_script` with `create_remote` set)
    /// can run `gh repo create` against the user's REAL GitHub account — a
    /// public side effect, not a hermetic no-op. This happened once already
    /// during development: a login shell (`bash -lc`, not `-c`) re-prepended
    /// Homebrew's `bin` ahead of an intended `gh` stub, and the real `gh`
    /// only failed to actually create anything because the stripped
    /// environment had no auth token — luck, not a guarantee, since on
    /// macOS `gh` can read its token straight out of the system keychain
    /// regardless of `HOME`/env.
    ///
    /// So every call here gets a SECOND, always-present `PATH` entry, right
    /// after `extra_path`: a throwaway directory holding a `gh` script that
    /// does nothing but print `BLOCKED: the real gh must never run from a
    /// test` to stderr and exit 99. A test that supplies its own `gh` stub
    /// in `extra_path` still wins (it comes first on `PATH`); a test that
    /// passes an empty `extra_path` — or a future reordering of the script
    /// that reaches the `gh` stage before whatever earlier check used to
    /// make a missing stub safe — hits this guard instead of falling
    /// through to a real `gh`.
    ///
    /// `PATH` deliberately excludes `/usr/local/bin` and `/opt/homebrew/bin`:
    /// those are exactly where a real, authenticated `gh` lives (Homebrew on
    /// macOS; common manual installs on Linux land in `/usr/local/bin`).
    /// `/usr/bin` and `/bin` are enough to find `git`/`bash` on both macOS
    /// (Apple Git) and standard Linux. Keep it this way — widening `PATH`
    /// back out reopens the exact hole this guard exists to close.
    async fn run_hermetic(
        script: &str,
        extra_path: &Path,
        home: &Path,
        global_gitconfig: &str,
    ) -> std::process::Output {
        let xdg = tempfile::tempdir().unwrap();
        let gh_guard = tempfile::tempdir().unwrap();
        write_stub_gh(
            gh_guard.path(),
            "echo 'BLOCKED: the real gh must never run from a test' >&2\nexit 99\n",
        );
        tokio::process::Command::new("bash")
            .arg("-c")
            .arg(script)
            .env_clear()
            .env(
                "PATH",
                format!(
                    "{}:{}:/usr/bin:/bin",
                    extra_path.display(),
                    gh_guard.path().display()
                ),
            )
            .env("HOME", home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("XDG_CONFIG_HOME", xdg.path())
            .env("GIT_CONFIG_GLOBAL", global_gitconfig)
            .output()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn run_hermetic_blocks_a_real_gh_invocation_when_no_stub_is_supplied() {
        // The regression test for the whole class of bug this hardening
        // fixes: a script that reaches `gh` with no test-supplied stub on
        // `extra_path` must hit the always-present guard — exit 99 with the
        // BLOCKED marker on stderr — and never a real `gh` on the system
        // `PATH`.
        let home = tempfile::tempdir().unwrap();
        let empty_bin = tempfile::tempdir().unwrap();
        let out = run_hermetic(
            "gh repo create acme/widget --private",
            empty_bin.path(),
            home.path(),
            "/dev/null",
        )
        .await;
        assert_eq!(out.status.code(), Some(99), "{out:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr)
                .contains("BLOCKED: the real gh must never run from a test"),
            "{out:?}"
        );
    }

    fn author_of(dest: &Path) -> String {
        let head = std::process::Command::new("git")
            .args([
                "-C",
                dest.to_str().unwrap(),
                "log",
                "-1",
                "--format=%an <%ae>",
            ])
            .output()
            .unwrap();
        String::from_utf8_lossy(&head.stdout).trim().to_string()
    }

    #[tokio::test]
    async fn new_project_commits_with_the_hosts_own_identity_when_one_is_configured() {
        // Runs the REAL script (no FakeSsh) with a git identity configured
        // ONLY via `GIT_CONFIG_GLOBAL`, proving the "already configured"
        // branch is the one that actually executes and that it keeps the
        // user's own identity rather than the placeholder.
        let home = tempfile::tempdir().unwrap();
        let empty_bin = tempfile::tempdir().unwrap();
        let global_gitconfig = home.path().join("global.gitconfig");
        std::fs::write(
            &global_gitconfig,
            "[user]\n\tname = Real User\n\temail = real@example.com\n",
        )
        .unwrap();
        let dest_parent = tempfile::tempdir().unwrap();
        let dest = dest_parent.path().join("configured-identity");
        let script = new_project_script(dest.to_str().unwrap(), "acme", "widget", false);

        let out = run_hermetic(
            &script,
            empty_bin.path(),
            home.path(),
            global_gitconfig.to_str().unwrap(),
        )
        .await;
        assert!(out.status.success(), "{out:?}");
        assert_eq!(
            author_of(&dest),
            "Real User <real@example.com>",
            "a configured identity must be kept, not replaced by the placeholder"
        );
    }

    #[tokio::test]
    async fn new_project_falls_back_to_a_placeholder_identity_when_none_is_configured() {
        // Same real-execution proof as above, for the opposite branch: NO
        // identity anywhere (`GIT_CONFIG_GLOBAL=/dev/null`, no system
        // config) must still produce a commit, via the placeholder.
        let home = tempfile::tempdir().unwrap();
        let empty_bin = tempfile::tempdir().unwrap();
        let dest_parent = tempfile::tempdir().unwrap();
        let dest = dest_parent.path().join("no-identity");
        let script = new_project_script(dest.to_str().unwrap(), "acme", "widget", false);

        let out = run_hermetic(&script, empty_bin.path(), home.path(), "/dev/null").await;
        assert!(out.status.success(), "{out:?}");
        assert_eq!(
            author_of(&dest),
            format!("{FALLBACK_GIT_NAME} <{FALLBACK_GIT_EMAIL}>"),
            "with no identity configured anywhere, the placeholder must be used"
        );
    }

    #[tokio::test]
    async fn new_project_partial_identity_only_substitutes_the_missing_field() {
        // Only `user.email` configured: the REAL email must be kept, and
        // only the missing `user.name` substituted — not both wholesale.
        let home = tempfile::tempdir().unwrap();
        let empty_bin = tempfile::tempdir().unwrap();
        let global_gitconfig = home.path().join("global.gitconfig");
        std::fs::write(&global_gitconfig, "[user]\n\temail = real@example.com\n").unwrap();
        let dest_parent = tempfile::tempdir().unwrap();
        let dest = dest_parent.path().join("partial-identity");
        let script = new_project_script(dest.to_str().unwrap(), "acme", "widget", false);

        let out = run_hermetic(
            &script,
            empty_bin.path(),
            home.path(),
            global_gitconfig.to_str().unwrap(),
        )
        .await;
        assert!(out.status.success(), "{out:?}");
        assert_eq!(
            author_of(&dest),
            format!("{FALLBACK_GIT_NAME} <real@example.com>"),
            "the real email must survive; only the missing name is substituted"
        );
    }

    #[tokio::test]
    async fn new_with_create_remote_refuses_a_missing_identity_instead_of_the_placeholder() {
        // With `create_remote` on, a placeholder author must NEVER reach a
        // real GitHub repository — refuse outright before any commit at
        // all, rather than substituting one.
        let home = tempfile::tempdir().unwrap();
        let empty_bin = tempfile::tempdir().unwrap();
        let dest_parent = tempfile::tempdir().unwrap();
        let dest = dest_parent.path().join("no-identity-remote");
        let script = new_project_script(dest.to_str().unwrap(), "acme", "widget", true);

        let out = run_hermetic(&script, empty_bin.path(), home.path(), "/dev/null").await;
        assert!(!out.status.success());
        assert_eq!(out.status.code(), Some(5));
        assert!(String::from_utf8_lossy(&out.stderr).contains(NO_GIT_IDENTITY_MARKER));

        let err = new_project_error("local", "acme", "widget", dest.to_str().unwrap(), &out);
        assert_eq!(err.code, codes::E_INVALID);
        assert!(
            err.message.contains("git config --global"),
            "{}",
            err.message
        );

        assert!(
            !dest.exists(),
            "the identity check must run before mkdir, leaving nothing behind to resume"
        );
    }

    // ── new_project_error: pure, stage-marker-driven mapping ────────────────

    fn output_with(code: i32, stderr: &str) -> std::process::Output {
        std::process::Output {
            status: std::os::unix::process::ExitStatusExt::from_raw(code << 8),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn new_project_error_reports_already_exists_distinctly_from_not_a_git_repo() {
        let already = output_with(3, ALREADY_CLONED_MARKER);
        let err = new_project_error("vps", "o", "r", "/p/o/r", &already);
        assert_eq!(err.code, codes::E_EXISTS);
        assert!(err.message.contains("already exists"), "{}", err.message);

        let not_git = output_with(4, NOT_A_GIT_REPO_MARKER);
        let err = new_project_error("vps", "o", "r", "/p/o/r", &not_git);
        assert_eq!(err.code, codes::E_EXISTS);
        assert!(
            err.message.contains("not a git repository"),
            "{}",
            err.message
        );
    }

    #[test]
    fn new_project_error_at_the_gh_create_stage_says_the_local_repo_is_kept() {
        let out = output_with(
            1,
            &format!("{STAGE_INIT}\n{STAGE_COMMIT}\n{STAGE_GH_CREATE}\nsome gh failure"),
        );
        let err = new_project_error("vps", "acme", "widget", "/p/acme/widget", &out);
        assert_eq!(err.code, codes::E_GH);
        assert!(err.message.contains("some gh failure"), "{}", err.message);
        assert!(err.message.contains("is kept"), "{}", err.message);
        assert!(err.message.contains("retry"), "{}", err.message);
    }

    #[test]
    fn new_project_error_at_the_gh_create_stage_with_exit_127_says_install_gh() {
        let out = output_with(127, &format!("{STAGE_COMMIT}\n{STAGE_GH_CREATE}\n"));
        let err = new_project_error("vps", "acme", "widget", "/p/acme/widget", &out);
        assert_eq!(err.code, codes::E_GH);
        assert!(
            err.message.to_lowercase().contains("install"),
            "{}",
            err.message
        );
        assert!(err.message.contains("is kept"), "{}", err.message);
    }

    #[test]
    fn new_project_error_at_the_gh_push_stage_says_the_repo_may_already_exist() {
        let out = output_with(
            1,
            &format!("{STAGE_COMMIT}\n{STAGE_GH_CREATE}\n{STAGE_GH_PUSH}\npush failed"),
        );
        let err = new_project_error("vps", "acme", "widget", "/p/acme/widget", &out);
        assert_eq!(err.code, codes::E_GH);
        assert!(err.message.contains("push failed"), "{}", err.message);
        assert!(err.message.contains("may already exist"), "{}", err.message);
        assert!(err.message.contains("is kept"), "{}", err.message);
    }

    #[test]
    fn new_project_error_before_any_stage_marker_is_a_generic_git_setup_failure() {
        let out = output_with(1, "mkdir: permission denied");
        let err = new_project_error("vps", "acme", "widget", "/p/acme/widget", &out);
        assert_eq!(err.code, codes::E_GIT_SETUP);
        assert!(err.message.contains("permission denied"), "{}", err.message);
    }

    #[test]
    fn describe_register_failure_after_gh_success_mentions_github() {
        let e = describe_register_failure_after_gh_success(
            "acme",
            "widget",
            IpcError::new(codes::E_LOCK, "store mutex poisoned"),
        );
        assert_eq!(e.code, codes::E_LOCK);
        assert!(e.message.contains("GitHub repository"), "{}", e.message);
        assert!(e.message.contains("was created"), "{}", e.message);
        assert!(e.message.contains("store mutex poisoned"), "{}", e.message);
    }

    #[test]
    fn note_unknown_github_state_on_timeout_hedges_only_for_create_remote_timeouts() {
        let hedged = note_unknown_github_state_on_timeout(
            IpcError::new(codes::E_TIMEOUT, "local script exceeded 600s"),
            true,
            "acme",
            "widget",
        );
        assert_eq!(hedged.code, codes::E_TIMEOUT);
        assert!(
            hedged.message.contains("GitHub state is unknown"),
            "{}",
            hedged.message
        );

        // Not a timeout code -> passed through unchanged.
        let unrelated = note_unknown_github_state_on_timeout(
            IpcError::new(codes::E_GH, "some other failure"),
            true,
            "acme",
            "widget",
        );
        assert_eq!(unrelated.message, "some other failure");

        // create_remote off -> no GitHub state to hedge about.
        let local_only = note_unknown_github_state_on_timeout(
            IpcError::new(codes::E_TIMEOUT, "local script exceeded 600s"),
            false,
            "acme",
            "widget",
        );
        assert_eq!(local_only.message, "local script exceeded 600s");
    }

    // ── real gh-stage failure / resumability, `gh` always a local stub ──────

    #[tokio::test]
    async fn new_with_create_remote_registers_the_project_when_only_the_gh_stage_fails() {
        // Remote host + `FakeSsh`, not a real local execution: `FakeSsh`
        // answers the WHOLE script call with one canned `Output`, so the
        // reply is crafted here to look exactly like a real run that got
        // through init/commit and failed only at the gh-create stage — the
        // script's OWN correctness (that it really emits these markers) is
        // covered separately by the real-execution tests above.
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        fake.on(Match::Any, Reply::ok("")).with_home("/home/u");
        fake.on(
            Match::script_contains("git init"),
            Reply::fail(
                1,
                &format!(
                    "{STAGE_INIT}\n{STAGE_COMMIT}\n{STAGE_GH_CREATE}\ngh: network unreachable"
                ),
            ),
        );

        let token = confirm_tokens().mint("vps", "acme", "widget", Instant::now());
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "vps".into(),
                source: AddProjectSource::New {
                    owner: "acme".into(),
                    repo: "widget".into(),
                    create_remote: true,
                    confirm: Some(token),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_GH);
        assert!(err.message.contains("is kept"), "{}", err.message);
        assert!(
            err.message.contains("network unreachable"),
            "{}",
            err.message
        );

        // The local repo is real and usable, so it must be registered
        // anyway rather than leaving an orphan, unlisted directory.
        let rows = store.lock().unwrap().list_projects().unwrap();
        assert_eq!(
            rows.len(),
            1,
            "the project must be registered despite the gh failure"
        );
        assert_eq!(
            (rows[0].owner.as_str(), rows[0].repo.as_str()),
            ("acme", "widget")
        );
    }

    #[tokio::test]
    async fn new_project_script_resumes_after_a_failed_gh_stage() {
        // Prove `new_project_script` is safe to re-run after `gh` fails: it
        // must NOT redo mkdir/init/commit (the directory already has one)
        // and must go straight to the gh stage. `gh` is ALWAYS a local stub
        // here — this never touches real GitHub, matching the project's
        // absolute rule against a real `gh` write.
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("acme").join("widget");
        let script = new_project_script(dest.to_str().unwrap(), "acme", "widget", true);

        let home = tempfile::tempdir().unwrap();
        let global_gitconfig = home.path().join("global.gitconfig");
        std::fs::write(&global_gitconfig, "[user]\n\tname = T\n\temail = t@t\n").unwrap();

        // Attempt 1: `gh` always fails (simulating e.g. a network error).
        let failing_gh = tempfile::tempdir().unwrap();
        write_stub_gh(failing_gh.path(), "exit 7\n");
        let out1 = run_hermetic(
            &script,
            failing_gh.path(),
            home.path(),
            global_gitconfig.to_str().unwrap(),
        )
        .await;
        assert!(!out1.status.success());
        let stderr1 = String::from_utf8_lossy(&out1.stderr).to_string();
        assert!(stderr1.contains(STAGE_GH_CREATE), "{stderr1}");
        assert!(
            !stderr1.contains(STAGE_GH_PUSH),
            "must not reach push when create itself failed: {stderr1}"
        );
        assert!(dest.join(".git").is_dir(), "the local commit must survive");
        let commits_after_attempt_1 = std::process::Command::new("git")
            .args(["-C", dest.to_str().unwrap(), "log", "--oneline"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&commits_after_attempt_1.stdout)
                .lines()
                .count(),
            1
        );

        // A bare repo standing in for "GitHub" — a working `gh` stub wires
        // it up as `origin`, exactly like the real CLI would with a real
        // remote, so the SAME real `git push` in the script has somewhere
        // to push to.
        let bare = tempfile::tempdir().unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q", "--bare"])
            .arg(bare.path())
            .output()
            .unwrap()
            .status
            .success());

        // Attempt 2 (retry): a working `gh` stub.
        let working_gh = tempfile::tempdir().unwrap();
        write_stub_gh(
            working_gh.path(),
            &format!(
                "git remote add origin {}\n",
                crate::shell::quote(bare.path().to_str().unwrap())
            ),
        );
        let out2 = run_hermetic(
            &script,
            working_gh.path(),
            home.path(),
            global_gitconfig.to_str().unwrap(),
        )
        .await;
        assert!(out2.status.success(), "{out2:?}");
        assert!(
            String::from_utf8_lossy(&out2.stderr).contains(STAGE_GH_PUSH),
            "{}",
            String::from_utf8_lossy(&out2.stderr)
        );

        // Resumed, not reinitialized: still exactly the ONE commit from
        // attempt 1, now actually pushed to the bare repo.
        let commits_after_attempt_2 = std::process::Command::new("git")
            .args(["-C", dest.to_str().unwrap(), "log", "--oneline"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&commits_after_attempt_2.stdout)
                .lines()
                .count(),
            1,
            "still exactly one commit — resumed, not reinitialized"
        );
        let pushed = std::process::Command::new("git")
            .args([
                "-C",
                bare.path().to_str().unwrap(),
                "log",
                "--oneline",
                "main",
            ])
            .output()
            .unwrap();
        assert!(pushed.status.success(), "{pushed:?}");
        assert_eq!(String::from_utf8_lossy(&pushed.stdout).lines().count(), 1);
    }

    // ── recovery: retries the error messages prescribe must really work ─────

    /// Run git with the TEST process's environment, for setting up and
    /// inspecting fixtures only — never for the code under test.
    fn git_at(dir: &Path, args: &[&str]) -> std::process::Output {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap()
    }

    fn commit_count(dir: &Path) -> usize {
        let out = git_at(dir, &["log", "--oneline"]);
        if !out.status.success() {
            return 0;
        }
        String::from_utf8_lossy(&out.stdout).lines().count()
    }

    fn bare_repo() -> tempfile::TempDir {
        let bare = tempfile::tempdir().unwrap();
        assert!(git_at(bare.path(), &["init", "-q", "--bare"])
            .status
            .success());
        bare
    }

    /// A `gh` stub that records it was called (by touching `called`) and
    /// wires `bare` up as `origin`, exactly like `gh repo create --remote
    /// origin` would with a real GitHub repository.
    fn write_working_gh(dir: &Path, called: &Path, bare: &Path) {
        write_stub_gh(
            dir,
            &format!(
                "touch {}\ngit remote add origin {}\n",
                crate::shell::quote(called.to_str().unwrap()),
                crate::shell::quote(bare.to_str().unwrap())
            ),
        );
    }

    fn identity_gitconfig(home: &Path) -> std::path::PathBuf {
        let path = home.join("global.gitconfig");
        std::fs::write(&path, "[user]\n\tname = T\n\temail = t@t\n").unwrap();
        path
    }

    /// Tag `dest` exactly as `new_project_script` tags a repository it
    /// creates.
    fn tag_repo(dest: &Path, slug: &str) {
        assert!(
            git_at(dest, &["config", "--local", NEW_PROJECT_TAG_KEY, slug])
                .status
                .success()
        );
    }

    fn tag_of(dest: &Path) -> Option<String> {
        let out = git_at(dest, &["config", "--local", "--get", NEW_PROJECT_TAG_KEY]);
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// A repository at `dest` with one commit, made outside `new` (so
    /// untagged unless the caller tags it).
    fn repo_with_a_commit(dest: &Path) {
        std::fs::create_dir_all(dest).unwrap();
        assert!(git_at(dest, &["init", "-q"]).status.success());
        assert!(git_at(dest, &["symbolic-ref", "HEAD", "refs/heads/main"])
            .status
            .success());
        assert!(git_at(
            dest,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "existing history"
            ]
        )
        .status
        .success());
    }

    #[tokio::test]
    async fn an_untagged_existing_repo_is_refused_and_gh_never_runs() {
        // A local-only repository with history that `new` did not create —
        // the user merely typed its name. Publishing it would upload
        // contents the confirmation never named.
        let home = tempfile::tempdir().unwrap();
        let gitconfig = identity_gitconfig(home.path());
        let root = tempfile::tempdir().unwrap();
        let bare = bare_repo();
        let gh_bin = tempfile::tempdir().unwrap();
        let called = gh_bin.path().join("called");
        write_working_gh(gh_bin.path(), &called, bare.path());

        // Without origin: the no-origin resume path.
        let no_origin = root.path().join("no-origin");
        repo_with_a_commit(&no_origin);
        let script = new_project_script(no_origin.to_str().unwrap(), "acme", "widget", true);
        let out = run_hermetic(
            &script,
            gh_bin.path(),
            home.path(),
            gitconfig.to_str().unwrap(),
        )
        .await;
        assert_eq!(out.status.code(), Some(3), "{out:?}");
        assert!(!called.exists(), "gh must never run for an untagged repo");
        assert!(!git_at(&no_origin, &["remote", "get-url", "origin"])
            .status
            .success());
        let err = new_project_error("vps", "acme", "widget", "/p/acme/widget", &out);
        assert_eq!(err.code, codes::E_EXISTS);

        // With origin and a commit: the push-only path must not even be
        // offered (no exit 6, no URL frame).
        let with_origin = root.path().join("with-origin");
        repo_with_a_commit(&with_origin);
        assert!(git_at(
            &with_origin,
            &["remote", "add", "origin", bare.path().to_str().unwrap()]
        )
        .status
        .success());
        let script = new_project_script(with_origin.to_str().unwrap(), "acme", "widget", true);
        let out = run_hermetic(
            &script,
            gh_bin.path(),
            home.path(),
            gitconfig.to_str().unwrap(),
        )
        .await;
        assert_eq!(out.status.code(), Some(3), "{out:?}");
        assert!(!origin_already_set(&out));
        assert!(!called.exists());
        assert_eq!(git_at(bare.path(), &["for-each-ref"]).stdout, b"");
    }

    #[tokio::test]
    async fn a_repo_tagged_for_a_different_project_is_refused_without_gh_or_a_push() {
        let home = tempfile::tempdir().unwrap();
        let gitconfig = identity_gitconfig(home.path());
        let root = tempfile::tempdir().unwrap();
        let bare = bare_repo();
        let gh_bin = tempfile::tempdir().unwrap();
        let called = gh_bin.path().join("called");
        write_working_gh(gh_bin.path(), &called, bare.path());

        let no_origin = root.path().join("no-origin");
        repo_with_a_commit(&no_origin);
        tag_repo(&no_origin, "acme/other");
        let with_origin = root.path().join("with-origin");
        repo_with_a_commit(&with_origin);
        tag_repo(&with_origin, "acme/other");
        assert!(git_at(
            &with_origin,
            &["remote", "add", "origin", bare.path().to_str().unwrap()]
        )
        .status
        .success());

        for dest in [&no_origin, &with_origin] {
            let script = new_project_script(dest.to_str().unwrap(), "acme", "widget", true);
            let out = run_hermetic(
                &script,
                gh_bin.path(),
                home.path(),
                gitconfig.to_str().unwrap(),
            )
            .await;
            assert_eq!(out.status.code(), Some(3), "{}: {out:?}", dest.display());
            assert!(!origin_already_set(&out));
        }
        assert!(!called.exists(), "gh must never run");
        assert_eq!(
            git_at(bare.path(), &["for-each-ref"]).stdout,
            b"",
            "nothing may be pushed"
        );
    }

    #[tokio::test]
    async fn the_fresh_branch_tags_only_a_create_remote_repository() {
        let home = tempfile::tempdir().unwrap();
        let gitconfig = identity_gitconfig(home.path());
        let root = tempfile::tempdir().unwrap();
        let bare = bare_repo();
        let gh_bin = tempfile::tempdir().unwrap();
        let called = gh_bin.path().join("called");
        write_working_gh(gh_bin.path(), &called, bare.path());

        let remote = root.path().join("Acme").join("Widget");
        let script = new_project_script(remote.to_str().unwrap(), "Acme", "Widget", true);
        let out = run_hermetic(
            &script,
            gh_bin.path(),
            home.path(),
            gitconfig.to_str().unwrap(),
        )
        .await;
        assert!(out.status.success(), "{out:?}");
        assert_eq!(tag_of(&remote).as_deref(), Some("Acme/Widget"));

        let local_only = root.path().join("local-only");
        let empty_bin = tempfile::tempdir().unwrap();
        let script = new_project_script(local_only.to_str().unwrap(), "acme", "local-only", false);
        let out = run_hermetic(&script, empty_bin.path(), home.path(), "/dev/null").await;
        assert!(out.status.success(), "{out:?}");
        assert_eq!(
            tag_of(&local_only),
            None,
            "a local-only project is never tagged, so a later create_remote cannot publish it"
        );
    }

    #[tokio::test]
    async fn a_host_without_git_is_left_untouched_with_a_clear_message() {
        // `/bin/bash` by absolute path under a PATH holding nothing at all:
        // no git (and no gh) can be found.
        let empty_bin = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        for create_remote in [true, false] {
            let dest = root.path().join("acme").join("widget");
            let script =
                new_project_script(dest.to_str().unwrap(), "acme", "widget", create_remote);
            let out = tokio::process::Command::new("/bin/bash")
                .arg("-c")
                .arg(&script)
                .env_clear()
                .env("PATH", empty_bin.path())
                .env("HOME", home.path())
                .output()
                .await
                .unwrap();
            assert_eq!(out.status.code(), Some(7), "{out:?}");
            assert!(!root.path().join("acme").exists(), "nothing may be created");
            let err = new_project_error("vps", "acme", "widget", dest.to_str().unwrap(), &out);
            assert_eq!(err.code, codes::E_GIT_SETUP);
            assert!(
                err.message.contains("git is not installed on vps"),
                "{}",
                err.message
            );
            assert!(err.message.contains("install git"), "{}", err.message);
            assert!(!err.message.contains("__"), "{}", err.message);
        }
    }

    #[test]
    fn already_exists_promises_a_refresh_only_on_the_local_host() {
        let local = already_exists_error("local", "acme", "widget", "/p/acme/widget");
        assert!(
            local.message.contains("after a refresh"),
            "{}",
            local.message
        );
        let remote = already_exists_error("vps", "acme", "widget", "/home/u/acme/widget");
        assert_eq!(remote.code, codes::E_EXISTS);
        assert!(!remote.message.contains("refresh"), "{}", remote.message);
        assert!(remote.message.contains("on vps"), "{}", remote.message);
    }

    #[tokio::test]
    async fn a_missing_identity_leaves_nothing_behind_and_the_retry_commits_and_pushes() {
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("acme").join("widget");
        let script = new_project_script(dest.to_str().unwrap(), "acme", "widget", true);

        // Attempt 1: no identity anywhere. No stub either — the guard `gh`
        // would exit 99 if the script ever got that far.
        let empty_bin = tempfile::tempdir().unwrap();
        let out1 = run_hermetic(&script, empty_bin.path(), home.path(), "/dev/null").await;
        assert_eq!(out1.status.code(), Some(5), "{out1:?}");
        assert!(
            !dest.exists(),
            "a refused identity must leave no directory to resume from"
        );

        // The user follows the message: configure an identity, then retry.
        let gitconfig = identity_gitconfig(home.path());
        let bare = bare_repo();
        let gh_bin = tempfile::tempdir().unwrap();
        let called = gh_bin.path().join("called");
        write_working_gh(gh_bin.path(), &called, bare.path());
        let out2 = run_hermetic(
            &script,
            gh_bin.path(),
            home.path(),
            gitconfig.to_str().unwrap(),
        )
        .await;
        assert!(out2.status.success(), "{out2:?}");
        assert_eq!(commit_count(&dest), 1, "the retry must make the commit");
        assert!(called.exists(), "the gh stub must have been called");
        let pushed = git_at(bare.path(), &["log", "--oneline", "main"]);
        assert!(
            pushed.status.success(),
            "main must reach the remote: {pushed:?}"
        );
        assert_eq!(String::from_utf8_lossy(&pushed.stdout).lines().count(), 1);
    }

    #[tokio::test]
    async fn resuming_a_repo_whose_commit_failed_commits_before_the_gh_stage() {
        // `git init` succeeded but the commit did not (e.g. `commit.gpgsign`
        // with no TTY): a repo with no HEAD and no origin. The retry must
        // commit before `gh`, or the push has no `main` to send.
        let home = tempfile::tempdir().unwrap();
        let gitconfig = identity_gitconfig(home.path());
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("widget");
        std::fs::create_dir_all(&dest).unwrap();
        assert!(git_at(&dest, &["init", "-q"]).status.success());
        assert!(git_at(&dest, &["symbolic-ref", "HEAD", "refs/heads/main"])
            .status
            .success());
        // Tagged the way `new` tags what it creates — in a different case,
        // which the guard must accept.
        tag_repo(&dest, "Acme/Widget");
        assert_eq!(commit_count(&dest), 0);

        let bare = bare_repo();
        let gh_bin = tempfile::tempdir().unwrap();
        let called = gh_bin.path().join("called");
        write_working_gh(gh_bin.path(), &called, bare.path());
        let script = new_project_script(dest.to_str().unwrap(), "acme", "widget", true);
        let out = run_hermetic(
            &script,
            gh_bin.path(),
            home.path(),
            gitconfig.to_str().unwrap(),
        )
        .await;
        assert!(out.status.success(), "{out:?}");
        assert_eq!(commit_count(&dest), 1);
        assert!(called.exists());
        assert!(git_at(bare.path(), &["rev-parse", "--verify", "main"])
            .status
            .success());

        // Without create_remote the same resume must not report success on
        // an empty repo either.
        let dest2 = root.path().join("local-only");
        std::fs::create_dir_all(&dest2).unwrap();
        assert!(git_at(&dest2, &["init", "-q"]).status.success());
        let empty_bin = tempfile::tempdir().unwrap();
        let script2 = new_project_script(dest2.to_str().unwrap(), "acme", "local-only", false);
        let out2 = run_hermetic(&script2, empty_bin.path(), home.path(), "/dev/null").await;
        assert!(out2.status.success(), "{out2:?}");
        assert_eq!(commit_count(&dest2), 1);
    }

    #[tokio::test]
    async fn a_repo_with_origin_and_a_commit_reports_its_push_url_and_pushes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let gitconfig = identity_gitconfig(home.path());
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("widget");
        std::fs::create_dir_all(&dest).unwrap();
        let bare = bare_repo();
        assert!(git_at(&dest, &["init", "-q"]).status.success());
        assert!(git_at(
            &dest,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "x"
            ]
        )
        .status
        .success());
        assert!(git_at(
            &dest,
            &["remote", "add", "origin", bare.path().to_str().unwrap()]
        )
        .status
        .success());

        tag_repo(&dest, "acme/widget");

        // No stub: reaching `gh` would hit the guard (exit 99).
        let empty_bin = tempfile::tempdir().unwrap();
        let script = new_project_script(dest.to_str().unwrap(), "acme", "widget", true);
        let out = run_hermetic(
            &script,
            empty_bin.path(),
            home.path(),
            gitconfig.to_str().unwrap(),
        )
        .await;
        assert_eq!(out.status.code(), Some(6), "{out:?}");
        assert!(origin_already_set(&out));
        assert_eq!(
            origin_url_from(&out).as_deref(),
            Some(bare.path().to_str().unwrap())
        );
        assert_eq!(
            git_at(bare.path(), &["for-each-ref"]).stdout,
            b"",
            "nothing may be pushed before the caller verifies the origin"
        );
    }

    #[tokio::test]
    async fn push_only_script_pushes_to_the_verified_origin_and_refuses_a_changed_one() {
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("widget");
        std::fs::create_dir_all(&dest).unwrap();
        let bare = bare_repo();
        assert!(git_at(&dest, &["init", "-q"]).status.success());
        assert!(git_at(&dest, &["symbolic-ref", "HEAD", "refs/heads/main"])
            .status
            .success());
        assert!(git_at(
            &dest,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "x"
            ]
        )
        .status
        .success());
        let bare_url = bare.path().to_str().unwrap();
        assert!(git_at(&dest, &["remote", "add", "origin", bare_url])
            .status
            .success());
        let empty_bin = tempfile::tempdir().unwrap();

        // The verified URL is not what `origin` holds: refuse, push nothing.
        let changed =
            push_only_script(dest.to_str().unwrap(), "https://github.com/acme/widget.git");
        let out = run_hermetic(&changed, empty_bin.path(), home.path(), "/dev/null").await;
        assert_eq!(out.status.code(), Some(3), "{out:?}");
        assert_eq!(git_at(bare.path(), &["for-each-ref"]).stdout, b"");
        let err = new_project_error("vps", "acme", "widget", "/p/acme/widget", &out);
        assert_eq!(err.code, codes::E_EXISTS);
        assert!(
            err.message.contains("nothing was pushed"),
            "{}",
            err.message
        );

        // The verified URL matches: exactly the push, and it lands.
        let ok = push_only_script(dest.to_str().unwrap(), bare_url);
        let out = run_hermetic(&ok, empty_bin.path(), home.path(), "/dev/null").await;
        assert!(out.status.success(), "{out:?}");
        assert!(git_at(bare.path(), &["rev-parse", "--verify", "main"])
            .status
            .success());
    }

    #[test]
    fn origin_is_github_repo_accepts_only_this_repository_on_github() {
        for url in [
            "https://github.com/acme/widget.git",
            "https://github.com/Acme/Widget",
            "git@github.com:acme/widget.git",
            "ssh://git@github.com/acme/widget",
        ] {
            assert!(origin_is_github_repo(url, "acme", "widget"), "{url}");
        }
        for url in [
            "https://github.com/acme/other.git",
            "git@github.com:someone/widget.git",
            "https://gitlab.com/acme/widget.git",
            "acme/widget", // a local relative path, not GitHub
            "/srv/git/acme/widget.git",
            "https://token@github.com/acme/widget.git",
            "",
        ] {
            assert!(!origin_is_github_repo(url, "acme", "widget"), "{url}");
        }
    }

    #[test]
    fn new_project_scripts_never_let_git_prompt_for_credentials() {
        // The same script text runs locally and over SSH, so asserting the
        // text covers both paths.
        for script in [
            new_project_script("/p/acme/widget", "acme", "widget", true),
            new_project_script("/p/acme/widget", "acme", "widget", false),
            push_only_script("/p/acme/widget", "https://github.com/acme/widget.git"),
        ] {
            assert!(
                script.contains("export GIT_TERMINAL_PROMPT=0\n"),
                "{script}"
            );
        }
    }

    #[test]
    fn new_project_error_shows_only_the_stderr_after_the_last_stage_marker() {
        let out = output_with(
            1,
            &format!(
                "{STAGE_INIT}\nhint: Using 'master' as the name for the initial branch.\n\
                 hint: to suppress this, set init.defaultBranch\n\
                 Initialized empty Git repository in /p/acme/widget/.git/\n\
                 {STAGE_COMMIT}\nearlier noise\n{STAGE_GH_CREATE}\n\
                 hint: stray hint\nHTTP 422: name already exists on this account\n"
            ),
        );
        let err = new_project_error("vps", "acme", "widget", "/p/acme/widget", &out);
        assert_eq!(err.code, codes::E_GH);
        assert!(err.message.contains("HTTP 422"), "{}", err.message);
        for leaked in [
            "__",
            "stage=",
            "hint:",
            "earlier noise",
            "Initialized empty",
        ] {
            assert!(!err.message.contains(leaked), "{leaked}: {}", err.message);
        }
    }

    /// `(host, local_base)` exactly as `new_source` would compute them for
    /// `acme/widget` on `vps`.
    fn local_base_for(store: &Mutex<Store>) -> String {
        let (_, local_root, layout) = roots(store, "vps").unwrap();
        layout.project_dir(&local_root, "acme", "widget")
    }

    fn new_remote_args(confirm: Option<String>) -> AddProjectArgs {
        AddProjectArgs {
            host_alias: "vps".into(),
            source: AddProjectSource::New {
                owner: "acme".into(),
                repo: "widget".into(),
                create_remote: true,
                confirm,
            },
            call_id: None,
        }
    }

    fn mint_vps_widget() -> String {
        confirm_tokens().mint("vps", "acme", "widget", Instant::now())
    }

    fn reply_with_stderr(code: i32, stderr: &str) -> Reply {
        Reply::Exit {
            code,
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[tokio::test]
    async fn a_create_remote_retry_after_a_gh_failure_resumes_instead_of_refusing() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        fake.on(Match::Any, Reply::ok("")).with_home("/home/u");
        fake.on(
            Match::script_contains("git init"),
            reply_with_stderr(
                1,
                &format!("{STAGE_INIT}\n{STAGE_COMMIT}\n{STAGE_GH_CREATE}\ngh: HTTP 502"),
            ),
        );

        // Attempt 1 fails at the gh stage; the repo has a HEAD (the probe
        // answers ok), so the row is registered.
        let err = add_project_with(new_remote_args(Some(mint_vps_widget())), &store, &fake)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_GH);
        assert!(err.message.contains("retry"), "{}", err.message);
        assert_eq!(store.lock().unwrap().list_projects().unwrap().len(), 1);

        // Attempt 2, as the message prescribes, with a fresh confirmation:
        // it must NOT be refused as "already a fleet project".
        fake.on(
            Match::script_contains("git init"),
            reply_with_stderr(0, &format!("{STAGE_GH_CREATE}\n{STAGE_GH_PUSH}\n")),
        );
        let row = add_project_with(new_remote_args(Some(mint_vps_widget())), &store, &fake)
            .await
            .unwrap();
        assert_eq!(
            (row.project.owner.as_str(), row.project.repo.as_str()),
            ("acme", "widget")
        );
        let gh_scripts = fake
            .calls_for("vps")
            .iter()
            .filter_map(|c| c.script())
            .filter(|s| s.contains("gh repo create"))
            .count();
        assert_eq!(gh_scripts, 2, "the retry must actually run the script");
        assert_eq!(
            store.lock().unwrap().list_projects().unwrap().len(),
            1,
            "the retry re-registers the same row"
        );
    }

    #[tokio::test]
    async fn a_gh_stage_failure_with_no_head_is_not_registered() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        fake.on(Match::Any, Reply::ok("")).with_home("/home/u");
        // The HEAD probe finds no commit.
        fake.on(
            Match::script_contains("rev-parse --verify -q HEAD"),
            Reply::fail(1, ""),
        );
        fake.on(
            Match::script_contains("git init"),
            reply_with_stderr(
                1,
                &format!("{STAGE_GH_CREATE}\n{STAGE_GH_PUSH}\nsrc refspec main does not match any"),
            ),
        );
        let err = add_project_with(new_remote_args(Some(mint_vps_widget())), &store, &fake)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_GH);
        assert!(
            store.lock().unwrap().list_projects().unwrap().is_empty(),
            "a repo without a commit must not be registered"
        );
    }

    /// A `FakeSsh` for the "origin already set" retry: the main script
    /// answers with the exit-6 frame naming `origin`, the read-only remote
    /// branch check answers `ls_remote`, and a push-only script succeeds.
    fn fake_with_origin_set(origin: &str, ls_remote: Reply) -> FakeSsh {
        let fake = FakeSsh::new();
        fake.on(Match::Any, Reply::ok("")).with_home("/home/u");
        fake.on(
            Match::script_contains("git init"),
            reply_with_stderr(
                6,
                &format!("{ORIGIN_URL_BEGIN_MARKER}\n{origin}\n{ORIGIN_URL_END_MARKER}\n"),
            ),
        );
        fake.on(Match::script_contains("ls-remote --exit-code"), ls_remote);
        fake.on(
            Match::script_contains(ORIGIN_CHANGED_MARKER),
            reply_with_stderr(0, &format!("{STAGE_GH_PUSH}\n")),
        );
        fake
    }

    fn scripts_containing(fake: &FakeSsh, needle: &str) -> Vec<String> {
        fake.calls_for("vps")
            .iter()
            .filter_map(|c| c.script())
            .filter(|s| s.contains(needle))
            .collect()
    }

    /// No push of any shape was issued: neither the push-only script nor a
    /// bare `git push`.
    fn assert_no_push(fake: &FakeSsh) {
        assert!(
            scripts_containing(fake, ORIGIN_CHANGED_MARKER).is_empty(),
            "no push-only script may run"
        );
        let pushes = scripts_containing(fake, "git push")
            .into_iter()
            .filter(|s| !s.contains("git init"))
            .count();
        assert_eq!(pushes, 0, "no push may run");
    }

    /// Register `acme/widget` at exactly the path `new_source` computes, in
    /// the casing given — what a previous failed `create_remote` attempt
    /// leaves behind.
    fn register_widget_as(store: &Mutex<Store>, owner: &str, repo: &str) {
        let local_base = local_base_for(store);
        store
            .lock()
            .unwrap()
            .upsert_project(owner, repo, &local_base)
            .unwrap();
    }

    #[tokio::test]
    async fn a_registered_project_whose_remote_lacks_main_gets_exactly_one_push() {
        let store = store_with_no_projects();
        // Registered in a different casing, so the returned row proves the
        // retry re-registered THAT row rather than adding a twin.
        register_widget_as(&store, "Acme", "Widget");
        let fake = fake_with_origin_set(
            "https://github.com/acme/widget.git",
            Reply::fail(2, ""), // ls-remote: the remote answered, no main
        );
        let row = add_project_with(new_remote_args(Some(mint_vps_widget())), &store, &fake)
            .await
            .unwrap();
        assert_eq!(
            (row.project.owner.as_str(), row.project.repo.as_str()),
            ("Acme", "Widget")
        );
        assert_eq!(store.lock().unwrap().list_projects().unwrap().len(), 1);
        assert_eq!(scripts_containing(&fake, "ls-remote --exit-code").len(), 1);
        let push_scripts = scripts_containing(&fake, ORIGIN_CHANGED_MARKER);
        assert_eq!(push_scripts.len(), 1, "the push runs exactly once");
        assert!(
            push_scripts[0].contains("'https://github.com/acme/widget.git'"),
            "{}",
            push_scripts[0]
        );
        assert!(!push_scripts[0].contains("gh repo create"));
    }

    #[tokio::test]
    async fn a_tagged_registered_project_whose_remote_already_has_main_succeeds_without_a_push() {
        // The canned exit-6 reply stands for a repo that passed the script's
        // tag guard. `main` is already on GitHub (e.g. the push landed but
        // its SSH session died): the creation is complete.
        let store = store_with_no_projects();
        register_widget_as(&store, "Acme", "Widget");
        let fake = fake_with_origin_set(
            "git@github.com:acme/widget.git",
            Reply::ok("0123456789abcdef0123456789abcdef01234567\trefs/heads/main\n"),
        );
        let row = add_project_with(new_remote_args(Some(mint_vps_widget())), &store, &fake)
            .await
            .unwrap();
        assert_eq!(
            (row.project.owner.as_str(), row.project.repo.as_str()),
            ("Acme", "Widget"),
            "the registered row is returned"
        );
        assert_eq!(store.lock().unwrap().list_projects().unwrap().len(), 1);
        assert_no_push(&fake);
    }

    #[tokio::test]
    async fn a_tagged_unregistered_project_whose_remote_already_has_main_is_registered() {
        // The remote-host stuck case: the push landed but the run timed out
        // (or `register` failed), so no row exists and a local refresh can
        // never find it. The retry registers it without pushing.
        let store = store_with_no_projects();
        let fake = fake_with_origin_set(
            "https://github.com/acme/widget.git",
            Reply::ok("0123456789abcdef0123456789abcdef01234567\trefs/heads/main\n"),
        );
        let row = add_project_with(new_remote_args(Some(mint_vps_widget())), &store, &fake)
            .await
            .unwrap();
        assert_eq!(row.project.repo, "widget");
        assert_eq!(store.lock().unwrap().list_projects().unwrap().len(), 1);
        assert_no_push(&fake);
    }

    #[tokio::test]
    async fn a_failing_remote_branch_check_is_an_error_and_pushes_nothing() {
        let store = store_with_no_projects();
        register_widget_as(&store, "acme", "widget");
        let fake = fake_with_origin_set(
            "https://github.com/acme/widget.git",
            Reply::fail(
                128,
                "fatal: unable to access 'https://github.com/acme/widget.git/'",
            ),
        );
        let err = add_project_with(new_remote_args(Some(mint_vps_widget())), &store, &fake)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_GH);
        assert!(err.message.contains("couldn't check"), "{}", err.message);
        assert!(
            err.message.contains("Nothing was pushed"),
            "{}",
            err.message
        );
        assert_no_push(&fake);
    }

    #[tokio::test]
    async fn a_tagged_repo_that_failed_at_push_with_no_row_is_finished_by_one_push() {
        // `new` created and tagged the repo, `gh repo create` set origin, the
        // push failed, and no row exists (e.g. a timeout on a remote host).
        // The canned exit-6 reply is what the script prints only after its
        // tag guard passed; the tag guard itself is covered hermetically.
        let store = store_with_no_projects();
        let fake = fake_with_origin_set("https://github.com/acme/widget.git", Reply::fail(2, ""));
        let row = add_project_with(new_remote_args(Some(mint_vps_widget())), &store, &fake)
            .await
            .unwrap();
        assert_eq!(row.project.repo, "widget");
        assert_eq!(scripts_containing(&fake, "ls-remote --exit-code").len(), 1);
        assert_eq!(
            scripts_containing(&fake, ORIGIN_CHANGED_MARKER).len(),
            1,
            "the push runs exactly once"
        );
        assert_eq!(store.lock().unwrap().list_projects().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_origin_pointing_at_a_different_repository_is_refused_without_a_push() {
        // The canned exit-6 reply stands for a repo that passed the tag
        // guard, so the refusal below is the origin check itself.
        let store = store_with_no_projects();
        register_widget_as(&store, "acme", "widget");
        let fake =
            fake_with_origin_set("git@github.com:someone-else/widget.git", Reply::fail(2, ""));
        let err = add_project_with(new_remote_args(Some(mint_vps_widget())), &store, &fake)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_EXISTS);
        assert!(err.message.contains("refusing to push"), "{}", err.message);
        assert!(!err.message.contains("__"), "{}", err.message);
        assert!(
            scripts_containing(&fake, "ls-remote --exit-code").is_empty(),
            "an unverified origin is never even queried"
        );
        assert_no_push(&fake);
    }

    #[tokio::test]
    async fn remote_branch_check_script_exits_2_only_when_main_is_absent() {
        // Real git against a local bare repo standing in for GitHub; never a
        // network remote.
        let home = tempfile::tempdir().unwrap();
        let empty_bin = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("widget");
        std::fs::create_dir_all(&dest).unwrap();
        let bare = bare_repo();
        assert!(git_at(&dest, &["init", "-q"]).status.success());
        assert!(git_at(&dest, &["symbolic-ref", "HEAD", "refs/heads/main"])
            .status
            .success());
        assert!(git_at(
            &dest,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "x"
            ]
        )
        .status
        .success());
        assert!(git_at(
            &dest,
            &["remote", "add", "origin", bare.path().to_str().unwrap()]
        )
        .status
        .success());
        let check = remote_branch_check_script(dest.to_str().unwrap());

        let absent = run_hermetic(&check, empty_bin.path(), home.path(), "/dev/null").await;
        assert_eq!(absent.status.code(), Some(2), "{absent:?}");

        assert!(git_at(&dest, &["push", "-q", "origin", "main"])
            .status
            .success());
        let present = run_hermetic(&check, empty_bin.path(), home.path(), "/dev/null").await;
        assert_eq!(present.status.code(), Some(0), "{present:?}");

        // An unreachable remote is neither 0 nor 2.
        assert!(git_at(
            &dest,
            &[
                "remote",
                "set-url",
                "origin",
                root.path().join("gone").to_str().unwrap()
            ]
        )
        .status
        .success());
        let unreachable = run_hermetic(&check, empty_bin.path(), home.path(), "/dev/null").await;
        assert!(
            !matches!(unreachable.status.code(), Some(0) | Some(2)),
            "{unreachable:?}"
        );
    }

    #[tokio::test]
    async fn an_existing_project_elsewhere_is_refused_without_consuming_the_token() {
        let store = store_with_no_projects();
        store
            .lock()
            .unwrap()
            .upsert_project("Acme", "Widget", "/somewhere/else")
            .unwrap();
        let fake = FakeSsh::new();
        let token = mint_vps_widget();
        let err = add_project_with(new_remote_args(Some(token.clone())), &store, &fake)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_EXISTS);
        assert!(fake.calls().is_empty());
        assert!(
            confirm_tokens().consume(&token, "vps", "acme", "widget", Instant::now()),
            "a refused call must not burn the user's confirmation"
        );
    }

    #[tokio::test]
    async fn a_registered_project_at_the_same_path_still_refuses_without_create_remote() {
        let store = store_with_no_projects();
        let local_base = local_base_for(&store);
        store
            .lock()
            .unwrap()
            .upsert_project("acme", "widget", &local_base)
            .unwrap();
        let fake = FakeSsh::new();
        let mut args = new_remote_args(None);
        if let AddProjectSource::New { create_remote, .. } = &mut args.source {
            *create_remote = false;
        }
        let err = add_project_with(args, &store, &fake).await.unwrap_err();
        assert_eq!(err.code, codes::E_EXISTS);
        assert!(fake.calls().is_empty());
    }

    #[tokio::test]
    async fn create_remote_on_the_local_host_is_refused_under_test() {
        let projects_root = tempfile::tempdir().unwrap();
        let store = store_with_no_projects();
        set_local_projects_root(&store, projects_root.path());
        let fake = FakeSsh::new();
        let token = confirm_tokens().mint("local", "acme", "widget", Instant::now());
        let err = add_project_with(
            AddProjectArgs {
                host_alias: "local".into(),
                source: AddProjectSource::New {
                    owner: "acme".into(),
                    repo: "widget".into(),
                    create_remote: true,
                    confirm: Some(token),
                },
                call_id: None,
            },
            &store,
            &fake,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_FORBIDDEN);
        assert!(err.message.contains("real gh"), "{}", err.message);
        assert!(!projects_root.path().join("acme").exists());
        assert!(store.lock().unwrap().list_projects().unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_github_repos_maps_the_json_and_surfaces_a_failure() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        fake.with_home("/home/u").on(
            Match::script_contains("gh repo list"),
            Reply::ok(
                r#"[{"nameWithOwner":"acme/widget","description":"w","isPrivate":true,"updatedAt":"2026-09-01T10:00:00Z"}]"#,
            ),
        );
        let repos = list_github_repos_with("vps", &store, &fake).await.unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name_with_owner, "acme/widget");
        assert!(repos[0].is_private);
        assert_eq!(repos[0].description.as_deref(), Some("w"));
        assert_eq!(repos[0].updated_at.as_deref(), Some("2026-09-01T10:00:00Z"));

        let bad = FakeSsh::new();
        bad.with_home("/home/u").on(
            Match::script_contains("gh repo list"),
            Reply::fail(
                4,
                "gh: To get started with GitHub CLI, please run: gh auth login",
            ),
        );
        let err = list_github_repos_with("vps", &store, &bad)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_GH);
        assert!(err.message.contains("gh auth login"), "{}", err.message);
    }

    #[tokio::test]
    async fn list_github_repos_reports_a_missing_gh_with_a_remedy() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        fake.with_home("/home/u").on(
            Match::script_contains("gh repo list"),
            Reply::fail(127, "bash: line 1: gh: command not found"),
        );
        let err = list_github_repos_with("vps", &store, &fake)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_GH);
        assert!(
            err.message.to_lowercase().contains("install"),
            "{}",
            err.message
        );
        assert!(err.message.contains("vps"), "{}", err.message);
    }

    #[tokio::test]
    async fn list_github_repos_reports_a_json_parse_failure() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        fake.with_home("/home/u").on(
            Match::script_contains("gh repo list"),
            Reply::ok("not json at all"),
        );
        let err = list_github_repos_with("vps", &store, &fake)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_GH);
        assert!(err.message.contains("not json at all"), "{}", err.message);
    }

    #[tokio::test]
    async fn list_github_repos_validates_the_host_alias_before_any_ssh() {
        let store = store_with_no_projects();
        let fake = FakeSsh::new();
        let err = list_github_repos_with("-oProxyCommand=evil", &store, &fake)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID);
        assert!(fake.calls().is_empty());
    }
}
