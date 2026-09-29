//! MCP tools: application updates (update-channel design §9). The fleet's
//! update picture and the operator's pins; the update wire itself is
//! `/update/*` (`mcp::update_route`), not a tool.

use super::*;
use crate::service::update;

#[tool_router(router = updates_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Fleet updates: the verified release channel, \
        each target's version, phase and what the hub would tell it now, \
        per-component counts, pins; with target, why. A per-host or \
        org-bound token sees itself only.")]
    pub(super) async fn update_status(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<UpdateStatusParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "update_status",
            &p.target.as_deref().unwrap_or("").escape_debug().to_string(),
        );
        if let Some(target) = p.target.as_deref() {
            let d = update::check_for(
                &self.store,
                &caller,
                target,
                &update::trusted_keys(),
                crate::store::now_unix(),
            )
            .map_err(to_mcp_err)?;
            return ok_json_compact(&d);
        }
        let st = update::status(
            &self.store,
            &caller,
            &update::trusted_keys(),
            crate::store::now_unix(),
        )
        .map_err(to_mcp_err)?;
        ok_json_compact(&st)
    }

    #[tool(description = "Update admin, master only: pin a version for a \
        component or target (below installed = rollback), unpin, or \
        refresh the signed channel. E_INVALID, E_UPDATE_UNVERIFIED.")]
    pub(super) async fn update_admin(
        &self,
        Parameters(p): Parameters<UpdateAdminParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "update_admin",
            &format!(
                "action={} component={}",
                p.action.escape_debug(),
                p.component.as_deref().unwrap_or("").escape_debug()
            ),
        );
        let now = crate::store::now_unix();
        let component = || {
            p.component.as_deref().ok_or_else(|| {
                mcp_err(
                    codes::E_INVALID,
                    format!("{} needs a component", p.action),
                    None,
                )
            })
        };
        let target = p.target.as_deref().unwrap_or("");
        match p.action.as_str() {
            "pin" => {
                let version = p
                    .version
                    .as_deref()
                    .ok_or_else(|| mcp_err(codes::E_INVALID, "pin needs a version", None))?;
                let row = update::pin(
                    &self.store,
                    component()?,
                    target,
                    version,
                    p.mandatory.unwrap_or(false),
                    p.reason.clone(),
                    now,
                )
                .map_err(to_mcp_err)?;
                tracing::info!(component = %row.component, target = %row.target, version = %row.version, "[mcp] pinned an update");
                ok_json_compact(&row)
            }
            "unpin" => {
                let removed =
                    update::unpin(&self.store, component()?, target).map_err(to_mcp_err)?;
                ok_json_compact(&serde_json::json!({ "removed": removed }))
            }
            "refresh" => {
                let base = update::channel_base_url();
                let fetch = update::HttpsFetch::new(Some(&base));
                let o = update::refresh(&self.store, &fetch, &base, &update::trusted_keys(), now)
                    .await
                    .map_err(to_mcp_err)?;
                ok_json_compact(&o)
            }
            other => Err(mcp_err(
                codes::E_INVALID,
                format!("action must be pin | unpin | refresh, got {other:?}"),
                None,
            )),
        }
    }
}
