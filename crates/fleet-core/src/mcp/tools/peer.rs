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

    #[tool(description = "Link this hub to another fleet's hub: redeem a \
        peer code minted there (fleet-hub pair --mode peer) against its \
        https URL. Answers the new link; it connects within seconds. The \
        master, or the hub owner's trusted full device.")]
    pub(super) async fn link_peer(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<LinkPeerParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("link_peer", &guard::scrub_line(&p.url));
        peer_writer(&caller)?;
        let id = crate::service::peer::link::link(&self.store, &p.url, &p.code)
            .await
            .map_err(to_mcp_err)?;
        let row = lock(&self.store)
            .map_err(to_mcp_err)?
            .peer_link_summaries()
            .map_err(to_mcp_err)?
            .into_iter()
            .find(|r| r.id == id);
        ok_json_compact(&row)
    }

    #[tool(description = "Remove a link to another fleet's hub by its id: \
        messages waiting for it fail back to their senders. The master, or \
        the hub owner's trusted full device.")]
    pub(super) async fn unlink_peer(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<UnlinkPeerParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("unlink_peer", &format!("id={}", p.id));
        peer_writer(&caller)?;
        let s = lock(&self.store).map_err(to_mcp_err)?;
        let live = s
            .peer_link_summaries()
            .map_err(to_mcp_err)?
            .into_iter()
            .any(|r| r.id == p.id && r.revoked_at.is_none());
        if !live {
            return Err(mcp_err("E_NOTFOUND", "no live link with that id", None));
        }
        let failed = s
            .revoke_peer_link(p.id, crate::store::now_unix())
            .map_err(to_mcp_err)?;
        ok_json_compact(&serde_json::json!({ "id": p.id, "failed_messages": failed }))
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct LinkPeerParams {
    /// The other hub's https URL, e.g. https://hub.example.
    pub url: String,
    /// The peer code its operator minted with `fleet-hub pair --mode peer`.
    pub code: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct UnlinkPeerParams {
    /// The link's id, as list_peer_links answers it.
    pub id: i64,
}

/// Who may change this hub's links (Orbit Fleet 11.5): the gate
/// (`Access::Person`) admits the master and the hub owner's own device; a
/// device must also be trusted and full, as a settings write must
/// (`settings_writer`) — a link names another fleet's hub and carries its
/// sessions' messages.
fn peer_writer(caller: &Caller) -> Result<(), McpError> {
    if caller.is_master() || (caller.is_trusted_client() && caller.mode == TokenMode::Full) {
        return Ok(());
    }
    Err(mcp_err(
        "E_FORBIDDEN",
        "linking hubs needs a trusted full device (fleet-hub client trust), or the master",
        None,
    ))
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
