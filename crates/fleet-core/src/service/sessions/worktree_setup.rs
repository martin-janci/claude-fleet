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
/// a cold cache is minutes, not seconds). It must fit inside the start's
/// own call: `new_session` and `work_link` are cut at 5 minutes
/// (`Deadline::Lifecycle`), so the setup gets 4 and the worktree, tmux and
/// the prompt the rest. Past it the start fails with "Setup failed", not a
/// dropped call.
pub(crate) const SETUP_TIMEOUT: Duration = Duration::from_secs(240);

/// The ssh connect budget: an unreachable host fails within it rather than
/// after [`SETUP_TIMEOUT`].
const SETUP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How many output lines a failure keeps.
const TAIL_LINES: usize = 20;
/// How many bytes of output the host sends back: the script's output goes
/// to a temp file there and only its tail is printed.
const TAIL_BYTES: usize = 4000;
/// How many characters of one kept line a failure shows.
const LINE_CHARS: usize = 300;

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
/// The copy loop runs in a subshell, so its `shopt`s and `IFS` never reach
/// the command (under `nullglob`, `pip install -e .[dev]` lost its last
/// word). It never reads from or writes into another worktree: the
/// worktrees live inside the base checkout (`WORKTREE_BASE_SNIPPET`), so a
/// match under `.worktrees/` or `.claude/worktrees/` is skipped, as is one
/// with a `.`, `..` or empty component (`config/.*` matches `config/..`
/// where bash has no `globskipdots`), and a matched `.claude` folder is
/// copied without its `worktrees`.
pub(crate) fn setup_script(base: &str, wt: &str, setup: &WorktreeSetup) -> String {
    let mut s = String::new();
    s.push_str(&format!("base={}\nwt={}\n", quote(base), quote(wt)));
    s.push_str(&format!(
        "cd \"$base\" || exit {NO_DIR}\n[ -d \"$wt\" ] || exit {NO_DIR}\n"
    ));
    if !setup.copy.is_empty() {
        s.push_str("(\nshopt -s nullglob dotglob\nshopt -s globstar 2>/dev/null\n");
        s.push_str("IFS=$'\\n'\n");
        let pats: Vec<String> = setup.copy.iter().map(|g| quote(g)).collect();
        s.push_str(&format!("for pat in {}; do\n", pats.join(" ")));
        s.push_str(&format!(
            "  for f in $pat; do\n\
             \x20   while [ \"${{f#./}}\" != \"$f\" ]; do f=${{f#./}}; done\n\
             \x20   f=${{f%/}}\n\
             \x20   [ -e \"./$f\" ] || continue\n\
             \x20   case \"/$f/\" in\n\
             \x20     */.git/*|/.worktrees/*|/.claude/worktrees/*|*/./*|*/../*|*//*) continue ;;\n\
             \x20   esac\n\
             \x20   echo \"copy $f\"\n\
             \x20   if [ -d \"./$f\" ]; then\n\
             \x20     mkdir -p -- \"$wt/$f\" || exit {COPY_FAILED}\n\
             \x20     if [ \"$f\" = .claude ] && [ -e .claude/worktrees ]; then\n\
             \x20       echo \"  without .claude/worktrees: other worktrees are never copied\"\n\
             \x20       for c in .claude/*; do\n\
             \x20         [ \"$c\" = .claude/worktrees ] || cp -R -- \"./$c\" \"$wt/.claude/\" || exit {COPY_FAILED}\n\
             \x20       done\n\
             \x20     else\n\
             \x20       cp -R -- \"./$f/.\" \"$wt/$f/\" || exit {COPY_FAILED}\n\
             \x20     fi\n\
             \x20   else\n\
             \x20     mkdir -p -- \"$wt/$(dirname -- \"$f\")\" && cp -p -- \"./$f\" \"$wt/$f\" || exit {COPY_FAILED}\n\
             \x20   fi\n\
             \x20 done\n\
             done\n\
             ) || exit $?\n"
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

/// The last [`TAIL_LINES`] non-empty lines of stdout and stderr together,
/// each cut at [`LINE_CHARS`] characters.
fn tail(stdout: &[u8], stderr: &[u8]) -> String {
    let all = format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    );
    let lines: Vec<String> = all
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| match l.char_indices().nth(LINE_CHARS) {
            Some((i, _)) => format!("{}…", &l[..i]),
            None => l.to_string(),
        })
        .collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
}

/// `script` under a wrapper that sends its output to a temp file on the
/// host and prints only the last [`TAIL_BYTES`] of it, so a noisy install
/// cannot stream megabytes back; it exits with the script's code. The
/// script is one quoted word for an inner `bash -c`, which inherits the
/// outer login shell's environment.
fn bounded_script(script: &str) -> String {
    format!(
        "log=$(mktemp 2>/dev/null) || log=/dev/null\n\
         bash -c {} >\"$log\" 2>&1\n\
         rc=$?\n\
         tail -c {TAIL_BYTES} \"$log\" 2>/dev/null\n\
         [ \"$log\" = /dev/null ] || rm -f \"$log\"\n\
         exit $rc\n",
        quote(script)
    )
}

/// The refusal for a script that exited `code` (`None`: killed or timed out).
pub(crate) fn failure(code: Option<i32>, wt: &str, out: &str) -> IpcError {
    let what = match code {
        Some(COPY_FAILED) => "copying files from the project folder failed".to_string(),
        Some(NO_DIR) => "the project folder or the new worktree is missing".to_string(),
        Some(c) => format!("the setup command exited {c}"),
        None => format!(
            "the setup did not finish within {} minutes and was stopped \
             (processes it started may still be running on the host)",
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
    let script = bounded_script(&setup_script(base, wt, setup));
    let local = host == crate::service::projects::LOCAL_HOST;
    let ran = if local {
        // Dropping the run on a cancel kills its `bash` (`kill_on_drop`).
        tokio::select! {
            r = crate::ssh::run_shell_bounded(ssh, host, &script, SETUP_CONNECT_TIMEOUT, SETUP_TIMEOUT) => r,
            _ = token.cancelled() => Err(IpcError::new(codes::E_CANCELLED, "start cancelled during setup")),
        }
    } else {
        ssh.run_bounded_cancellable(
            host,
            &["bash", "-lc", &quote(&script)],
            SETUP_CONNECT_TIMEOUT,
            SETUP_TIMEOUT,
            token,
        )
        .await
    };
    let out = match ran {
        Ok(out) => out,
        Err(e) if e.code == codes::E_TIMEOUT || e.code == codes::E_SSH_TIMEOUT => {
            return Err(failure(None, wt, ""))
        }
        Err(e) => return Err(e),
    };
    if out.status.success() {
        return Ok(());
    }
    let kept = tail(&out.stdout, &out.stderr);
    if !local && out.status.code() == Some(SSH_FAILED) {
        return Err(unreachable(host, wt, &kept));
    }
    Err(failure(out.status.code(), wt, &kept))
}

/// ssh's own exit code: it could not connect or lost the connection.
const SSH_FAILED: i32 = 255;

/// The refusal when ssh itself failed, so the message does not blame the
/// command. The worktree may or may not have been set up.
fn unreachable(host: &str, wt: &str, out: &str) -> IpcError {
    let mut msg = format!(
        "Setup failed: ssh to {host} failed (exit {SSH_FAILED}) before the setup finished. \
         The worktree is at {wt}; starting a session on it skips setup."
    );
    if !out.is_empty() {
        msg.push_str("\n\n");
        msg.push_str(out);
    }
    IpcError::new(codes::E_SETUP_FAILED, msg)
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

    #[test]
    fn the_copy_loop_skips_other_worktrees_and_dot_components() {
        let s = setup_script(
            "/b",
            "/b/.worktrees/w",
            &WorktreeSetup {
                copy: vec!["**/.env.local".into(), "-x'y".into()],
                command: None,
            },
        );
        assert!(
            s.contains("*/.git/*|/.worktrees/*|/.claude/worktrees/*|*/./*|*/../*|*//*) continue"),
            "{s}"
        );
        assert!(s.contains("cp -p -- \"./$f\" \"$wt/$f\""), "{s}");
        assert!(s.contains("dirname -- \"$f\""), "{s}");
        assert!(s.contains("cp -R -- \"./$f/.\" \"$wt/$f/\""), "{s}");
        assert!(
            s.contains("[ \"$c\" = .claude/worktrees ] || cp -R -- \"./$c\" \"$wt/.claude/\""),
            "a matched .claude is copied without its worktrees: {s}"
        );
        assert!(
            s.contains("for pat in '**/.env.local' '-x'\\''y'; do"),
            "{s}"
        );
        assert!(
            !s.contains("cp -p \"") && !s.contains("cp -R \"") && !s.contains("dirname \""),
            "no cp or dirname without --: {s}"
        );
    }

    #[test]
    fn the_command_runs_outside_the_copy_loops_shell_options() {
        let s = setup_script(
            "/b",
            "/w",
            &WorktreeSetup {
                copy: vec![".env".into()],
                command: Some("pip install -e .[dev]".into()),
            },
        );
        let open = s
            .find("(\nshopt -s nullglob dotglob\n")
            .expect("subshell opens");
        let close = s.find("\n) || exit $?\n").expect("subshell closes");
        let cmd = s.find("pip install -e .[dev]").unwrap();
        assert!(open < close && close < cmd, "{s}");
        assert_eq!(s.matches("shopt").count(), 2, "{s}");
        assert!(s[..open].find("IFS").is_none() && s[close..].find("IFS").is_none());
        // No globs: no loop, and so no options at all.
        let bare = setup_script(
            "/b",
            "/w",
            &WorktreeSetup {
                copy: vec![],
                command: Some("make".into()),
            },
        );
        assert!(!bare.contains("shopt") && !bare.contains("IFS"), "{bare}");
    }

    // Worktrees live inside the base checkout: `**/.env.local` matched the
    // other worktrees' copies, and a matched `.claude` held all of them.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_copy_never_reads_or_writes_another_worktree() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().join("base");
        let wt = base.join(".claude/worktrees/new");
        let other = base.join(".claude/worktrees/other");
        let flat = base.join(".worktrees/flat");
        for p in [&wt, &other, &flat, &base.join("config"), &base.join("pkg")] {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(base.join(".env.local"), "root").unwrap();
        std::fs::write(base.join("pkg/.env.local"), "pkg").unwrap();
        std::fs::write(other.join(".env.local"), "other").unwrap();
        std::fs::write(flat.join(".env.local"), "flat").unwrap();
        std::fs::write(base.join(".claude/settings.local.json"), "{}").unwrap();
        std::fs::write(base.join("config/.secret"), "s").unwrap();
        std::fs::write(base.join("-dash"), "d").unwrap();
        let s = setup(
            &[
                "**/.env.local",
                "*/.env.local",
                ".env.local",
                "./pkg/.env.local",
                ".claude",
                "config/.*",
                "-dash",
                "nope[x]",
            ],
            Some("echo .[dev] nope* > args"),
        );
        run_local(&base, &wt, &s).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(wt.join(".env.local")).unwrap(),
            "root"
        );
        assert_eq!(
            std::fs::read_to_string(wt.join("pkg/.env.local")).unwrap(),
            "pkg"
        );
        assert!(wt.join(".claude/settings.local.json").exists());
        assert!(wt.join("config/.secret").exists());
        assert!(wt.join("-dash").exists());
        assert!(!wt.join(".worktrees").exists(), "no sibling worktree");
        assert!(
            !wt.join(".claude/worktrees").exists(),
            "no sibling worktree"
        );
        assert!(!wt.join("config/pkg").exists(), "config/.. is not copied");
        assert!(!other.join("pkg").exists() && !flat.join("pkg").exists());
        assert_eq!(
            std::fs::read_to_string(wt.join("args")).unwrap().trim(),
            ".[dev] nope*",
            "the command's own words are not globbed away"
        );
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

    // The wrapper `run` puts around the script: only the output's tail comes
    // back, and the exit code is the script's.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_host_sends_back_only_the_tail_and_the_scripts_exit_code() {
        let script =
            bounded_script("for i in $(seq 1 5000); do echo line $i; done; echo oops >&2; exit 7");
        let out = crate::proc::command("bash")
            .args(["-c", &script])
            .output()
            .await
            .unwrap();
        assert_eq!(out.status.code(), Some(7));
        assert!(out.stdout.len() <= TAIL_BYTES, "{}", out.stdout.len());
        let kept = tail(&out.stdout, &out.stderr);
        assert!(kept.ends_with("oops"), "{kept}");
        assert_eq!(kept.lines().count(), TAIL_LINES);
    }

    // Review finding 5: the env file is written after the setup, which can
    // outlast the tick's sweep of env files, and checked before the worktree.
    #[test]
    fn a_new_session_writes_its_env_file_after_the_setup() {
        let src = crate::repo_files::read("crates/fleet-core/src/service/sessions/lifecycle.rs");
        let body = &src[src.find("async fn new_session_inner(").unwrap()..];
        let body = &body[..body.find("\n}\n").unwrap()];
        let at = |needle: &str| {
            body.find(needle)
                .unwrap_or_else(|| panic!("{needle} not in new_session_inner"))
        };
        let check = at("env::check(");
        let worktree = at("create_worktree_local");
        let setup = at("worktree_setup::run(");
        let prepare = at("env::prepare(");
        assert!(check < worktree, "checked before any workspace step");
        assert!(setup < prepare, "the file is written after the setup");
        assert!(
            prepare < at("ensure_for_new_session("),
            "repair launches the wrapped command"
        );
    }

    #[test]
    fn a_kept_line_is_cut_and_a_timeout_or_ssh_failure_says_so() {
        let long = "é".repeat(LINE_CHARS + 50);
        let kept = tail(long.as_bytes(), b"");
        assert_eq!(
            kept.chars().count(),
            LINE_CHARS + 1,
            "cut plus the ellipsis"
        );
        let t = failure(None, "/w", "");
        assert!(t.message.contains("within 4 minutes"), "{}", t.message);
        assert!(t.message.contains("may still be running"), "{}", t.message);
        let u = unreachable("box", "/w", "Connection refused");
        assert_eq!(u.code, codes::E_SETUP_FAILED);
        assert!(u.message.contains("ssh to box failed"), "{}", u.message);
        assert!(!u.message.contains("setup command exited"), "{}", u.message);
        assert!(u.message.ends_with("Connection refused"));
    }
}
