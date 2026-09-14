//! MCP tools: the asset catalog (skills, agents, hooks, MCP servers, plugin
//! refs) — listing it with per-host drift state, re-scanning hosts, and
//! importing a host's Claude config into the catalog working tree.

use super::*;

#[tool_router(router = assets_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "List the asset catalog (skills, agents, hooks, MCP \
        servers, plugin refs) with each asset's per-host drift state from the \
        last scan, plus unmanaged assets found on hosts and catalog parse \
        problems. Requires catalog_configure + catalog_load in the app. Returns JSON.")]
    pub(super) async fn list_assets(&self) -> Result<CallToolResult, McpError> {
        audit("list_assets", "");
        ok_json_compact(&catalog::list_assets(&self.store).map_err(to_mcp_err)?)
    }

    #[tool(description = "Scan hosts for installed skills/agents/hooks/MCP \
        servers/plugins and recompute each catalog asset's state (in_sync | \
        drifted | missing | unmanaged | unsupported). Read-only on hosts. \
        Returns per-host results as JSON.")]
    pub(super) async fn scan_assets(
        &self,
        Parameters(p): Parameters<ScanAssetsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "scan_assets",
            &format!("host_alias={}", p.host_alias.as_deref().unwrap_or("*")),
        );
        let res = catalog::inventory::scan_hosts(&self.store, &self.ssh, p.host_alias.as_deref())
            .await
            .map_err(to_mcp_err)?;
        ok_json(&res)
    }

    #[tool(description = "Import a host's Claude config (~/.claude skills, \
        agents, hooks, ~/.claude.json MCP servers, installed plugins) into the \
        catalog repo working tree as IR assets. Never overwrites; collisions \
        are reported. Only host_alias `local` is supported. Returns the import \
        report as JSON.")]
    pub(super) async fn import_assets(
        &self,
        Parameters(p): Parameters<ImportAssetsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "import_assets",
            &format!("host_alias={} dry_run={}", p.host_alias, p.dry_run),
        );
        let token = {
            let s = self
                .store
                .lock()
                .map_err(|_| mcp_err(codes::E_LOCK, "store mutex poisoned", None))?;
            s.get_setting(crate::mcp::SETTING_TOKEN)
                .map_err(|e| to_mcp_err(e.into()))?
        };
        let args = catalog::ImportArgs {
            host_alias: p.host_alias,
            dry_run: p.dry_run,
        };
        let rep = catalog::import_host(args, &self.store, token.as_deref()).map_err(to_mcp_err)?;
        ok_json(&rep)
    }
}
