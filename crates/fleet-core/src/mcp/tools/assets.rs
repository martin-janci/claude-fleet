//! MCP tools: the asset catalog (skills, agents, hooks, MCP servers, plugin
//! refs) — listing it with per-host drift state, re-scanning hosts, and
//! importing a host's Claude config into the catalog working tree.

use super::*;

#[tool_router(router = assets_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "The asset catalog (skills, agents, hooks, MCP \
        servers, plugin refs) with each asset's per-host drift state from \
        the last scan, plus unmanaged assets on hosts and catalog parse \
        problems. E_CATALOG_NOT_CONFIGURED until a catalog is set (in the \
        app, or `fleet-hub catalog set` on a hub).")]
    pub(super) async fn list_assets(&self) -> Result<CallToolResult, McpError> {
        audit("list_assets", "");
        catalog::ensure_fresh(&self.store).map_err(to_mcp_err)?;
        ok_json_compact(&catalog::list_assets(&self.store).map_err(to_mcp_err)?)
    }

    #[tool(description = "Scan hosts for installed skills/agents/hooks/MCP \
        servers/plugins and recompute each catalog asset's state (in_sync | \
        drifted | missing | unmanaged | unsupported | orphan). Read-only on \
        hosts.")]
    pub(super) async fn scan_assets(
        &self,
        Parameters(p): Parameters<ScanAssetsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "scan_assets",
            &format!("host_alias={}", p.host_alias.as_deref().unwrap_or("*")),
        );
        catalog::ensure_fresh(&self.store).map_err(to_mcp_err)?;
        let res = catalog::inventory::scan_hosts(&self.store, &self.ssh, p.host_alias.as_deref())
            .await
            .map_err(to_mcp_err)?;
        ok_json_compact(&res)
    }

    #[tool(description = "Import a host's Claude config (~/.claude skills, \
        agents, hooks, ~/.claude.json MCP servers, installed plugins) into \
        the catalog working tree as IR assets. Never overwrites; collisions \
        are reported. Any host: `local` reads this machine, others are read \
        over SSH. `only` limits it to `<kind>:<name>` assets. Master or a \
        client granted `assets`.")]
    pub(super) async fn import_assets(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<catalog::ImportArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "import_assets",
            &format!(
                "host_alias={} dry_run={} caller={}",
                args.host_alias,
                args.dry_run,
                caller.label()
            ),
        );
        // CRITICAL: `import_assets` is `Access::Client` (guard.rs) so a
        // per-host token or an ungranted paired client can reach this body —
        // and with a remote `host_alias` it now makes the hub SSH into
        // another host and write into the catalog. `may_admin_catalog`'s own
        // doc says a per-host token must never edit the catalog; this is the
        // same gate `catalog_admin` runs, so the same caller who may drive
        // that tool is the only one who may drive this one.
        if !may_admin_catalog(&caller, &self.store, catalog::catalogs::PERSONAL)? {
            return Err(mcp_err(
                "E_FORBIDDEN",
                format!(
                    "import_assets needs the master token or a paired client granted the \
                     asset catalog ({} refused); on the hub: fleet-hub client grant <name> assets",
                    caller.label()
                ),
                None,
            ));
        }
        let token = {
            let s = self
                .store
                .lock()
                .map_err(|_| mcp_err(codes::E_LOCK, "store mutex poisoned", None))?;
            s.get_setting(crate::mcp::SETTING_TOKEN)
                .map_err(|e| to_mcp_err(e.into()))?
        };
        let rep = catalog::import_host(args, &self.store, &self.ssh, token.as_deref())
            .await
            .map_err(to_mcp_err)?;
        ok_json(&rep)
    }

    #[tool(description = "Compute a sync plan: scan the hosts, compare every \
        catalog asset with what is installed, and return per-host actions \
        (create | update | overwrite | adopt | remove | plugin_install | \
        plugin_update | noop | blocked) plus a plan_id for apply_sync. \
        plugin_update fires when a pinned plugin's catalog version changes; \
        a host left on the old version stays blocked. orphan: in the host's \
        fleet manifest, no longer in the catalog. A remote host with no \
        layers assigned is skipped (it would otherwise get the whole \
        catalog) unless allow_unlayered is set. Nothing is written.")]
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
            allow_unlayered: p.allow_unlayered.unwrap_or(false),
        };
        catalog::ensure_fresh(&self.store).map_err(to_mcp_err)?;
        let plan = catalog::sync::plan_sync(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json_compact(&plan)
    }

    #[tool(description = "Apply a plan_sync plan: writes files with \
        compare-and-swap, backs up overwritten files, merges config (files \
        end up mode 0600), installs plugins, writes the managed manifest, \
        then re-scans. Master token only; requires confirmation. \
        restart_required marks hosts whose Claude must be restarted.")]
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

    #[tool(description = "The Assets tab's catalog operations as one tool, \
        plus the set of catalogs and host admissions. Master, or a client \
        granted the catalog the action touches; list_catalogs an unbound \
        one, add_catalog the master.")]
    pub(super) async fn catalog_admin(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<CatalogAdminParams>,
    ) -> Result<CallToolResult, McpError> {
        use catalog::admin::{AdminCall, Touches};
        let mut wire = serde_json::json!({ "action": p.action });
        if let Some(args) = p.args.filter(|a| !a.is_null()) {
            wire["args"] = args;
        }
        // Parsed first: what a call touches depends on which call it is (R11).
        let parsed = serde_json::from_value::<AdminCall>(wire)
            .map_err(|e| {
                mcp_err(
                    "E_INVALID",
                    format!("catalog_admin {}: {e}", p.action),
                    None,
                )
            })
            .and_then(|call| {
                let touches = call.touches(p.catalog.as_deref()).map_err(to_mcp_err)?;
                Ok((call, touches))
            });
        // The catalog the call really touches, not the raw parameter.
        let touched = match &parsed {
            Ok((_, Touches::Catalog(name))) => name.as_str(),
            Ok((_, Touches::NewCatalog)) => "(new)",
            Ok((_, Touches::Nothing)) => "-",
            Err(_) => "(invalid)",
        };
        audit(
            "catalog_admin",
            &format!(
                "action={} catalog={touched} caller={}",
                p.action,
                caller.label()
            ),
        );
        let (mut call, touches) = parsed?;
        let allowed = match &touches {
            // Every catalog's paths, remotes and grantees, across orgs: the
            // master's, or a person's own unbound full device.
            Touches::Nothing => {
                caller.is_master() || (caller.is_person_device() && caller.mode == TokenMode::Full)
            }
            Touches::NewCatalog => caller.is_master(),
            Touches::Catalog(name) => may_admin_catalog(&caller, &self.store, name)?,
        };
        if !allowed {
            return Err(forbidden(&touches, &caller));
        }
        // R11: applying a plan writes from every catalog it plans, not only
        // the one the call names. Peeked, not taken. Fails closed: a plan a
        // client cannot see (unknown, expired, or taken by an apply in
        // flight) is stale for it, as `apply_sync` would say; the master
        // goes on and `apply_sync` reports it.
        if let AdminCall::ApplySync(a) = &call {
            let written = catalog::sync::plan::registry_catalogs_written(&a.plan_id);
            let Some(written) = written.or_else(|| caller.is_master().then(Default::default))
            else {
                return Err(to_mcp_err(catalog::sync::stale_plan()));
            };
            for name in written {
                if !may_admin_catalog(&caller, &self.store, &name)? {
                    return Err(forbidden(&Touches::Catalog(name), &caller));
                }
            }
        }
        self.prepare_admin_call(&mut call, p.confirm_nonce.as_deref(), &caller)?;
        let value = catalog::admin::run(
            call,
            p.catalog.as_deref(),
            &self.store,
            &self.ssh,
            &self.reg,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&value)
    }

    #[tool(description = "Store a value for a catalog ${NAME} placeholder \
        (global, or per host with host_alias). Master token only. The value \
        is never returned or logged.")]
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

    #[tool(description = "The catalog's layer definitions (layers/*.yaml) \
        and each host's role + active contexts. Read-only. Requires \
        catalog_configure + catalog_load in the app.")]
    pub(super) async fn list_layers(&self) -> Result<CallToolResult, McpError> {
        audit("list_layers", "");
        catalog::ensure_fresh(&self.store).map_err(to_mcp_err)?;
        let out = catalog::list_layers(&self.store).map_err(to_mcp_err)?;
        ok_json_compact(&out)
    }

    #[tool(description = "One host's effective asset set after its role and \
        contexts resolve: provenance, what was excluded and why, every \
        refused asset (scope or collision), every private asset an org \
        host withheld silently, and each held-back catalog with why. \
        Nothing is written. Requires \
        catalog_configure + catalog_load in the app.")]
    pub(super) async fn resolve_preview(
        &self,
        Parameters(p): Parameters<ResolvePreviewParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("resolve_preview", &format!("host_alias={}", p.host_alias));
        catalog::ensure_fresh(&self.store).map_err(to_mcp_err)?;
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
            "refused": res.refused,
            "withheld": res.withheld,
            "held_back": res.held_back,
            "assets": assets,
        }))
    }

    #[tool(description = "Propose a layer split from the last scan, grouping \
        assets by the exact set of hosts they are on: the largest group \
        becomes 'core'; single-host assets come back separately for triage. \
        Read-only.")]
    pub(super) async fn propose_layers(&self) -> Result<CallToolResult, McpError> {
        audit("propose_layers", "");
        catalog::ensure_fresh(&self.store).map_err(to_mcp_err)?;
        let out = catalog::propose::propose_layers(&self.store).map_err(to_mcp_err)?;
        ok_json_compact(&out)
    }

    #[tool(description = "Replace a host's layer assignment: one optional \
        role plus context layers. Edits fleet state only, never catalog \
        files. Requires a configured catalog (in the app, or `fleet-hub \
        catalog set` on a hub). Master token only.")]
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
        catalog::ensure_fresh(&self.store).map_err(to_mcp_err)?;
        let out = catalog::set_host_layers(
            &p.host_alias,
            p.role.as_deref(),
            &p.contexts.iter().map(String::as_str).collect::<Vec<_>>(),
            &self.store,
        )
        .map_err(to_mcp_err)?;
        ok_json(&out)
    }

    #[tool(description = "Choose which harnesses the asset catalog syncs \
        on one host. harnesses null = auto: Claude, plus Codex where a scan \
        finds the codex CLI, ~/.codex/auth.json or ~/.codex/sessions, or \
        where fleet already manages Codex assets. Otherwise a list that must include \"claude\"; \
        [\"claude\"] turns Codex off, and the next sync then removes what \
        fleet installed for Codex there. Edits fleet state only. Master \
        token only.")]
    pub(super) async fn set_host_harnesses(
        &self,
        Parameters(p): Parameters<SetHostHarnessesParams>,
    ) -> Result<CallToolResult, McpError> {
        // Master-only, like set_host_layers (`guard::TOOL_POLICIES`): which
        // harnesses a host serves decides what the NEXT apply_sync writes to
        // — or removes from — its filesystem.
        audit(
            "set_host_harnesses",
            &format!("host_alias={} harnesses={:?}", p.host_alias, p.harnesses),
        );
        let row = catalog::harness_set::set_host_harnesses(
            &p.host_alias,
            p.harnesses.as_deref(),
            &self.store,
        )
        .map_err(to_mcp_err)?;
        ok_json(&row)
    }
}

impl FleetTools {
    /// What `catalog_admin` does between parsing a call and running it:
    /// `apply_sync` passes the same confirm gate as the `apply_sync` tool and
    /// loses the caller's `call_id`; every call but config / configure /
    /// load, the catalog-set calls and apply_sync reads a fresh catalog.
    /// `apply_sync`, like the tool, does not refresh: it applies a plan
    /// already computed and held in the registry, and needs the catalog only
    /// for the post-apply re-scan, which it skips rather than fail the sync
    /// when no catalog is there.
    pub(super) fn prepare_admin_call(
        &self,
        call: &mut catalog::admin::AdminCall,
        confirm_nonce: Option<&str>,
        caller: &Caller,
    ) -> Result<(), McpError> {
        use catalog::admin::AdminCall;
        match call {
            AdminCall::ApplySync(a) => {
                let summary = format!("plan_id={} force_partial={}", a.plan_id, a.force_partial);
                self.confirm_gate("apply_sync", confirm_nonce, &summary, caller)?;
                // A cancellation id means something only in the process that
                // minted it: the caller's, not this one.
                a.call_id = None;
                // No `ensure_fresh`: the plan is already computed, and a
                // failed reload must not refuse a sync the `apply_sync`
                // tool would run.
            }
            // Loading and configuring are what `ensure_fresh` would do; the
            // catalog-set calls read or write the store and load what they
            // add themselves (`list_catalogs` refreshes best-effort in `run`);
            // every other call reads the catalog this process last loaded.
            AdminCall::Config
            | AdminCall::Configure(_)
            | AdminCall::Load(_)
            | AdminCall::ListCatalogs
            | AdminCall::AddCatalog(_)
            | AdminCall::RemoveCatalog(_)
            | AdminCall::AdmitCatalog(_)
            | AdminCall::UnadmitCatalog(_) => {}
            _ => catalog::ensure_fresh(&self.store).map_err(to_mcp_err)?,
        }
        Ok(())
    }
}

/// True when `caller` may touch the catalog named `catalog` (spec:
/// `may_admin_catalog(caller, catalog_id)`): the master, or a live `full`
/// paired client, bound to no org, holding a grant on it (personal: the
/// assets grant, R2). Read from the store on every call, so an un-grant or a
/// revoke holds from the next call on. A per-host token never may: a host's
/// Claude editing what Sync then writes to every host is exactly what the
/// master gate exists to prevent. An unknown name is "no" for a client, so
/// the refusal does not tell it which catalogs exist.
fn may_admin_catalog(
    caller: &Caller,
    store: &std::sync::Mutex<Store>,
    catalog: &str,
) -> Result<bool, McpError> {
    if caller.is_master() {
        return Ok(true);
    }
    let (None, Some(c)) = (&caller.host_alias, &caller.client) else {
        return Ok(false);
    };
    let s = store
        .lock()
        .map_err(|_| mcp_err(codes::E_LOCK, "store mutex poisoned", None))?;
    let ok = if catalog == catalog::catalogs::PERSONAL {
        s.client_is_assets_admin(c.id)
    } else {
        match s.get_catalog_by_name(catalog) {
            Ok(Some(row)) => s.client_may_admin_catalog(c.id, row.id),
            Ok(None) => Ok(false),
            Err(e) => Err(e),
        }
    };
    ok.map_err(|e| to_mcp_err(e.into()))
}

/// The `E_FORBIDDEN` for a call `may_admin_catalog` refused, naming the
/// operator's remedy on the hub.
fn forbidden(touches: &catalog::admin::Touches, caller: &Caller) -> McpError {
    use catalog::admin::Touches;
    let message = match touches {
        Touches::NewCatalog => format!(
            "add_catalog needs the master token ({} refused); on the hub: fleet-hub catalog \
             add <name> <path> --org <org>",
            caller.label()
        ),
        Touches::Catalog(name) if name == catalog::catalogs::PERSONAL => format!(
            "catalog_admin needs the master token or a paired client granted the asset catalog \
             ({} refused); on the hub: fleet-hub client grant <name> assets",
            caller.label()
        ),
        Touches::Catalog(name) => format!(
            "catalog_admin on catalog {name} needs the master token or a paired client granted \
             that catalog ({} refused); on the hub: fleet-hub client grant <name> assets \
             --catalog {name}",
            caller.label()
        ),
        Touches::Nothing => format!(
            "list_catalogs needs the master token or a full paired client bound to no org \
             ({} refused)",
            caller.label()
        ),
    };
    mcp_err("E_FORBIDDEN", message, None)
}

/// Parse an MCP `kind` filter string into a `Kind`, using the same
/// snake_case names the JSON representation already uses elsewhere
/// (`skill`, `agent`, `hook`, `mcp_server`, `plugin_ref`).
fn parse_kind(s: &str) -> Result<catalog::model::Kind, McpError> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .map_err(|_| mcp_err(codes::E_INVALID, format!("unknown asset kind '{s}'"), None))
}
