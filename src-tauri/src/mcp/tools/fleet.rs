//! MCP tools: fleet health, usage, hosts, accounts and provisioning.

use super::*;

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
            let s = self
                .store
                .lock()
                .map_err(|_| mcp_err("E_LOCK", "store mutex poisoned", None))?;
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
        Parameters(p): Parameters<AddHostParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "add_host",
            &format!("alias={} ssh_alias={}", p.alias, p.ssh_alias),
        );
        let args = hosts::AddHostArgs {
            alias: p.alias,
            ssh_alias: p.ssh_alias,
        };
        let row = hosts::add_host(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Re-probe a registered host's reachability and \
        versions. Returns the updated host row as JSON.")]
    pub(super) async fn probe_host(
        &self,
        Parameters(p): Parameters<HostAliasParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("probe_host", &format!("alias={}", p.alias));
        let args = hosts::HostAliasArgs { alias: p.alias };
        let row = hosts::probe_host(args, &self.store, &self.ssh, &self.reg)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Remove a registered host. Its sessions are orphaned. \
        Returns the removed host row as JSON.")]
    pub(super) async fn remove_host(
        &self,
        Parameters(p): Parameters<HostAliasParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("remove_host", &format!("alias={}", p.alias));
        let args = hosts::HostAliasArgs { alias: p.alias };
        ok_json(&hosts::remove_host(args, &self.store).map_err(to_mcp_err)?)
    }

    #[tool(description = "Hide or show a host. Hidden hosts are skipped during \
        reconcile. Returns the updated host row as JSON.")]
    pub(super) async fn hide_host(
        &self,
        Parameters(p): Parameters<HideHostParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "hide_host",
            &format!("alias={} hidden={}", p.alias, p.hidden),
        );
        let args = hosts::HideHostArgs {
            alias: p.alias,
            hidden: p.hidden,
        };
        ok_json(&hosts::hide_host(args, &self.store).map_err(to_mcp_err)?)
    }

    // ---- projects ----

    #[tool(description = "Install fleet skills, the Stop / UserPromptSubmit / \
        EnterWorktree http hooks, and this fleet's MCP server entry (with a per-host bearer \
        token) into every reachable host's ~/.claude.json (reverse SSH tunnel \
        for remote hosts). rotate=true mints fresh per-host tokens. Returns a \
        per-host status list; each host must restart Claude to load the \
        server.")]
    pub(super) async fn provision_hosts(
        &self,
        Parameters(p): Parameters<ProvisionHostsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("provision_hosts", &format!("rotate={}", p.rotate));
        let port = {
            let s = self
                .store
                .lock()
                .map_err(|_| to_mcp_err(IpcError::new("E_LOCK", "store mutex poisoned")))?;
            let has_master = s
                .get_setting(crate::mcp::SETTING_TOKEN)
                .map_err(|e| to_mcp_err(IpcError::from(e)))?
                .is_some_and(|t| !t.is_empty());
            if !has_master {
                return Err(to_mcp_err(IpcError::new(
                    "E_PROVISION",
                    "control API has no token yet",
                )));
            }
            s.get_setting(crate::mcp::SETTING_PORT)
                .map_err(|e| to_mcp_err(IpcError::from(e)))?
                .and_then(|p| p.parse().ok())
                .unwrap_or(crate::mcp::DEFAULT_PORT)
        };
        let res = crate::service::provision::provision_hosts(
            &self.store,
            &self.ssh,
            &self.tunnels,
            port,
            p.rotate,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&res)
    }

    // ---- workspace repair ----
}
