//! MCP tool: autofix (`service::autofix`). Known broken fleet states with a
//! plain-language explanation and, where one exists, a fix. The tick applies
//! the safe ones; here a person's trusted device applies the rest. The tool's
//! `scan` only syncs findings and never auto-applies.

use super::*;
use crate::ipc_error::lock;
use crate::service::autofix::apply::{apply_one, body_of, gather, live_agents, SshFixExec};
use crate::service::autofix::scan_all;
use crate::service::autofix::sync::{sync, Scope};
use crate::store::{now_unix, AutofixRow};

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

fn row_json(r: &AutofixRow) -> serde_json::Value {
    let body = body_of(r).ok();
    serde_json::json!({
        "id": r.id, "kind": r.kind, "subject": r.subject, "host_alias": r.host_alias,
        "severity": r.severity, "state": r.state,
        "first_seen_at": r.first_seen_at, "last_seen_at": r.last_seen_at,
        "attempts": r.attempts, "last_error": r.last_error, "dismissed_until": r.dismissed_until,
        "title": body.as_ref().map(|b| b.title.clone()),
        "explanation": body.as_ref().map(|b| b.explanation.clone()),
        "evidence": body.as_ref().map(|b| b.evidence.clone()),
        "fix": body.and_then(|b| b.fix),
    })
}

fn db_err(e: rusqlite::Error) -> McpError {
    to_mcp_err(e.into())
}

#[tool_router(router = autofix_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(
        description = "Known broken fleet states, each with a plain-language explanation and, where one exists, a fix: \
        list {state?} (the live ones: open, verifying, apply_failed, dismissed), explain {id}, scan (check now), apply {id}, dismiss {id, until?} (later; default 24 h). \
        Safe fixes run by themselves (setting autofix.auto_safe); the rest wait for a person. \
        apply and dismiss need a trusted full device; Control may list and explain only."
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
                    let found = scan_all(&cx);
                    sync(&s, &found, Scope::All, now).map_err(to_mcp_err)?;
                }
                let s = lock(&self.store).map_err(to_mcp_err)?;
                let rows: Vec<_> = s
                    .live_autofix_findings()
                    .map_err(db_err)?
                    .iter()
                    .filter(|r| p.state.as_deref().is_none_or(|st| r.state == st))
                    .map(row_json)
                    .collect();
                ok_json_compact(&serde_json::json!({ "findings": rows }))
            }
            "explain" => {
                let id = need_id()?;
                let s = lock(&self.store).map_err(to_mcp_err)?;
                let r = s
                    .autofix_finding(id)
                    .map_err(db_err)?
                    .ok_or_else(|| mcp_err("E_NOTFOUND", "no such finding", None))?;
                ok_json(&row_json(&r))
            }
            "apply" => {
                autofix_writer(&caller)?;
                let id = need_id()?;
                let live = live_agents(&self.ssh);
                let exec = SshFixExec {
                    store: &self.store,
                    ssh: &*self.ssh,
                };
                let r = apply_one(&self.store, &exec, id, &caller.label(), &live, now)
                    .await
                    .map_err(to_mcp_err)?;
                ok_json(&row_json(&r))
            }
            "dismiss" => {
                autofix_writer(&caller)?;
                let id = need_id()?;
                let s = lock(&self.store).map_err(to_mcp_err)?;
                let ok = s
                    .dismiss_autofix_finding(id, p.until.unwrap_or(now + 86_400))
                    .map_err(db_err)?;
                if !ok {
                    return Err(mcp_err("E_NOTFOUND", "no live finding with that id", None));
                }
                let r = s
                    .autofix_finding(id)
                    .map_err(db_err)?
                    .ok_or_else(|| mcp_err("E_NOTFOUND", "no such finding", None))?;
                ok_json(&row_json(&r))
            }
            other => Err(mcp_err(
                "E_INVALID",
                format!("autofix has no action {other:?}: list | explain | scan | apply | dismiss"),
                None,
            )),
        }
    }
}
