//! Tauri IPC wrappers for the MCP control-API Settings panel.
//!
//! `mcp_status` reports the current state; `mcp_configure` persists the
//! enable/port/token settings and starts or stops the server live (no app
//! restart). The server logic lives in the top-level `mcp` module.

use crate::cancel::CancellationRegistry;
use crate::ipc_error::IpcError;
use crate::mcp::{self, McpRuntime};
use crate::ssh::SshClient;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::State;

#[derive(Serialize)]
pub struct McpStatus {
    /// The persisted on/off setting — may be `true` even when `running` is
    /// `false` (e.g. the configured port was already in use).
    pub enabled: bool,
    /// Whether the server is actually listening right now.
    pub running: bool,
    pub port: u16,
    /// The bearer token clients must present. Generated on first read.
    pub token: String,
    /// Convenience: the full streamable-HTTP endpoint URL.
    pub url: String,
    /// The most recent start failure, if any.
    pub bind_error: Option<String>,
}

#[derive(Deserialize)]
pub struct McpConfigureArgs {
    /// Desired on/off state.
    pub enabled: bool,
    /// New localhost port; `None` keeps the current one.
    pub port: Option<u16>,
    /// When `true`, mint a fresh bearer token (invalidates existing clients).
    pub regenerate_token: bool,
}

fn lock_err() -> IpcError {
    IpcError::new("E_LOCK", "store mutex poisoned")
}

/// Read the persisted settings + live runtime into an `McpStatus`. Generates
/// and persists a token on first call so the UI always has one to display.
fn status(store: &Mutex<Store>, runtime: &Mutex<McpRuntime>) -> Result<McpStatus, IpcError> {
    let (enabled, port, token) = {
        let s = store.lock().map_err(|_| lock_err())?;
        let enabled = s.get_setting(mcp::SETTING_ENABLED)?.as_deref() == Some("true");
        let port = s
            .get_setting(mcp::SETTING_PORT)?
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(mcp::DEFAULT_PORT);
        let token = match s.get_setting(mcp::SETTING_TOKEN)? {
            Some(t) if !t.is_empty() => t,
            _ => {
                let fresh = mcp::generate_token();
                s.set_setting(mcp::SETTING_TOKEN, &fresh)?;
                fresh
            }
        };
        (enabled, port, token)
    };
    let rt = runtime.lock().map_err(|_| lock_err())?;
    Ok(McpStatus {
        enabled,
        running: rt.is_running(),
        port,
        token,
        url: format!("http://127.0.0.1:{port}/mcp"),
        bind_error: rt.last_error().map(str::to_string),
    })
}

#[tauri::command]
pub fn mcp_status(
    store: State<'_, Arc<Mutex<Store>>>,
    runtime: State<'_, Mutex<McpRuntime>>,
) -> Result<McpStatus, IpcError> {
    status(&store, &runtime)
}

#[tauri::command]
pub async fn mcp_configure(
    args: McpConfigureArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
    runtime: State<'_, Mutex<McpRuntime>>,
    tunnels: State<'_, Arc<crate::service::tunnel::TunnelSupervisor>>,
) -> Result<McpStatus, IpcError> {
    // 1. Persist the requested settings.
    {
        let s = store.lock().map_err(|_| lock_err())?;
        if let Some(p) = args.port {
            s.set_setting(mcp::SETTING_PORT, &p.to_string())?;
        }
        if args.regenerate_token {
            s.set_setting(mcp::SETTING_TOKEN, &mcp::generate_token())?;
        }
        s.set_setting(
            mcp::SETTING_ENABLED,
            if args.enabled { "true" } else { "false" },
        )?;
    }

    // 2. Stop whatever is running — a port/token change is applied by restart.
    {
        let mut rt = runtime.lock().map_err(|_| lock_err())?;
        rt.stop();
    }
    if !args.enabled {
        tunnels.stop_all();
    }

    // 3. If enabled, (re)start with the persisted port + token.
    if args.enabled {
        let (port, token) = {
            let s = store.lock().map_err(|_| lock_err())?;
            let port = s
                .get_setting(mcp::SETTING_PORT)?
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(mcp::DEFAULT_PORT);
            let token = match s.get_setting(mcp::SETTING_TOKEN)? {
                Some(t) if !t.is_empty() => t,
                _ => {
                    let fresh = mcp::generate_token();
                    s.set_setting(mcp::SETTING_TOKEN, &fresh)?;
                    fresh
                }
            };
            (port, token)
        };
        let result = mcp::start(
            Arc::clone(&store),
            Arc::clone(&ssh),
            Arc::clone(&reg),
            Arc::clone(&tunnels),
            port,
            token,
        )
        .await;
        let mut rt = runtime.lock().map_err(|_| lock_err())?;
        match result {
            Ok(shutdown) => {
                // Re-establish tunnels for already-provisioned hosts (best-effort).
                if let Err(e) =
                    crate::service::provision::reestablish_tunnels(&store, &tunnels, port)
                {
                    eprintln!("[mcp] reestablish_tunnels: {e}");
                }
                rt.set_running(shutdown);
            }
            Err(e) => rt.set_error(e),
        }
    }

    // 4. Return the resulting status.
    status(&store, &runtime)
}

#[tauri::command]
pub async fn provision_hosts(
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    tunnels: State<'_, Arc<crate::service::tunnel::TunnelSupervisor>>,
) -> Result<Vec<crate::service::provision::HostProvisionResult>, IpcError> {
    let (port, token) = {
        let s = store.lock().map_err(|_| lock_err())?;
        let port = s
            .get_setting(mcp::SETTING_PORT)?
            .and_then(|p| p.parse().ok())
            .unwrap_or(mcp::DEFAULT_PORT);
        let token = s.get_setting(mcp::SETTING_TOKEN)?.unwrap_or_default();
        (port, token)
    };
    if token.is_empty() {
        return Err(IpcError::new(
            "E_PROVISION",
            "enable the control API first (no token yet)",
        ));
    }
    crate::service::provision::provision_hosts(&store, &ssh, &tunnels, port, &token).await
}

// ---------------------------------------------------------------------------
// Hook installation
// ---------------------------------------------------------------------------

/// Build the hook entry array for one event type.
// Part of the hook-endpoint feature; wired up in a follow-up — used by tests today.
#[allow(dead_code)]
fn build_hook_block(url: &str, matcher: &str) -> serde_json::Value {
    serde_json::json!([{
        "matcher": matcher,
        "hooks": [{ "type": "http", "url": url }]
    }])
}

/// Build the full settings fragment (for tests only; real command merges into file).
// Part of the hook-endpoint feature; wired up in a follow-up — used by tests today.
#[allow(dead_code)]
pub fn build_hook_config(url: &str) -> String {
    let v = serde_json::json!({
        "hooks": {
            "Stop": build_hook_block(url, ""),
            "PostToolUse": build_hook_block(url, "WorktreeCreate")
        }
    });
    serde_json::to_string_pretty(&v).unwrap()
}

/// Install (or update) the fleet hook in the local `~/.claude/settings.json`.
///
/// Reads the current port+token from the store, constructs the hook URL,
/// then merges the hook entries into the file without disturbing other settings.
/// Any existing fleet hooks pointing at the same port are replaced to avoid
/// duplicates.
///
/// Only supports `host_alias == "local"` — remote machines can't reach
/// 127.0.0.1 on this machine without a tunnel.
#[tauri::command]
pub fn install_fleet_hook(
    host_alias: String,
    store: State<'_, Arc<Mutex<Store>>>,
    runtime: State<'_, Mutex<McpRuntime>>,
) -> Result<String, IpcError> {
    if host_alias != "local" {
        return Err(IpcError::new(
            "E_UNSUPPORTED",
            "install_fleet_hook only supports the local host",
        ));
    }

    let (port, token) = {
        let s = store.lock().map_err(|_| lock_err())?;
        let port = s
            .get_setting(mcp::SETTING_PORT)?
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(mcp::DEFAULT_PORT);
        let token = s.get_setting(mcp::SETTING_TOKEN)?.unwrap_or_default();
        (port, token)
    };

    if token.is_empty() {
        return Err(IpcError::new(
            "E_NO_TOKEN",
            "MCP token not configured — enable the MCP server first",
        ));
    }

    {
        let rt = runtime.lock().map_err(|_| lock_err())?;
        if !rt.is_running() {
            return Err(IpcError::new(
                "E_NOT_RUNNING",
                "MCP server is not running — enable it in Settings > MCP first",
            ));
        }
    }

    let hook_url = format!("http://127.0.0.1:{port}/hook?token={token}");

    let settings_path = dirs::home_dir()
        .ok_or_else(|| IpcError::new("E_HOME", "cannot determine home directory"))?
        .join(".claude")
        .join("settings.json");

    let mut settings: serde_json::Value = if settings_path.exists() {
        let content = std::fs::read_to_string(&settings_path)
            .map_err(|e| IpcError::new("E_IO", format!("read settings.json: {e}")))?;
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let hooks = settings
        .as_object_mut()
        .ok_or_else(|| IpcError::new("E_PARSE", "settings.json root is not an object"))?
        .entry("hooks")
        .or_insert(serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| IpcError::new("E_PARSE", "hooks is not an object"))?;

    let fleet_prefix = format!("http://127.0.0.1:{port}/hook");
    let strip_fleet = |arr: &serde_json::Value| -> serde_json::Value {
        let items = arr.as_array().cloned().unwrap_or_default();
        serde_json::Value::Array(
            items
                .into_iter()
                .filter(|block| {
                    let hooks_arr = block.get("hooks").and_then(|h| h.as_array());
                    hooks_arr.is_none_or(|hs| {
                        !hs.iter().any(|h| {
                            h.get("url")
                                .and_then(|u| u.as_str())
                                .is_some_and(|u| u.starts_with(&fleet_prefix))
                        })
                    })
                })
                .collect(),
        )
    };

    let mut stop_arr = strip_fleet(hooks.get("Stop").unwrap_or(&serde_json::json!([])));
    stop_arr.as_array_mut().unwrap().push(serde_json::json!({
        "matcher": "",
        "hooks": [{ "type": "http", "url": hook_url }]
    }));
    hooks.insert("Stop".into(), stop_arr);

    let mut ptu_arr = strip_fleet(hooks.get("PostToolUse").unwrap_or(&serde_json::json!([])));
    ptu_arr.as_array_mut().unwrap().push(serde_json::json!({
        "matcher": "WorktreeCreate",
        "hooks": [{ "type": "http", "url": hook_url }]
    }));
    hooks.insert("PostToolUse".into(), ptu_arr);

    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| IpcError::new("E_IO", format!("create .claude dir: {e}")))?;
    }
    let json = serde_json::to_string_pretty(&settings)
        .map_err(|e| IpcError::new("E_SERIALIZE", e.to_string()))?;
    std::fs::write(&settings_path, &json)
        .map_err(|e| IpcError::new("E_IO", format!("write settings.json: {e}")))?;

    Ok(format!(
        "Hook installed at {hook_url}\nSettings written to {}",
        settings_path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_hook_config_produces_valid_json() {
        let cfg = build_hook_config("http://127.0.0.1:4180/hook?token=abc");
        let v: serde_json::Value = serde_json::from_str(&cfg).unwrap();
        assert!(v["hooks"]["Stop"].is_array());
        assert!(v["hooks"]["PostToolUse"].is_array());
        let url = v["hooks"]["Stop"][0]["hooks"][0]["url"].as_str().unwrap();
        assert!(url.contains("4180"));
    }
}
