//! Async wrappers around the `claude` CLI for background sessions and log peeking.
//!
//! IMPORTANT: `claude` is invoked via `bash -lc` even locally so the user's
//! PATH (which includes ~/.local/bin where claude lives) is honoured.

use crate::ipc_error::IpcError;
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::validate;
use std::sync::Arc;
use std::time::Duration;

/// Timeout for `claude logs` and `claude project purge` (fast local operations).
const CLAUDE_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for `claude --bg` (involves Anthropic API handshake + session setup).
const CLAUDE_BG_TIMEOUT: Duration = Duration::from_secs(300);

/// Extract the Claude session ID from `claude --bg` stdout.
///
/// Tries, in order:
///   1. A line beginning with `session id:` / `session:` (case-insensitive) —
///      take the first whitespace-delimited token after the prefix.
///   2. As a last resort, scan the WHOLE output for a bare UUID
///      (`xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`). This rescues boxed/banner
///      output where the id is printed inside a drawn frame with no `session:`
///      prefix on its own line.
pub fn parse_session_id_from_bg_output(output: &str) -> Option<String> {
    for line in output.lines() {
        let lower = line.to_lowercase();
        let prefix_len = if lower.starts_with("session id:") {
            "session id:".len()
        } else if lower.starts_with("session:") {
            "session:".len()
        } else {
            continue;
        };
        if let Some(token) = line[prefix_len..].split_whitespace().next() {
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    // Last resort: a bare UUID anywhere in the output.
    static UUID_RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(
            r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}",
        )
        .expect("static regex")
    });
    UUID_RE.find(output).map(|m| m.as_str().to_string())
}

// ─── script builders ─────────────────────────────────────────────────────────
//
// Every `claude` invocation is assembled here from validated parts. Positional
// arguments are preceded by `--` wherever the CLI accepts it (the top-level
// `[prompt]` and `project purge [path]`), so a value that starts with `-` can
// never be parsed as an option. `claude logs <id>` / `claude stop <id>` reject
// `--` with "unknown option" (verified against Claude Code 2.1.x), so those
// two rely on `validate::claude_session_id` — a strict lowercase-UUID shape —
// instead. Pure functions so the argv shape is unit-testable.

/// `claude --bg --name <name> -- <prompt>`.
pub fn bg_script(name: &str, prompt: &str) -> Result<String, IpcError> {
    validate::not_option_like("session name", name)?;
    validate::not_option_like("prompt", prompt)?;
    Ok(format!(
        "claude --bg --name {} -- {}",
        quote(name),
        quote(prompt)
    ))
}

/// `claude logs <session_id>` (no `--`: the subcommand rejects it).
pub fn logs_script(session_id: &str) -> Result<String, IpcError> {
    validate::claude_session_id(session_id)?;
    Ok(format!("claude logs {}", quote(session_id)))
}

/// `claude stop <session_id>` (no `--`: the subcommand rejects it).
pub fn stop_script(session_id: &str) -> Result<String, IpcError> {
    validate::claude_session_id(session_id)?;
    Ok(format!("claude stop {}", quote(session_id)))
}

/// `claude project purge --yes -- <project_path>`.
pub fn purge_script(project_path: &str) -> Result<String, IpcError> {
    validate::not_option_like("project_path", project_path)?;
    Ok(format!(
        "claude project purge --yes -- {}",
        quote(project_path)
    ))
}

/// Launch `claude --bg --name <name> -- <prompt>` on `host_alias`.
/// Returns the session ID extracted from CLI output, if present.
pub async fn claude_bg(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    name: &str,
    prompt: &str,
) -> Result<Option<String>, IpcError> {
    let script = bg_script(name, prompt)?;
    let output = run_claude_script(ssh, host_alias, &script, CLAUDE_BG_TIMEOUT).await?;
    Ok(parse_session_id_from_bg_output(&output))
}

/// `claude logs <id>` only knows about background *jobs*. For an interactive
/// session (which has a resumable `claude_session_id` but no background job),
/// it fails with "No job matching '<id>'…". Detect that so `claude_logs` can
/// degrade to a friendly message instead of surfacing it as an error.
fn is_no_running_job(stderr: &str) -> bool {
    stderr.contains("No job matching")
}

/// Message shown when peeking a session that isn't a background job.
const NO_BG_LOGS_MSG: &str =
    "No background logs — this is an interactive session. Resume it by opening the session.";

/// Run `claude logs <session_id>` on `host_alias`. Interactive sessions have a
/// resumable id but no background job, so a "No job matching" failure is
/// reported as an informational message rather than an error.
pub async fn claude_logs(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    session_id: &str,
) -> Result<String, IpcError> {
    let script = logs_script(session_id)?;
    match run_claude_script(ssh, host_alias, &script, CLAUDE_TIMEOUT).await {
        Ok(out) => Ok(out),
        Err(e) if is_no_running_job(&e.message) => Ok(NO_BG_LOGS_MSG.to_string()),
        Err(e) => Err(e),
    }
}

/// Run `claude stop <session_id>` on `host_alias` to stop a background
/// (`claude --bg`) session. Idempotent: a "no job matching" response — the
/// session already exited or was stopped elsewhere — is success, so callers
/// can use this to clear a stale fleet row without racing the agent's own
/// exit. Returns `true` when a live job was actually stopped, `false` when
/// there was nothing left to stop.
pub async fn claude_stop(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    session_id: &str,
) -> Result<bool, IpcError> {
    let script = stop_script(session_id)?;
    match run_claude_script(ssh, host_alias, &script, CLAUDE_TIMEOUT).await {
        Ok(_) => Ok(true),
        Err(e) if is_no_running_job(&e.message) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Run `claude project purge --yes -- <project_path>` on `host_alias`.
pub async fn claude_purge_project(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    project_path: &str,
) -> Result<(), IpcError> {
    let script = purge_script(project_path)?;
    run_claude_script(ssh, host_alias, &script, CLAUDE_TIMEOUT).await?;
    Ok(())
}

/// Run `script` via `bash -lc` either locally or over SSH depending on
/// `host_alias`. Returns stdout on success; maps non-zero exit to `E_CLAUDE_CLI`.
/// The alias is validated here (not only at `add_host`) because the MCP tools
/// and DevTools reach this path with caller-supplied values.
async fn run_claude_script(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    script: &str,
    timeout: Duration,
) -> Result<String, IpcError> {
    validate::host_alias(host_alias)?;
    if host_alias == "local" {
        let output = tokio::time::timeout(
            timeout,
            tokio::process::Command::new("bash")
                .args(["-lc", script])
                .output(),
        )
        .await
        .map_err(|_| {
            IpcError::new(
                "E_TIMEOUT",
                format!("claude CLI timed out after {:.0}s", timeout.as_secs_f64()),
            )
        })?
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
            .run(host_alias, &["bash", "-lc", &quoted_script], timeout)
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
    fn is_no_running_job_detects_interactive_session() {
        assert!(is_no_running_job(
            "No job matching '7e3f6059-3604-482c-a8a1-265d12fe5533'. Run 'claude agents' to list running sessions."
        ));
        assert!(!is_no_running_job("some other error"));
        assert!(!is_no_running_job(""));
    }

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

    #[test]
    fn parse_bg_output_extracts_bare_uuid_from_banner() {
        // Boxed/banner output with NO `session:` prefix — only a bare UUID
        // drawn inside a frame. The last-resort scan must still find it.
        let output = "\
╭──────────────────────────────────────────────╮
│  Background session started                    │
│  7e3f6059-3604-482c-a8a1-265d12fe5533          │
╰──────────────────────────────────────────────╯
";
        let id = parse_session_id_from_bg_output(output);
        assert_eq!(id, Some("7e3f6059-3604-482c-a8a1-265d12fe5533".to_string()));
    }

    #[test]
    fn parse_bg_output_prefix_wins_over_bare_uuid() {
        // When BOTH a prefixed line and a stray UUID are present, the explicit
        // `Session ID:` prefix takes precedence over the last-resort scan.
        let output = "noise 11111111-2222-3333-4444-555555555555\nSession ID: the-real-id\n";
        let id = parse_session_id_from_bg_output(output);
        assert_eq!(id, Some("the-real-id".to_string()));
    }

    #[test]
    fn parse_bg_output_session_id_matching_prefix_text() {
        // "session" appears in both prefix AND value — must return the VALUE
        let output = "Session ID: session-abc-123\n";
        let id = parse_session_id_from_bg_output(output);
        assert_eq!(id, Some("session-abc-123".to_string()));
    }

    // ─── script builders ─────────────────────────────────────────────────

    const UUID: &str = "550e8400-e29b-41d4-a716-446655440000";

    #[test]
    fn bg_script_places_end_of_options_before_prompt() {
        let s = bg_script("review-1", "Summarise the diff").unwrap();
        assert_eq!(s, "claude --bg --name 'review-1' -- 'Summarise the diff'");
        // `--` sits immediately before the (quoted) prompt and after --name's value.
        let idx = s.find(" -- ").expect("has -- separator");
        assert!(s[..idx].ends_with("'review-1'"));
        assert!(s[idx + 4..].starts_with("'Summarise"));
    }

    #[test]
    fn bg_script_rejects_option_like_name_and_prompt() {
        for bad in ["--foo", "-n", "--help"] {
            let err = bg_script(bad, "ok prompt").unwrap_err();
            assert_eq!(err.code, "E_INVALID", "name {bad:?}");
            let err = bg_script("ok-name", bad).unwrap_err();
            assert_eq!(err.code, "E_INVALID", "prompt {bad:?}");
        }
        assert!(bg_script("", "x").is_err());
        assert!(bg_script("x", "").is_err());
    }

    #[test]
    fn bg_script_quotes_shell_metacharacters() {
        let s = bg_script("n", "it's $(rm -rf /) `x`").unwrap();
        assert_eq!(s, "claude --bg --name 'n' -- 'it'\\''s $(rm -rf /) `x`'");
    }

    #[test]
    fn logs_and_stop_scripts_require_uuid_and_skip_double_dash() {
        assert_eq!(logs_script(UUID).unwrap(), format!("claude logs '{UUID}'"));
        assert_eq!(stop_script(UUID).unwrap(), format!("claude stop '{UUID}'"));
        // `claude logs -- <id>` is rejected by the CLI, so no `--` here…
        assert!(!logs_script(UUID).unwrap().contains(" -- "));
        // …and an option-shaped or non-UUID id is refused up front instead.
        for bad in [
            "--foo",
            "-h",
            "abc-123",
            "",
            "550E8400-E29B-41D4-A716-446655440000",
        ] {
            assert_eq!(logs_script(bad).unwrap_err().code, "E_INVALID", "{bad:?}");
            assert_eq!(stop_script(bad).unwrap_err().code, "E_INVALID", "{bad:?}");
        }
    }

    #[test]
    fn purge_script_places_end_of_options_before_path() {
        let s = purge_script("/home/me/projects/x").unwrap();
        assert_eq!(s, "claude project purge --yes -- '/home/me/projects/x'");
        for bad in ["--all", "-rf", ""] {
            assert_eq!(purge_script(bad).unwrap_err().code, "E_INVALID", "{bad:?}");
        }
    }

    #[tokio::test]
    async fn run_claude_script_rejects_option_like_host_alias() {
        let ssh = Arc::new(SshClient::new());
        let err = run_claude_script(&ssh, "-oProxyCommand=id", "true", CLAUDE_TIMEOUT)
            .await
            .unwrap_err();
        assert_eq!(err.code, "E_INVALID");
    }
}
