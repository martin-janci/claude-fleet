//! Provision a host's Claude with the fleet-control skill + MCP server entry.

use crate::ipc_error::IpcError;
use crate::service::tunnel::TunnelSupervisor;
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::Store;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PROVISION_TIMEOUT: Duration = Duration::from_secs(15);

const FLEET_SKILL: &str = include_str!("../../../skills/claude-fleet-control/SKILL.md");
const SKILL_DIR: &str = "~/.claude/skills/claude-fleet-control";
const SKILL_PATH: &str = "~/.claude/skills/claude-fleet-control/SKILL.md";
const FRIENDLY_NAME_SKILL: &str = include_str!("../../../skills/fleet-friendly-name/SKILL.md");
const FRIENDLY_NAME_SKILL_DIR: &str = "~/.claude/skills/fleet-friendly-name";
const FRIENDLY_NAME_SKILL_PATH: &str = "~/.claude/skills/fleet-friendly-name/SKILL.md";
const CLAUDE_JSON: &str = "~/.claude.json";
const CLAUDE_DIR: &str = "~/.claude";
const CLAUDE_MD_PATH: &str = "~/.claude/CLAUDE.md";
const TMUX_CONF: &str = "~/.tmux.conf";

/// Sentinel-delimited block claude-fleet maintains in each host's
/// `~/.claude/CLAUDE.md`. Idempotent: appended if absent, refreshed in place
/// if the body changed, left alone if up to date. Anything outside the
/// sentinels is the user's own content and untouched.
const CLAUDE_MD_BEGIN: &str = "<!-- BEGIN claude-fleet managed (do not edit between sentinels) -->";
const CLAUDE_MD_END: &str = "<!-- END claude-fleet managed -->";
const CLAUDE_MD_BODY: &str = "## claude-fleet session labels

If you are running inside a claude-fleet-managed tmux session and the
`mcp__claude-fleet__set_friendly_name` tool is available, label this session
at the start of every new task and whenever the task changes significantly.
Use the **fleet-friendly-name** skill — it is the single, token-cheap path
for this. Do not chat about the label; just set it.

The `host_alias` is configuration, not a fact derivable from `hostname`.
Never call `set_friendly_name` with a guessed alias (e.g. `hostname -s`) —
it silently misses whenever the user renames a host in the picker. Always
look it up: `list_sessions {}`, find the row whose `tmux_name` matches
`tmux display-message -p '#S'`, then call `set_friendly_name` with that
row's `host_alias`. The skill spells out the full flow.";

/// Install the skill + merge the MCP entry on one host. `url` is the MCP
/// endpoint that host should use. Reads `~/.claude.json`, merges (preserving
/// siblings), backs it up, writes it back. Parse errors abort BEFORE any write.
pub async fn provision_one(
    ssh: &Arc<SshClient>,
    host: &str,
    url: &str,
    token: &str,
) -> Result<(), IpcError> {
    // 1. Skills (live-discovered, no restart). Both ship from the repo so
    //    every fleet host gets the same shared copy.
    write_host_file(ssh, host, SKILL_DIR, SKILL_PATH, FLEET_SKILL).await?;
    write_host_file(
        ssh,
        host,
        FRIENDLY_NAME_SKILL_DIR,
        FRIENDLY_NAME_SKILL_PATH,
        FRIENDLY_NAME_SKILL,
    )
    .await?;
    // 1b. Global ~/.claude/CLAUDE.md — keep a managed block in sync so the
    //     fleet-friendly-name skill is invoked on every task start without
    //     the user editing CLAUDE.md by hand.
    provision_claude_md(ssh, host).await?;
    // 2. MCP entry: read → merge (preserve siblings) → back up → write.
    let existing = read_host_file(ssh, host, CLAUDE_JSON).await?;
    let merged = merge_mcp_entry(&existing, url, token)?; // errors before any write
    if !existing.trim().is_empty() {
        write_host_file(
            ssh,
            host,
            CLAUDE_DIR,
            &format!("{CLAUDE_JSON}.fleet-bak"),
            &existing,
        )
        .await?;
    }
    write_host_file(ssh, host, CLAUDE_DIR, CLAUDE_JSON, &merged).await?;
    // 3. Ensure tmux clipboard passthrough for OSC 52.
    provision_tmux_clipboard(ssh, host).await?;
    Ok(())
}

/// Ensure `~/.tmux.conf` has `set -g set-clipboard on` for OSC 52 passthrough.
/// Appends the setting if not already present; creates the file if missing.
pub async fn provision_tmux_clipboard(ssh: &Arc<SshClient>, host: &str) -> Result<(), IpcError> {
    let existing = read_host_file(ssh, host, TMUX_CONF).await?;
    if has_tmux_clipboard_setting(&existing) {
        return Ok(());
    }
    let home_dir = if host == "local" {
        std::env::var("HOME").unwrap_or_default()
    } else {
        "~".to_string()
    };
    let dir = &home_dir;
    let addition = "\n# Enable OSC 52 clipboard (added by claude-fleet)\nset -g set-clipboard on\n";
    let merged = format!("{}{}", existing.trim_end(), addition);
    write_host_file(ssh, host, dir, TMUX_CONF, &merged).await
}

/// Ensure `~/.claude/CLAUDE.md` on `host` contains the claude-fleet managed
/// block (sentinel-delimited). Idempotent: writes only when the block is
/// missing or its body drifted. Everything outside the sentinels is the
/// user's own content and is preserved verbatim.
pub async fn provision_claude_md(ssh: &Arc<SshClient>, host: &str) -> Result<(), IpcError> {
    let existing = read_host_file(ssh, host, CLAUDE_MD_PATH).await?;
    let Some(merged) = merge_claude_md(&existing, CLAUDE_MD_BEGIN, CLAUDE_MD_END, CLAUDE_MD_BODY)
    else {
        return Ok(()); // already up to date
    };
    write_host_file(ssh, host, CLAUDE_DIR, CLAUDE_MD_PATH, &merged).await
}

/// Pure: merge the sentinel-delimited block into `existing`. Returns `None`
/// when the block is already present and the body matches (no write needed);
/// `Some(new)` with the updated content otherwise. New content is appended at
/// EOF when the block is missing entirely.
fn merge_claude_md(existing: &str, begin: &str, end: &str, body: &str) -> Option<String> {
    let block = format!("{begin}\n{body}\n{end}");
    if let Some(b) = existing.find(begin) {
        if let Some(e_rel) = existing[b..].find(end) {
            let e = b + e_rel + end.len();
            let current = &existing[b..e];
            if current == block {
                return None;
            }
            let mut out = String::with_capacity(existing.len() + body.len());
            out.push_str(&existing[..b]);
            out.push_str(&block);
            out.push_str(&existing[e..]);
            return Some(out);
        }
        // Begin sentinel present but end missing — user damaged the block.
        // Treat as "needs refresh" and append a fresh one rather than guess
        // where the corrupted block ends.
    }
    let sep = if existing.is_empty() || existing.ends_with("\n\n") {
        ""
    } else if existing.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    Some(format!("{existing}{sep}{block}\n"))
}

/// Check if tmux.conf already has a set-clipboard directive.
fn has_tmux_clipboard_setting(content: &str) -> bool {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        // Match: set -g set-clipboard, set-option -g set-clipboard, etc.
        if trimmed.contains("set-clipboard") {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostProvisionResult {
    pub host: String,
    /// "provisioned" | "skipped" | "failed"
    pub status: String,
    pub detail: Option<String>,
}

/// Provision every non-hidden host. `local` gets a direct localhost URL + no
/// tunnel; remote hosts get the reverse tunnel + a localhost:<mcp_port> URL.
/// Per-host failures never abort the others.
pub async fn provision_hosts(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    tunnels: &Arc<TunnelSupervisor>,
    mcp_port: u16,
    token: &str,
) -> Result<Vec<HostProvisionResult>, IpcError> {
    let hosts = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.list_hosts()?
    };
    let mut results = Vec::new();
    for h in hosts {
        if h.hidden {
            continue;
        }
        if h.alias != "local" && !h.reachable {
            results.push(HostProvisionResult {
                host: h.alias,
                status: "skipped".into(),
                detail: Some("unreachable".into()),
            });
            continue;
        }
        let url = format!("http://127.0.0.1:{mcp_port}/mcp");
        match provision_one(ssh, &h.alias, &url, token).await {
            Ok(()) => {
                if h.alias != "local" {
                    tunnels.ensure(&h.alias, mcp_port, mcp_port);
                }
                if let Ok(s) = store.lock() {
                    let _ = s.set_host_provisioned(&h.alias, true);
                }
                results.push(HostProvisionResult {
                    host: h.alias,
                    status: "provisioned".into(),
                    detail: Some("restart Claude on this host to load the MCP server".into()),
                });
            }
            Err(e) => results.push(HostProvisionResult {
                host: h.alias,
                status: "failed".into(),
                detail: Some(e.message),
            }),
        }
    }
    Ok(results)
}

/// Re-establish tunnels for already-provisioned remote hosts (app start / MCP
/// re-enable). Does NOT re-write config.
pub fn reestablish_tunnels(
    store: &Mutex<Store>,
    tunnels: &Arc<TunnelSupervisor>,
    mcp_port: u16,
) -> Result<(), IpcError> {
    let hosts = {
        store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?
            .list_hosts()?
    };
    for h in hosts {
        if h.provisioned && h.alias != "local" && !h.hidden {
            tunnels.ensure(&h.alias, mcp_port, mcp_port);
        }
    }
    Ok(())
}

/// Read a file from a host. `local` → `std::fs`; remote → `cat` over SSH.
/// Missing file → `Ok(String::new())` (caller treats as empty config).
pub async fn read_host_file(
    ssh: &Arc<SshClient>,
    host: &str,
    path: &str,
) -> Result<String, IpcError> {
    if host == "local" {
        let expanded = expand_home_local(path)?;
        return Ok(std::fs::read_to_string(&expanded).unwrap_or_default());
    }
    // Outer `quote` makes the whole script cross the SSH boundary as ONE shell
    // word — ssh space-joins argv, so an unquoted multi-word script would be
    // re-split by the remote login shell (mirrors claude_cli.rs).
    let script = quote(&remote_read_script(path));
    let out = ssh
        .run(host, &["bash", "-lc", &script], PROVISION_TIMEOUT)
        .await?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Write a file to a host (creating parent dirs). `local` → fs; remote → a
/// shell that `mkdir -p`s the parent and `printf '%s'`s the (shell-quoted)
/// content to `path`. `dir` is the parent dir; `path` the file.
pub async fn write_host_file(
    ssh: &Arc<SshClient>,
    host: &str,
    dir: &str,
    path: &str,
    content: &str,
) -> Result<(), IpcError> {
    if host == "local" {
        let edir = expand_home_local(dir)?;
        std::fs::create_dir_all(&edir)
            .map_err(|e| IpcError::new("E_PROVISION", format!("mkdir {edir}: {e}")))?;
        let epath = expand_home_local(path)?;
        std::fs::write(&epath, content)
            .map_err(|e| IpcError::new("E_PROVISION", format!("write {epath}: {e}")))?;
        return Ok(());
    }
    let script = quote(&remote_write_script(dir, path, content));
    let out = ssh
        .run(host, &["bash", "-lc", &script], PROVISION_TIMEOUT)
        .await?;
    if !out.status.success() {
        return Err(IpcError::new(
            "E_PROVISION",
            format!(
                "write {path} on {host}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }
    Ok(())
}

/// Render a path as a token for a remote `bash -lc` script. A leading `~/` is
/// emitted as `"$HOME"/<rest>` so the home dir expands on the remote — `quote`
/// would single-quote `~` and defeat tilde expansion, creating a literal `~`
/// directory. `$HOME` is double-quoted (literal through the outer `quote`, then
/// expanded by the remote `bash`); the rest of the path is `quote`-quoted inert.
fn remote_path(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => format!("\"$HOME\"/{}", quote(rest)),
        None => quote(path),
    }
}

/// Remote `bash -lc` script body that reads `path` (missing file → empty stdout).
fn remote_read_script(path: &str) -> String {
    format!("cat {} 2>/dev/null || true", remote_path(path))
}

/// Remote `bash -lc` script body that creates `dir` then writes `content` to `path`.
fn remote_write_script(dir: &str, path: &str, content: &str) -> String {
    format!(
        "mkdir -p {} && printf '%s' {} > {}",
        remote_path(dir),
        quote(content),
        remote_path(path)
    )
}

/// Expand a leading `~/` against the LOCAL home dir.
fn expand_home_local(path: &str) -> Result<String, IpcError> {
    if let Some(rest) = path.strip_prefix("~/") {
        let home =
            std::env::var("HOME").map_err(|_| IpcError::new("E_PROVISION", "HOME not set"))?;
        Ok(format!("{home}/{rest}"))
    } else {
        Ok(path.to_string())
    }
}

/// Merge the claude-fleet HTTP MCP server entry into a host's `~/.claude.json`
/// content, preserving every existing key. Returns the new JSON (pretty).
/// Errors if `existing` is non-empty and not valid JSON.
pub fn merge_mcp_entry(existing: &str, url: &str, token: &str) -> Result<String, IpcError> {
    let mut root: serde_json::Value = if existing.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(existing).map_err(|e| {
            IpcError::new(
                "E_PROVISION",
                format!("~/.claude.json is not valid JSON: {e}"),
            )
        })?
    };
    if !root.is_object() {
        return Err(IpcError::new(
            "E_PROVISION",
            "~/.claude.json is not a JSON object",
        ));
    }
    let servers = root
        .as_object_mut()
        .unwrap()
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    if !servers.is_object() {
        return Err(IpcError::new(
            "E_PROVISION",
            "mcpServers is not a JSON object",
        ));
    }
    servers.as_object_mut().unwrap().insert(
        "claude-fleet".to_string(),
        serde_json::json!({
            "type": "http",
            "url": url,
            "headers": { "Authorization": format!("Bearer {token}") }
        }),
    );
    serde_json::to_string_pretty(&root)
        .map_err(|e| IpcError::new("E_PROVISION", format!("serialize: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_adds_entry_to_empty() {
        let out = merge_mcp_entry("", "http://127.0.0.1:4180/mcp", "tok").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["mcpServers"]["claude-fleet"]["type"], "http");
        assert_eq!(
            v["mcpServers"]["claude-fleet"]["url"],
            "http://127.0.0.1:4180/mcp"
        );
        assert_eq!(
            v["mcpServers"]["claude-fleet"]["headers"]["Authorization"],
            "Bearer tok"
        );
    }

    #[test]
    fn merge_preserves_siblings_and_is_idempotent() {
        let existing = r#"{"oauthAccount":{"email":"x@y.z"},"mcpServers":{"other":{"type":"http","url":"u"}}}"#;
        let out = merge_mcp_entry(existing, "http://127.0.0.1:4180/mcp", "tok").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["oauthAccount"]["email"], "x@y.z");
        assert_eq!(v["mcpServers"]["other"]["url"], "u");
        assert_eq!(
            v["mcpServers"]["claude-fleet"]["url"],
            "http://127.0.0.1:4180/mcp"
        );
        let out2 = merge_mcp_entry(&out, "http://127.0.0.1:4180/mcp", "tok2").unwrap();
        let v2: serde_json::Value = serde_json::from_str(&out2).unwrap();
        assert_eq!(
            v2["mcpServers"]["claude-fleet"]["headers"]["Authorization"],
            "Bearer tok2"
        );
        assert_eq!(v2["mcpServers"]["other"]["url"], "u");
    }

    #[test]
    fn merge_rejects_invalid_json() {
        assert!(merge_mcp_entry("not json", "u", "t").is_err());
    }

    #[test]
    fn remote_path_expands_home_not_quotes_tilde() {
        // `~/` must become an expandable `$HOME` token, NOT a single-quoted
        // literal `~` (which the remote would treat as a directory named `~`).
        let t = remote_path("~/.claude/skills/claude-fleet-control");
        assert_eq!(t, "\"$HOME\"/'.claude/skills/claude-fleet-control'");
        assert!(!t.starts_with("'~"));
        // Absolute paths are quoted whole.
        assert_eq!(remote_path("/etc/hosts"), "'/etc/hosts'");
    }

    #[test]
    fn remote_read_script_targets_home() {
        assert_eq!(
            remote_read_script("~/.claude.json"),
            "cat \"$HOME\"/'.claude.json' 2>/dev/null || true"
        );
    }

    #[test]
    fn remote_write_script_mkdirs_and_writes_under_home() {
        let s = remote_write_script("~/.claude", "~/.claude.json", "{\"a\":1}");
        assert_eq!(
            s,
            "mkdir -p \"$HOME\"/'.claude' && printf '%s' '{\"a\":1}' > \"$HOME\"/'.claude.json'"
        );
        // The whole script survives the SSH boundary as one shell word once
        // wrapped — outer quote leaves the inner `"$HOME"` intact for the
        // remote bash to expand.
        let quoted = crate::shell::quote(&s);
        assert!(quoted.starts_with('\'') && quoted.ends_with('\''));
        assert!(quoted.contains("\"$HOME\""));
    }

    #[test]
    fn expand_home_local_expands_tilde() {
        std::env::set_var("HOME", "/Users/test");
        assert_eq!(
            super::expand_home_local("~/.claude.json").unwrap(),
            "/Users/test/.claude.json"
        );
        assert_eq!(super::expand_home_local("/abs/path").unwrap(), "/abs/path");
    }

    #[test]
    fn has_tmux_clipboard_detects_setting() {
        assert!(!super::has_tmux_clipboard_setting(""));
        assert!(!super::has_tmux_clipboard_setting("set -g mouse on\n"));
        assert!(super::has_tmux_clipboard_setting(
            "set -g set-clipboard on\n"
        ));
        assert!(super::has_tmux_clipboard_setting(
            "set-option -g set-clipboard on\n"
        ));
        assert!(super::has_tmux_clipboard_setting(
            "  set -g set-clipboard external\n"
        ));
        // Comments don't count
        assert!(!super::has_tmux_clipboard_setting(
            "# set -g set-clipboard on\n"
        ));
    }

    const B: &str = "<!-- BEGIN -->";
    const E: &str = "<!-- END -->";

    #[test]
    fn merge_claude_md_appends_when_block_missing() {
        let out = super::merge_claude_md("# my notes\n", B, E, "body").unwrap();
        assert!(out.starts_with("# my notes\n"));
        assert!(out.contains(&format!("{B}\nbody\n{E}")));
    }

    #[test]
    fn merge_claude_md_appends_to_empty_file() {
        let out = super::merge_claude_md("", B, E, "body").unwrap();
        assert_eq!(out, format!("{B}\nbody\n{E}\n"));
    }

    #[test]
    fn merge_claude_md_is_idempotent_when_body_matches() {
        let block = format!("{B}\nbody\n{E}");
        let initial = format!("intro\n\n{block}\n");
        assert!(super::merge_claude_md(&initial, B, E, "body").is_none());
    }

    #[test]
    fn merge_claude_md_refreshes_when_body_drifted() {
        let initial = format!("intro\n\n{B}\nold body\n{E}\n");
        let out = super::merge_claude_md(&initial, B, E, "new body").unwrap();
        assert!(out.contains(&format!("{B}\nnew body\n{E}")));
        assert!(!out.contains("old body"));
        assert!(out.starts_with("intro\n"));
    }

    #[test]
    fn merge_claude_md_preserves_content_outside_sentinels() {
        let initial = format!("before\n{B}\nold\n{E}\nafter\n");
        let out = super::merge_claude_md(&initial, B, E, "new").unwrap();
        assert!(out.starts_with("before\n"));
        assert!(out.ends_with("after\n"));
    }

    #[test]
    fn claude_md_body_documents_programmatic_verification() {
        // Regression: the managed CLAUDE.md block must steer the in-session
        // agent toward `list_sessions` lookup on `E_NOTFOUND` rather than a
        // hardcoded hostname-guessing fallback chain. The previous wording
        // listed `hostname -s` / `hostname` / `local` as fallbacks, which
        // silently broke on any host whose alias was renamed in the picker
        // (alias is configuration, not a function of `hostname`).
        assert!(
            super::CLAUDE_MD_BODY.contains("list_sessions"),
            "managed CLAUDE.md block must point at `list_sessions` for \
             programmatic alias verification, not a hardcoded fallback chain"
        );
        assert!(
            super::CLAUDE_MD_BODY.contains("configuration"),
            "managed CLAUDE.md block must call out that `host_alias` is \
             configuration, so the agent doesn't try to derive it from \
             `hostname`"
        );
    }
}
