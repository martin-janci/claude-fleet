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
        the project, and optionally the host, a start lands in, before the \
        key's history and Jev. Fleet offers one after five identical starts. \
        list: offers, active and dismissed rules; save {rule, rule_id?}: \
        the whole rule, active; accept {rule_id}: an offer, replacing the \
        pattern's other rule; dismiss {rule_id}: never offered again; delete \
        {rule_id}. E_NOTFOUND, E_INVALID, E_EXISTS.")]
    pub(super) async fn start_rules(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<StartRulesParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "start_rules",
            &format!("action={} rule_id={:?}", p.action.escape_debug(), p.rule_id),
        );
        let scope = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            caller.view_scope(&s).map_err(to_mcp_err)?
        };
        let id = || {
            p.rule_id.ok_or_else(|| {
                mcp_err(
                    codes::E_INVALID,
                    format!("{} needs rule_id", p.action),
                    None,
                )
            })
        };
        let store = &*self.store;
        match p.action.as_str() {
            "list" => ok_json_compact(&start_rules::list(store, &scope).map_err(to_mcp_err)?),
            "save" => {
                let input = p
                    .rule
                    .as_ref()
                    .ok_or_else(|| mcp_err(codes::E_INVALID, "save needs rule", None))?;
                ok_json_compact(
                    &start_rules::save(store, &scope, p.rule_id, input).map_err(to_mcp_err)?,
                )
            }
            "accept" => {
                ok_json_compact(&start_rules::accept(store, &scope, id()?).map_err(to_mcp_err)?)
            }
            "dismiss" => {
                ok_json_compact(&start_rules::dismiss(store, &scope, id()?).map_err(to_mcp_err)?)
            }
            "delete" => {
                let removed = start_rules::delete(store, &scope, id()?).map_err(to_mcp_err)?;
                ok_json_compact(&serde_json::json!({ "removed": removed }))
            }
            other => Err(mcp_err(
                codes::E_INVALID,
                format!("action must be list | save | accept | dismiss | delete, got {other:?}"),
                None,
            )),
        }
    }
}
