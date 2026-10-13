//! A run's own workspace: a detached git worktree of exactly the commit the
//! job is about, never a person's session worktree.
//!
//! For a checkout at `<root>` the worktrees live beside it, in
//! `<root>.fleet-agents/job-<id>`, so they never show up in the checkout's
//! own `git status`. The commit is fetched into a private ref,
//! `refs/fleet-agents/job-<id>`, not `FETCH_HEAD`: two jobs on one repo may
//! fetch at once, and `FETCH_HEAD` is one file. [`prepare_script`] fetches
//! and adds the worktree (refusing a head that moved past what the job
//! expected), [`cleanup_script`] removes one job's worktree and ref, and
//! [`prune_script`] removes what a crashed run left behind.
//!
//! The prune runs at the start of a run pass, before the pass claims jobs;
//! passes are single-flight, so it never races a job of this process.

use super::vehicle::path_ok;
use crate::ipc_error::{codes, IpcError};
use crate::shell::quote;
use crate::ssh::SshClient;
use std::time::Duration;

/// The tag the workspace scripts print.
pub const TAG: &str = "FLEET_AGENT_WS=";

/// A workspace older than this is a crashed run's, and the prune removes it.
pub const STALE_WORKSPACE_MINS: u32 = 60;

/// What the job's commit is fetched from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A GitHub pull request's head: `pull/<n>/head`.
    PullHead(u64),
    /// A branch on `origin`.
    Branch(String),
}

/// PURE: whether `b` is a branch name a script may fetch: git's own rules,
/// narrowed (no leading `-`, no `..`, no `@{`, no control or glob
/// characters, no trailing `.lock` or `/`).
fn branch_ok(b: &str) -> bool {
    !b.is_empty()
        && b.len() <= 200
        && !b.starts_with(['-', '/', '.'])
        && !b.ends_with(['/', '.'])
        && !b.ends_with(".lock")
        && !b.contains("..")
        && !b.contains("@{")
        && !b.contains("//")
        && b.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
}

fn sha_ok(s: &str) -> bool {
    (7..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// PURE: the directory a job's worktree lives in.
pub fn workspace_dir(root: &str, job_id: i64) -> String {
    format!("{}.fleet-agents/job-{job_id}", root.trim_end_matches('/'))
}

fn job_ref(job_id: i64) -> String {
    format!("refs/fleet-agents/job-{job_id}")
}

fn check_root(root: &str) -> Result<&str, String> {
    let r = root.trim_end_matches('/');
    if r.is_empty() || !path_ok(r) {
        return Err(format!("refusing checkout path {root:?}"));
    }
    Ok(r)
}

/// PURE: fetch the job's commit and add its worktree. Prints exactly one
/// tag line: `noroot` (no git checkout at `root`), `fetchfail`, `moved`
/// (the fetched head is not `expect_sha`), `failed` (the worktree could not
/// be added) or `ready`.
pub fn prepare_script(
    root: &str,
    job_id: i64,
    source: &Source,
    expect_sha: Option<&str>,
) -> Result<String, String> {
    let r = check_root(root)?;
    let refspec = match source {
        Source::PullHead(n) if *n > 0 => format!("+pull/{n}/head:{}", job_ref(job_id)),
        Source::Branch(b) if branch_ok(b) => format!("+refs/heads/{b}:{}", job_ref(job_id)),
        other => return Err(format!("refusing source {other:?}")),
    };
    let expect = match expect_sha {
        None => String::new(),
        Some(sha) if sha_ok(sha) => format!(
            "case \"$(git rev-parse \"$ref\")\" in {}*) ;; \
             *) git update-ref -d \"$ref\"; echo {TAG}moved; exit 0;; esac; ",
            sha.to_ascii_lowercase()
        ),
        Some(sha) => return Err(format!("refusing commit {sha:?}")),
    };
    let w = workspace_dir(r, job_id);
    let t = TAG;
    Ok(format!(
        "w={w}; ref={jref}; \
         cd -- {r} 2>/dev/null && git rev-parse --git-dir >/dev/null 2>&1 || {{ echo {t}noroot; exit 0; }}; \
         git fetch --no-tags --quiet origin {refspec} >/dev/null 2>&1 || {{ echo {t}fetchfail; exit 0; }}; \
         {expect}\
         if [ -e \"$w\" ]; then git worktree remove --force \"$w\" >/dev/null 2>&1 || rm -rf -- \"$w\"; fi; \
         mkdir -p -- {parent} && git worktree add --detach --quiet \"$w\" \"$ref\" >/dev/null 2>&1 \
           || {{ git update-ref -d \"$ref\"; echo {t}failed; exit 0; }}; \
         echo {t}ready",
        w = quote(&w),
        jref = quote(&job_ref(job_id)),
        r = quote(r),
        refspec = quote(&refspec),
        parent = quote(&format!("{r}.fleet-agents")),
    ))
}

/// PURE: remove job `job_id`'s worktree and ref. Prints `<tag>clean`.
pub fn cleanup_script(root: &str, job_id: i64) -> Result<String, String> {
    let r = check_root(root)?;
    let t = TAG;
    Ok(format!(
        "w={w}; cd -- {r} 2>/dev/null || {{ echo {t}clean; exit 0; }}; \
         git worktree remove --force \"$w\" >/dev/null 2>&1 || rm -rf -- \"$w\"; \
         git worktree prune >/dev/null 2>&1; \
         git update-ref -d {jref} >/dev/null 2>&1; \
         rmdir -- {parent} 2>/dev/null; \
         echo {t}clean",
        w = quote(&workspace_dir(r, job_id)),
        r = quote(r),
        jref = quote(&job_ref(job_id)),
        parent = quote(&format!("{r}.fleet-agents")),
    ))
}

/// PURE: remove every job worktree beside `root` older than
/// `older_than_mins`, and every `refs/fleet-agents/*` ref whose worktree is
/// gone. Prints `<tag>pruned`.
pub fn prune_script(root: &str, older_than_mins: u32) -> Result<String, String> {
    let r = check_root(root)?;
    let t = TAG;
    Ok(format!(
        "p={parent}; cd -- {r} 2>/dev/null || {{ echo {t}pruned; exit 0; }}; \
         if [ -d \"$p\" ]; then \
           find \"$p\" -mindepth 1 -maxdepth 1 -type d -name 'job-*' -mmin +{older_than_mins} \
             | while IFS= read -r w; do git worktree remove --force \"$w\" >/dev/null 2>&1 || rm -rf -- \"$w\"; done; \
         fi; \
         git worktree prune >/dev/null 2>&1; \
         git for-each-ref --format='%(refname)' refs/fleet-agents/ 2>/dev/null \
           | while IFS= read -r ref; do [ -d \"$p/${{ref##*/}}\" ] || git update-ref -d \"$ref\"; done; \
         rmdir -- \"$p\" 2>/dev/null; \
         echo {t}pruned",
        parent = quote(&format!("{r}.fleet-agents")),
        r = quote(r),
    ))
}

/// What [`prepare_script`] said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prepared {
    Ready,
    NoRoot,
    FetchFailed,
    /// The head moved past the commit the job was about.
    Moved,
    Failed,
    /// No tag: the shell failed before it said anything.
    Nothing,
}

/// PURE: read [`prepare_script`]'s stdout.
pub fn parse_prepared(stdout: &str) -> Prepared {
    match crate::service::claude_print::parse_tagged(
        stdout,
        TAG,
        &["ready", "noroot", "fetchfail", "moved", "failed"],
    ) {
        Some(("ready", _)) => Prepared::Ready,
        Some(("noroot", _)) => Prepared::NoRoot,
        Some(("fetchfail", _)) => Prepared::FetchFailed,
        Some(("moved", _)) => Prepared::Moved,
        Some(_) => Prepared::Failed,
        None => Prepared::Nothing,
    }
}

const CONNECT: Duration = Duration::from_secs(15);
/// A fetch of a large repo legitimately takes a while.
const PREPARE_WALL_CLOCK: Duration = Duration::from_secs(180);
const CLEANUP_WALL_CLOCK: Duration = Duration::from_secs(60);

fn script_err(e: String) -> IpcError {
    IpcError::new(codes::E_VALIDATE, e)
}

/// Prepare job `job_id`'s workspace on `host`; its path when ready.
pub async fn prepare(
    ssh: &SshClient,
    host: &str,
    root: &str,
    job_id: i64,
    source: &Source,
    expect_sha: Option<&str>,
) -> Result<(Prepared, String), IpcError> {
    let script = prepare_script(root, job_id, source, expect_sha).map_err(script_err)?;
    let out =
        crate::ssh::run_shell_bounded(ssh, host, &script, CONNECT, PREPARE_WALL_CLOCK).await?;
    Ok((
        parse_prepared(&String::from_utf8_lossy(&out.stdout)),
        workspace_dir(root, job_id),
    ))
}

/// Remove job `job_id`'s workspace on `host`. Best effort: the prune
/// catches what this misses.
pub async fn cleanup(ssh: &SshClient, host: &str, root: &str, job_id: i64) {
    let Ok(script) = cleanup_script(root, job_id) else {
        return;
    };
    if let Err(e) =
        crate::ssh::run_shell_bounded(ssh, host, &script, CONNECT, CLEANUP_WALL_CLOCK).await
    {
        tracing::warn!(host, job_id, error = %e.message, "[agents] workspace cleanup failed");
    }
}

/// Remove crashed runs' workspaces beside `root` on `host`. Best effort.
pub async fn prune(ssh: &SshClient, host: &str, root: &str) {
    let Ok(script) = prune_script(root, STALE_WORKSPACE_MINS) else {
        return;
    };
    if let Err(e) =
        crate::ssh::run_shell_bounded(ssh, host, &script, CONNECT, CLEANUP_WALL_CLOCK).await
    {
        tracing::warn!(host, error = %e.message, "[agents] workspace prune failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workspace_lives_beside_the_checkout() {
        assert_eq!(workspace_dir("/p/o/r", 7), "/p/o/r.fleet-agents/job-7");
        assert_eq!(workspace_dir("/p/o/r/", 7), "/p/o/r.fleet-agents/job-7");
    }

    #[test]
    fn prepare_fetches_into_the_jobs_own_ref() {
        let sc = prepare_script("/p/o/r", 7, &Source::PullHead(12), Some("ABC1234")).unwrap();
        assert!(
            sc.contains(
                "git fetch --no-tags --quiet origin '+pull/12/head:refs/fleet-agents/job-7'"
            ),
            "{sc}"
        );
        assert!(sc.contains("ref='refs/fleet-agents/job-7'"));
        assert!(sc.contains("w='/p/o/r.fleet-agents/job-7'"));
        assert!(sc.contains("in abc1234*) ;;"), "{sc}");
        assert!(sc.contains("git worktree add --detach --quiet \"$w\" \"$ref\""));
        assert!(!sc.contains("FETCH_HEAD"));
        let sc = prepare_script("/p/o/r", 7, &Source::Branch("main".into()), None).unwrap();
        assert!(
            sc.contains("'+refs/heads/main:refs/fleet-agents/job-7'"),
            "{sc}"
        );
        assert!(!sc.contains("moved"), "no expected commit, no check");
    }

    #[test]
    fn prepare_refuses_what_it_cannot_quote_safely() {
        for (root, src, sha) in [
            ("relative", Source::PullHead(1), None),
            ("/p/../etc", Source::PullHead(1), None),
            ("/", Source::PullHead(1), None),
            ("/p/o/r", Source::PullHead(0), None),
            ("/p/o/r", Source::Branch("-x".into()), None),
            ("/p/o/r", Source::Branch("a..b".into()), None),
            ("/p/o/r", Source::Branch("a b".into()), None),
            ("/p/o/r", Source::Branch("x.lock".into()), None),
            ("/p/o/r", Source::Branch("a@{1}".into()), None),
            ("/p/o/r", Source::PullHead(1), Some("not-a-sha")),
            ("/p/o/r", Source::PullHead(1), Some("abc")),
        ] {
            assert!(
                prepare_script(root, 1, &src, sha).is_err(),
                "{root} {src:?} {sha:?}"
            );
        }
        assert!(prepare_script("/p/o/r", 1, &Source::Branch("feat/x-1.2_y".into()), None).is_ok());
    }

    #[test]
    fn cleanup_and_prune_stay_beside_the_checkout() {
        let sc = cleanup_script("/p/o/r", 7).unwrap();
        assert!(sc.contains("w='/p/o/r.fleet-agents/job-7'"));
        assert!(sc.contains("git update-ref -d 'refs/fleet-agents/job-7'"));
        assert!(sc.contains("rmdir -- '/p/o/r.fleet-agents'"));
        let sc = prune_script("/p/o/r", 60).unwrap();
        assert!(sc.contains("p='/p/o/r.fleet-agents'"));
        assert!(sc.contains("-name 'job-*' -mmin +60"), "{sc}");
        assert!(sc.contains("refs/fleet-agents/"));
        assert!(prune_script("x", 60).is_err() && cleanup_script("", 1).is_err());
    }

    #[test]
    fn the_first_tag_decides() {
        assert_eq!(
            parse_prepared("motd\nFLEET_AGENT_WS=ready\n"),
            Prepared::Ready
        );
        assert_eq!(parse_prepared("FLEET_AGENT_WS=moved\n"), Prepared::Moved);
        assert_eq!(parse_prepared("FLEET_AGENT_WS=noroot\n"), Prepared::NoRoot);
        assert_eq!(
            parse_prepared("FLEET_AGENT_WS=fetchfail\n"),
            Prepared::FetchFailed
        );
        assert_eq!(parse_prepared(""), Prepared::Nothing);
    }

    /// The scripts against a real git repository: a fake `origin` with a
    /// PR head, a clone of it, then prepare, a moved head, cleanup, prune.
    /// Unix only: the scripts run on a fleet host's shell, and Git Bash on
    /// Windows does not take these paths.
    #[cfg(unix)]
    #[test]
    fn the_scripts_work_on_a_real_repository() {
        if crate::proc::std_command("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_str().unwrap().to_string();
        let sh = |script: &str| {
            let out = crate::proc::std_command("bash")
                .args(["-c", script])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let q = |s: &str| quote(s);
        let setup = format!(
            "set -e; export GIT_CONFIG_GLOBAL=/dev/null GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t \
             GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t; \
             git init -q -b main {o}; cd {o}; echo a > f; git add f; git commit -qm a; \
             git checkout -qb pr; echo b > f; git commit -qam b; \
             git update-ref refs/pull/3/head HEAD; git checkout -q main; \
             git clone -q {o} {c}; git -C {o} rev-parse refs/pull/3/head",
            o = q(&format!("{base}/origin")),
            c = q(&format!("{base}/clone")),
        );
        let head = sh(&setup).trim().to_string();
        assert_eq!(head.len(), 40, "setup failed");
        let root = format!("{base}/clone");
        let env = "export GIT_CONFIG_GLOBAL=/dev/null; ";
        let run = |sc: String| sh(&format!("{env}{sc}"));

        let got = run(prepare_script(&root, 9, &Source::PullHead(3), Some(&head[..12])).unwrap());
        assert_eq!(parse_prepared(&got), Prepared::Ready, "{got}");
        let w = workspace_dir(&root, 9);
        assert_eq!(std::fs::read_to_string(format!("{w}/f")).unwrap(), "b\n");
        let status = sh(&format!("{env}git -C {} status --porcelain", q(&root)));
        assert_eq!(status, "", "the checkout stays clean");

        let got = run(prepare_script(&root, 10, &Source::PullHead(3), Some("0000000")).unwrap());
        assert_eq!(parse_prepared(&got), Prepared::Moved, "{got}");
        let refs = sh(&format!(
            "{env}git -C {} for-each-ref refs/fleet-agents/",
            q(&root)
        ));
        assert!(
            !refs.contains("job-10"),
            "a moved head leaves no ref: {refs}"
        );

        let got = run(prepare_script(&root, 11, &Source::Branch("main".into()), None).unwrap());
        assert_eq!(parse_prepared(&got), Prepared::Ready, "{got}");

        run(cleanup_script(&root, 9).unwrap());
        assert!(!std::path::Path::new(&w).exists());
        // Job 11's worktree, aged past the stale line, is a crashed run's:
        // the prune removes it, its ref and the empty parent.
        // Set by the test itself: `touch -d '2 hours ago'` is GNU only.
        std::fs::File::open(workspace_dir(&root, 11))
            .unwrap()
            .set_modified(
                std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 60 * 60),
            )
            .unwrap();
        run(prune_script(&root, 60).unwrap());
        let refs = sh(&format!(
            "{env}git -C {} for-each-ref refs/fleet-agents/",
            q(&root)
        ));
        assert_eq!(refs, "", "{refs}");
        assert!(!std::path::Path::new(&format!("{root}.fleet-agents")).exists());

        let got =
            run(prepare_script(&format!("{base}/none"), 1, &Source::PullHead(3), None).unwrap());
        assert_eq!(parse_prepared(&got), Prepared::NoRoot);
    }
}
