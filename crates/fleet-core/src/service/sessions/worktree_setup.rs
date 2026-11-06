//! New worktree setup (docs/work-graph.md#new-worktree-setup): after a start
//! creates a worktree, copy the project's untracked files (`.env`, local
//! config) from the base checkout and run its setup command there, before
//! tmux starts. What to copy and run is the project's
//! [`crate::store::WorktreeSetup`] (migration 221), which a person types in
//! the project's menu.
//!
//! A failure refuses the start with `E_SETUP_FAILED` and leaves the worktree:
//! starting a session on it later skips setup, so the person can fix the
//! command, or the checkout, and go on.

use crate::ipc_error::{codes, IpcError};
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::WorktreeSetup;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// How long the copy and the command may take together (`pnpm install` on
/// a cold cache is minutes, not seconds).
pub(crate) const SETUP_TIMEOUT: Duration = Duration::from_secs(600);

/// How many output lines a failure keeps.
const TAIL_LINES: usize = 20;

/// Exit code of the copy step: kept apart from the command's own codes so
/// the message can say which part failed.
const COPY_FAILED: i32 = 97;
/// Exit code when a folder is missing.
const NO_DIR: i32 = 96;

/// The bash script that sets up worktree `wt` from checkout `base`. Every
/// value is quoted with [`quote`]; a glob is expanded by bash from an
/// unquoted variable with `IFS` set to a newline, so a space inside a glob
/// is part of it and nothing else in it is ever run. `**` reaches into
/// subfolders where bash has `globstar` (bash 4+; macOS's bash 3.2 reads it
/// as `*`). The command is the person's own and runs as typed, in `wt`.
pub(crate) fn setup_script(base: &str, wt: &str, setup: &WorktreeSetup) -> String {
    let mut s = String::new();
    s.push_str(&format!("base={}\nwt={}\n", quote(base), quote(wt)));
    s.push_str(&format!(
        "cd \"$base\" || exit {NO_DIR}\n[ -d \"$wt\" ] || exit {NO_DIR}\n"
    ));
    if !setup.copy.is_empty() {
        s.push_str("shopt -s nullglob dotglob\nshopt -s globstar 2>/dev/null\n");
        s.push_str("IFS=$'\\n'\n");
        let pats: Vec<String> = setup.copy.iter().map(|g| quote(g)).collect();
        s.push_str(&format!("for pat in {}; do\n", pats.join(" ")));
        s.push_str(&format!(
            "  for f in $pat; do\n\
             \x20   [ -e \"$f\" ] || continue\n\
             \x20   case \"/$f/\" in */.git/*) continue ;; esac\n\
             \x20   echo \"copy $f\"\n\
             \x20   if [ -d \"$f\" ]; then\n\
             \x20     mkdir -p \"$wt/$f\" && cp -R \"$f/.\" \"$wt/$f/\" || exit {COPY_FAILED}\n\
             \x20   else\n\
             \x20     mkdir -p \"$wt/$(dirname \"$f\")\" && cp -p \"$f\" \"$wt/$f\" || exit {COPY_FAILED}\n\
             \x20   fi\n\
             \x20 done\n\
             done\n\
             unset IFS\n"
        ));
    }
    if let Some(cmd) = setup.command.as_deref() {
        s.push_str("cd \"$wt\" || exit ");
        s.push_str(&NO_DIR.to_string());
        s.push('\n');
        s.push_str(cmd);
        s.push('\n');
    }
    s
}

/// The last [`TAIL_LINES`] non-empty lines of stdout and stderr together.
fn tail(stdout: &[u8], stderr: &[u8]) -> String {
    let all = format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    );
    let lines: Vec<&str> = all.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
}

/// The refusal for a script that exited `code` (`None`: killed or timed out).
pub(crate) fn failure(code: Option<i32>, wt: &str, out: &str) -> IpcError {
    let what = match code {
        Some(COPY_FAILED) => "copying files from the project folder failed".to_string(),
        Some(NO_DIR) => "the project folder or the new worktree is missing".to_string(),
        Some(c) => format!("the setup command exited {c}"),
        None => format!(
            "the setup did not finish within {} minutes",
            SETUP_TIMEOUT.as_secs() / 60
        ),
    };
    let mut msg = format!(
        "Setup failed: {what}. The worktree is at {wt}; starting a session on it skips setup."
    );
    if !out.is_empty() {
        msg.push_str("\n\n");
        msg.push_str(out);
    }
    IpcError::new(codes::E_SETUP_FAILED, msg)
}

/// Run `setup` for the new worktree `wt` of checkout `base` on `host`.
/// Nothing to do returns at once.
pub(crate) async fn run(
    ssh: &SshClient,
    host: &str,
    base: &str,
    wt: &str,
    setup: &WorktreeSetup,
    token: CancellationToken,
) -> Result<(), IpcError> {
    if setup.is_empty() {
        return Ok(());
    }
    let script = setup_script(base, wt, setup);
    let out = if host == crate::service::projects::LOCAL_HOST {
        let mut cmd = crate::proc::command("bash");
        cmd.args(["-lc", &script]).kill_on_drop(true);
        let fut = cmd.output();
        tokio::select! {
            r = tokio::time::timeout(SETUP_TIMEOUT, fut) => match r {
                Ok(r) => r.map_err(|e| IpcError::new(codes::E_SHELL, format!("bash: {e}")))?,
                Err(_) => return Err(failure(None, wt, "")),
            },
            _ = token.cancelled() => {
                return Err(IpcError::new(codes::E_CANCELLED, "start cancelled during setup"));
            }
        }
    } else {
        let quoted = quote(&script);
        match ssh
            .run_cancellable(host, &["bash", "-lc", &quoted], SETUP_TIMEOUT, token)
            .await
        {
            Ok(out) => out,
            Err(e) if e.code == codes::E_TIMEOUT || e.code == codes::E_SSH_TIMEOUT => {
                return Err(failure(None, wt, ""))
            }
            Err(e) => return Err(e),
        }
    };
    if out.status.success() {
        return Ok(());
    }
    Err(failure(
        out.status.code(),
        wt,
        &tail(&out.stdout, &out.stderr),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn setup(copy: &[&str], command: Option<&str>) -> WorktreeSetup {
        WorktreeSetup {
            copy: copy.iter().map(|s| s.to_string()).collect(),
            command: command.map(str::to_string),
        }
    }

    // Bash with POSIX paths: the Windows runner has neither.
    #[cfg(unix)]
    async fn run_local(
        base: &std::path::Path,
        wt: &std::path::Path,
        s: &WorktreeSetup,
    ) -> Result<(), IpcError> {
        let script = setup_script(&base.to_string_lossy(), &wt.to_string_lossy(), s);
        let out = crate::proc::command("bash")
            .args(["-c", &script])
            .output()
            .await
            .unwrap();
        if out.status.success() {
            Ok(())
        } else {
            Err(failure(
                out.status.code(),
                &wt.to_string_lossy(),
                &tail(&out.stdout, &out.stderr),
            ))
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn copies_the_matching_files_and_runs_the_command_in_the_worktree() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().join("base dir");
        let wt = d.path().join("wt");
        std::fs::create_dir_all(base.join("config/deep")).unwrap();
        std::fs::create_dir_all(base.join(".git")).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(base.join(".env"), "A=1").unwrap();
        std::fs::write(base.join("config/app.local"), "x").unwrap();
        std::fs::write(base.join("config/deep/b.local"), "y").unwrap();
        std::fs::write(base.join("config/app.yml"), "no").unwrap();
        std::fs::write(base.join(".git/config"), "secret").unwrap();
        std::fs::write(base.join("we ird;$(touch pwned).txt"), "z").unwrap();
        let s = setup(
            &[
                ".env",
                "config/*.local",
                "missing.txt",
                "we ird;$(touch pwned).txt",
                ".gi*",
            ],
            Some("echo ran > marker"),
        );
        run_local(&base, &wt, &s).await.unwrap();
        assert_eq!(std::fs::read_to_string(wt.join(".env")).unwrap(), "A=1");
        assert!(wt.join("config/app.local").exists());
        assert!(
            !wt.join("config/deep/b.local").exists(),
            "one * stays in its folder"
        );
        assert!(!wt.join("config/app.yml").exists());
        assert!(
            !wt.join(".git").exists(),
            "git's own files are never copied"
        );
        assert!(wt.join("we ird;$(touch pwned).txt").exists());
        assert!(!base.join("pwned").exists() && !wt.join("pwned").exists());
        assert_eq!(
            std::fs::read_to_string(wt.join("marker")).unwrap().trim(),
            "ran"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failing_command_says_setup_failed_and_keeps_its_output() {
        let d = tempfile::tempdir().unwrap();
        let (base, wt) = (d.path().join("b"), d.path().join("w"));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        let err = run_local(
            &base,
            &wt,
            &setup(&[], Some("echo lockfile is stale >&2; exit 3")),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_SETUP_FAILED);
        assert!(
            err.message
                .starts_with("Setup failed: the setup command exited 3."),
            "{}",
            err.message
        );
        assert!(err.message.contains("skips setup"));
        assert!(
            err.message.ends_with("lockfile is stale"),
            "{}",
            err.message
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_missing_worktree_is_not_blamed_on_the_command() {
        let d = tempfile::tempdir().unwrap();
        let err = run_local(d.path(), &d.path().join("gone"), &setup(&[".env"], None))
            .await
            .unwrap_err();
        assert!(err.message.contains("missing"), "{}", err.message);
    }

    #[tokio::test]
    async fn nothing_to_do_runs_nothing() {
        let ssh = SshClient::new();
        run(
            &ssh,
            "nowhere",
            "/x",
            "/y",
            &WorktreeSetup::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    }
}
