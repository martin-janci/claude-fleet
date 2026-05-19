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
            if stderr.contains("no server running") {
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
}
