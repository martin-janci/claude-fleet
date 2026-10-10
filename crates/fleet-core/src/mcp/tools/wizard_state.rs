//! MCP tool: wizard_state (Orbit Fleet M15 step G7.2,
//! `service::wizard_state`). A wizard's step and answers, kept on the hub
//! so it resumes on the person's other device; a per-host token is not
//! served it (`NOT_FOR_HOST_TOKENS`).

use super::*;
use crate::ipc_error::lock;
use crate::service::wizard_state;

#[tool_router(router = wizard_state_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "A wizard left half-way, kept so it resumes on \
        another of your devices. kind: add_host | add_project | add_account \
        | new_session | link_peer | form; key: which one of the kind (absent \
        = your one wizard of it). list {kind?}: yours, newest first; get \
        {kind, key?}: the row or null; save {kind, key?, step, answers, \
        label?}: where you are (answers never hold a secret); clear {kind, \
        key?}: finished or discarded. Each row names the device that saved \
        it last. E_INVALID.")]
    pub(super) async fn wizard_state(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<wizard_state::WizardStateArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "wizard_state",
            &format!(
                "action={} kind={:?}",
                p.action.escape_debug(),
                p.kind.as_deref().unwrap_or("")
            ),
        );
        let scope = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            caller.view_scope(&s).map_err(to_mcp_err)?
        };
        let device = caller.client.as_ref().map(|c| c.name.as_str());
        ok_json_compact(&wizard_state::run(&self.store, &scope, &p, device).map_err(to_mcp_err)?)
    }
}
