//! Start the embedded MCP control API at launch when the user enabled it.

use crate::store::Store;
use crate::{cancel, mcp, ssh};
use std::sync::Mutex;

/// Read the MCP control-API settings and, if the user enabled it, start the
/// server, recording the outcome in the managed `McpRuntime`. Generates and
/// persists a bearer token on first enable. Off by default — a fresh install
/// never opens a listener.
pub(crate) fn maybe_start_mcp(
    app: &tauri::AppHandle,
    store: &std::sync::Arc<Mutex<Store>>,
    ssh: &std::sync::Arc<ssh::SshClient>,
    reg: &std::sync::Arc<cancel::CancellationRegistry>,
    tunnels: &std::sync::Arc<crate::service::tunnel::TunnelSupervisor>,
    guards: &mcp::McpGuards,
) {
    use tauri::Manager;
    let (enabled, port, token) = {
        let Ok(s) = store.lock() else {
            return;
        };
        let enabled = s
            .get_setting(mcp::SETTING_ENABLED)
            .ok()
            .flatten()
            .as_deref()
            == Some("true");
        let port = s
            .get_setting(mcp::SETTING_PORT)
            .ok()
            .flatten()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(mcp::DEFAULT_PORT);
        let token = s.get_setting(mcp::SETTING_TOKEN).ok().flatten();
        (enabled, port, token)
    };
    if !enabled {
        tracing::info!("control API disabled (mcp.enabled is not true)");
        return;
    }
    // Ensure a token exists before the listener binds — never a tokenless API.
    let token = match token {
        Some(t) if !t.is_empty() => t,
        _ => {
            let fresh = mcp::generate_token();
            if let Ok(s) = store.lock() {
                let _ = s.set_setting(mcp::SETTING_TOKEN, &fresh);
            }
            fresh
        }
    };
    let result = tauri::async_runtime::block_on(async {
        let r = mcp::start(
            std::sync::Arc::clone(store),
            std::sync::Arc::clone(ssh),
            std::sync::Arc::clone(reg),
            std::sync::Arc::clone(tunnels),
            guards.clone(),
            port,
            token,
        )
        .await;
        if r.is_ok() {
            if let Err(e) = crate::service::provision::reestablish_tunnels(store, tunnels, port) {
                tracing::warn!("control API: reestablish_tunnels failed: {e}");
            }
        }
        r
    });
    if let Some(runtime) = app.try_state::<Mutex<mcp::McpRuntime>>() {
        if let Ok(mut rt) = runtime.lock() {
            match result {
                Ok(shutdown) => {
                    tracing::info!("control API bound on http://127.0.0.1:{port}/mcp");
                    rt.set_running(shutdown);
                }
                Err(e) => {
                    tracing::warn!("control API failed to start: {e}");
                    rt.set_error(e);
                }
            }
        }
    }
}
