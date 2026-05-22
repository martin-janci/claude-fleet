//! Async wrappers around the `claude` CLI for background sessions and log peeking.
//!
//! IMPORTANT: `claude` is invoked via `bash -lc` even locally so the user's
//! PATH (which includes ~/.local/bin where claude lives) is honoured.

use crate::ipc_error::IpcError;
use crate::shell::quote;
use crate::ssh::SshClient;
use std::sync::Arc;
use std::time::Duration;

const CLAUDE_TIMEOUT: Duration = Duration::from_secs(30);

/// Extract the Claude session ID from `claude --bg` stdout.
/// The CLI prints a line like `Session ID: <id>` or `session: <id>`.
pub fn parse_session_id_from_bg_output(output: &str) -> Option<String> {
    for line in output.lines() {
        let lower = line.to_lowercase();
        if let Some(rest) = lower
            .strip_prefix("session id:")
            .or_else(|| lower.strip_prefix("session:"))
        {
            let token = rest.trim().split_whitespace().next()?;
            if !token.is_empty() {
                let start = line.to_lowercase().find(token)?;
                return Some(line[start..start + token.len()].to_string());
            }
        }
    }
    None
}

/// Launch `claude --bg --name <name> <prompt>` on `host_alias`.
/// Returns the session ID extracted from CLI output, if present.
pub async fn claude_bg(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    name: &str,
    prompt: &str,
) -> Result<Option<String>, IpcError> {
    let quoted_name = quote(name);
    let quoted_prompt = quote(prompt);
    let script = format!("claude --bg --name {quoted_name} {quoted_prompt}");
    let output = run_claude_script(ssh, host_alias, &script).await?;
    Ok(parse_session_id_from_bg_output(&output))
}

/// Run `claude logs <session_id>` on `host_alias`.
pub async fn claude_logs(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    session_id: &str,
) -> Result<String, IpcError> {
    let quoted_id = quote(session_id);
    let script = format!("claude logs {quoted_id}");
    run_claude_script(ssh, host_alias, &script).await
}

/// Run `claude project purge <project_path> --yes` on `host_alias`.
pub async fn claude_purge_project(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    project_path: &str,
) -> Result<(), IpcError> {
    let quoted_path = quote(project_path);
    let script = format!("claude project purge {quoted_path} --yes");
    run_claude_script(ssh, host_alias, &script).await?;
    Ok(())
}

/// Run `script` via `bash -lc` either locally or over SSH depending on
/// `host_alias`. Returns stdout on success; maps non-zero exit to `E_CLAUDE_CLI`.
async fn run_claude_script(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    script: &str,
) -> Result<String, IpcError> {
    if host_alias == "local" {
        let output = tokio::process::Command::new("bash")
            .args(["-lc", script])
            .output()
            .await
            .map_err(|e| IpcError::new("E_SPAWN", format!("spawn bash: {e}")))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(IpcError::new(
                "E_CLAUDE_CLI",
                format!(
                    "claude CLI failed (exit {}): {}",
                    output.status.code().unwrap_or(-1),
                    stderr.trim()
                ),
            ))
        }
    } else {
        // Remote: wrap the script in `bash -lc '<script>'` so the remote
        // login env (PATH, etc.) is sourced — mirrors the RemoteTmux pattern.
        // The outer `quote()` ensures the whole script crosses the SSH boundary
        // as a single shell word.
        let quoted_script = quote(script);
        let output = ssh
            .run(
                host_alias,
                &["bash", "-lc", &quoted_script],
                CLAUDE_TIMEOUT,
            )
            .await?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(IpcError::new(
                "E_CLAUDE_CLI",
                format!(
                    "claude CLI failed on {host_alias} (exit {}): {}",
                    output.status.code().unwrap_or(-1),
                    stderr.trim()
                ),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bg_output_extracts_session_id() {
        let output = "Starting background session...\nSession ID: abc-123-def\n";
        let id = parse_session_id_from_bg_output(output);
        assert_eq!(id, Some("abc-123-def".to_string()));
    }

    #[test]
    fn parse_bg_output_none_when_no_match() {
        let output = "error: claude not found\n";
        assert!(parse_session_id_from_bg_output(output).is_none());
    }

    #[test]
    fn parse_bg_output_session_colon_prefix() {
        let output = "session: xyz-789\n";
        let id = parse_session_id_from_bg_output(output);
        assert_eq!(id, Some("xyz-789".to_string()));
    }
}
