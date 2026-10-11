//! MCP tool: dashboards (persona design step 6, `service::dashboards`).
//! The dashboards a person pinned from a `dashboard` block, kept on the
//! hub so every device of theirs shows them. The agent (Control's
//! operator) reads them; only a person pins and unpins. A per-host token
//! is not served it (`NOT_FOR_HOST_TOKENS`).

use super::*;
use crate::ipc_error::lock;
use crate::service::dashboards;
use crate::service::operator::OPERATOR_CLIENT_NAME;

#[tool_router(router = dashboards_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "The dashboards the person pinned from a `dashboard` \
        block (docs/chat-blocks.md): list | get {id} | pin {dashboard, why?} | \
        unpin {id}. Read one before you propose a change to it, and answer with \
        a block of the same id; the person pins it, so pin and unpin are \
        refused to the agent (E_FORBIDDEN). E_INVALID.")]
    pub(super) async fn dashboards(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<dashboards::DashboardsArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "dashboards",
            &format!(
                "action={} id={:?}",
                p.action.escape_debug(),
                p.id.as_deref().unwrap_or("")
            ),
        );
        let scope = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            caller.view_scope(&s).map_err(to_mcp_err)?
        };
        let device = caller.client.as_ref().map(|c| c.name.as_str());
        let may_pin = device != Some(OPERATOR_CLIENT_NAME);
        ok_json_compact(
            &dashboards::run(&self.store, &scope, &p, device, may_pin).map_err(to_mcp_err)?,
        )
    }
}
