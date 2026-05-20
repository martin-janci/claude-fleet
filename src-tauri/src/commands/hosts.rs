//! Tauri commands for SSH host management. Each command is a thin wrapper
//! around `store.rs` helpers plus `ssh_config.rs` (for discovery) and
//! `ssh::SshClient` (for probing).

use crate::ipc_error::IpcError;
use crate::ssh::SshClient;
use crate::ssh_config::{self, SshHost};
use crate::store::{HostRow, Store};
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::State;

#[tauri::command]
pub fn discover_hosts() -> Result<Vec<SshHost>, IpcError> {
    Ok(ssh_config::load_user_config())
}

#[tauri::command]
pub fn list_hosts(store: State<'_, Mutex<Store>>) -> Result<Vec<HostRow>, IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.list_hosts().map_err(IpcError::from)
}

#[derive(Deserialize)]
pub struct AddHostArgs {
    pub alias: String,
    pub ssh_alias: String,
}

#[tauri::command]
pub fn add_host(
    args: AddHostArgs,
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<HostRow, IpcError> {
    // Probe first; we don't want to persist a host we can't talk to.
    let (reachable, claude_ver, tmux_ver) = probe(&ssh, &args.ssh_alias)?;
    {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.insert_host(&args.alias, Some(&args.ssh_alias))?;
        s.update_host_probe(
            &args.alias,
            reachable,
            claude_ver.as_deref(),
            tmux_ver.as_deref(),
            now_unix(),
        )?;
    }
    list_one(&store, &args.alias)
}

#[derive(Deserialize)]
pub struct HostAliasArgs {
    pub alias: String,
}

#[tauri::command]
pub fn probe_host(
    args: HostAliasArgs,
    store: State<'_, Mutex<Store>>,
    ssh: State<'_, Arc<SshClient>>,
) -> Result<HostRow, IpcError> {
    let ssh_alias = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.list_hosts()?
            .into_iter()
            .find(|h| h.alias == args.alias)
            .and_then(|h| h.ssh_alias)
    };
    let target = ssh_alias.as_deref().unwrap_or(&args.alias);
    // The `local` host has no ssh_alias; probe is best-effort via local shell.
    // For remote hosts we use the lenient probe so a Re-probe of an
    // unreachable host updates `reachable=false` instead of returning an
    // error to the UI.
    let (reachable, claude_ver, tmux_ver) = if args.alias == "local" {
        probe_local()
    } else {
        probe_lenient(&ssh, target)
    };
    {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.update_host_probe(
            &args.alias,
            reachable,
            claude_ver.as_deref(),
            tmux_ver.as_deref(),
            now_unix(),
        )?;
    }
    list_one(&store, &args.alias)
}

#[tauri::command]
pub fn remove_host(
    args: HostAliasArgs,
    store: State<'_, Mutex<Store>>,
) -> Result<(), IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.delete_host(&args.alias).map_err(IpcError::from)
}

#[derive(Deserialize)]
pub struct HideHostArgs {
    pub alias: String,
    pub hidden: bool,
}

#[tauri::command]
pub fn hide_host(
    args: HideHostArgs,
    store: State<'_, Mutex<Store>>,
) -> Result<(), IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.set_host_hidden(&args.alias, args.hidden).map_err(IpcError::from)
}

// --- helpers ---

fn list_one(
    store: &State<'_, Mutex<Store>>,
    alias: &str,
) -> Result<HostRow, IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.list_hosts()?
        .into_iter()
        .find(|h| h.alias == alias)
        .ok_or_else(|| IpcError::new("E_NOTFOUND", format!("host {alias} not found")))
}

/// Strict probe — returns `Err(E_PROBE)` if the SSH round trip fails or the
/// remote shell exits non-zero. Used by `add_host` so we refuse to persist
/// a host we can't reach. `probe_host` uses [`probe_lenient`] instead, which
/// turns failures into a `(false, None, None)` outcome the caller can record.
fn probe(
    ssh: &Arc<SshClient>,
    host: &str,
) -> Result<(bool, Option<String>, Option<String>), IpcError> {
    // Single round trip: print both versions, semicolon-separated, so a
    // missing claude doesn't drop the tmux probe.
    let script = "tmux -V 2>/dev/null || true; echo ---; claude --version 2>/dev/null || true";
    let out = ssh
        .run(host, &["bash", "-lc", script], Duration::from_secs(5))
        .map_err(|e| IpcError::new("E_PROBE", format!("ssh {host}: {}", e.message)))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(IpcError::new(
            "E_PROBE",
            format!("ssh {host} exited {:?}: {}", out.status.code(), stderr.trim()),
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut parts = stdout.split("---");
    let tmux_line = parts.next().unwrap_or("").trim().to_string();
    let claude_line = parts.next().unwrap_or("").trim().to_string();
    Ok((
        true,
        parse_claude_version(&claude_line),
        parse_tmux_version(&tmux_line),
    ))
}

/// Lenient probe — never errors. Used by `probe_host` (the user-triggered
/// Re-probe in Settings) and by background reconcile, where a failed probe
/// should just bump `reachable=false` rather than break the caller.
fn probe_lenient(
    ssh: &Arc<SshClient>,
    host: &str,
) -> (bool, Option<String>, Option<String>) {
    match probe(ssh, host) {
        Ok(v) => v,
        Err(_) => (false, None, None),
    }
}

fn probe_local() -> (bool, Option<String>, Option<String>) {
    let tmux = std::process::Command::new("tmux")
        .arg("-V")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        });
    let claude = std::process::Command::new("claude")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        });
    (
        true,
        parse_claude_version(claude.as_deref().unwrap_or("")),
        parse_tmux_version(tmux.as_deref().unwrap_or("")),
    )
}

fn parse_tmux_version(line: &str) -> Option<String> {
    // `tmux 3.6a` → "3.6a"
    line.strip_prefix("tmux ")
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn parse_claude_version(line: &str) -> Option<String> {
    // `2.1.144 (Claude Code)` → "2.1.144"
    line.split_whitespace().next().map(|v| v.to_string())
        .filter(|v| !v.is_empty())
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tmux_version_extracts_version() {
        assert_eq!(parse_tmux_version("tmux 3.6a").as_deref(), Some("3.6a"));
        assert_eq!(parse_tmux_version("tmux 3.5"), Some("3.5".into()));
        assert_eq!(parse_tmux_version(""), None);
        assert_eq!(parse_tmux_version("not a version"), None);
    }

    #[test]
    fn parse_claude_version_extracts_first_token() {
        assert_eq!(parse_claude_version("2.1.144 (Claude Code)").as_deref(), Some("2.1.144"));
        assert_eq!(parse_claude_version("  2.1.12  "), Some("2.1.12".into()));
        assert_eq!(parse_claude_version(""), None);
    }
}
