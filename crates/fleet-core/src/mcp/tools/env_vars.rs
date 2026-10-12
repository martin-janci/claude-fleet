//! MCP tool: env_vars (spec 2026-10-11-session-env-and-secrets-design.md,
//! `service::env`). Session environment variables by scope. A person's tool
//! (`Access::Person`: the master and a paired device bound to no org); a
//! write needs the hub owner's trusted full device (`owner_device_admin`),
//! and a reference is the master's alone. A value is never in the audit line.

use super::*;
use crate::service::env;

#[tool_router(router = env_vars_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(
        description = "Session environment variables for the next start; global < host < \
        project < session. list; set {scope, scope_key, name, value|ref, secret?}; unset \
        {scope, scope_key, name}. Secrets and refs are never listed; ref (env:NAME|file:/abs) \
        is master-only. E_INVALID, E_FORBIDDEN."
    )]
    pub(super) async fn env_vars(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<env::EnvVarsArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "env_vars",
            &format!(
                "action={} scope={:?} scope_key={:?} name={:?}",
                p.action.escape_debug(),
                p.scope,
                p.scope_key,
                p.name
            ),
        );
        if p.action != "list" {
            super::fleet::owner_device_admin(&caller, "changing session environment variables")?;
        }
        ok_json_compact(&env::run(&self.store, &p, caller.is_master()).map_err(to_mcp_err)?)
    }
}
