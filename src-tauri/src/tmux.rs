use crate::ipc_error::IpcError;
use crate::shell::quote as shell_quote;
use async_trait::async_trait;
use serde::Serialize;
use std::path::PathBuf;

use crate::ssh::SshClient;
use std::sync::Arc;

/// Backend-agnostic tmux operations. Implementations differ only in how
/// the `tmux` binary is invoked: locally or wrapped in `ssh <host>`.
#[async_trait]
pub trait TmuxExec: Send + Sync {
    async fn list_sessions(&self) -> Result<Vec<TmuxSession>, IpcError>;
    async fn new_session(&self, name: &str, working_dir: &std::path::Path) -> Result<(), IpcError>;
    async fn kill_session(&self, name: &str) -> Result<(), IpcError>;
    async fn rename_session(&self, old: &str, new: &str) -> Result<(), IpcError>;
    async fn restart_session(&self, name: &str) -> Result<(), IpcError>;
    async fn capture_pane(&self, name: &str) -> Result<String, IpcError>;
}

pub struct LocalTmux;

#[async_trait]
impl TmuxExec for LocalTmux {
    async fn list_sessions(&self) -> Result<Vec<TmuxSession>, IpcError> {
        list_local_sessions().await
    }
    async fn new_session(&self, name: &str, cwd: &std::path::Path) -> Result<(), IpcError> {
        new_session(name, cwd).await
    }
    async fn kill_session(&self, name: &str) -> Result<(), IpcError> {
        kill_session(name).await
    }
    async fn rename_session(&self, old: &str, new: &str) -> Result<(), IpcError> {
        rename_session(old, new).await
    }
    async fn restart_session(&self, name: &str) -> Result<(), IpcError> {
        restart_session(name).await
    }
    async fn capture_pane(&self, name: &str) -> Result<String, IpcError> {
        let output = tokio::process::Command::new("tmux")
            .args(["capture-pane", "-t", name, "-p"])
            .output()
            .await
            .map_err(|e| IpcError::new("E_TMUX", format!("spawn tmux failed: {e}")))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            // Non-existent pane: return empty so the poller keeps waiting.
            Ok(String::new())
        }
    }
}

pub struct RemoteTmux {
    pub client: Arc<SshClient>,
    pub host: String,
}

impl RemoteTmux {
    /// We always wrap remote tmux invocations in `bash -lc '…'` so the
    /// remote user's login env (PATH, LANG, etc.) is sourced. sshd may have
    /// `AcceptEnv` disabled which would silently drop SendEnv vars; the
    /// login shell route is portable.
    ///
    /// CRITICAL: `ssh <host> bash -lc <script>` joins ALL trailing argv with
    /// spaces before sending to the remote sshd. The remote shell then re-
    /// tokenizes, so any spaces in `<script>` would break `bash -c` (it
    /// would get just the first token as the script and everything else as
    /// positional args). We therefore single-quote the WHOLE script via
    /// `shell_quote` so it crosses the ssh boundary as one shell word.
    /// `shell_quote` already escapes the embedded `'` characters used by
    /// per-arg quoting inside `script`.
    async fn remote_bash(&self, script: &str) -> Result<std::process::Output, IpcError> {
        let quoted = shell_quote(script);
        self.client
            .run(
                &self.host,
                &["bash", "-lc", &quoted],
                std::time::Duration::from_secs(10),
            )
            .await
    }
}

#[async_trait]
impl TmuxExec for RemoteTmux {
    async fn list_sessions(&self) -> Result<Vec<TmuxSession>, IpcError> {
        let script = "tmux list-sessions -F '#{session_name}|#{session_created}|#{session_activity}|#{session_attached}|#{pane_current_path}' 2>&1";
        let output = self.remote_bash(script).await?;
        let combined = String::from_utf8_lossy(&output.stdout).into_owned();
        if output.status.success() {
            return Ok(parse_sessions(&combined));
        }
        if is_no_server_running(&combined) {
            return Ok(Vec::new());
        }
        Err(IpcError::new("E_TMUX", combined.trim()))
    }

    async fn new_session(&self, name: &str, cwd: &std::path::Path) -> Result<(), IpcError> {
        // Build the `tmux new-session` command identically to LocalTmux but
        // shell-escape arguments since we're sending a single script string.
        let mut script = String::from("tmux new-session -d");
        script.push_str(&format!(" -s {}", shell_quote(name)));
        script.push_str(&format!(" -c {}", shell_quote(&cwd.to_string_lossy())));
        // Forward env explicitly — remote sshd typically doesn't pass LANG.
        script.push_str(" -e COLORTERM=truecolor -e TERM=xterm-256color");
        script.push_str(&format!(
            " -e LANG={}",
            shell_quote(&std::env::var("LANG").unwrap_or_else(|_| "en_US.UTF-8".into()))
        ));
        script.push(' ');
        script.push_str(&shell_quote(&pane_command()));
        let output = self.remote_bash(&script).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(IpcError::new(
                "E_TMUX",
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }

    async fn kill_session(&self, name: &str) -> Result<(), IpcError> {
        let script = format!("tmux kill-session -t {}", shell_quote(name));
        let output = self.remote_bash(&script).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(IpcError::new(
                "E_TMUX",
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }

    async fn rename_session(&self, old: &str, new: &str) -> Result<(), IpcError> {
        let trimmed = new.trim();
        if trimmed.is_empty() {
            return Err(IpcError::new(
                "E_TMUX",
                "new session name must not be empty",
            ));
        }
        if trimmed.contains(|c: char| c.is_whitespace() || c == '.' || c == ':') {
            return Err(IpcError::new(
                "E_TMUX",
                "tmux session name must not contain whitespace, `.`, or `:`",
            ));
        }
        if trimmed == old {
            return Ok(());
        }
        let script = format!(
            "tmux rename-session -t {} {}",
            shell_quote(old),
            shell_quote(trimmed)
        );
        let output = self.remote_bash(&script).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(IpcError::new(
                "E_TMUX",
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }

    async fn restart_session(&self, name: &str) -> Result<(), IpcError> {
        let script = format!(
            "tmux respawn-pane -k -t {}: {}",
            shell_quote(name),
            shell_quote(&pane_command())
        );
        let output = self.remote_bash(&script).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(IpcError::new(
                "E_TMUX",
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }

    async fn capture_pane(&self, name: &str) -> Result<String, IpcError> {
        let script = format!("tmux capture-pane -t {} -p", shell_quote(name));
        let output = self.remote_bash(&script).await?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            // Non-existent pane: return empty so the poller keeps waiting.
            Ok(String::new())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TmuxSession {
    pub name: String,
    pub created: i64,
    pub last_activity: i64,
    pub attached: bool,
    pub path: PathBuf,
}

/// Lists tmux sessions on the local host. Returns an empty Vec (not an error)
/// when the tmux server isn't running.
pub async fn list_local_sessions() -> Result<Vec<TmuxSession>, IpcError> {
    let output = tokio::process::Command::new("tmux")
        .args([
            "list-sessions",
            "-F",
            "#{session_name}|#{session_created}|#{session_activity}|#{session_attached}|#{pane_current_path}",
        ])
        .output()
        .await;
    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
            Ok(parse_sessions(&stdout))
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            if is_no_server_running(&stderr) {
                Ok(Vec::new())
            } else {
                Err(IpcError::new("E_TMUX", stderr.trim()))
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(IpcError::new("E_TMUX", "tmux binary not found on PATH"))
        }
        Err(e) => Err(IpcError::new("E_TMUX", format!("spawn tmux failed: {e}"))),
    }
}

/// True for any tmux stderr that means "no server is running on this socket"
/// — i.e. an empty `list_local_sessions()` return rather than an error.
///
/// Variants observed:
/// - "no server running on /tmp/tmux-501/default"  (server was started then exited)
/// - "error connecting to /private/tmp/tmux-501/default (No such file or directory)"
///   (no server has ever been started — the socket file doesn't exist)
fn is_no_server_running(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    s.contains("no server running")
        || (s.contains("error connecting to") && s.contains("no such file or directory"))
}

fn parse_sessions(input: &str) -> Vec<TmuxSession> {
    input
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() != 5 {
                return None;
            }
            let created = parts[1].parse::<i64>().ok()?;
            let last_activity = parts[2].parse::<i64>().ok()?;
            let attached_int = parts[3].parse::<i64>().ok()?;
            Some(TmuxSession {
                name: parts[0].to_string(),
                created,
                last_activity,
                attached: attached_int > 0,
                path: PathBuf::from(parts[4]),
            })
        })
        .collect()
}

/// Command tmux runs as the pane's initial process. We use `cl --continue || cl`
/// to resume the user's last claude conversation if any, otherwise start fresh.
/// CRUCIAL: after claude exits we `exec $SHELL -l` so the tmux pane stays
/// alive as a normal interactive shell. Without that, claude returning 0
/// would close the pane and the whole session would disappear — the user
/// would lose the ability to "restart" or even attach to it.
pub fn pane_command() -> String {
    "cl --continue || cl; exec ${SHELL:-/bin/zsh} -l".to_string()
}

pub async fn new_session(name: &str, working_dir: &std::path::Path) -> Result<(), IpcError> {
    // Push env into the session explicitly via `-e KEY=VAL`. This matters
    // because the tmux SERVER may already be running with stale env (e.g.
    // started before claude-fleet imported the user's locale from their
    // login shell). `-e` overrides the server env for processes started
    // in this session, so the spawned `cl`/`bash` reliably sees UTF-8.
    let mut cmd = tokio::process::Command::new("tmux");
    cmd.args([
        "new-session",
        "-d",
        "-s",
        name,
        "-c",
        &working_dir.to_string_lossy(),
    ]);
    for var in ["LANG", "LC_ALL", "LC_CTYPE", "PATH"] {
        if let Ok(val) = std::env::var(var) {
            if !val.is_empty() {
                cmd.args(["-e", &format!("{var}={val}")]);
            }
        }
    }
    cmd.args(["-e", "COLORTERM=truecolor", "-e", "TERM=xterm-256color"]);
    cmd.arg(pane_command());
    let output = cmd
        .output()
        .await
        .map_err(|e| IpcError::new("E_TMUX", format!("spawn tmux failed: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(IpcError::new("E_TMUX", stderr.trim()))
    }
}

/// Rename an existing tmux session. New name must follow tmux's naming rules
/// (no `.`, `:`, or whitespace; non-empty). Validation here keeps the caller
/// from getting a cryptic tmux error.
pub async fn rename_session(old: &str, new: &str) -> Result<(), IpcError> {
    let trimmed = new.trim();
    if trimmed.is_empty() {
        return Err(IpcError::new(
            "E_TMUX",
            "new session name must not be empty",
        ));
    }
    if trimmed.contains(|c: char| c.is_whitespace() || c == '.' || c == ':') {
        return Err(IpcError::new(
            "E_TMUX",
            "tmux session name must not contain whitespace, `.`, or `:`",
        ));
    }
    if trimmed == old {
        return Ok(()); // no-op
    }
    let output = tokio::process::Command::new("tmux")
        .args(["rename-session", "-t", old, trimmed])
        .output()
        .await
        .map_err(|e| IpcError::new("E_TMUX", format!("spawn tmux failed: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(IpcError::new("E_TMUX", stderr.trim()))
    }
}

/// Restart the pane's process by killing claude (or whatever's running) and
/// respawning with the same command the session was created with. Uses
/// `respawn-pane -k` so we don't need to know if claude is currently running
/// or already dropped to shell.
pub async fn restart_session(name: &str) -> Result<(), IpcError> {
    let output = tokio::process::Command::new("tmux")
        .args([
            "respawn-pane",
            "-k",
            "-t",
            &format!("{name}:"),
            &pane_command(),
        ])
        .output()
        .await
        .map_err(|e| IpcError::new("E_TMUX", format!("spawn tmux failed: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(IpcError::new("E_TMUX", stderr.trim()))
    }
}

pub async fn kill_session(name: &str) -> Result<(), IpcError> {
    let output = tokio::process::Command::new("tmux")
        .args(["kill-session", "-t", name])
        .output()
        .await
        .map_err(|e| IpcError::new("E_TMUX", format!("spawn tmux failed: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(IpcError::new("E_TMUX", stderr.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_two_sessions() {
        let input = "dev-foo|1716000000|1716100000|1|/repos/foo\ndev-bar|1716000100|1716200000|0|/repos/bar\n";
        let sessions = parse_sessions(input);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].name, "dev-foo");
        assert!(sessions[0].attached);
        assert_eq!(sessions[0].path, PathBuf::from("/repos/foo"));
        assert_eq!(sessions[1].name, "dev-bar");
        assert!(!sessions[1].attached);
    }

    #[test]
    fn parse_skips_malformed_lines() {
        let input = "good|1716000000|1716100000|1|/x\nbad-line-without-pipes\nempty||||\n";
        let sessions = parse_sessions(input);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "good");
    }

    #[test]
    fn parse_empty_input() {
        assert!(parse_sessions("").is_empty());
    }

    #[test]
    fn detects_no_server_running_classic() {
        assert!(is_no_server_running(
            "no server running on /tmp/tmux-501/default\n"
        ));
    }

    #[test]
    fn detects_socket_file_missing_macos() {
        // What `tmux list-sessions` actually prints on macOS when no server
        // was ever started — the user-reported bug.
        assert!(is_no_server_running(
            "error connecting to /private/tmp/tmux-501/default (No such file or directory)\n"
        ));
    }

    #[test]
    fn detects_socket_file_missing_case_insensitive() {
        assert!(is_no_server_running(
            "Error connecting to /tmp/tmux-501/default (No such file or directory)"
        ));
    }

    #[test]
    fn does_not_swallow_unrelated_errors() {
        assert!(!is_no_server_running("can't find session: dev-foo"));
        assert!(!is_no_server_running("ambiguous option"));
        assert!(!is_no_server_running(""));
    }

    #[test]
    fn pane_command_falls_back_to_shell_after_claude_exits() {
        let cmd = pane_command();
        // The semicolon (NOT `||`) after the second `cl` is the whole point:
        // it makes the shell always continue to the exec regardless of `cl`'s
        // exit status. Regression test that the next person who edits this
        // doesn't accidentally use `||` and resurrect the "session dies on
        // /exit" bug.
        assert!(cmd.contains("cl --continue || cl;"), "got: {cmd}");
        assert!(cmd.contains("exec ${SHELL:-/bin/zsh}"), "got: {cmd}");
    }

    #[tokio::test]
    async fn rename_rejects_whitespace_dots_colons_and_empty() {
        // Can't actually run tmux in unit tests; just exercise the validation
        // path. tmux command is never reached.
        assert!(rename_session("a", "").await.is_err());
        assert!(rename_session("a", "   ").await.is_err());
        assert!(rename_session("a", "has space").await.is_err());
        assert!(rename_session("a", "has.dot").await.is_err());
        assert!(rename_session("a", "has:colon").await.is_err());
    }

    #[test]
    fn shell_quote_wraps_basic_strings_in_single_quotes() {
        assert_eq!(shell_quote("foo"), "'foo'");
        assert_eq!(shell_quote("dev-foo"), "'dev-foo'");
    }

    #[test]
    fn shell_quote_escapes_embedded_single_quotes() {
        assert_eq!(shell_quote("don't"), "'don'\\''t'");
    }

    #[test]
    fn shell_quote_handles_paths_with_spaces() {
        assert_eq!(shell_quote("/tmp/with space"), "'/tmp/with space'");
    }
}
