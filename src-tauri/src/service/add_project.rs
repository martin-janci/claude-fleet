//! Adding a project fleet does not know yet: clone a GitHub repo, adopt an
//! existing checkout, or create a new one. The New-session flow's entry
//! point for "this repo is not on disk yet".
//!
//! This module implements the `clone` source (Task 2 of
//! `docs/superpowers/plans/2026-09-12-add-project.md`); `folder` and `new`
//! are Tasks 3 and 4.

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
    #[allow(dead_code)] // Task 3 wires this in.
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
        // Tasks 3 and 4.
        AddProjectSource::Folder { .. } | AddProjectSource::New { .. } => Err(IpcError::new(
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
    register(store, &owner, &repo, &local_base)
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
/// shape `service::projects::list_projects` builds. `upsert_project` already
/// emits `project:updated`; worktrees are empty until the next scan/refresh.
fn register(
    store: &Mutex<Store>,
    owner: &str,
    repo: &str,
    base_path: &str,
) -> Result<ProjectTreeRow, IpcError> {
    let s = store.lock().map_err(|_| IpcError::lock())?;
    s.upsert_project(owner, repo, base_path)?;
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
}
