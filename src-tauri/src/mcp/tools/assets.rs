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
        drifted | missing | unmanaged | unsupported | orphan). Read-only on \
        hosts. Returns per-host results as JSON.")]
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
        Parameters(args): Parameters<catalog::ImportArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "import_assets",
            &format!("host_alias={} dry_run={}", args.host_alias, args.dry_run),
        );
        let token = {
            let s = self
                .store
                .lock()
                .map_err(|_| mcp_err(codes::E_LOCK, "store mutex poisoned", None))?;
            s.get_setting(crate::mcp::SETTING_TOKEN)
                .map_err(|e| to_mcp_err(e.into()))?
        };
        let rep = catalog::import_host(args, &self.store, token.as_deref()).map_err(to_mcp_err)?;
        ok_json(&rep)
    }

    #[tool(description = "Compute a sync plan: scan the selected hosts, \
        compare every catalog asset with what is installed, and return \
        per-host actions (create | update | overwrite | adopt | remove | \
        plugin_install | noop | blocked) plus a plan_id valid for 10 \
        minutes. Inventory states now include orphan (in the host's fleet \
        manifest, no longer in the catalog). Nothing is written. Pass the \
        plan_id to apply_sync.")]
    pub(super) async fn plan_sync(
        &self,
        Parameters(p): Parameters<PlanSyncParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "plan_sync",
            &format!(
                "host_alias={} kind={} name={}",
                p.host_alias.as_deref().unwrap_or("*"),
                p.kind.as_deref().unwrap_or("*"),
                p.name.as_deref().unwrap_or("*"),
            ),
        );
        let kind = match &p.kind {
            Some(k) => Some(parse_kind(k)?),
            None => None,
        };
        let args = catalog::sync::PlanArgs {
            host_alias: p.host_alias,
            kind,
            name: p.name,
        };
        let plan = catalog::sync::plan_sync(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json_compact(&plan)
    }

    #[tool(description = "Apply a plan from plan_sync on hosts: writes files \
        with compare-and-swap, backs up overwritten files, merges config \
        (files end up mode 0600), installs plugins, writes the managed \
        manifest, then re-scans. Master token only; requires confirmation. \
        Returns per-host results; restart_required marks hosts whose Claude \
        must be restarted.")]
    pub(super) async fn apply_sync(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<ApplySyncParams>,
    ) -> Result<CallToolResult, McpError> {
        let confirm_summary = format!("plan_id={} force_partial={}", p.plan_id, p.force_partial);
        audit("apply_sync", &confirm_summary);
        self.confirm_gate(
            "apply_sync",
            p.confirm_nonce.as_deref(),
            &confirm_summary,
            &caller,
        )?;
        // Master-only enforcement already happened centrally in
        // `ServerHandler::call_tool` (`enforce_admin` runs there before the
        // tool router ever dispatches here) — `apply_sync` is in
        // `guard::ADMIN_TOOLS`, so a non-master caller never reaches this body.
        let args = catalog::sync::ApplyArgs {
            plan_id: p.plan_id,
            force_partial: p.force_partial,
            call_id: None,
        };
        let summary = catalog::sync::apply_sync(args, &self.store, &self.ssh, &self.reg)
            .await
            .map_err(to_mcp_err)?;
        ok_json_compact(&summary)
    }

    #[tool(description = "Store a value for a ${NAME} placeholder used by \
        the catalog (global, or a per-host override with host_alias). \
        Master token only. The value is never returned or logged.")]
    pub(super) async fn set_secret(
        &self,
        Parameters(p): Parameters<SetSecretParams>,
    ) -> Result<CallToolResult, McpError> {
        // Master-only enforcement already happened centrally in
        // `ServerHandler::call_tool` (`enforce_admin` runs there before the
        // tool router ever dispatches here) — `set_secret` is in
        // `guard::ADMIN_TOOLS`, so a non-master caller never reaches this body.
        audit(
            "set_secret",
            &format!(
                "name={} host={}",
                p.name,
                p.host_alias.as_deref().unwrap_or("global")
            ),
        );
        if !catalog::sync::secrets::is_valid_secret_name(&p.name) {
            return Err(mcp_err(
                codes::E_INVALID,
                format!("invalid secret name '{}'; use [A-Z0-9_]+", p.name),
                None,
            ));
        }
        let s = self
            .store
            .lock()
            .map_err(|_| mcp_err(codes::E_LOCK, "store mutex poisoned", None))?;
        s.set_secret(&p.name, p.host_alias.as_deref(), &p.value)
            .map_err(|e| to_mcp_err(IpcError::from(e)))?;
        drop(s);
        ok_json(&serde_json::json!({ "ok": true }))
    }

    #[tool(description = "List the catalog's layer definitions (layers/*.yaml) \
        and each host's role + active contexts. Read-only. Requires \
        catalog_configure + catalog_load in the app. Returns JSON.")]
    pub(super) async fn list_layers(&self) -> Result<CallToolResult, McpError> {
        audit("list_layers", "");
        let out = catalog::list_layers(&self.store).map_err(to_mcp_err)?;
        ok_json(&out)
    }

    #[tool(description = "Compute the effective asset set for one host after \
        its role and contexts are resolved, with provenance: which layer \
        introduced each asset, which layers overrode it, and which layer \
        excluded anything missing. Nothing is written. Requires \
        catalog_configure + catalog_load in the app. Returns JSON.")]
    pub(super) async fn resolve_preview(
        &self,
        Parameters(p): Parameters<ResolvePreviewParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("resolve_preview", &format!("host_alias={}", p.host_alias));
        let res = catalog::resolve_preview(&p.host_alias, &self.store).map_err(to_mcp_err)?;
        // Project to a summary shape at the MCP boundary: `Resolution` is a
        // full `Catalog`, and `Asset`'s serializer emits `body` in full plus
        // every `Resource`'s base64 `bytes` — sending that uncapped over MCP
        // blows past token caps on any fleet-sized catalog (see the same
        // warning on `ok_json_compact` below). `list_assets` already returns
        // a summary shape for the same reason; this mirrors it. The Tauri
        // command for the desktop UI keeps the full `Resolution`.
        let assets: Vec<serde_json::Value> = res
            .catalog
            .assets
            .iter()
            .map(|a| {
                serde_json::json!({
                    "kind": a.kind().as_str(),
                    "name": a.header.name,
                    "version": a.header.version,
                })
            })
            .collect();
        ok_json(&serde_json::json!({
            "provenance": res.provenance,
            "excluded": res.excluded,
            "assets": assets,
        }))
    }

    #[tool(description = "Propose an initial layer split from the last scan, \
        grouping assets by the exact set of hosts they are installed on. The \
        largest group becomes 'core'; assets on a single host are returned \
        separately for triage. Read-only: writes nothing. Returns JSON.")]
    pub(super) async fn propose_layers(&self) -> Result<CallToolResult, McpError> {
        audit("propose_layers", "");
        let out = catalog::propose::propose_layers(&self.store).map_err(to_mcp_err)?;
        ok_json(&out)
    }

    #[tool(description = "Replace a host's layer assignment: one optional role \
        plus context layers in application order. Edits fleet state only, never \
        catalog files. Requires catalog_configure + catalog_load in the app. \
        Master token only. Returns the host's new assignment as JSON.")]
    pub(super) async fn set_host_layers(
        &self,
        Parameters(p): Parameters<SetHostLayersParams>,
    ) -> Result<CallToolResult, McpError> {
        // Master-only enforcement already happened centrally in
        // `ServerHandler::call_tool` (`enforce_admin` runs there before the
        // tool router ever dispatches here) — `set_host_layers` is in
        // `guard::ADMIN_TOOLS`, so a non-master caller never reaches this
        // body. A host's layer assignment decides what the next apply_sync
        // writes to its filesystem, so it is gated the same as apply_sync
        // and set_secret.
        audit(
            "set_host_layers",
            &format!(
                "host_alias={} role={:?} contexts={}",
                p.host_alias,
                p.role,
                p.contexts.len()
            ),
        );
        let out = catalog::set_host_layers(
            &p.host_alias,
            p.role.as_deref(),
            &p.contexts.iter().map(String::as_str).collect::<Vec<_>>(),
            &self.store,
        )
        .map_err(to_mcp_err)?;
        ok_json(&out)
    }
}

/// Parse an MCP `kind` filter string into a `Kind`, using the same
/// snake_case names the JSON representation already uses elsewhere
/// (`skill`, `agent`, `hook`, `mcp_server`, `plugin_ref`).
fn parse_kind(s: &str) -> Result<catalog::model::Kind, McpError> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .map_err(|_| mcp_err(codes::E_INVALID, format!("unknown asset kind '{s}'"), None))
}
