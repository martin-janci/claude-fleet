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

/// How long reaching a remote host for the setup may take: a dead host
/// fails in seconds, not after the setup's whole budget.
const SETUP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

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
///
/// New worktrees live inside the checkout (`.worktrees/`,
/// `.claude/worktrees/`), so the copy never takes from a worktree: not a
/// match inside one (`**/.env` would copy every sibling's), and not one
/// inside a matched folder (`.claude` holds `.claude/worktrees/`, the new
/// worktree among them), which is copied entry by entry around it. The
/// worktrees are fleet's two folders and every one `git worktree list`
/// names. `.` and `..` are never a match (bash before 5.2 lets `.*` match
/// them), nor is any `.git`.
pub(crate) fn setup_script(base: &str, wt: &str, setup: &WorktreeSetup) -> String {
    let mut s = String::new();
    s.push_str(&format!("base={}\nwt={}\n", quote(base), quote(wt)));
    s.push_str(&format!(
        "cd \"$base\" || exit {NO_DIR}\n[ -d \"$wt\" ] || exit {NO_DIR}\n"
    ));
    if !setup.copy.is_empty() {
        s.push_str(
            "shopt -s nullglob dotglob\nshopt -s globstar 2>/dev/null\n\
             shopt -s globskipdots 2>/dev/null\n",
        );
        // The worktrees under the checkout, relative to it, one per line.
        s.push_str(&format!(
            "top=$(pwd -P)\n\
             wtp=$(cd \"$wt\" && pwd -P) || exit {NO_DIR}\n\
             skips=\n\
             while IFS= read -r p; do\n\
             \x20 case \"$p\" in \"$top\"|'') continue ;; \"$top\"/*) p=${{p#\"$top\"/}} ;; /*) continue ;; esac\n\
             \x20 skips=\"$skips$p\"$'\\n'\n\
             done <<FLEET_WORKTREES\n\
             .worktrees\n\
             .claude/worktrees\n\
             $wtp\n\
             $(git worktree list --porcelain 2>/dev/null | sed -n 's/^worktree //p')\n\
             FLEET_WORKTREES\n"
        ));
        // `under P`: P is a worktree or inside one. `holds D`: folder D has
        // one inside it.
        s.push_str(
            "under() {\n\
             \x20 while IFS= read -r k; do\n\
             \x20   [ -n \"$k\" ] || continue\n\
             \x20   case \"$1/\" in \"$k\"/*) return 0 ;; esac\n\
             \x20 done <<< \"$skips\"\n\
             \x20 return 1\n\
             }\n\
             holds() {\n\
             \x20 while IFS= read -r k; do\n\
             \x20   [ -n \"$k\" ] || continue\n\
             \x20   case \"$k/\" in \"$1\"/*) return 0 ;; esac\n\
             \x20 done <<< \"$skips\"\n\
             \x20 return 1\n\
             }\n",
        );
        s.push_str(&format!(
            "copy_dir() {{\n\
             \x20 if ! holds \"$1\"; then\n\
             \x20   mkdir -p \"$wt/$1\" && cp -R \"$1/.\" \"$wt/$1/\" || exit {COPY_FAILED}\n\
             \x20   return\n\
             \x20 fi\n\
             \x20 mkdir -p \"$wt/$1\" || exit {COPY_FAILED}\n\
             \x20 local e\n\
             \x20 for e in \"$1\"/*; do\n\
             \x20   case \"${{e##*/}}\" in .|..|.git) continue ;; esac\n\
             \x20   under \"$e\" && continue\n\
             \x20   if [ -d \"$e\" ] && [ ! -L \"$e\" ]; then\n\
             \x20     copy_dir \"$e\"\n\
             \x20   else\n\
             \x20     cp -pR \"$e\" \"$wt/$e\" || exit {COPY_FAILED}\n\
             \x20   fi\n\
             \x20 done\n\
             }}\n"
        ));
        s.push_str("IFS=$'\\n'\n");
        let pats: Vec<String> = setup.copy.iter().map(|g| quote(g)).collect();
        s.push_str(&format!("for pat in {}; do\n", pats.join(" ")));
        s.push_str(&format!(
            "  for f in $pat; do\n\
             \x20   while case \"$f\" in ./*) true ;; *) false ;; esac; do f=${{f#./}}; done\n\
             \x20   case \"$f\" in .|..|*/.|*/..|'') continue ;; esac\n\
             \x20   [ -e \"$f\" ] || continue\n\
             \x20   case \"/$f/\" in */.git/*) continue ;; esac\n\
             \x20   under \"$f\" && continue\n\
             \x20   echo \"copy $f\"\n\
             \x20   if [ -d \"$f\" ]; then\n\
             \x20     copy_dir \"$f\"\n\
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
        // A connect bound of its own: `run_cancellable` would make the 600 s
        // budget ssh's ConnectTimeout and its wall clock 3 × that (30 min).
        match ssh
            .run_bounded_cancellable(
                host,
                &["bash", "-lc", &quoted],
                SETUP_CONNECT_TIMEOUT,
                SETUP_TIMEOUT,
                token,
            )
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

    /// New worktrees live inside the checkout: a `**` glob does not copy
    /// the other worktrees' files, and a matched folder that holds them
    /// (`.claude` around `.claude/worktrees/`) is copied around them, the
    /// new worktree included, instead of into itself.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_copy_never_takes_from_a_worktree_inside_the_checkout() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().join("repo");
        for dir in [
            ".worktrees/a/app",
            ".claude/worktrees/b",
            ".claude/worktrees/new",
            ".claude/commands",
            "app",
        ] {
            std::fs::create_dir_all(base.join(dir)).unwrap();
        }
        std::fs::write(base.join(".env"), "TOP").unwrap();
        std::fs::write(base.join("app/.env"), "APP").unwrap();
        std::fs::write(base.join(".worktrees/a/.env"), "A").unwrap();
        std::fs::write(base.join(".worktrees/a/app/.env"), "A-APP").unwrap();
        std::fs::write(base.join(".worktrees/a/.git"), "gitdir: x").unwrap();
        std::fs::write(base.join(".claude/settings.json"), "{}").unwrap();
        std::fs::write(base.join(".claude/commands/go.md"), "go").unwrap();
        std::fs::write(base.join(".claude/worktrees/b/.env"), "B").unwrap();
        let wt = base.join(".claude/worktrees/new");
        run_local(&base, &wt, &setup(&["**/.env", ".claude"], None))
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(wt.join(".env")).unwrap(), "TOP");
        assert_eq!(std::fs::read_to_string(wt.join("app/.env")).unwrap(), "APP");
        assert!(!wt.join(".worktrees").exists(), "no sibling's files");
        assert!(wt.join(".claude/settings.json").exists());
        assert!(wt.join(".claude/commands/go.md").exists());
        assert!(
            !wt.join(".claude/worktrees").exists(),
            "the worktrees inside a copied folder stay out"
        );
    }

    /// A worktree `git worktree list` names, wherever it sits in the
    /// checkout, is not copied from either.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_worktree_git_lists_is_not_copied_from() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().join("repo");
        std::fs::create_dir_all(&base).unwrap();
        let git = |args: &[&str]| {
            let ok = crate::proc::std_command("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(&base)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "i"]);
        git(&["worktree", "add", "-q", "work/other", "-b", "other"]);
        std::fs::write(base.join("work/other/.env"), "OTHER").unwrap();
        std::fs::write(base.join(".env"), "TOP").unwrap();
        let wt = d.path().join("new");
        std::fs::create_dir_all(&wt).unwrap();
        run_local(&base, &wt, &setup(&["**/.env", "work"], None))
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(wt.join(".env")).unwrap(), "TOP");
        assert!(!wt.join("work/other").exists());
    }

    /// `.` and `..` are never a match: a `.` glob, or `.*` on a bash that
    /// lets it match them (before 5.2), does not copy the checkout over the
    /// new worktree.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_dot_glob_never_copies_the_checkout_itself() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().join("repo");
        std::fs::create_dir_all(base.join("src")).unwrap();
        std::fs::write(base.join("src/a.txt"), "uncommitted").unwrap();
        std::fs::write(base.join(".env"), "TOP").unwrap();
        let wt = base.join(".worktrees/new");
        std::fs::create_dir_all(wt.join("src")).unwrap();
        std::fs::write(wt.join("src/a.txt"), "branch").unwrap();
        run_local(&base, &wt, &setup(&[".", "./.", ".*", "./.env"], None))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(wt.join("src/a.txt")).unwrap(),
            "branch"
        );
        assert_eq!(std::fs::read_to_string(wt.join(".env")).unwrap(), "TOP");
        assert!(!wt.join(".worktrees").exists());
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
