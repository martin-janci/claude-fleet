//! MCP tool: named Control API tokens (M15 step G2.8,
//! `service::control_tokens`). Served to the master (and an admin token)
//! and to the hub owner's own paired device (`Access::Person`); creating or
//! revoking one from a device needs a trusted full device, and a device can
//! never create an admin token.

use super::*;
use crate::service::control_tokens::{self, ApiTokensArgs, Minter};

#[tool_router(router = api_tokens_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Named Control API tokens. list; create {name, \
        scope: read | act | admin, expires_in_days?, hosts?} answers the token \
        ONCE, with env_line; revoke {name}. A device creates read or act \
        only, and needs trust.")]
    pub(super) async fn api_tokens(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<ApiTokensArgs>,
    ) -> Result<CallToolResult, McpError> {
        // Identifying args only; there is no token among them.
        audit(
            "api_tokens",
            &format!(
                "action={} name={:?} scope={:?} expires_in_days={:?} hosts={:?}",
                args.action.escape_debug(),
                args.name.as_deref().map(str::escape_debug),
                args.scope.as_deref().map(str::escape_debug),
                args.expires_in_days,
                args.hosts.as_ref().map(|h| h.len()),
            ),
        );
        let out = control_tokens::run(
            &self.store,
            token_minter(&caller),
            &args,
            crate::store::now_unix(),
        )
        .map_err(to_mcp_err)?;
        if !args.is_read() {
            tracing::info!(
                action = %args.action.escape_debug(),
                token = ?out.get("name").and_then(|n| n.as_str()),
                "[mcp] changed a named Control API token"
            );
        }
        if args.action == "create" {
            ok_json(&out)
        } else {
            ok_json_compact(&out)
        }
    }
}

/// Who may create or revoke: the master (an admin token is the master), or
/// the owner's trusted full device. `Access::Person` already keeps every
/// other caller out of the tool; this narrows the writes.
fn token_minter(caller: &Caller) -> Result<Minter, IpcError> {
    if caller.is_master() {
        return Ok(Minter::Admin);
    }
    if caller.is_person_device()
        && caller.is_personal_owner
        && caller.is_trusted_client()
        && caller.mode == TokenMode::Full
    {
        return Ok(Minter::OwnerDevice);
    }
    Err(IpcError::new(
        codes::E_FORBIDDEN,
        format!(
            "{} may list tokens; creating or revoking one needs the master token or a \
             trusted full device of the hub's owner",
            caller.label()
        ),
    ))
}
