//! MCP tool: the agents' rules, jobs and pending actions
//! (`service::agents::admin`). Served only to the hub owner's own paired
//! device (`Access::PersonDevice`): a rule is a person's grant and an
//! approval a person's answer, so the master token an agent holds never
//! sees this tool. A write also needs a trusted full device.

use super::*;
use crate::service::agents::admin::{self, AgentsArgs};

#[tool_router(router = agents_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "The agents (PR reviewer, support, fleet agents from \
        the catalog): status {agent?, limit?} (rules, newest jobs, actions \
        waiting for a person), explain (per rule: polls or why not, next poll, \
        last poll error, jobs by state, the next and the last failed job), job \
        {id} (one job whole, its proposed actions included), grant {agent: \
        review | support | fleet, scope: gh:<owner>/<repo> | jira:<tracker \
        id>:<PROJECT> | <instance name>, host, level: watch | propose | act, \
        project_id?, config?, hours?, definition? (fleet), bindings? (fleet: \
        one per store role; each MCP binding is asked for its tools on the \
        host first)}, revoke {id}, pause_all, approve {id}, reject {id}, run \
        {id, request?} (a manual or scheduled fleet agent, now). The owner's \
        own device; writes need a trusted full device.")]
    pub(super) async fn agents(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<AgentsArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "agents",
            &format!(
                "action={} agent={:?} scope={:?} level={:?} id={:?}",
                args.action.escape_debug(),
                args.agent.as_deref().map(str::escape_debug),
                args.scope.as_deref().map(str::escape_debug),
                args.level.as_deref().map(str::escape_debug),
                args.id
            ),
        );
        if !args.is_read() {
            agents_writer(&caller)?;
        }
        let by = caller
            .client
            .as_ref()
            .map_or_else(|| "device".to_string(), |c| format!("device:{}", c.name));
        let now = crate::store::now_unix();
        if args.action == "grant" && args.agent.as_deref() == Some("fleet") {
            let done =
                crate::service::fleet_agents::grant::grant(&self.store, &self.ssh, &args, &by, now)
                    .await
                    .map_err(to_mcp_err)?;
            return ok_json_compact(&done);
        }
        ok_json_compact(&admin::call(&self.store, &args, &by, now).map_err(to_mcp_err)?)
    }
}

/// A rule lets an agent read and write outside fleet on its own, and an
/// approval sends a write: only a trusted full device does either.
fn agents_writer(caller: &Caller) -> Result<(), McpError> {
    if caller.is_trusted_client() && caller.mode == TokenMode::Full {
        return Ok(());
    }
    Err(mcp_err(
        "E_FORBIDDEN",
        format!(
            "this device may read the agents' rules; to change them or answer an \
             action, the hub's operator trusts it (a full device): fleet-hub client trust {}",
            caller.client.as_ref().map_or("<name>", |c| c.name.as_str())
        ),
        None,
    ))
}
