//! MCP tool: the agents' rules, jobs and pending actions
//! (`service::agents::admin`). Served only to the hub owner's own paired
//! device (`Access::PersonDevice`): a rule is a person's grant and an
//! approval a person's answer, so the master token an agent holds never
//! sees this tool. A write also needs a trusted full device.

use super::*;
use crate::ipc_error::lock;
use crate::service::agents::admin::{self, AgentsArgs};

#[tool_router(router = agents_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "The agents (PR reviewer, support, fleet agents from \
        the catalog): status {agent?, limit?} (rules, newest jobs, actions \
        waiting for a person), explain (per rule: polls or why not, next poll, \
        last poll error, jobs by state, the next and the last failed job), job \
        {id} (one job whole, its proposed actions included), compare (the \
        built-in reviewer and support agent next to the fleet instances \
        restating them: the same pull request head or ticket comment, side \
        by side, and whether verdict or severity and confidence agree), grant {agent: \
        review | support | fleet, scope: gh:<owner>/<repo> | jira:<tracker \
        id>:<PROJECT> | <instance name>, host, level: watch | propose | act, \
        project_id?, config?, hours?, definition? (fleet), bindings? (fleet: \
        one per store role; each MCP binding is asked for its tools on the \
        host first; or answers: the grant form's)}, definitions (the \
        catalog's fleet agents), gallery (the fleet agents claude-fleet \
        ships, each marked installed or not), install {definition} (add a \
        gallery definition to the catalog), form {definition, id?} (a fleet agent's \
        grant form, prefilled from rule id), revoke {id}, pause_all, approve \
        {id}, reject {id}, run {id, request?, run_scope? {minutes?, host?, \
        session?, component?}} (a manual or scheduled fleet agent, now). \
        The owner's own device; writes need a trusted full device.")]
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
            agents_writer(&caller, &args.action)?;
            if caller.is_operator() {
                operator_may_run(&self.store, args.id)?;
            }
        }
        let by = caller
            .client
            .as_ref()
            .map_or_else(|| "device".to_string(), |c| format!("device:{}", c.name));
        let now = crate::store::now_unix();
        if args.action == "grant" && args.agent.as_deref() == Some("fleet") {
            let done = crate::service::fleet_agents::grant::grant_or_answers(
                &self.store,
                &self.ssh,
                &args,
                &by,
                now,
            )
            .await
            .map_err(to_mcp_err)?;
            return ok_json_compact(&done);
        }
        if args.action == "install"
            && !super::assets::may_admin_catalog(&caller, &self.store, catalog::catalogs::PERSONAL)?
        {
            // The same gate every other personal-catalog write runs
            // (`catalog_admin`): a trusted device still needs the assets grant.
            return Err(mcp_err(
                "E_FORBIDDEN",
                format!(
                    "install writes the asset catalog: it needs a paired client granted it \
                     ({} refused); on the hub: fleet-hub client grant <name> assets",
                    caller.label()
                ),
                None,
            ));
        }
        ok_json_compact(
            &admin::call_async(&self.store, &args, &by, now)
                .await
                .map_err(to_mcp_err)?,
        )
    }
}

/// The definitions Control's `/debug` and `/propose` commands run
/// (`operator.rs`): the only rules the operator may `run` (w15-5).
pub(crate) const OPERATOR_RUNS: &[&str] = &["self-debug", "proposer"];

/// The operator's `run` names a rule of one of [`OPERATOR_RUNS`]; any other
/// (a manual agent at `act` with auto writes, a scheduled one) is a
/// person's to start.
fn operator_may_run(store: &Mutex<Store>, id: Option<i64>) -> Result<(), McpError> {
    let def = match id {
        Some(id) => lock(store)
            .map_err(to_mcp_err)?
            .agent_rule(id)
            .map_err(|e| to_mcp_err(IpcError::from(e)))?
            .and_then(|r| r.definition),
        None => None,
    };
    if def.as_deref().is_some_and(|d| OPERATOR_RUNS.contains(&d)) {
        return Ok(());
    }
    Err(mcp_err(
        "E_FORBIDDEN",
        format!(
            "Control runs only the {} agents (/debug, /propose); a person starts any \
             other rule, under Automation, Agents",
            OPERATOR_RUNS.join(" and ")
        ),
        None,
    ))
}

/// A rule lets an agent read and write outside fleet on its own, and an
/// approval sends a write: only a trusted full device does either.
///
/// The operator is never trusted (`Caller::is_trusted_client`), but Control's
/// `/debug` and `/propose` commands run an existing rule: `run` queues a job
/// for a rule a person already granted that is still active (`run_now` refuses
/// any other), so the operator may do that one write and nothing else.
fn agents_writer(caller: &Caller, action: &str) -> Result<(), McpError> {
    if caller.is_trusted_client() && caller.mode == TokenMode::Full {
        return Ok(());
    }
    if caller.is_operator() && action == "run" {
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
