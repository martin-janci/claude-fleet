//! MCP tool: pull requests (`service::prs`, redesign step 6.4). Every PR a
//! session's branch has had, as reconcile recorded it, filtered to the
//! sessions the caller may see.

use super::*;
use crate::ipc_error::lock;
use crate::service::prs::{self, PrsArgs};

#[tool_router(router = prs_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Pull requests sessions opened, newest first, \
        with state, CI, merge time and opener.")]
    pub(super) async fn prs(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<PrsArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "prs",
            &format!(
                "action={} state={:?} project_id={:?}",
                args.action.escape_debug(),
                args.state,
                args.project_id
            ),
        );
        let scope = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            caller.view_scope(&s).map_err(to_mcp_err)?
        };
        ok_json_compact(&prs::list(self.reader(), &scope, &args).map_err(to_mcp_err)?)
    }
}
