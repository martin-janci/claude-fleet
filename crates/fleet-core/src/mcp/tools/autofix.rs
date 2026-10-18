//! MCP tool: autofix (`service::autofix`). Known broken fleet states with a
//! plain-language explanation and, where one exists, a fix. The tick applies
//! the safe ones; here a person's trusted device applies the rest. The tool's
//! `scan` only syncs findings and never auto-applies.

use super::*;
use crate::ipc_error::lock;
use crate::service::autofix::api;
use crate::service::autofix::apply::{
    apply_one, gather, live_agents, scan_enabled, Actor, Config, SshFixExec,
};
use crate::service::autofix::sync::{sync, Scope};
use crate::store::now_unix;

/// Only a person's trusted full device decides a fix. Control may list and
/// explain, and post a `fix` card for the person to press.
fn autofix_writer(caller: &Caller) -> Result<(), McpError> {
    if caller.is_operator() {
        return Err(mcp_err(
            "E_FORBIDDEN",
            "Control can explain a fix but not apply or dismiss it: show the person the finding and let them decide",
            None,
        ));
    }
    if caller.is_trusted_client() && caller.mode == TokenMode::Full {
        return Ok(());
    }
    Err(mcp_err(
        "E_FORBIDDEN",
        "only a trusted device with full access can apply or dismiss a fix",
        None,
    ))
}

/// Who applied, for the `autofix.apply` admin event.
fn actor_of(caller: &Caller) -> Actor {
    Actor {
        label: caller.label(),
        person: caller.person(),
        device: caller.client.as_ref().map(|c| c.id),
    }
}

#[tool_router(router = autofix_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(
        description = "Known broken fleet states, each with a plain-language explanation and, where one exists, a fix: \
        list {state?} (the live ones: open, verifying, apply_failed, dismissed; also `fixed`, cleared in the last day after a fix ran), explain {id}, scan (check now), apply {id}, dismiss {id, until?} (later; default 24 h). \
        Safe fixes run by themselves (setting autofix.auto_safe); the rest wait for a person. \
        apply and dismiss need a trusted full device; Control may list and explain only, and shows the person a finding with a `fix` chat card (`{\"spec\":\"fleet.ui/1\",\"kind\":\"fix\",\"finding_id\":<id>}`)."
    )]
    pub(super) async fn autofix(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<AutofixParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "autofix",
            &format!("action={} id={:?}", p.action.escape_debug(), p.id),
        );
        let now = now_unix();
        let need_id = || {
            p.id.ok_or_else(|| mcp_err("E_INVALID", format!("{} needs an id", p.action), None))
        };
        match p.action.as_str() {
            "list" | "scan" => {
                if p.action == "scan" {
                    let live = live_agents(&self.ssh);
                    let s = lock(&self.store).map_err(to_mcp_err)?;
                    let cx = gather(&s, &live, now).map_err(to_mcp_err)?;
                    let found = scan_enabled(&cx, &Config::read(&s));
                    sync(&s, &found, Scope::All, now).map_err(to_mcp_err)?;
                }
                let s = lock(&self.store).map_err(to_mcp_err)?;
                let can_apply = autofix_writer(&caller).is_ok();
                let out = api::list(&s, p.state.as_deref(), can_apply, now).map_err(to_mcp_err)?;
                ok_json_compact(&out)
            }
            "explain" => {
                let id = need_id()?;
                let s = lock(&self.store).map_err(to_mcp_err)?;
                ok_json(&api::get(&s, id).map_err(to_mcp_err)?)
            }
            "apply" => {
                autofix_writer(&caller)?;
                let id = need_id()?;
                let live = live_agents(&self.ssh);
                let exec = SshFixExec {
                    store: &self.store,
                    ssh: &self.ssh,
                };
                let r = apply_one(&self.store, &exec, id, &actor_of(&caller), &live, now)
                    .await
                    .map_err(to_mcp_err)?;
                ok_json(&api::FindingView::from(&r))
            }
            "dismiss" => {
                autofix_writer(&caller)?;
                let id = need_id()?;
                let s = lock(&self.store).map_err(to_mcp_err)?;
                ok_json(&api::dismiss(&s, id, p.until, now).map_err(to_mcp_err)?)
            }
            other => Err(mcp_err(
                "E_INVALID",
                format!("autofix has no action {other:?}: list | explain | scan | apply | dismiss"),
                None,
            )),
        }
    }
}
