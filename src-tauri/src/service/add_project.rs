//! Adding a project fleet does not know yet: clone a GitHub repo, adopt an
//! existing checkout, or create a new one. The New-session flow's entry
//! point for "this repo is not on disk yet".
//!
//! This module implements the `clone` (Task 2) and `folder` (Task 3) sources
//! of `docs/superpowers/plans/2026-09-12-add-project.md`; `new` is Task 4.

use crate::ipc_error::{codes, IpcError};
use crate::projects::Layout;
use crate::repo_url::{clone_url_for, parse_repo_url};
use crate::service::projects::ProjectTreeRow;
use crate::shell::quote;
use crate::ssh::{SshClient, SshExec};
use crate::store::Store;
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    #[allow(dead_code)] // Task 4 wires this in.
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
        // Task 4.
        AddProjectSource::New { .. } => Err(IpcError::new(
            codes::E_INVALID,
            "source not implemented yet",
        )),
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

/// When `top` is a linked worktree, resolve and return its MAIN checkout's
/// path instead; otherwise return `top` unchanged.
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
    // Git's own ordering guarantee: the first `worktree ` entry is the main
    // worktree (see the identical assumption in `crate::projects::list_worktrees`).
    let main = list
        .lines()
        .find_map(|l| l.strip_prefix("worktree "))
        .ok_or_else(|| {
            IpcError::new(
                codes::E_INTERNAL,
                format!("{top}: `git worktree list` reported no worktrees"),
            )
        })?;
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

    fn store_with_no_projects() -> Mutex<Store> {
        Mutex::new(Store::open_in_memory().unwrap())
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
}
