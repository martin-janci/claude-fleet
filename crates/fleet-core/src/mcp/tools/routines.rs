//! MCP tool: routines (Orbit Fleet redesign step 8.5, `service::routines`).
//! A person's own scheduled prompts; a per-host token is not served it
//! (`NOT_FOR_HOST_TOKENS`): a session does not schedule sessions.

use super::*;
use crate::ipc_error::lock;
use crate::service::routines;

#[tool_router(router = routines_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Routines: a saved prompt that starts a session \
        on cron, a session or PR event or Run now. list; get \
        {routine_id}: runs and fixes; runs {routine_id, limit?}; \
        failing: Inbox; budget: fleet spend today; \
        save {routine, routine_id?}: the whole routine; preview: save's dry \
        run; delete; set_enabled {enabled}; skip_next {skip?}; run_now. Pause \
        all stops the schedule, not run_now. E_NOTFOUND, E_INVALID.")]
    pub(super) async fn routines(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<RoutinesParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "routines",
            &format!(
                "action={} routine_id={:?}",
                p.action.escape_debug(),
                p.routine_id
            ),
        );
        // A routine is a saved prompt that starts a session: what the
        // operator wrote in one would reach a session with no person in
        // between (transition plan, "Where AI never decides"). The operator
        // is an unbound client, so nothing below would stop it.
        if caller.is_operator() && matches!(p.action.as_str(), "save" | "run_now") {
            return Err(mcp_err(
                codes::E_FORBIDDEN,
                "a person saves and runs routines, from the desktop or the phone; \
                 the operator may only read them",
                None,
            ));
        }
        let scope = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            caller.view_scope(&s).map_err(to_mcp_err)?
        };
        let id = || {
            p.routine_id.ok_or_else(|| {
                mcp_err(
                    codes::E_INVALID,
                    format!("{} needs routine_id", p.action),
                    None,
                )
            })
        };
        let store = &*self.store;
        match p.action.as_str() {
            "list" => ok_json_compact(&routines::list(store, &scope).map_err(to_mcp_err)?),
            "get" => ok_json_compact(&routines::get(store, &scope, id()?).map_err(to_mcp_err)?),
            "failing" => ok_json_compact(&routines::failing(store, &scope).map_err(to_mcp_err)?),
            "budget" => ok_json_compact(
                &routines::budget(store, crate::store::now_unix()).map_err(to_mcp_err)?,
            ),
            "runs" => {
                ok_json_compact(&routines::runs(store, &scope, id()?, p.limit).map_err(to_mcp_err)?)
            }
            "save" => {
                let input = p
                    .routine
                    .as_ref()
                    .ok_or_else(|| mcp_err(codes::E_INVALID, "save needs routine", None))?;
                ok_json_compact(
                    &routines::save(store, &scope, p.routine_id, input).map_err(to_mcp_err)?,
                )
            }
            "preview" => {
                let input = p
                    .routine
                    .as_ref()
                    .ok_or_else(|| mcp_err(codes::E_INVALID, "preview needs routine", None))?;
                ok_json_compact(
                    &routines::preview(
                        store,
                        &scope,
                        p.routine_id,
                        input,
                        crate::store::now_unix(),
                    )
                    .map_err(to_mcp_err)?,
                )
            }
            "delete" => {
                let removed = routines::delete(store, &scope, id()?).map_err(to_mcp_err)?;
                ok_json_compact(&serde_json::json!({ "removed": removed }))
            }
            "set_enabled" => {
                let on = p
                    .enabled
                    .ok_or_else(|| mcp_err(codes::E_INVALID, "set_enabled needs enabled", None))?;
                ok_json_compact(
                    &routines::set_enabled(store, &scope, id()?, on).map_err(to_mcp_err)?,
                )
            }
            "skip_next" => ok_json_compact(
                &routines::skip_next(store, &scope, id()?, p.skip.unwrap_or(true))
                    .map_err(to_mcp_err)?,
            ),
            "run_now" => {
                let deps =
                    routines::Deps::live(self.store.clone(), self.ssh.clone(), self.reg.clone());
                let run = routines::run_now(&deps, &scope, id()?, crate::store::now_unix())
                    .await
                    .map_err(to_mcp_err)?;
                ok_json_compact(&run)
            }
            other => Err(mcp_err(
                codes::E_INVALID,
                format!(
                    "action must be list | get | runs | failing | budget | save | preview | delete | \
                     set_enabled | skip_next | run_now, got {other:?}"
                ),
                None,
            )),
        }
    }
}
