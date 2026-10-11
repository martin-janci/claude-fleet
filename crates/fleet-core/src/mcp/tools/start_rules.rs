//! MCP tool: start_rules (Orbit Fleet redesign step 8.11,
//! `service::start_rules`). Which project a task key starts in, decided
//! before the key's history and before Jev; a per-host token is not served
//! it (`NOT_FOR_HOST_TOKENS`).

use super::*;
use crate::ipc_error::lock;
use crate::service::start_rules;

#[tool_router(router = start_rules_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Start rules: a task key pattern (PD-*) that names \
        the project, and optionally the host (with a fallback host for when \
        it is offline), the account, model, effort and agent (claude | \
        codex) of a start, decided before the key's history and Jev. Fleet offers one after five identical starts. \
        list: offers, active and dismissed rules; save {rule, rule_id?}: \
        the whole rule, active; accept {rule_id}: an offer, replacing the \
        pattern's other rule; dismiss {rule_id}: never offered again; delete \
        {rule_id}. E_NOTFOUND, E_INVALID, E_EXISTS.")]
    pub(super) async fn start_rules(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<start_rules::StartRulesArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "start_rules",
            &format!("action={} rule_id={:?}", p.action.escape_debug(), p.rule_id),
        );
        // Where everyone's tasks start is a person's rule (transition plan,
        // "Where AI never decides"): the operator is an unbound client, so
        // nothing below would stop it making or accepting one.
        if caller.is_operator() && matches!(p.action.as_str(), "save" | "accept") {
            return Err(mcp_err(
                codes::E_FORBIDDEN,
                "a person makes and accepts start rules, from the desktop or the phone; \
                 the operator may only list them",
                None,
            ));
        }
        let scope = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            caller.view_scope(&s).map_err(to_mcp_err)?
        };
        ok_json_compact(&start_rules::run(&self.store, &scope, &p).map_err(to_mcp_err)?)
    }
}
