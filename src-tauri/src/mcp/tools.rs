//! MCP tool surface for claude-fleet.
//!
//! `FleetTools` holds the shared backend state (`Store`, `SshClient`,
//! `CancellationRegistry`) and exposes claude-fleet operations as MCP tools.
//! Each tool calls into the transport-agnostic `service` layer — the same
//! code path the Tauri IPC commands use.
//!
//! M2 ships a single tool (`fleet_health`) to prove the transport end to end;
//! M3 wires the remaining operations.

use crate::cancel::CancellationRegistry;
use crate::ssh::SshClient;
use crate::store::Store;
use rmcp::{
    handler::server::router::tool::ToolRouter, model::*, tool, tool_handler, tool_router,
    ErrorData as McpError, ServerHandler,
};
use std::sync::{Arc, Mutex};

/// The MCP server handler. Cloned per session by the streamable-HTTP service;
/// every clone shares the same backend state via the `Arc`s.
#[derive(Clone)]
pub struct FleetTools {
    store: Arc<Mutex<Store>>,
    // `ssh` and `reg` are unused until M3 wires the host/session tools; held
    // now so the handler is constructed with its full state from the start.
    #[allow(dead_code)]
    ssh: Arc<SshClient>,
    #[allow(dead_code)]
    reg: Arc<CancellationRegistry>,
    // Consumed by the `#[tool_router]` / `#[tool_handler]` macro-generated
    // dispatch; the field itself reads as dead to the lint.
    #[allow(dead_code)]
    tool_router: ToolRouter<FleetTools>,
}

#[tool_router]
impl FleetTools {
    pub fn new(
        store: Arc<Mutex<Store>>,
        ssh: Arc<SshClient>,
        reg: Arc<CancellationRegistry>,
    ) -> Self {
        Self {
            store,
            ssh,
            reg,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Report claude-fleet backend health: application version, SQLite schema version, and database readiness. Returns JSON."
    )]
    async fn fleet_health(&self) -> Result<CallToolResult, McpError> {
        let health = crate::service::health::health_check(&self.store);
        let json = serde_json::to_string_pretty(&health)
            .map_err(|e| McpError::internal_error(format!("serialize health: {e}"), None))?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }
}

#[tool_handler]
impl ServerHandler for FleetTools {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions(
                "claude-fleet control API. Drives long-lived Claude Code sessions \
                 running in tmux across multiple hosts."
                    .to_string(),
            )
    }
}
