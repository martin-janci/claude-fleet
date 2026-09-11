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
///
/// The name is an option *value* so it must not look like an option; the
/// prompt sits after `--` and may legitimately start with `-` (a markdown
/// list, say), so it is only checked for being non-blank.
pub fn bg_script(name: &str, prompt: &str) -> Result<String, IpcError> {
    validate::not_option_like("session name", name)?;
    validate::not_blank("prompt", prompt)?;
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

/// Marker prefixing every machine-readable line the purge script prints, so a
/// login shell's own stdout chatter (motd, profile echoes) is ignored.
const PURGE_MARK: &str = "CFPURGE";

/// What `claude project purge` prints (exit 1) when it holds no state for a
/// path — Claude Code 2.1.x: "No Claude Code project state found for <p> under <dir>."
const PURGE_NOT_FOUND: &str = "No Claude Code project state found";

/// Purge Claude Code state for `project_path` under BOTH of its path forms.
///
/// Claude keys `~/.claude/projects/<encoded>` on the *physical* cwd it was
/// launched in, while the fleet project scan records the *logical* path; the
/// two differ whenever a component is a symlink (`~/projects` ->
/// `/mnt/sda4/projects`). The script resolves the physical form on the target
/// host (`cd -- <path> && pwd -P`), runs `claude project purge --yes -- "$p"`,
/// and also purges the logical form when it differs. If the directory is gone
/// only the logical form is purged and an `unresolved` line is printed.
///
/// A purge counts as "not found" (success) only when claude exits 1 AND a
/// line of its output *starts with* [`PURGE_NOT_FOUND`]` for `: the output
/// echoes the path, so a substring match could be spoofed by a directory
/// name. Any other failure aborts with claude's output on stderr, so the
/// caller keeps the fleet row. `builtin cd` / `builtin pwd` keep a login
/// profile's `cd` function (rvm) from polluting the resolved path.
///
/// Stdout protocol, one tab-separated line each, prefixed with [`PURGE_MARK`]:
/// `physical <p>` or `unresolved`, then `purged <form>` / `not_found <form>`
/// per form attempted. Parsed by [`parse_purge_output`].
pub fn purge_script(project_path: &str) -> Result<String, IpcError> {
    validate::not_option_like("project_path", project_path)?;
    // Control characters (newline, tab) would break the line protocol and
    // have no business in a project path.
    if project_path.chars().any(|c| c.is_control()) {
        return Err(IpcError::new(
            "E_INVALID",
            "project_path must not contain control characters",
        ));
    }
    let q = quote(project_path);
    Ok(format!(
        r#"l={q}
p=$(CDPATH= builtin cd -- {q} 2>/dev/null && builtin pwd -P)
cf_purge() {{
out=$(claude project purge --yes -- "$1" 2>&1); rc=$?
if [ "$rc" -eq 0 ]; then printf '{m}\tpurged\t%s\n' "$1"; return 0; fi
if [ "$rc" -eq 1 ] && printf '%s\n' "$out" | grep -q '^{nf} for '; then
printf '{m}\tnot_found\t%s\n' "$1"; return 0
fi
printf '%s\n' "$out" >&2; exit "$rc"
}}
if [ -z "$p" ]; then
printf '{m}\tunresolved\n'
cf_purge "$l"
else
printf '{m}\tphysical\t%s\n' "$p"
cf_purge "$p"
if [ "$p" != "$l" ]; then cf_purge "$l"; fi
fi
"#,
        m = PURGE_MARK,
        nf = PURGE_NOT_FOUND,
    ))
}

/// Outcome of a project purge on one host, returned to the UI for its toast.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PurgeReport {
    pub host_alias: String,
    /// The path fleet recorded (from the projects scan).
    pub logical_path: String,
    /// `pwd -P` of `logical_path` on the target host; `None` when the
    /// directory no longer exists there, so only the logical form was tried.
    pub physical_path: Option<String>,
    /// Path forms whose Claude state was deleted.
    pub purged: Vec<String>,
    /// Path forms Claude held no state for (treated as success).
    pub not_found: Vec<String>,
}

/// Parse the [`purge_script`] stdout protocol. Errors when no form was
/// reported at all — the script exited 0 without running a purge, so the
/// caller must not assume Claude's state is gone.
pub fn parse_purge_output(
    host_alias: &str,
    logical_path: &str,
    stdout: &str,
) -> Result<PurgeReport, IpcError> {
    let mut report = PurgeReport {
        host_alias: host_alias.to_string(),
        logical_path: logical_path.to_string(),
        physical_path: None,
        purged: Vec::new(),
        not_found: Vec::new(),
    };
    for line in stdout.lines() {
        let mut parts = line.splitn(3, '\t');
        if parts.next() != Some(PURGE_MARK) {
            continue;
        }
        match (parts.next(), parts.next()) {
            (Some("physical"), Some(p)) => report.physical_path = Some(p.to_string()),
            (Some("purged"), Some(p)) => report.purged.push(p.to_string()),
            (Some("not_found"), Some(p)) => report.not_found.push(p.to_string()),
            _ => {}
        }
    }
    if report.purged.is_empty() && report.not_found.is_empty() {
        return Err(IpcError::new(
            "E_CLAUDE_CLI",
            format!("claude project purge on {host_alias} reported no result"),
        ));
    }
    Ok(report)
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

/// Purge Claude Code state for `project_path` on `host_alias`, under both
/// its physical and logical forms (see [`purge_script`]).
pub async fn claude_purge_project(
    ssh: &Arc<SshClient>,
    host_alias: &str,
    project_path: &str,
) -> Result<PurgeReport, IpcError> {
    let script = purge_script(project_path)?;
    // Up to two purges back to back, so twice the single-call budget.
    let out = run_claude_script(ssh, host_alias, &script, CLAUDE_TIMEOUT * 2).await?;
    parse_purge_output(host_alias, project_path, &out)
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
    fn bg_script_rejects_option_like_name_but_not_dash_prompt() {
        // The name is an option value: a leading `-` is refused.
        for bad in ["--foo", "-n", "--help"] {
            let err = bg_script(bad, "ok prompt").unwrap_err();
            assert_eq!(err.code, "E_INVALID", "name {bad:?}");
        }
        // The prompt sits after `--`, so a leading `-` is legitimate (e.g. a
        // markdown list) and must land verbatim after the separator.
        for prompt in ["- fix login\n- add test", "--foo", "-"] {
            let s = bg_script("ok-name", prompt).unwrap();
            let expected_tail = format!(" -- {}", quote(prompt));
            assert!(
                s.ends_with(&expected_tail),
                "{s:?} should end with {expected_tail:?}"
            );
        }
        // Blank values are still refused on both sides.
        assert!(bg_script("", "x").is_err());
        assert!(bg_script("x", "").is_err());
        assert!(bg_script("x", "   ").is_err());
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
    fn purge_script_resolves_physical_path_and_keeps_end_of_options() {
        let s = purge_script("/home/me/projects/x").unwrap();
        assert!(s.starts_with("l='/home/me/projects/x'\n"), "{s}");
        assert!(
            s.contains(
                "p=$(CDPATH= builtin cd -- '/home/me/projects/x' 2>/dev/null && builtin pwd -P)"
            ),
            "{s}"
        );
        // Every purge goes through `--` with the path as a quoted expansion.
        assert!(s.contains(r#"claude project purge --yes -- "$1""#), "{s}");
        assert!(
            !s.contains("purge --yes '"),
            "path must never follow a bare option"
        );
        for bad in ["--all", "-rf", "", "  ", "/a\nb", "/a\tb"] {
            assert_eq!(purge_script(bad).unwrap_err().code, "E_INVALID", "{bad:?}");
        }
    }

    #[test]
    fn purge_script_quotes_hostile_paths() {
        let s = purge_script("/p/it's $(rm -rf ~) `x`").unwrap();
        let q = "'/p/it'\\''s $(rm -rf ~) `x`'";
        assert!(s.starts_with(&format!("l={q}\n")), "{s}");
        assert!(s.contains(&format!("cd -- {q} 2>/dev/null")), "{s}");
    }

    #[test]
    fn parse_purge_output_ignores_shell_chatter_and_requires_a_result() {
        let out = "Welcome to box!\nCFPURGE\tphysical\t/mnt/p/x\nnoise\n\
                   CFPURGE\tpurged\t/mnt/p/x\nCFPURGE\tnot_found\t/home/u/p/x\n";
        let r = parse_purge_output("box", "/home/u/p/x", out).unwrap();
        assert_eq!(r.host_alias, "box");
        assert_eq!(r.physical_path.as_deref(), Some("/mnt/p/x"));
        assert_eq!(r.purged, vec!["/mnt/p/x"]);
        assert_eq!(r.not_found, vec!["/home/u/p/x"]);

        let err = parse_purge_output("box", "/x", "motd only\n").unwrap_err();
        assert_eq!(err.code, "E_CLAUDE_CLI");
    }

    /// Runs the real purge script under `bash -c` with a stub `claude` on PATH
    /// that logs each purged path (one per line) and answers "no state" for
    /// paths containing `CF_STUB_NOTFOUND`, or fails hard for `CF_STUB_FAIL`.
    #[cfg(unix)]
    mod purge_exec {
        use super::super::*;
        use std::os::unix::fs::{symlink, PermissionsExt};
        use std::path::{Path, PathBuf};
        use std::process::Output;

        const STUB: &str = r#"#!/bin/sh
[ "$#" -eq 5 ] && [ "$1 $2 $3 $4" = "project purge --yes --" ] || { echo "bad argv: $*" >&2; exit 64; }
printf '%s\n' "$5" >> "$CF_STUB_LOG"
if [ -n "$CF_STUB_NOTFOUND" ]; then case "$5" in *"$CF_STUB_NOTFOUND"*)
  echo "No Claude Code project state found for $5 under /stub/.claude." >&2; exit 1 ;; esac; fi
if [ -n "$CF_STUB_FAIL" ]; then case "$5" in *"$CF_STUB_FAIL"*)
  printf '%s\n' "${CF_STUB_FAIL_MSG:-kaboom while purging $5}" >&2; exit "${CF_STUB_FAIL_RC:-3}" ;; esac; fi
echo "Purged $5"
"#;

        struct Sandbox {
            _dir: tempfile::TempDir,
            root: PathBuf,
        }

        impl Sandbox {
            fn new() -> Self {
                let dir = tempfile::tempdir().unwrap();
                // Canonicalise so the sandbox root itself has no symlinks.
                let root = std::fs::canonicalize(dir.path()).unwrap();
                let bin = root.join("bin");
                std::fs::create_dir(&bin).unwrap();
                let stub = bin.join("claude");
                std::fs::write(&stub, STUB).unwrap();
                std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
                std::fs::create_dir(root.join("work")).unwrap();
                Sandbox { _dir: dir, root }
            }

            fn run(&self, logical: &Path, notfound: &str, fail: &str) -> (Output, Vec<String>) {
                self.run_env(logical, notfound, fail, &[])
            }

            fn run_env(
                &self,
                logical: &Path,
                notfound: &str,
                fail: &str,
                extra: &[(&str, &str)],
            ) -> (Output, Vec<String>) {
                let log = self.root.join("purge.log");
                let _ = std::fs::remove_file(&log);
                let script = purge_script(logical.to_str().unwrap()).unwrap();
                let out = std::process::Command::new("bash")
                    .arg("-c")
                    .arg(&script)
                    .current_dir(self.root.join("work"))
                    .env(
                        "PATH",
                        format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
                    )
                    .env("CF_STUB_LOG", &log)
                    .env("CF_STUB_NOTFOUND", notfound)
                    .env("CF_STUB_FAIL", fail)
                    .envs(extra.iter().copied())
                    .output()
                    .unwrap();
                let calls = std::fs::read_to_string(&log)
                    .unwrap_or_default()
                    .lines()
                    .map(str::to_string)
                    .collect();
                (out, calls)
            }

            /// `<root>/real/<name>` plus a symlink `<root>/link -> real`;
            /// returns (logical via the link, physical).
            fn symlinked(&self, name: &str) -> (PathBuf, PathBuf) {
                let physical = self.root.join("real").join(name);
                std::fs::create_dir_all(&physical).unwrap();
                symlink(self.root.join("real"), self.root.join("link")).unwrap();
                (self.root.join("link").join(name), physical)
            }
        }

        fn report(logical: &Path, out: &Output) -> PurgeReport {
            assert!(
                out.status.success(),
                "script failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            parse_purge_output(
                "local",
                logical.to_str().unwrap(),
                &String::from_utf8_lossy(&out.stdout),
            )
            .unwrap()
        }

        fn s(p: &Path) -> String {
            p.to_str().unwrap().to_string()
        }

        #[test]
        fn same_form_path_is_purged_once() {
            let sb = Sandbox::new();
            let dir = sb.root.join("real").join("proj");
            std::fs::create_dir_all(&dir).unwrap();
            let (out, calls) = sb.run(&dir, "", "");
            let r = report(&dir, &out);
            assert_eq!(calls, vec![s(&dir)]);
            assert_eq!(r.physical_path, Some(s(&dir)));
            assert_eq!(r.purged, vec![s(&dir)]);
            assert!(r.not_found.is_empty());
        }

        #[test]
        fn differing_forms_purge_physical_then_logical() {
            let sb = Sandbox::new();
            let (logical, physical) = sb.symlinked("proj");
            let (out, calls) = sb.run(&logical, "", "");
            let r = report(&logical, &out);
            assert_eq!(calls, vec![s(&physical), s(&logical)]);
            assert_eq!(r.physical_path, Some(s(&physical)));
            assert_eq!(r.purged, vec![s(&physical), s(&logical)]);
        }

        #[test]
        fn no_state_for_a_form_is_success_not_failure() {
            let sb = Sandbox::new();
            let (logical, physical) = sb.symlinked("proj");
            // The logical form (through `link`) has no Claude state.
            let (out, _) = sb.run(&logical, "/link/", "");
            let r = report(&logical, &out);
            assert_eq!(r.purged, vec![s(&physical)]);
            assert_eq!(r.not_found, vec![s(&logical)]);
        }

        #[test]
        fn missing_directory_purges_logical_form_only() {
            let sb = Sandbox::new();
            let gone = sb.root.join("gone").join("proj");
            let (out, calls) = sb.run(&gone, "", "");
            let r = report(&gone, &out);
            assert_eq!(calls, vec![s(&gone)]);
            assert_eq!(r.physical_path, None);
            assert_eq!(r.purged, vec![s(&gone)]);
        }

        #[test]
        fn other_claude_failures_abort_with_its_output() {
            let sb = Sandbox::new();
            let (logical, physical) = sb.symlinked("proj");
            let (out, calls) = sb.run(&logical, "", "/real/");
            assert!(!out.status.success());
            assert!(String::from_utf8_lossy(&out.stderr).contains("kaboom"));
            // Stopped at the failing physical purge; logical never attempted.
            assert_eq!(calls, vec![s(&physical)]);
        }

        #[test]
        fn not_found_phrase_in_the_path_cannot_mask_a_real_failure() {
            let sb = Sandbox::new();
            // A directory named after the not-found message, and a claude that
            // fails with exit 1 while echoing the path mid-line.
            let (logical, physical) =
                sb.symlinked("x No Claude Code project state found for y no such project");
            let (out, calls) = sb.run_env(&logical, "", "/real/", &[("CF_STUB_FAIL_RC", "1")]);
            assert!(
                !out.status.success(),
                "a spoofed not-found must stay an error"
            );
            assert_eq!(out.status.code(), Some(1));
            assert_eq!(calls, vec![s(&physical)]);
            assert!(!String::from_utf8_lossy(&out.stdout).contains("not_found"));
        }

        #[test]
        fn not_found_text_with_an_unexpected_exit_code_is_an_error() {
            let sb = Sandbox::new();
            let dir = sb.root.join("real").join("proj");
            std::fs::create_dir_all(&dir).unwrap();
            let msg = format!(
                "No Claude Code project state found for {} under /x.",
                dir.display()
            );
            let (out, _) = sb.run_env(&dir, "", "/real/", &[("CF_STUB_FAIL_MSG", &msg)]);
            assert!(!out.status.success(), "exit 3 is not a not-found");
        }

        #[test]
        fn hostile_path_reaches_claude_verbatim_and_runs_nothing() {
            let sb = Sandbox::new();
            let name = "it's $(touch pwned) `touch pwned2` \"q\" ; & *";
            let (logical, physical) = sb.symlinked(name);
            let (out, calls) = sb.run(&logical, "", "");
            let r = report(&logical, &out);
            assert_eq!(calls, vec![s(&physical), s(&logical)]);
            assert_eq!(r.purged, vec![s(&physical), s(&logical)]);
            assert!(!sb.root.join("work").join("pwned").exists());
            assert!(!sb.root.join("work").join("pwned2").exists());
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
