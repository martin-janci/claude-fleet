//! Git on the host, in a link's worktree by its path (Phases 2 and 3,
//! `docs/superpowers/specs/2026-10-07-local-workspace-handoff-design.md`).
//! Unlike `service::repo_read`, which finds the worktree through a session's
//! pane, these work with no session running: the link knows the path.
//!
//! Status and diff only read. Commit, discard and the conflict helpers write,
//! each only when a person pressed the button that says so; the sync itself
//! never runs any of them.

use super::remote::{nul_list, prologue, run};
use crate::ipc_error::{codes, IpcError};
use crate::service::repo::diff_from_bytes;
use crate::service::repo_read::{parse_status_z, ChangedFile, FileDiff};
use crate::shell::quote;
use crate::ssh::SshExec;
use std::time::Duration;

const GIT_MARK: &str = "@@FLEET-GIT@@\n";
const WALL_CLOCK: Duration = Duration::from_secs(120);
/// A status or diff answer, at most.
const MAX_OUTPUT: usize = 8 * 1024 * 1024;

/// Every path here is a file name a person picked, never a pattern:
/// `GIT_LITERAL_PATHSPECS` keeps `pages/[id].tsx` from also matching
/// `pages/i.tsx` in a commit, discard or diff (as `catalog::repo` does).
fn script(root: &str, body: &str) -> String {
    format!(
        "{prologue}export GIT_LITERAL_PATHSPECS=1\n\
         git rev-parse --git-dir >/dev/null 2>&1 || {{ printf '@@FLEET-ERR@@ notgit\\n'; exit 4; }}\n\
         f=$(mktemp) || exit 5\ntrap 'rm -f -- \"$f\"' EXIT\ncat > \"$f\"\n{body}",
        prologue = prologue(root)
    )
}

async fn git(
    ssh: &dyn SshExec,
    host: &str,
    root: &str,
    body: &str,
    stdin: Vec<u8>,
) -> Result<Vec<u8>, IpcError> {
    run(
        ssh,
        host,
        &script(root, body),
        stdin,
        GIT_MARK,
        WALL_CLOCK,
        MAX_OUTPUT,
    )
    .await
}

/// The branch checked out (`HEAD` when detached) and `git status`.
pub(super) fn status_body() -> String {
    format!(
        "b=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo HEAD)\n\
         printf '{GIT_MARK}%s\\0' \"$b\"\n\
         git status --porcelain=v1 -z -uall\n"
    )
}

pub(super) async fn status(
    ssh: &dyn SshExec,
    host: &str,
    root: &str,
) -> Result<(String, Vec<ChangedFile>), IpcError> {
    let body = git(ssh, host, root, &status_body(), Vec::new()).await?;
    let (branch, rest) = match body.iter().position(|b| *b == 0) {
        Some(i) => (
            String::from_utf8_lossy(&body[..i]).to_string(),
            &body[i + 1..],
        ),
        None => (String::new(), &body[..]),
    };
    Ok((branch, parse_status_z(rest)))
}

/// `git diff HEAD -- path`, or the whole file as added when git does not
/// track it (or there is no HEAD yet).
pub(super) fn diff_body(path: &str) -> String {
    let p = quote(path);
    format!(
        "printf '{GIT_MARK}'\n\
         if git rev-parse --verify -q HEAD >/dev/null 2>&1 &&\n\
            {{ git ls-files --error-unmatch -- {p} >/dev/null 2>&1 || git cat-file -e HEAD:{p} 2>/dev/null; }}; then\n\
           git diff --no-color HEAD -- {p}\n\
         else\n\
           git diff --no-color --no-index -- /dev/null {p}\n\
         fi\n\
         exit 0\n"
    )
}

pub(super) async fn diff(
    ssh: &dyn SshExec,
    host: &str,
    root: &str,
    path: &str,
) -> Result<FileDiff, IpcError> {
    let raw = git(ssh, host, root, &diff_body(path), Vec::new()).await?;
    let (diff, binary, truncated) = diff_from_bytes(&raw);
    Ok(FileDiff {
        path: path.to_string(),
        diff,
        binary,
        truncated,
    })
}

/// Commit exactly the paths on stdin (NUL-separated), additions and
/// deletions included, with `message`. Prints the new commit.
pub(super) fn commit_body(message: &str) -> String {
    format!(
        "git add -A --pathspec-from-file=\"$f\" --pathspec-file-nul || exit 6\n\
         git commit -q -m {m} --pathspec-from-file=\"$f\" --pathspec-file-nul || exit 7\n\
         printf '{GIT_MARK}%s' \"$(git rev-parse HEAD)\"\n",
        m = quote(message)
    )
}

pub(super) async fn commit(
    ssh: &dyn SshExec,
    host: &str,
    root: &str,
    message: &str,
    paths: &[String],
) -> Result<String, IpcError> {
    let out = git(ssh, host, root, &commit_body(message), nul_list(paths))
        .await
        .map_err(|e| git_failed(e, "commit"))?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

/// Put each path on stdin back to `HEAD`: a tracked file is restored (index
/// and worktree), one `HEAD` does not have is removed. `git checkout` and
/// `git reset` rather than `git restore`, which older hosts lack.
pub(super) fn discard_body() -> String {
    format!(
        "while IFS= read -r -d '' p; do\n\
           if git rev-parse --verify -q HEAD >/dev/null 2>&1 && git cat-file -e \"HEAD:$p\" 2>/dev/null; then\n\
             git reset -q -- \"$p\" >/dev/null 2>&1\n\
             git checkout -q HEAD -- \"$p\" || exit 6\n\
           else\n\
             git rm -q --cached --ignore-unmatch -- \"$p\" >/dev/null 2>&1\n\
             rm -f -- \"$p\"\n\
           fi\n\
         done < \"$f\"\n\
         printf '{GIT_MARK}'\n"
    )
}

pub(super) async fn discard(
    ssh: &dyn SshExec,
    host: &str,
    root: &str,
    paths: &[String],
) -> Result<(), IpcError> {
    git(ssh, host, root, &discard_body(), nul_list(paths))
        .await
        .map_err(|e| git_failed(e, "discard"))?;
    Ok(())
}

/// Diff the bytes on stdin (the local version, or nothing when `local` is
/// absent) against `path` in the worktree (or nothing when it is absent
/// there): local → remote.
pub(super) fn compare_body(path: &str, local: bool) -> String {
    let p = quote(path);
    let a = if local { "\"$f\"" } else { "/dev/null" };
    format!(
        "printf '{GIT_MARK}'\n\
         if [ -f {p} ]; then b={p}; else b=/dev/null; fi\n\
         git diff --no-color --no-index -- {a} \"$b\"\n\
         exit 0\n"
    )
}

pub(super) async fn compare(
    ssh: &dyn SshExec,
    host: &str,
    root: &str,
    path: &str,
    local: Option<Vec<u8>>,
) -> Result<FileDiff, IpcError> {
    let has_local = local.is_some();
    let raw = git(
        ssh,
        host,
        root,
        &compare_body(path, has_local),
        local.unwrap_or_default(),
    )
    .await?;
    let (diff, binary, truncated) = diff_from_bytes(&raw);
    Ok(FileDiff {
        path: path.to_string(),
        diff,
        binary,
        truncated,
    })
}

/// Write the bytes on stdin to `<path>.fleet-local` next to `path` (the
/// local version of a conflicting file, for the agent to merge from). The
/// sync never carries that name ([`super::excludes::FIXED_EXCLUDES`]).
pub(super) fn stage_local_copy_body(path: &str) -> String {
    let dst = quote(&format!("{path}.fleet-local"));
    format!(
        "d=$(dirname -- {dst}); mkdir -p -- \"$d\" || exit 6\n\
         cp -- \"$f\" {dst} || exit 6\n\
         printf '{GIT_MARK}'\n"
    )
}

pub(super) async fn stage_local_copy(
    ssh: &dyn SshExec,
    host: &str,
    root: &str,
    path: &str,
    bytes: Vec<u8>,
) -> Result<(), IpcError> {
    git(ssh, host, root, &stage_local_copy_body(path), bytes).await?;
    Ok(())
}

/// A script that exited before its marker failed at the git step: say
/// which, keeping git's own words.
fn git_failed(e: IpcError, what: &str) -> IpcError {
    if e.code == codes::E_IO && e.message.contains("did not run") {
        IpcError::new(codes::E_GIT, format!("git {what} failed: {}", e.message))
    } else {
        e
    }
}
