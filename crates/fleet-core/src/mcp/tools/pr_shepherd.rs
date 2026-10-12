//! MCP tool: the PR shepherd's rules (`service::pr_shepherd::admin`, step 2).
//! Served only to the hub owner's own paired device (`Access::PersonDevice`):
//! a standing rule is a person's grant, so the master token an agent holds
//! never sees this tool. A write also needs a trusted full device.

use super::*;
use crate::service::pr_shepherd::admin::{self, ShepherdArgs};

#[tool_router(router = pr_shepherd_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "The PR shepherd's standing rules per project: \
        status (rules, episodes, merges), grant {project_id, level: watch | \
        nudge | merge, hours?, recipes?}, revoke {project_id}, pause_all. \
        The owner's own device; writes need a trusted full device.")]
    pub(super) async fn pr_shepherd(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<ShepherdArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "pr_shepherd",
            &format!(
                "action={} project_id={:?} level={:?} hours={:?}",
                args.action.escape_debug(),
                args.project_id,
                args.level.as_deref().map(str::escape_debug),
                args.hours
            ),
        );
        if !args.is_read() {
            shepherd_writer(&caller)?;
        }
        let by = caller
            .client
            .as_ref()
            .map_or_else(|| "device".to_string(), |c| format!("device:{}", c.name));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        ok_json_compact(&admin::call(&self.store, &args, &by, now).map_err(to_mcp_err)?)
    }
}

/// A rule changes what fleet does to a project's PRs on its own, merging
/// included: only a trusted full device writes one.
fn shepherd_writer(caller: &Caller) -> Result<(), McpError> {
    if caller.is_trusted_client() && caller.mode == TokenMode::Full {
        return Ok(());
    }
    Err(mcp_err(
        "E_FORBIDDEN",
        format!(
            "this device may read the shepherd's rules; to change them, the hub's \
             operator trusts it (a full device): fleet-hub client trust {}",
            caller.client.as_ref().map_or("<name>", |c| c.name.as_str())
        ),
        None,
    ))
}
