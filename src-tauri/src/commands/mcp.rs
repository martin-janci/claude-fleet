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
            port,
            token,
        )
        .await;
        let mut rt = runtime.lock().map_err(|_| lock_err())?;
        match result {
            Ok(shutdown) => rt.set_running(shutdown),
            Err(e) => rt.set_error(e),
        }
    }

    // 4. Return the resulting status.
    status(&store, &runtime)
}
