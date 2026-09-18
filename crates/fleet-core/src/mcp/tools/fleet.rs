//! MCP tools: fleet health, usage, hosts, accounts and provisioning.

use super::*;
use crate::ipc_error::lock;

#[tool_router(router = fleet_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(
        description = "Report claude-fleet backend health: application version, SQLite schema version, database readiness, the cached fleet roll-up, and ESTIMATED token usage and cost (micro-USD) per host and per UTC day for the last 7 days. For a per-host token the usage fields cover only its own host. Returns JSON."
    )]
    pub(super) async fn fleet_health(
        &self,
        Extension(caller): Extension<Caller>,
    ) -> Result<CallToolResult, McpError> {
        audit("fleet_health", "");
        let mut h = health::health_check(&self.store);
        if let Some(host) = caller.host_alias.as_deref() {
            if let Ok(s) = self.store.lock() {
                health::scope_usage_to_host(&mut h, &s, host);
            }
        }
        ok_json(&h)
    }

    #[tool(description = "Report ESTIMATED token usage and cost per session, \
        host and UTC day, summed from each session's Claude Code transcript \
        (collected every usage.interval_secs). Costs are micro-USD from a \
        built-in per-model price table (override: usage.prices_json), not a \
        bill. total and by_host sum the live session rows, each over its \
        whole lifetime; by_day comes from the durable daily roll-up (killed \
        sessions included). Optional host_alias; since_secs keeps only \
        sessions whose usage changed in the last N seconds and scopes by_day \
        to that window (default: every session, last 30 days). Sessions are \
        sorted by cost, at most 200. A per-host token only sees its own \
        host. Returns JSON.")]
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
        ok_json(&report)
    }

    // ---- hosts ----

    #[tool(description = "List all registered hosts with their reachability, \
        claude/tmux versions, and linked account. Returns JSON.")]
    pub(super) async fn list_hosts(&self) -> Result<CallToolResult, McpError> {
        audit("list_hosts", "");
        ok_json(&hosts::list_hosts(&self.store).map_err(to_mcp_err)?)
    }

    #[tool(description = "Discover SSH hosts from the user's ~/.ssh/config. \
        These are candidates for add_host. Returns JSON.")]
    pub(super) async fn discover_hosts(&self) -> Result<CallToolResult, McpError> {
        audit("discover_hosts", "");
        ok_json(&hosts::discover_hosts().map_err(to_mcp_err)?)
    }

    #[tool(description = "List the cached Claude accounts seen across hosts. \
        Returns JSON.")]
    pub(super) async fn list_accounts(&self) -> Result<CallToolResult, McpError> {
        audit("list_accounts", "");
        ok_json(&hosts::list_accounts(&self.store).map_err(to_mcp_err)?)
    }

    #[tool(description = "Register a new SSH host. Probes it first; only \
        persists the host if it is reachable. Returns the host row as JSON.")]
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

    #[tool(description = "Re-probe a registered host's reachability and \
        versions. Returns the updated host row as JSON.")]
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

    #[tool(description = "Remove a registered host. Its sessions are orphaned. \
        Returns the removed host row as JSON.")]
    pub(super) async fn remove_host(
        &self,
        Parameters(args): Parameters<hosts::HostAliasArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit("remove_host", &format!("alias={}", args.alias));
        ok_json(&hosts::remove_host(args, &self.store).map_err(to_mcp_err)?)
    }

    #[tool(description = "Hide or show a host. Hidden hosts are skipped during \
        reconcile. Returns the updated host row as JSON.")]
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

    #[tool(description = "Install fleet skills, the Stop / UserPromptSubmit / \
        EnterWorktree http hooks, and this fleet's MCP server entry (with a per-host bearer \
        token) into every reachable host's ~/.claude.json (reverse SSH tunnel \
        for remote hosts when the hub is loopback-only; a hub with a public URL \
        is reached directly). rotate=true mints fresh per-host tokens. Returns a \
        per-host status list; each host must restart Claude to load the \
        server.")]
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
        ok_json(&res)
    }

    // ---- paired clients ----

    #[tool(description = "Mint a single-use pairing code for a new client \
        device (a phone, a laptop browser) and return the URL to show as a QR. \
        The code — not a token — travels in the URL FRAGMENT, so no proxy or \
        access log ever sees it; the device posts it to the hub's /pair once \
        and gets a token of its own back. name must be 1-64 characters with no \
        control characters and must not be one a live client already holds. \
        mode is full (drive sessions fleet-wide) or readonly (observe only); \
        fleet-admin tools are out of a client's reach either way. Codes live \
        in memory only, so a hub restart invalidates every outstanding one. \
        Master token only. Returns JSON { url, code, expires_in_s, name, mode }.")]
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
        audit(
            "pair_client",
            &format!("name={name} mode={mode} ttl_s={:?}", p.ttl_s),
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
        let req = self.guards.pairings.mint(&name, &mode, ttl);
        ok_json(&serde_json::json!({
            "url": crate::mcp::pair_url(&base.url, &req.code),
            "code": req.code,
            "expires_in_s": ttl.as_secs(),
            "name": req.name,
            "mode": req.mode,
        }))
    }

    #[tool(description = "List the paired client devices and what each one's \
        token may do. The stored token digest is never returned — a client's \
        token exists in plaintext only in the one /pair response that minted \
        it. include_revoked also returns clients whose token was revoked \
        (kept for the audit trail). Read-only, but master token only: the \
        list names every paired device, so it is not a phone's to read. \
        Returns JSON rows of \
        { id, name, mode, created_at, last_seen_at, revoked_at }.")]
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
        ok_json(&out)
    }

    #[tool(description = "Revoke a paired client's token by name. Its next \
        request is refused (the auth layer only resolves live rows) and the \
        name becomes free to pair again; the row itself is kept, revoked, for \
        the audit trail. E_NOTFOUND when no live client holds that name. \
        Master token only. Returns the revoked row as JSON.")]
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
