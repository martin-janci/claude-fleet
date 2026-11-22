//! MCP tool: Add account (M15 step G2.9, `service::add_account`). Fleet
//! administration like `add_host`: the master and the hub owner's trusted
//! full device (`Access::Person`, then `owner_device_admin`). The API key,
//! the setup token and the sign-in code are never in the audit line or a log.

use super::*;
use crate::service::add_account::{self, AddAccountArgs};

#[tool_router(router = add_account_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Add a Claude login profile on a host: a \
        subscription login, a setup token or an API key.")]
    pub(super) async fn add_account(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<AddAccountArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit("add_account", &args.audit_detail());
        // `login_status` stays open to a fleet admin's other devices, which
        // follow a sign-in a trusted one started. It is not only a read: it
        // syncs the host's profiles and writes the profile's `.claude.json`
        // over SSH, and shows the login pane and its sign-in link. Control
        // reaches it through its owner binding (`Access::Person`) and gets
        // none of that.
        if args.action != "login_status" {
            super::fleet::owner_device_admin(&caller, "adding an account")?;
        } else if caller.is_operator() {
            return Err(mcp_err(
                codes::E_FORBIDDEN,
                "the agent does not follow a Claude sign-in; the person does, on the Accounts page",
                None,
            ));
        }
        let out = add_account::run_with_client(&args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json_compact(&out)
    }
}
