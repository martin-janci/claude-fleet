//! Run a `bash -lc` script on THIS machine under a wall clock and an optional
//! cancellation token, and make sure a script that is abandoned is actually
//! gone — not just forgotten.
//!
//! `tokio::process::Command` defaults to `kill_on_drop(false)`, so wrapping
//! `.output()` in `tokio::time::timeout` only drops the future: the shell and
//! everything it started keep running. For a `git clone` that means the
//! orphan keeps writing into the destination and the user's retry fails on a
//! non-empty directory. `kill_on_drop(true)` alone is not enough either — it
//! signals `bash` only, and a multi-command script's children (`git`, and the
//! `git-remote-https` / `index-pack` under it) are not in its process tree
//! any more once `bash` dies. `SshClient::run_child` has the same
//! kill-and-reap shape for its `ssh` child; this is the local counterpart,
//! extended to the whole process group.
//!
//! On timeout or cancel the script's process group gets `SIGTERM` (git
//! removes a half-written clone directory on `SIGTERM`), then `SIGKILL` if
//! anything is still alive after [`TERM_GRACE`]. The call returns only once
//! the group is empty (bounded by [`KILL_GRACE`] after the `SIGKILL`), so a
//! retry never races the orphan.

use crate::ipc_error::{codes, IpcError};
use std::process::Output;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// How long a script's process group gets to exit on `SIGTERM` before it is
/// sent `SIGKILL`.
const TERM_GRACE: Duration = Duration::from_secs(2);

/// How long to wait for the group to disappear after `SIGKILL` before giving
/// up waiting (the signal is already delivered; this only bounds the return).
const KILL_GRACE: Duration = Duration::from_secs(1);

/// Why [`run_bash_script`] did not produce an `Output`.
#[derive(Debug)]
pub enum LocalScriptError {
    /// `bash` could not be spawned, or waiting on it failed.
    Io(std::io::Error),
    /// The wall clock elapsed; the script's process group was killed.
    TimedOut(Duration),
    /// The token fired; the script's process group was killed.
    Cancelled,
}

impl From<LocalScriptError> for IpcError {
    /// The codes and messages the local-script call sites have always used:
    /// `E_SHELL` for a spawn failure, `E_TIMEOUT` for the wall clock.
    fn from(e: LocalScriptError) -> Self {
        match e {
            LocalScriptError::Io(e) => IpcError::new(codes::E_SHELL, format!("spawn bash: {e}")),
            LocalScriptError::TimedOut(wall) => IpcError::new(
                codes::E_TIMEOUT,
                format!("local script exceeded {}s", wall.as_secs()),
            ),
            LocalScriptError::Cancelled => {
                IpcError::new(codes::E_CANCELLED, "local script cancelled")
            }
        }
    }
}

/// Run `script` via `bash -lc`, bounded by `wall_clock` and, if given,
/// `token`. Returns the raw `Output` for ANY exit status — same contract as
/// `SshExec::run`; mapping a failed script to an error is the caller's job.
///
/// Exits in priority order (`biased`, so a cancel racing the deadline wins):
/// token fired → [`LocalScriptError::Cancelled`]; wall clock elapsed →
/// [`LocalScriptError::TimedOut`]; script exited → `Ok`. Both abandon arms
/// kill the whole process group and reap `bash` before returning, so a
/// timed-out call can take up to `wall_clock + TERM_GRACE + KILL_GRACE`.
///
/// Every value the caller interpolates into `script` must already be quoted
/// with `crate::shell::quote`; `script` itself is passed as one argv word.
pub async fn run_bash_script(
    script: &str,
    wall_clock: Duration,
    token: Option<CancellationToken>,
) -> Result<Output, LocalScriptError> {
    let mut child = tokio::process::Command::new("bash")
        .arg("-lc")
        .arg(script)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // Own process group (pgid = bash's pid) so the abandon arms can
        // signal everything the script started, not just `bash`.
        .process_group(0)
        // Belt-and-suspenders: if this future is dropped mid-wait (the
        // caller's own timeout/select), at least `bash` dies with it.
        .kill_on_drop(true)
        .spawn()
        .map_err(LocalScriptError::Io)?;
    // `id()` is `Some` until the child is reaped, which cannot have happened
    // yet. It doubles as the process-group id.
    let pgid = child.id().map(|p| p as libc::pid_t);

    // Drain both pipes concurrently so a chatty script can never block on a
    // full pipe while we wait on it.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_task = tokio::spawn(read_all(stdout));
    let stderr_task = tokio::spawn(read_all(stderr));

    let cancelled = async {
        match token {
            Some(t) => t.cancelled().await,
            None => std::future::pending::<()>().await,
        }
    };

    let abandoned = tokio::select! {
        biased;
        _ = cancelled => LocalScriptError::Cancelled,
        _ = tokio::time::sleep(wall_clock) => LocalScriptError::TimedOut(wall_clock),
        status = child.wait() => {
            let status = status.map_err(LocalScriptError::Io)?;
            let stdout = stdout_task.await.unwrap_or_default();
            let stderr = stderr_task.await.unwrap_or_default();
            return Ok(Output { status, stdout, stderr });
        }
    };

    kill_group_and_reap(&mut child, pgid).await;
    // The readers would normally finish on EOF, but a process that escaped
    // the group (`setsid`) could still hold a pipe open and leak the task.
    stdout_task.abort();
    stderr_task.abort();
    Err(abandoned)
}

async fn read_all<R: tokio::io::AsyncRead + Unpin>(pipe: Option<R>) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(mut p) = pipe {
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut p, &mut buf).await;
    }
    buf
}

/// `SIGTERM` the group, escalate to `SIGKILL` after [`TERM_GRACE`], and wait
/// (bounded) until no member is left. `bash` itself is reaped here; members
/// it orphaned are reaped by init, which the liveness poll waits out.
async fn kill_group_and_reap(child: &mut tokio::process::Child, pgid: Option<libc::pid_t>) {
    let Some(pgid) = pgid else {
        let _ = child.start_kill();
        let _ = child.wait().await;
        return;
    };
    signal_group(pgid, libc::SIGTERM);
    if !wait_group_gone(child, pgid, TERM_GRACE).await {
        signal_group(pgid, libc::SIGKILL);
        wait_group_gone(child, pgid, KILL_GRACE).await;
    }
    // Whatever happened above, `bash` must not be left a zombie.
    let _ = child.start_kill();
    let _ = child.wait().await;
}

fn signal_group(pgid: libc::pid_t, sig: libc::c_int) {
    // SAFETY: `killpg` has no memory-safety preconditions; an already-empty
    // group just yields ESRCH, which is the outcome we want anyway.
    unsafe {
        libc::killpg(pgid, sig);
    }
}

/// Poll until the process group has no members or `within` elapses; returns
/// whether it emptied. Reaps `bash` along the way — an unreaped zombie still
/// counts as a group member.
async fn wait_group_gone(
    child: &mut tokio::process::Child,
    pgid: libc::pid_t,
    within: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let _ = child.try_wait();
        if !group_alive(pgid) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn group_alive(pgid: libc::pid_t) -> bool {
    // SAFETY: signal 0 only checks existence/permission; nothing is sent.
    unsafe { libc::killpg(pgid, 0) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn pid_alive(pid: libc::pid_t) -> bool {
        // SAFETY: signal 0 only checks existence.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    /// A script that records its own pid and a background grandchild's pid in
    /// `pidfile`, then blocks on the grandchild — the `bash` → `git clone`
    /// shape. `trap '' TERM` in the grandchild forces the SIGKILL escalation.
    fn long_script(pidfile: &Path, ignore_term: bool) -> String {
        let trap = if ignore_term { "trap '' TERM; " } else { "" };
        format!(
            "{trap}sleep 300 & echo \"$$ $!\" > {f}.tmp && mv {f}.tmp {f}; wait",
            f = crate::shell::quote(&pidfile.to_string_lossy()),
        )
    }

    async fn read_pids(pidfile: &Path) -> Vec<libc::pid_t> {
        for _ in 0..250 {
            if let Ok(s) = std::fs::read_to_string(pidfile) {
                return s.split_whitespace().map(|p| p.parse().unwrap()).collect();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("script never wrote {}", pidfile.display());
    }

    fn assert_all_dead(pids: &[libc::pid_t]) {
        assert_eq!(pids.len(), 2, "expected bash + grandchild pids: {pids:?}");
        for &pid in pids {
            assert!(
                !pid_alive(pid),
                "pid {pid} still alive after the call returned"
            );
        }
    }

    #[tokio::test]
    async fn returns_output_for_any_exit_status() {
        let out = run_bash_script(
            "echo hi; echo oops >&2; exit 7",
            Duration::from_secs(10),
            None,
        )
        .await
        .unwrap();
        assert_eq!(out.status.code(), Some(7));
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");
        assert_eq!(String::from_utf8_lossy(&out.stderr).trim(), "oops");
    }

    #[tokio::test]
    async fn a_timed_out_script_and_its_children_are_gone() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pids");
        let started = std::time::Instant::now();
        let err = run_bash_script(&long_script(&pidfile, false), Duration::from_secs(1), None)
            .await
            .unwrap_err();
        assert!(matches!(err, LocalScriptError::TimedOut(d) if d == Duration::from_secs(1)));
        assert!(
            started.elapsed()
                < Duration::from_secs(1) + TERM_GRACE + KILL_GRACE + Duration::from_secs(2)
        );
        assert_all_dead(&read_pids(&pidfile).await);
    }

    #[tokio::test]
    async fn a_child_ignoring_sigterm_is_killed_on_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pids");
        let err = run_bash_script(&long_script(&pidfile, true), Duration::from_secs(1), None)
            .await
            .unwrap_err();
        assert!(matches!(err, LocalScriptError::TimedOut(_)));
        assert_all_dead(&read_pids(&pidfile).await);
    }

    #[tokio::test]
    async fn a_cancelled_script_and_its_children_are_gone() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pids");
        let token = CancellationToken::new();
        let script = long_script(&pidfile, false);
        let call = tokio::spawn({
            let token = token.clone();
            async move { run_bash_script(&script, Duration::from_secs(300), Some(token)).await }
        });
        // Cancel only once the grandchild is running, so the kill has
        // something to find.
        let pids = read_pids(&pidfile).await;
        assert!(pids.iter().all(|&p| pid_alive(p)));
        token.cancel();
        let err = tokio::time::timeout(Duration::from_secs(10), call)
            .await
            .expect("cancel did not return promptly")
            .unwrap()
            .unwrap_err();
        assert!(matches!(err, LocalScriptError::Cancelled));
        assert_all_dead(&pids);
    }

    #[tokio::test]
    async fn a_killed_script_stops_writing() {
        // The user-visible symptom: an orphan keeps writing after the call
        // gave up. Once the call returns, the file must stop growing. The
        // writer is a background subshell, like `git` under `bash`.
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let script = format!(
            "( while :; do echo x >> {f}; sleep 0.05; done ) & wait",
            f = crate::shell::quote(&out.to_string_lossy())
        );
        let err = run_bash_script(&script, Duration::from_millis(500), None)
            .await
            .unwrap_err();
        assert!(matches!(err, LocalScriptError::TimedOut(_)));
        let len = std::fs::metadata(&out).unwrap().len();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(std::fs::metadata(&out).unwrap().len(), len);
    }

    #[test]
    fn errors_map_to_the_historic_codes_and_messages() {
        let e: IpcError = LocalScriptError::TimedOut(Duration::from_secs(90)).into();
        assert_eq!(e.code, codes::E_TIMEOUT);
        assert_eq!(e.message, "local script exceeded 90s");
        let e: IpcError =
            LocalScriptError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "nope")).into();
        assert_eq!(e.code, codes::E_SHELL);
        assert_eq!(e.message, "spawn bash: nope");
        let e: IpcError = LocalScriptError::Cancelled.into();
        assert_eq!(e.code, codes::E_CANCELLED);
    }
}
