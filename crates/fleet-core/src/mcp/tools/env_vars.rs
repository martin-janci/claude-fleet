//! MCP tool: env_vars (spec 2026-10-11-session-env-and-secrets-design.md,
//! `service::env`). Session environment variables by scope.
//! `Access::PersonOrAdmin`: the master, a fleet admin's device, and any
//! person's paired device. The shared scopes (global, host, project,
//! session) are the admin's: listed to an admin only, and a write needs the
//! hub owner's trusted full device (`owner_device_admin`). The `person`
//! scope is each person's own (`owner_for`), listed and changed by that
//! person only. A reference is the master's alone. A value is never in the
//! audit line.

use super::*;
use crate::service::env;

#[tool_router(router = env_vars_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(
        description = "Session environment variables for the next start; global < host < \
        project < person < session (person: the session owner's, yours only, scope_key '' = \
        you). list; set {scope, scope_key, name, value|ref, secret?}; unset {scope, scope_key, \
        name}. Secrets and refs are never listed; ref (env:NAME|file:/abs) is master-only. \
        E_INVALID, E_FORBIDDEN."
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
        let personal = p.scope.as_deref() == Some("person");
        if p.action != "list" && !personal {
            super::fleet::owner_device_admin(&caller, "changing session environment variables")?;
        }
        let who = {
            let s = crate::ipc_error::lock(&self.store).map_err(to_mcp_err)?;
            env::EnvCaller {
                refs_allowed: caller.is_master(),
                admin: caller.is_master_or_fleet_admin_device(),
                person: super::fleet::owner_for(&caller, &s),
            }
        };
        ok_json_compact(&env::run(&self.store, &p, &who).map_err(to_mcp_err)?)
    }
}
