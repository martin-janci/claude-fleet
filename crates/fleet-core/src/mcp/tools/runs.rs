//! MCP tool: runs (`service::runs`, Orbit Fleet redesign step 8.3). One
//! tool by `action`: every run on the fleet's behalf — dispatched tasks, a
//! mission's actions and brakes, Jev's decisions, fleet's own `claude -p`
//! runs, routine fires — newest first, each linked to its sessions, cut to what the
//! caller may see.

use super::*;
use crate::ipc_error::lock;
use crate::service::runs::{self, RunsArgs};

#[tool_router(router = runs_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Runs, newest first: tasks, mission steps, Jev \
        decisions, planner and summary runs, routine fires. list: {runs, \
        total}; each run \
        has kind, owner, outcome (ok | failed | needs_person | nothing_to_do \
        | running), duration_ms, cost_micros and session_ids.")]
    pub(super) async fn runs(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<RunsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "runs",
            &format!(
                "action={} kind={:?} outcome={:?} session_id={:?} mission_id={:?}",
                p.action.escape_debug(),
                p.kind,
                p.outcome,
                p.session_id,
                p.mission_id
            ),
        );
        match p.action.as_str() {
            "list" => {
                let args = RunsArgs {
                    since: p.since,
                    until: p.until,
                    kind: p.kind,
                    outcome: p.outcome,
                    org_id: p.org_id,
                    mission_id: p.mission_id,
                    session_id: p.session_id,
                    routine_id: p.routine_id,
                    limit: p.limit,
                    offset: p.offset,
                };
                let s = lock(&self.store).map_err(to_mcp_err)?;
                let scope = caller.view_scope(&s).map_err(to_mcp_err)?;
                let page = runs::list_in(&s, &scope, &args).map_err(to_mcp_err)?;
                ok_json_compact(&page)
            }
            other => Err(mcp_err(
                codes::E_INVALID,
                format!("runs has no action {other:?}: list"),
                None,
            )),
        }
    }
}
