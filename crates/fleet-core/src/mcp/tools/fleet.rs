//! MCP tools: fleet health, usage, hosts, accounts and provisioning.

use super::*;
use crate::ipc_error::lock;

#[tool_router(router = fleet_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Backend health: app and schema version, database \
        readiness, the cached fleet roll-up, per-host reverse-tunnel health \
        (tunnels_flapping: supervised but crash-looping, so the Control API \
        is unreachable from that host), and ESTIMATED token usage and cost \
        (micro-USD) per host and UTC day for 7 days. A per-host token's \
        usage covers its own host only.")]
    pub(super) async fn fleet_health(
        &self,
        Extension(caller): Extension<Caller>,
    ) -> Result<CallToolResult, McpError> {
        audit("fleet_health", "");
        let mut h = health::health_check(&self.store);
        h.set_tunnels(self.tunnels.health());
        if let Some(host) = caller.host_alias.as_deref() {
            if let Ok(s) = self.store.lock() {
                health::scope_usage_to_host(&mut h, &s, host);
            }
        }
        ok_json_compact(&h)
    }

    #[tool(description = "ESTIMATED token usage and cost per session, host \
        and UTC day, from each session's transcript (collected every \
        usage.interval_secs). Costs are micro-USD from a built-in price \
        table (usage.prices_json), not a bill. total and by_host sum live \
        rows over their lifetime; by_day is the durable daily roll-up \
        (killed sessions included). Sessions sorted by cost, at most 200. A \
        per-host token only sees its own host.")]
    pub(super) async fn usage_report(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<UsageReportParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "usage_report",
            &format!("host={:?} since_secs={:?}", p.host_alias, p.since_secs),
        );
        let host = usage_scope(&caller, p.host_alias.as_deref())?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let report = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            usage::report(&s, host.as_deref(), p.since_secs, now).map_err(to_mcp_err)?
        };
        ok_json_compact(&report)
    }

    // ---- hosts ----

    #[tool(description = "Registered hosts: reachability, claude/tmux \
        versions, linked account.")]
    pub(super) async fn list_hosts(&self) -> Result<CallToolResult, McpError> {
        audit("list_hosts", "");
        ok_json_compact(&hosts::list_hosts(&self.store).map_err(to_mcp_err)?)
    }

    #[tool(description = "Which agent hosts (transport \"agent\") have a \
        fleet-agent connected: since (unix s), version, host name, OS. \
        Offline ones show connected=false; a call for one fails fast with \
        E_AGENT_OFFLINE. enabled=false where no agents are accepted (the \
        desktop).")]
    pub(super) async fn agent_status(&self) -> Result<CallToolResult, McpError> {
        audit("agent_status", "");
        let registry = self.ssh.agent_registry().map(|r| r.as_ref());
        ok_json_compact(&hosts::agent_status(&self.store, registry).map_err(to_mcp_err)?)
    }

    #[tool(description = "SSH hosts in the user's ~/.ssh/config: candidates \
        for add_host.")]
    pub(super) async fn discover_hosts(&self) -> Result<CallToolResult, McpError> {
        audit("discover_hosts", "");
        ok_json_compact(&hosts::discover_hosts().map_err(to_mcp_err)?)
    }

    #[tool(description = "The cached Claude accounts seen across hosts.")]
    pub(super) async fn list_accounts(&self) -> Result<CallToolResult, McpError> {
        audit("list_accounts", "");
        ok_json_compact(&hosts::list_accounts(&self.store).map_err(to_mcp_err)?)
    }

    #[tool(description = "Register a host. transport \"ssh\" (default) is \
        probed first and persisted only if reachable; \"agent\" (a host the \
        hub cannot reach; it runs fleet-agent and dials in) is persisted \
        unprobed, unreachable until its agent connects (token: `fleet-hub \
        agent-token <alias>` on the hub). Returns the host row.")]
    pub(super) async fn add_host(
        &self,
        Parameters(args): Parameters<hosts::AddHostArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit("add_host", &add_host_audit_detail(&args));
        let row = hosts::add_host(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Re-probe a host's reachability and versions. \
        Returns the host row.")]
    pub(super) async fn probe_host(
        &self,
        Parameters(args): Parameters<hosts::HostAliasArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit("probe_host", &format!("alias={}", args.alias));
        let row = hosts::probe_host(args, &self.store, &self.ssh, &self.reg)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Remove a host; its sessions are orphaned. Returns \
        the removed row.")]
    pub(super) async fn remove_host(
        &self,
        Parameters(args): Parameters<hosts::HostAliasArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit("remove_host", &format!("alias={}", args.alias));
        ok_json(&hosts::remove_host(args, &self.store).map_err(to_mcp_err)?)
    }

    #[tool(description = "Hide or show a host (hidden: skipped by \
        reconcile). Returns the host row.")]
    pub(super) async fn hide_host(
        &self,
        Parameters(args): Parameters<hosts::HideHostArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "hide_host",
            &format!("alias={} hidden={}", args.alias, args.hidden),
        );
        ok_json(&hosts::hide_host(args, &self.store).map_err(to_mcp_err)?)
    }

    // ---- projects ----

    #[tool(description = "Install fleet skills, the Stop / UserPromptSubmit \
        / EnterWorktree http hooks and this fleet's MCP server entry \
        (per-host bearer token) into every reachable host's ~/.claude.json \
        (a reverse SSH tunnel when the hub is loopback-only). Returns \
        per-host status; each host must restart Claude to load it.")]
    pub(super) async fn provision_hosts(
        &self,
        Parameters(p): Parameters<ProvisionHostsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("provision_hosts", &format!("rotate={}", p.rotate));
        let base = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            crate::service::hub::HubBase::read(&s).map_err(to_mcp_err)?
        };
        let res = crate::service::provision::provision_hosts(
            &self.store,
            &self.ssh,
            &self.tunnels,
            &base,
            p.rotate,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json_compact(&res)
    }

    // ---- paired clients ----

    #[tool(description = "Mint a single-use pairing code for a new client \
        device (phone, browser) and return the URL to show as a QR. The code \
        (not a token) travels in the URL FRAGMENT, so no proxy or access log \
        sees it; the device posts it to /pair once for a token of its own. \
        name: 1-64 chars, no control characters, not a live client's. mode \
        full drives sessions fleet-wide, readonly observes, peer is another \
        hub's link (see peer_exchange); fleet-admin tools stay out of a \
        client's reach. Codes are in memory only: a hub restart voids them. \
        Master token only. Returns { url, code, expires_in_s, name, mode, \
        trusted }.")]
    // The master-only gate is `enforce_admin` in `call_tool` (`pair_client`
    // is in `guard::ADMIN_TOOLS`), so no caller extractor is needed here.
    pub(super) async fn pair_client(
        &self,
        Parameters(p): Parameters<PairClientParams>,
    ) -> Result<CallToolResult, McpError> {
        // Validate BEFORE auditing: the name reaches a `tracing` line, and a
        // refused mint must not be able to put a line break (or an ANSI
        // escape) into the hub's log through it. The validator also returns
        // the trimmed form, which is what gets stored.
        let name = crate::store::validate_client_name(&p.name).map_err(to_mcp_err)?;
        let mode = p.mode.unwrap_or_else(|| "full".to_string());
        crate::store::validate_client_mode(&mode).map_err(to_mcp_err)?;
        if mode == "peer" && p.trusted {
            return Err(mcp_err(
                codes::E_VALIDATE,
                "a peer hub link is never trusted; drop trusted",
                None,
            ));
        }
        audit(
            "pair_client",
            &format!(
                "name={name} mode={mode} trusted={} ttl_s={:?}",
                p.trusted, p.ttl_s
            ),
        );
        let ttl = pair_ttl(p.ttl_s);
        // Both reads under one lock, released before the mint.
        let base = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            // A code minted for a name a live client already holds could only
            // ever fail at redemption (the partial unique index), wasting the
            // code and the walk to the phone. Refuse it here instead.
            let taken = s
                .active_client_tokens()
                .map_err(to_mcp_err)?
                .into_iter()
                .any(|c| c.name == name);
            if taken {
                return Err(mcp_err(
                    codes::E_EXISTS,
                    format!(
                        "a live client named '{name}' already exists — revoke_client it first, \
                         or pair under another name"
                    ),
                    None,
                ));
            }
            crate::service::hub::HubBase::read(&s).map_err(to_mcp_err)?
        };
        let req = self.guards.pairings.mint(&name, &mode, p.trusted, ttl);
        ok_json(&serde_json::json!({
            "url": crate::mcp::pair_url(&base.url, &req.code),
            "code": req.code,
            "expires_in_s": ttl.as_secs(),
            "name": req.name,
            "mode": req.mode,
            "trusted": req.trusted,
        }))
    }

    #[tool(description = "Paired client devices and what each token may do. \
        The token digest is never returned: a token exists in plaintext only \
        in the /pair response that minted it. Read-only but master token \
        only (it names every paired device). Rows: { id, name, mode, \
        created_at, last_seen_at, revoked_at, trusted_at }.")]
    pub(super) async fn list_clients(
        &self,
        Parameters(p): Parameters<ListClientsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "list_clients",
            &format!("include_revoked={}", p.include_revoked),
        );
        let rows = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            s.list_client_tokens(p.include_revoked)
                .map_err(to_mcp_err)?
        };
        let out: Vec<ClientSummary> = rows.into_iter().map(ClientSummary::from).collect();
        ok_json_compact(&out)
    }

    #[tool(description = "Revoke a paired client's token by name: its next \
        request is refused and the name is free to pair again; the row is \
        kept, revoked, for the audit trail. E_NOTFOUND when no live client \
        has that name. Master token only.")]
    pub(super) async fn revoke_client(
        &self,
        Parameters(p): Parameters<RevokeClientParams>,
    ) -> Result<CallToolResult, McpError> {
        // A revoke takes any name — even one no client holds — so there is
        // nothing to validate first. `escape_debug` is what keeps a line
        // break or an ANSI escape in a bogus name out of the log line.
        audit("revoke_client", &format!("name={}", p.name.escape_debug()));
        let row = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            s.revoke_client_token(&p.name).map_err(to_mcp_err)?
        };
        tracing::info!(client = %row.name, "[mcp] revoked a client token");
        ok_json(&ClientSummary::from(row))
    }

    #[tool(description = "Grant or withdraw trust in a paired client by \
        name: a trusted client's prompts and messages are delivered without \
        the untrusted-content marker, like the master's raw=true. Trust a \
        device you type on, never an agent's token. E_NOTFOUND for an \
        unknown live name. Master token only.")]
    pub(super) async fn set_client_trust(
        &self,
        Parameters(p): Parameters<SetClientTrustParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "set_client_trust",
            &format!("name={} trusted={}", p.name.escape_debug(), p.trusted),
        );
        let row = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            s.set_client_trust(&p.name, p.trusted).map_err(to_mcp_err)?
        };
        tracing::info!(
            client = %row.name,
            trusted = row.trusted_at.is_some(),
            "[mcp] changed a client's trust"
        );
        ok_json(&ClientSummary::from(row))
    }

    // ---- workspace repair ----
}

/// How long a pairing code stays valid: the caller's `ttl_s` clamped to
/// 30 s…1 h, or [`crate::mcp::pairing::DEFAULT_TTL`] when it names none. The
/// upper bound is what keeps "mint one and leave it running" from becoming a
/// standing invitation; the lower bound leaves time to walk to the phone.
fn pair_ttl(ttl_s: Option<u64>) -> std::time::Duration {
    const MIN: u64 = 30;
    const MAX: u64 = 60 * 60;
    match ttl_s {
        None => crate::mcp::pairing::DEFAULT_TTL,
        Some(s) => std::time::Duration::from_secs(s.clamp(MIN, MAX)),
    }
}

/// The `add_host` audit line's identifying detail. A transport change is
/// exactly the kind of thing the audit trail should carry, so it rides
/// alongside `alias`/`ssh_alias` — defaulted the same way `add_host` itself
/// resolves an unset transport, so the log always names the effective value.
fn add_host_audit_detail(args: &hosts::AddHostArgs) -> String {
    format!(
        "alias={} ssh_alias={} transport={}",
        args.alias,
        args.ssh_alias,
        args.transport.as_deref().unwrap_or("ssh")
    )
}

#[cfg(test)]
mod tests {
    use super::{add_host_audit_detail, pair_ttl};
    use crate::service::hosts;
    use std::time::Duration;

    #[test]
    fn pair_ttl_defaults_and_clamps() {
        assert_eq!(pair_ttl(None), Duration::from_secs(600));
        assert_eq!(pair_ttl(Some(60)), Duration::from_secs(60));
        assert_eq!(pair_ttl(Some(0)), Duration::from_secs(30), "floor");
        assert_eq!(
            pair_ttl(Some(u64::MAX)),
            Duration::from_secs(3600),
            "ceiling"
        );
    }

    #[test]
    fn add_host_audit_detail_names_an_explicit_transport() {
        let args = hosts::AddHostArgs {
            alias: "h".into(),
            ssh_alias: "h.example".into(),
            transport: Some("agent".into()),
        };
        assert_eq!(
            add_host_audit_detail(&args),
            "alias=h ssh_alias=h.example transport=agent"
        );
    }

    #[test]
    fn add_host_audit_detail_names_the_default_transport_when_unset() {
        let args = hosts::AddHostArgs {
            alias: "h".into(),
            ssh_alias: "h.example".into(),
            transport: None,
        };
        assert_eq!(
            add_host_audit_detail(&args),
            "alias=h ssh_alias=h.example transport=ssh"
        );
    }
}
