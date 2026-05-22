//! Provision a host's Claude with the fleet-control skill + MCP server entry.

use crate::ipc_error::IpcError;
use crate::service::tunnel::TunnelSupervisor;
use crate::shell::quote as shq;
use crate::ssh::SshClient;
use crate::store::Store;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PROVISION_TIMEOUT: Duration = Duration::from_secs(15);

const FLEET_SKILL: &str = include_str!("../../../skills/claude-fleet-control/SKILL.md");
const SKILL_DIR: &str = "~/.claude/skills/claude-fleet-control";
const SKILL_PATH: &str = "~/.claude/skills/claude-fleet-control/SKILL.md";
const CLAUDE_JSON: &str = "~/.claude.json";
const CLAUDE_DIR: &str = "~/.claude";

/// Install the skill + merge the MCP entry on one host. `url` is the MCP
/// endpoint that host should use. Reads `~/.claude.json`, merges (preserving
/// siblings), backs it up, writes it back. Parse errors abort BEFORE any write.
pub async fn provision_one(
    ssh: &Arc<SshClient>,
    host: &str,
    url: &str,
    token: &str,
) -> Result<(), IpcError> {
    // 1. Skill (live-discovered, no restart).
    write_host_file(ssh, host, SKILL_DIR, SKILL_PATH, FLEET_SKILL).await?;
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
    Ok(())
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
// wired into the provision_hosts command + mcp_configure in the next task; remove then.
#[allow(dead_code)]
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
// wired into the provision_hosts command + mcp_configure in the next task; remove then.
#[allow(dead_code)]
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
    let script = format!("cat {} 2>/dev/null || true", shq(path));
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
    let script = format!(
        "mkdir -p {} && printf '%s' {} > {}",
        shq(dir),
        shq(content),
        shq(path)
    );
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
    fn expand_home_local_expands_tilde() {
        std::env::set_var("HOME", "/Users/test");
        assert_eq!(
            super::expand_home_local("~/.claude.json").unwrap(),
            "/Users/test/.claude.json"
        );
        assert_eq!(super::expand_home_local("/abs/path").unwrap(), "/abs/path");
    }
}
