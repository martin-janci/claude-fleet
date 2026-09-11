//! MCP tool surface for claude-fleet.
//!
//! `FleetTools` holds the shared backend state (`Store`, `SshClient`,
//! `CancellationRegistry`) and exposes claude-fleet operations as MCP tools.
//! Every tool calls into the transport-agnostic `service` layer — the exact
//! same code path the Tauri IPC commands use; neither path is privileged.
//!
//! Tool arguments are MCP-specific structs (deriving `JsonSchema` so the AI
//! sees a typed schema). They deliberately omit the `call_id` cancellation
//! field the frontend uses — MCP tool calls run to completion.

use super::auth::{Caller, TokenMode};
use super::guard::{self, ConfirmState};
use super::McpGuards;
use crate::cancel::CancellationRegistry;
use crate::ipc_error::{codes, IpcError};
use crate::service::pane_intel::{ClaudeStatus, StuckKind};
use crate::service::{
    health, hosts, projects, safe_kill, sessions, tasks, transcript, usage, worktrees,
};
use crate::ssh::SshClient;
use crate::store::Store;
use rmcp::{
    handler::server::{
        router::tool::ToolRouter,
        tool::{Extension, ToolCallContext},
        wrapper::Parameters,
    },
    model::*,
    schemars,
    service::RequestContext,
    tool, tool_router, ErrorData as McpError, RoleServer, ServerHandler,
};
use std::sync::{Arc, Mutex};

mod fleet;
mod lifecycle;
mod messaging;
mod orchestration;
mod params;
mod repo;
mod session_ops;
mod support;
#[cfg(test)]
mod tests;

// `rmcp::model::*` also exports a `CancelTaskParams`. Name ours explicitly:
// an explicit import outranks both globs, here and in every child module
// that reaches it through `use super::*`.
use params::CancelTaskParams;
use params::*;
use support::*;

/// The MCP server handler. Cloned per session by the streamable-HTTP service;
/// every clone shares the same backend state via the `Arc`s.
#[derive(Clone)]
pub struct FleetTools {
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    reg: Arc<CancellationRegistry>,
    tunnels: Arc<crate::service::tunnel::TunnelSupervisor>,
    /// Rate limiter, pending confirmations and the desktop notifier.
    guards: McpGuards,
    /// Per-caller cap on concurrent bounded waits (S4). Created once in
    /// `new`; every per-MCP-session clone shares it.
    long_polls: Arc<guard::LongPollLimiter>,
    tool_router: ToolRouter<FleetTools>,
}

/// Server-level instructions handed to every MCP client on `initialize`.
///
/// The status vocabularies are rendered from `ClaudeStatus::vocabulary_doc()`
/// / `StuckKind::vocabulary_doc()` so they cannot drift from the enums. Keep
/// the surrounding wording stable — every edit invalidates connected clients'
/// cached tool definitions.
fn server_instructions() -> String {
    format!(
        "claude-fleet control API. Drives long-lived Claude Code sessions \
         running in tmux across multiple hosts. Call list_sessions to see \
         fleet state, new_session to spawn one, and send_prompt to steer it. \
         Session rows carry two status fields: claude_status is one of {} \
         (null when unknown), and stuck_kind is one of {} (null when not stuck).",
        ClaudeStatus::vocabulary_doc(),
        StuckKind::vocabulary_doc(),
    )
}

// --- tools -----------------------------------------------------------------

impl FleetTools {
    pub fn new(
        store: Arc<Mutex<Store>>,
        ssh: Arc<SshClient>,
        reg: Arc<CancellationRegistry>,
        tunnels: Arc<crate::service::tunnel::TunnelSupervisor>,
        guards: McpGuards,
    ) -> Self {
        Self {
            store,
            ssh,
            reg,
            tunnels,
            guards,
            long_polls: guard::LongPollLimiter::new(guard::MAX_LONG_POLLS_PER_CALLER),
            tool_router: Self::tool_router(),
        }
    }

    /// Every tool, summed from the per-domain `#[tool_router]` blocks. Both
    /// `new()` and `tool_router_for_doc()` call this, so the tools served and
    /// the generated reference cannot drift apart.
    fn tool_router() -> ToolRouter<FleetTools> {
        Self::fleet_router()
            + Self::session_ops_router()
            + Self::lifecycle_router()
            + Self::messaging_router()
            + Self::orchestration_router()
            + Self::repo_router()
    }
}

/// Hand-written (not `#[tool_handler]`) so every call passes through one
/// gate: readonly-mode enforcement and the persisted audit row happen here,
/// before the router dispatches to the tool. The [`Caller`] is then made
/// available to tools as an `Extension<Caller>` extractor.
impl ServerHandler for FleetTools {
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        mut context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        // Fail closed: a request that somehow bypassed the auth middleware
        // has no caller and gets nothing.
        let caller = caller_from_context(&context)
            .ok_or_else(|| mcp_err("E_FORBIDDEN", "request carries no caller identity", None))?;
        let tool = request.name.to_string();
        // Audit first so refused calls are on the timeline too.
        persist_audit(&self.store, &tool, request.arguments.as_ref(), &caller);
        enforce_mode(&caller, &tool)?;
        enforce_admin(&caller, &tool)?;
        context.extensions.insert(caller);
        let tcc = ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: self.tool_router.list_all(),
            meta: None,
            next_cursor: None,
        })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router.get(name).cloned()
    }

    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions(server_instructions())
    }
}

/// Test-only: expose the summed `tool_router()` so `doc_gen` can call
/// `FleetTools::tool_router_for_doc()` without needing access to the private
/// associated function.
#[cfg(test)]
impl FleetTools {
    pub(crate) fn tool_router_for_doc(
    ) -> rmcp::handler::server::router::tool::ToolRouter<FleetTools> {
        Self::tool_router()
    }
}
