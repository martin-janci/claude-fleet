//! MCP tools: hub↔hub federation. `peer_exchange` is the one tool a `peer`
//! token reaches, and only a peer token reaches it (`enforce_mode`). The
//! work is `service::peer::listen::exchange`; this is the wire.

use super::*;

#[tool_router(router = peer_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Hub-to-hub link exchange (peer tokens only): \
        deliver messages and acks, receive this hub's messages for the caller. \
        Long-polls up to wait_ms. See docs/hub.md, Link two hubs.")]
    pub(super) async fn peer_exchange(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<crate::service::peer::wire::ExchangeRequest>,
    ) -> Result<CallToolResult, McpError> {
        // Counts only: a peer's bodies never reach a log line. The fleet id
        // is unvalidated here, so it is scrubbed onto one line.
        audit(
            "peer_exchange",
            &format!(
                "fleet_id={} send={} results={} after={} wait_ms={}",
                guard::scrub_line(&p.fleet_id),
                p.send.len(),
                p.results.len(),
                p.after,
                p.wait_ms
            ),
        );
        let Some(client) = caller.client.as_ref() else {
            return Err(mcp_err(
                "E_FORBIDDEN",
                "peer_exchange needs a peer client token",
                None,
            ));
        };
        let _permit = self.long_poll_permit(&caller, "peer_exchange")?;
        let resp = crate::service::peer::listen::exchange(&self.store, &self.ssh, client.id, p)
            .await
            .map_err(to_mcp_err)?;
        ok_json_compact(&resp)
    }
}
