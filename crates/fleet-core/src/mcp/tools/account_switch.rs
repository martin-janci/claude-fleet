//! MCP tool: automatic account switching, the caller's own answer
//! (`service::account_switch`). A person's tool: a per-host token speaks
//! for no person and is not served it.

use super::*;
use crate::ipc_error::lock;
use crate::service::account_switch::{self, AutoSwitchArgs};

#[tool_router(router = account_switch_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Your own answer to automatic account switching: \
        when one of your sessions' Claude account runs out, resume it under \
        the login on its host with the most headroom. Pass enabled to turn \
        it on or off for your sessions; leave it out to read it. It only \
        acts while the fleet's accounts.auto_switch is on too. Returns \
        { enabled, fleet_enabled }.")]
    pub(super) async fn auto_switch_account(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<AutoSwitchArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "auto_switch_account",
            &format!("enabled={:?}", args.enabled),
        );
        let s = lock(&self.store).map_err(to_mcp_err)?;
        let person = super::fleet::owner_for(&caller, &s);
        let view = account_switch::my_auto_switch(&s, person, &args).map_err(to_mcp_err)?;
        ok_json_compact(&view)
    }
}
