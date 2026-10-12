//! MCP tool: presence (redesign step 11.7b, `service::presence`). Who has a
//! session open right now, kept in memory on the hub and nowhere else.

use super::*;
use crate::ipc_error::lock;
use crate::service::presence;

#[tool_router(router = presence_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Presence: report that you have a session open \
        (again every heartbeat_secs; leaving: true when you close it) and \
        read who else has. The owner sees everyone; others see the owner \
        and themselves. Returns { session_id, viewers: [{ person_id, name, \
        device?, since, you? }], heartbeat_secs }.")]
    pub(super) async fn session_presence(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<presence::SessionPresenceArgs>,
    ) -> Result<CallToolResult, McpError> {
        let s = lock(&self.store).map_err(to_mcp_err)?;
        // `Read`: being on a session you may read is exactly what a watch
        // share is for; a caller who cannot see the row gets E_NOTFOUND and
        // learns nobody is there.
        let row = resolve_row_and_gate(
            &s,
            &caller,
            Some(args.session_id),
            None,
            None,
            Reach::Read,
            "the session on screen",
        )?;
        let person = super::fleet::owner_for(&caller, &s);
        let device = caller.client.as_ref().map(|c| c.name.as_str());
        let by_grant = caller
            .view_scope(&s)
            .map_err(to_mcp_err)?
            .grants
            .level(row.id)
            .is_some();
        let owner = presence::presence_owner(
            row.owner_person_id,
            row.visibility == crate::store::VISIBILITY_UNCLAIMED,
            by_grant,
            person,
        );
        let view = presence::session_presence(
            &s,
            &self.presence,
            owner,
            person,
            device,
            &args,
            crate::store::now_unix(),
        )
        .map_err(to_mcp_err)?;
        ok_json_compact(&view)
    }
}
