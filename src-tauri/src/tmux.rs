use crate::ipc_error::IpcError;
use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;

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
pub fn list_local_sessions() -> Result<Vec<TmuxSession>, IpcError> {
    let output = Command::new("tmux")
        .args([
            "list-sessions",
            "-F",
            "#{session_name}|#{session_created}|#{session_activity}|#{session_attached}|#{pane_current_path}",
        ])
        .output();
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

pub fn new_session(name: &str, working_dir: &std::path::Path) -> Result<(), IpcError> {
    let output = Command::new("tmux")
        .args([
            "new-session",
            "-d",
            "-s",
            name,
            "-c",
            &working_dir.to_string_lossy(),
            "cl --continue || cl || bash",
        ])
        .output()
        .map_err(|e| IpcError::new("E_TMUX", format!("spawn tmux failed: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(IpcError::new("E_TMUX", stderr.trim()))
    }
}

pub fn kill_session(name: &str) -> Result<(), IpcError> {
    let output = Command::new("tmux")
        .args(["kill-session", "-t", name])
        .output()
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
}
