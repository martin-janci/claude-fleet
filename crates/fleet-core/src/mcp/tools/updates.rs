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

    #[tool(description = "Update admin, master only: pin a version (below \
        installed = rollback), unpin, update_now, refresh the channel, \
        rollout_* in waves, or an org's policy (set_policy / clear_policy). \
        E_INVALID, E_CONFLICT, E_UPDATE_UNVERIFIED.")]
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
            "update_now" => {
                let r = update::update_now(
                    &self.store,
                    component()?,
                    target,
                    p.version.as_deref(),
                    &update::trusted_keys(),
                    now,
                )
                .map_err(to_mcp_err)?;
                // Each agent host's updater, through the agent itself: a
                // file its path unit watches. Best effort and in parallel;
                // an offline agent installs on its next timer pass.
                let script = crate::shell::quote(update::AGENT_POKE_SCRIPT);
                let pokes = r.agents.iter().map(|alias| {
                    let script = script.clone();
                    async move {
                        let out = self
                            .ssh
                            .run(
                                alias,
                                &["bash", "-c", &script],
                                std::time::Duration::from_secs(10),
                            )
                            .await;
                        let result = match out {
                            Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
                            Err(e) => e.code,
                        };
                        serde_json::json!({ "host": alias, "result": result })
                    }
                });
                let agents = futures_util::future::join_all(pokes).await;
                tracing::info!(component = %r.pin.component, target = %r.pin.target, version = %r.pin.version, "[mcp] update now");
                ok_json_compact(&serde_json::json!({
                    "pin": r.pin,
                    "hub_woken": r.hub_woken,
                    "agents": agents,
                }))
            }
            "set_policy" | "clear_policy" => {
                let org_id = p.org_id.ok_or_else(|| {
                    mcp_err(
                        codes::E_INVALID,
                        format!("{} needs an org_id", p.action),
                        None,
                    )
                })?;
                if p.action == "clear_policy" {
                    let removed = update::clear_org_policy(&self.store, org_id, component()?)
                        .map_err(to_mcp_err)?;
                    return ok_json_compact(&serde_json::json!({ "removed": removed }));
                }
                let row = update::set_org_policy(
                    &self.store,
                    org_id,
                    component()?,
                    update::OrgPolicyInput {
                        mode: p.mode.clone(),
                        minimum: p.minimum.clone(),
                        window: p.window.clone(),
                        pin_version: p.version.clone(),
                        pin_mandatory: p.mandatory.unwrap_or(false),
                        reason: p.reason.clone(),
                    },
                    now,
                )
                .map_err(to_mcp_err)?;
                tracing::info!(org_id, component = %row.component, "[mcp] set an org's update policy");
                ok_json_compact(&row)
            }
            "rollout_start" => {
                let version = p.version.as_deref().ok_or_else(|| {
                    mcp_err(codes::E_INVALID, "rollout_start needs a version", None)
                })?;
                let row = update::rollout_start(
                    &self.store,
                    component()?,
                    version,
                    p.waves.clone(),
                    p.halt_failure_ratio,
                    &update::trusted_keys(),
                    now,
                )
                .map_err(to_mcp_err)?;
                tracing::info!(component = %row.component, version = %row.version, waves = ?row.waves, "[mcp] started an update rollout");
                ok_json_compact(&row)
            }
            "rollout_pause" => ok_json_compact(
                &update::rollout_pause(&self.store, component()?, p.reason.as_deref(), now)
                    .map_err(to_mcp_err)?,
            ),
            "rollout_resume" => ok_json_compact(
                &update::rollout_resume(&self.store, component()?, now).map_err(to_mcp_err)?,
            ),
            "rollout_abort" => ok_json_compact(
                &update::rollout_abort(&self.store, component()?, now).map_err(to_mcp_err)?,
            ),
            other => Err(mcp_err(
                codes::E_INVALID,
                format!(
                    "action must be pin | unpin | update_now | refresh | rollout_start | \
                     rollout_pause | rollout_resume | rollout_abort | set_policy | clear_policy, \
                     got {other:?}"
                ),
                None,
            )),
        }
    }
}
