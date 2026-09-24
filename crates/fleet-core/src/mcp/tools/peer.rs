//! MCP tools: hub↔hub federation. `peer_exchange` is the one tool a `peer`
//! token reaches, and only a peer token reaches it (`enforce_mode`). The
//! work is `service::peer::listen::exchange`; this is the wire.

use super::*;
use crate::ipc_error::lock;

/// Longest prefix of a peer-chosen `fleet_id` that reaches the `audit` log
/// line (G25c). `fleet_id` is unvalidated at this point — length included —
/// so without a cap a peer could put an arbitrarily long string on the line;
/// 64 chars is comfortably longer than any real fleet id.
const AUDIT_FLEET_ID_MAX_CHARS: usize = 64;

/// The one-line `audit` detail for `peer_exchange`: counts only (a peer's
/// message bodies never reach a log line), the fleet id capped and scrubbed
/// so an oversized or line-breaking value cannot blow up or forge the line.
/// PURE so the truncation is independently testable without a tracing
/// subscriber.
fn peer_exchange_audit_detail(p: &crate::service::peer::wire::ExchangeRequest) -> String {
    let fleet_id: String = p.fleet_id.chars().take(AUDIT_FLEET_ID_MAX_CHARS).collect();
    format!(
        "fleet_id={} send={} results={} after={} wait_ms={}",
        guard::scrub_line(&fleet_id),
        p.send.len(),
        p.results.len(),
        p.after,
        p.wait_ms
    )
}

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
        audit("peer_exchange", &peer_exchange_audit_detail(&p));
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

    #[tool(description = "List this hub's links to other fleets' hubs: \
        fleet, role, state, pending count, last exchange and error. Never a \
        token. Read-only, master token only.")]
    pub(super) async fn list_peer_links(&self) -> Result<CallToolResult, McpError> {
        audit("list_peer_links", "");
        let rows = lock(&self.store)
            .map_err(to_mcp_err)?
            .peer_link_summaries()
            .map_err(to_mcp_err)?;
        ok_json_compact(&rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::peer::wire::ExchangeRequest;

    fn req(fleet_id: &str) -> ExchangeRequest {
        ExchangeRequest {
            proto: 1,
            fleet_id: fleet_id.to_string(),
            send: Vec::new(),
            after: 3,
            results: Vec::new(),
            wait_ms: 25_000,
        }
    }

    /// G25c: an over-long, peer-chosen `fleet_id` must not reach the audit
    /// log line unbounded — capped at `AUDIT_FLEET_ID_MAX_CHARS` before the
    /// line-breaking scrub.
    #[test]
    fn peer_exchange_audit_detail_caps_an_oversized_fleet_id() {
        let long = "x".repeat(500);
        let detail = peer_exchange_audit_detail(&req(&long));
        let expected_prefix = format!("fleet_id={}", "x".repeat(AUDIT_FLEET_ID_MAX_CHARS));
        assert!(detail.starts_with(&expected_prefix), "{detail}");
        assert!(
            !detail.contains(&"x".repeat(AUDIT_FLEET_ID_MAX_CHARS + 1)),
            "{detail}"
        );
    }

    #[test]
    fn peer_exchange_audit_detail_keeps_a_short_fleet_id_and_the_counts() {
        let detail = peer_exchange_audit_detail(&req("fleet-a"));
        assert_eq!(
            detail,
            "fleet_id=fleet-a send=0 results=0 after=3 wait_ms=25000"
        );
    }
}
