//! Tauri IPC wrappers for the MCP control-API Settings panel.
//!
//! `mcp_status` reports the current state; `mcp_configure` persists the
//! enable/port/token settings and starts or stops the server live (no app
//! restart). The server logic lives in the top-level `mcp` module.
//!
//! Track B additions: per-host token management (`list_host_tokens`,
//! `set_host_token_mode`, `rotate_host_token`), the destructive-call
//! confirmation toggle (`confirm_destructive` on `mcp_configure`) and its
//! answer path (`mcp_confirm`, `mcp_pending_confirms`).

use fleet_core::cancel::CancellationRegistry;
use fleet_core::ipc_error::lock;
use fleet_core::ipc_error::{codes, IpcError};
use fleet_core::mcp::settings::{ensure_master_token, McpSettings};
use fleet_core::mcp::{self, McpGuards, McpRuntime};
use fleet_core::service::hooks_install;
use fleet_core::service::hub::HubBase;
use fleet_core::ssh::SshClient;
use fleet_core::store::Store;
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
    /// The master bearer token (desktop / local clients). Generated on first
    /// read. Provisioned hosts get their own tokens — see `list_host_tokens`.
    pub token: String,
    /// Convenience: the full streamable-HTTP endpoint URL.
    pub url: String,
    /// The most recent start failure, if any.
    pub bind_error: Option<String>,
    /// `mcp.confirm_destructive`: every tool in `mcp::guard::CONFIRM_TOOLS`
    /// (broadcast, kill, delete_worktree, set_clipboard, repair_session,
    /// cancel_task, move_session) needs a desktop confirmation. Default off.
    pub confirm_destructive: bool,
}

#[derive(Deserialize)]
pub struct McpConfigureArgs {
    /// Desired on/off state.
    pub enabled: bool,
    /// New localhost port; `None` keeps the current one.
    pub port: Option<u16>,
    /// When `true`, mint a fresh master token (invalidates existing clients).
    pub regenerate_token: bool,
    /// New value for `mcp.confirm_destructive`; `None` keeps the current one.
    #[serde(default)]
    pub confirm_destructive: Option<bool>,
}

/// Read the persisted settings + live runtime into an `McpStatus`. Generates
/// and persists a token on first call so the UI always has one to display.
fn status(store: &Mutex<Store>, runtime: &Mutex<McpRuntime>) -> Result<McpStatus, IpcError> {
    let (enabled, port, token, confirm_destructive) = {
        let s = lock(store)?;
        let cfg = McpSettings::read(&s)?;
        let token = ensure_master_token(&s)?;
        (cfg.enabled, cfg.port, token, cfg.confirm_destructive)
    };
    let rt = lock(runtime)?;
    Ok(McpStatus {
        enabled,
        running: rt.is_running(),
        port,
        token,
        url: format!("http://127.0.0.1:{port}/mcp"),
        bind_error: rt.last_error().map(str::to_string),
        confirm_destructive,
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
    tunnels: State<'_, Arc<fleet_core::service::tunnel::TunnelSupervisor>>,
    guards: State<'_, McpGuards>,
) -> Result<McpStatus, IpcError> {
    // 1. Persist the requested settings.
    {
        let s = lock(&store)?;
        if let Some(p) = args.port {
            s.set_setting(mcp::SETTING_PORT, &p.to_string())?;
        }
        if args.regenerate_token {
            s.set_setting(mcp::SETTING_TOKEN, &mcp::generate_token())?;
        }
        if let Some(c) = args.confirm_destructive {
            s.set_setting(
                mcp::guard::SETTING_CONFIRM_DESTRUCTIVE,
                if c { "true" } else { "false" },
            )?;
        }
        s.set_setting(
            mcp::SETTING_ENABLED,
            if args.enabled { "true" } else { "false" },
        )?;
    }

    // 2. Stop whatever is running — a port/token change is applied by restart.
    {
        let mut rt = lock(&runtime)?;
        rt.stop();
    }
    if !args.enabled {
        tunnels.stop_all();
    }

    // 3. If enabled, (re)start with the persisted port + token.
    if args.enabled {
        let (port, token) = {
            let s = lock(&store)?;
            (McpSettings::read(&s)?.port, ensure_master_token(&s)?)
        };
        let result = mcp::start(
            Arc::clone(&store),
            Arc::clone(&ssh),
            Arc::clone(&reg),
            Arc::clone(&tunnels),
            guards.inner().clone(),
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            port,
            token,
            Vec::new(),
        )
        .await;
        // Best-effort: the server is already up, so a bad `hub.public_url`
        // must not abort before its shutdown handle is recorded below.
        let base = match lock(&store).and_then(|s| HubBase::read(&s)) {
            Ok(base) => Some(base),
            Err(e) => {
                tracing::warn!(error = %e, "[mcp] cannot resolve the hub base URL");
                None
            }
        };
        let mut rt = lock(&runtime)?;
        match result {
            Ok(shutdown) => {
                // Re-establish tunnels for already-provisioned hosts (best-effort).
                if let Some(base) = &base {
                    if let Err(e) =
                        fleet_core::service::provision::reestablish_tunnels(&store, &tunnels, base)
                    {
                        tracing::warn!(error = %e, "[mcp] re-establishing host tunnels failed");
                    }
                }
                rt.set_running(shutdown);
            }
            Err(e) => rt.set_error(e),
        }
        let started = rt.is_running();
        drop(rt);
        // Q8 / R10: a local host with no fleet hook never reports turns, so
        // `turn_seq` never moves. Enabling the API installs it (best-effort,
        // same idempotent merge as the Settings button, user hooks kept).
        if let (true, Some(base)) = (started, &base) {
            hooks_install::auto_install_local_hook(&store, base);
        }
    }

    // 4. Return the resulting status.
    status(&store, &runtime)
}

#[tauri::command]
pub async fn provision_hosts(
    rotate: Option<bool>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    tunnels: State<'_, Arc<fleet_core::service::tunnel::TunnelSupervisor>>,
) -> Result<Vec<fleet_core::service::provision::HostProvisionResult>, IpcError> {
    let base = HubBase::read(&*lock(&store)?)?;
    fleet_core::service::provision::provision_hosts(
        &store,
        &*ssh,
        &tunnels,
        &base,
        rotate.unwrap_or(false),
    )
    .await
}

// ---------------------------------------------------------------------------
// Per-host tokens
// ---------------------------------------------------------------------------

/// A host's token row WITHOUT the token itself — the frontend only needs to
/// know that one exists, its mode, and when it was minted.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct HostTokenInfo {
    pub host_alias: String,
    /// `full` | `readonly`.
    pub mode: String,
    pub created_at: i64,
}

impl From<fleet_core::store::HostTokenRow> for HostTokenInfo {
    fn from(r: fleet_core::store::HostTokenRow) -> Self {
        Self {
            host_alias: r.host_alias,
            mode: r.mode,
            created_at: r.created_at,
        }
    }
}

#[tauri::command]
pub fn list_host_tokens(
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<HostTokenInfo>, IpcError> {
    let s = lock(&store)?;
    Ok(s.list_host_tokens()?
        .into_iter()
        .map(HostTokenInfo::from)
        .collect())
}

/// Pure: validate a mode string from the frontend.
pub fn parse_mode(mode: &str) -> Result<&'static str, IpcError> {
    match mode {
        "full" => Ok("full"),
        "readonly" => Ok("readonly"),
        other => Err(IpcError::new(
            codes::E_INVALID,
            format!("token mode must be 'full' or 'readonly', got {other:?}"),
        )),
    }
}

#[tauri::command]
pub fn set_host_token_mode(
    host_alias: String,
    mode: String,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<HostTokenInfo, IpcError> {
    fleet_core::validate::host_alias(&host_alias)?;
    let mode = parse_mode(&mode)?;
    let s = lock(&store)?;
    s.set_host_token_mode(&host_alias, mode)?;
    s.get_host_token(&host_alias)?
        .map(HostTokenInfo::from)
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, "host token vanished"))
}

/// Mint a fresh token for one host and re-provision it (the new token is
/// written to the host's `~/.claude.json` + hook block and only persisted
/// once that succeeded, so an unreachable host keeps its old token).
#[tauri::command]
pub async fn rotate_host_token(
    host_alias: String,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    tunnels: State<'_, Arc<fleet_core::service::tunnel::TunnelSupervisor>>,
) -> Result<HostTokenInfo, IpcError> {
    fleet_core::validate::host_alias(&host_alias)?;
    let base = HubBase::read(&*lock(&store)?)?;
    fleet_core::service::provision::provision_host_with_token(
        &store,
        &*ssh,
        &tunnels,
        &host_alias,
        &base,
        true,
    )
    .await?;
    let s = lock(&store)?;
    s.get_host_token(&host_alias)?
        .map(HostTokenInfo::from)
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, "host token vanished"))
}

// ---------------------------------------------------------------------------
// Destructive-call confirmation
// ---------------------------------------------------------------------------

/// Answer a `mcp:confirm-required` prompt. Returns `false` when the nonce is
/// unknown or expired (the agent will be handed a fresh one on retry).
#[tauri::command]
pub fn mcp_confirm(
    nonce: String,
    approved: bool,
    guards: State<'_, McpGuards>,
) -> Result<bool, IpcError> {
    Ok(guards.confirms.resolve(&nonce, approved))
}

#[derive(Serialize)]
pub struct PendingConfirm {
    pub nonce: String,
    pub tool: String,
}

/// Outstanding confirmation requests (oldest first) — lets the desktop
/// re-render its queue after a reload.
#[tauri::command]
pub fn mcp_pending_confirms(guards: State<'_, McpGuards>) -> Result<Vec<PendingConfirm>, IpcError> {
    Ok(guards
        .confirms
        .pending_tools()
        .into_iter()
        .map(|(nonce, tool)| PendingConfirm { nonce, tool })
        .collect())
}

// ---------------------------------------------------------------------------
// Hook installation (logic in `service::hooks_install`)
// ---------------------------------------------------------------------------

/// Install (or update) the fleet hook in the local `~/.claude/settings.json`.
///
/// Uses the `local` host's own per-host token (minted here if the host was
/// never provisioned), so hook traffic from local sessions is attributed to
/// `local` like any other host. The file is written 0600 because it now
/// carries the token.
///
/// Only supports `host_alias == "local"` — remote hosts get their hook block
/// from `provision_hosts`, which also sets up the tunnel the hook needs.
#[tauri::command]
pub fn install_fleet_hook(
    host_alias: String,
    store: State<'_, Arc<Mutex<Store>>>,
    runtime: State<'_, Mutex<McpRuntime>>,
) -> Result<String, IpcError> {
    if host_alias != "local" {
        return Err(IpcError::new(
            codes::E_UNSUPPORTED,
            "install_fleet_hook only supports the local host; use Provision hosts for remote ones",
        ));
    }

    // Token first: with no master token it refuses with `E_NO_TOKEN`.
    let token = hooks_install::local_hook_token(&store)?;
    let base = HubBase::read(&*lock(&store)?)?;

    {
        let rt = lock(&runtime)?;
        if !rt.is_running() {
            return Err(IpcError::new(
                codes::E_NOT_RUNNING,
                "MCP server is not running — enable it in Settings > MCP first",
            ));
        }
    }

    let settings_path = hooks_install::local_settings_path()?;
    hooks_install::install_hook_at(&settings_path, &base.hook_url(), &token)?;

    Ok(format!(
        "Hook installed at {} (http hook, bearer header)\nSettings written to {}",
        base.hook_url(),
        settings_path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_mode_accepts_only_known_modes() {
        assert_eq!(parse_mode("full").unwrap(), "full");
        assert_eq!(parse_mode("readonly").unwrap(), "readonly");
        assert_eq!(parse_mode("admin").unwrap_err().code, "E_INVALID");
        assert_eq!(parse_mode("").unwrap_err().code, "E_INVALID");
    }

    #[test]
    fn host_token_info_never_carries_the_token() {
        let info = HostTokenInfo::from(fleet_core::store::HostTokenRow {
            host_alias: "mefistos".into(),
            token: "s3cret".into(),
            created_at: 7,
            mode: "readonly".into(),
        });
        let json = serde_json::to_string(&info).unwrap();
        assert!(!json.contains("s3cret"), "{json}");
        assert_eq!(
            info,
            HostTokenInfo {
                host_alias: "mefistos".into(),
                mode: "readonly".into(),
                created_at: 7
            }
        );
    }
}
