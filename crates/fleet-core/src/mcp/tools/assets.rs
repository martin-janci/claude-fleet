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
    pub(super) async fn list_assets(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<ListAssetsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "list_assets",
            &format!("all_catalogs={} caller={}", p.all_catalogs, caller.label()),
        );
        catalog::ensure_fresh_blocking(&self.store)
            .await
            .map_err(to_mcp_err)?;
        // Fix round 1: personal-only unless asked — an older desktop keys
        // its list by name and must never get a second catalog's asset.
        let scope = if p.all_catalogs {
            listing_scope(&caller)
        } else {
            catalog::ListingScope::Personal
        };
        ok_json_compact(&catalog::list_assets_in(&self.store, scope).map_err(to_mcp_err)?)
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
        catalog::ensure_fresh_blocking(&self.store)
            .await
            .map_err(to_mcp_err)?;
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
        // PF7: an import writes the checkout; it waits for an apply in flight.
        let _busy = catalog::changesets::authoring_lock().await;
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
        catalog::ensure_fresh_blocking(&self.store)
            .await
            .map_err(to_mcp_err)?;
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
        one, add/remove_catalog the master.")]
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
            Ok((_, Touches::MasterOnly(None))) => "(new)",
            Ok((_, Touches::MasterOnly(Some(name)))) => name.as_str(),
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
        // R22 (M-d): a named catalog's row is read once, here; the gate and
        // `run` both use it. The master naming an unknown catalog for a
        // per-catalog call is told so now — it must never fall through to
        // personal; a client gets the ungranted refusal below, as before.
        // (`admit`/`unadmit_catalog` name theirs in args and keep their own
        // not-found answer.)
        let target: Option<crate::store::CatalogRow> = match &touches {
            Touches::Catalog(name) if name != catalog::catalogs::PERSONAL => {
                let row = lookup_catalog(&self.store, name)?;
                if row.is_none() && caller.is_master() && call.is_per_catalog() {
                    return Err(mcp_err(
                        codes::E_NOTFOUND,
                        format!("no catalog named {name}; list them with list_catalogs"),
                        None,
                    ));
                }
                row
            }
            _ => None,
        };
        let allowed = match &touches {
            // Every catalog's paths, remotes and grantees, across orgs.
            Touches::Nothing => may_list_every_catalog(&caller),
            Touches::MasterOnly(_) => caller.is_master(),
            Touches::Catalog(name) => {
                may_admin_catalog_row(&caller, &self.store, name, target.as_ref())?
            }
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
                    // Final review M-f: no grant can be made on a catalog
                    // removed since the plan; the remedy is a new plan.
                    if catalog_is_gone(&self.store, &name)? {
                        return Err(mcp_err(
                            codes::E_SYNC_PLAN_STALE,
                            format!(
                                "catalog {name} no longer exists; this plan writes from it, so \
                                 re-plan (plan_sync)"
                            ),
                            None,
                        ));
                    }
                    return Err(forbidden(&Touches::Catalog(name), &caller));
                }
            }
        }
        // Final review M2: importing into an org catalog reads the source
        // host's whole Claude config. A host of that org (bound to it, or
        // admitted to the catalog) is the org's to read; any other host —
        // `local` included — also needs the personal grant, as it did
        // before import became per catalog (R21).
        if let (AdminCall::ImportHost(a), Some(row)) = (&call, target.as_ref()) {
            if row.org_id.is_some()
                && !host_serves_catalog(&self.store, &a.host_alias, row)?
                && !may_admin_catalog(&caller, &self.store, catalog::catalogs::PERSONAL)?
            {
                return Err(mcp_err(
                    "E_FORBIDDEN",
                    format!(
                        "import_host from {}, a host outside catalog {}'s org, needs the master \
                         token or a paired client granted catalog personal too ({} refused); on \
                         the hub: fleet-hub client grant <name> assets",
                        a.host_alias,
                        row.name,
                        caller.label()
                    ),
                    None,
                ));
            }
        }
        self.prepare_admin_call(&mut call, p.confirm_nonce.as_deref(), &caller)?;
        let value = catalog::admin::run(call, target.as_ref(), &self.store, &self.ssh, &self.reg)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&value)
    }

    #[tool(description = "Changeset cards that adopt, sync and fix assets: \
        list (one in full with id), propose (rebuild from the last scan), \
        propose_layer (a layer change as a card), apply (positions picks \
        items; a drift card takes one), undo (the latest applied card per \
        catalog), dismiss, reject_item. Mutating actions need a grant on \
        every catalog the card names.")]
    pub(super) async fn changesets(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<ChangesetsParams>,
    ) -> Result<CallToolResult, McpError> {
        use catalog::changesets as cs;
        audit(
            "changesets",
            &format!(
                "action={} id={} caller={}",
                p.action,
                p.id.map_or_else(|| "-".to_string(), |i| i.to_string()),
                caller.label()
            ),
        );
        let personal = catalog::catalogs::PERSONAL;
        // PF15 / fix round 1: every card names catalogs and hosts across
        // orgs, so the tool is the master's or a person's own unbound full
        // device, as `list_catalogs` — checked before any card is read or
        // the lock awaited, so an org-bound client learns nothing, not even
        // which card ids exist.
        if !(caller.is_master() || (caller.is_person_device() && caller.mode == TokenMode::Full)) {
            return Err(changesets_forbidden(&p.action, None, &caller));
        }
        match p.action.as_str() {
            "list" => match p.id {
                Some(id) => ok_json(&cs::get(id, &self.store).map_err(to_mcp_err)?),
                None => ok_json_compact(&cs::list(&self.store).map_err(to_mcp_err)?),
            },
            "propose" => {
                if !may_admin_catalog(&caller, &self.store, personal)? {
                    return Err(changesets_forbidden("propose", Some(personal), &caller));
                }
                catalog::ensure_fresh_blocking(&self.store)
                    .await
                    .map_err(to_mcp_err)?;
                ok_json_compact(&cs::propose(&self.store).await.map_err(to_mcp_err)?)
            }
            "propose_layer" => {
                let change = p.change.ok_or_else(|| {
                    mcp_err(codes::E_INVALID, "propose_layer needs a change", None)
                })?;
                // Assets M6 (R5): a grant on the catalog the change names
                // (an unknown name is "no" for a client, R22).
                let name = change.catalog().to_string();
                if !may_admin_catalog(&caller, &self.store, &name)? {
                    return Err(changesets_forbidden("propose_layer", Some(&name), &caller));
                }
                ok_json(
                    &cs::propose_layer(change, &self.store)
                        .await
                        .map_err(to_mcp_err)?,
                )
            }
            "apply" | "undo" | "dismiss" | "reject_item" => {
                let id = p.id.ok_or_else(|| {
                    mcp_err(
                        codes::E_INVALID,
                        format!("changesets {} needs an id", p.action),
                        None,
                    )
                })?;
                // Lock-free pre-check (fix round 1): a client holding no
                // grant at all can act on no card; refused before the lock
                // and the card, the same way for every id.
                if !self.holds_any_catalog_grant(&caller)? {
                    return Err(changesets_forbidden(&p.action, None, &caller));
                }
                // The card is read, the items the action runs selected and
                // the grants checked under the lock the action runs under,
                // so a refresh in between cannot change what was authorized.
                let busy = cs::authoring_lock().await;
                let (card, items) = match cs::card(id, &self.store) {
                    Ok(found) => found,
                    // R22: a client is not told which cards exist.
                    Err(e) if e.code == codes::E_NOTFOUND && !caller.is_master() => {
                        return Err(changesets_forbidden(&p.action, None, &caller));
                    }
                    Err(e) => return Err(to_mcp_err(e)),
                };
                let view = match p.action.as_str() {
                    "apply" => {
                        // Exactly the items `apply` runs — the same
                        // function, under the same lock — so what the grants
                        // and the confirm gate see is what runs. Fix round
                        // 1: with no `positions` apply runs every pending
                        // item, so `writes_hosts` is never read from the raw
                        // parameter.
                        let selected =
                            match cs::apply::applicable(&card, &items, p.positions.as_deref()) {
                                Ok(selected) => selected,
                                Err(e) => {
                                    // Nothing runs; why (a closed card, a
                                    // bad position) is told only to a caller
                                    // that may act on the card as a whole.
                                    let all: Vec<_> = items.iter().collect();
                                    let hosts = cs::writes_hosts(&card, &all);
                                    self.check_card_grants(&caller, "apply", &all, hosts)?;
                                    return Err(to_mcp_err(e));
                                }
                            };
                        let writes_hosts = cs::writes_hosts(&card, &selected);
                        self.check_card_grants(&caller, "apply", &selected, writes_hosts)?;
                        if writes_hosts {
                            self.confirm_gate(
                                "apply_sync",
                                p.confirm_nonce.as_deref(),
                                &confirm_summary(&card, &selected),
                                &caller,
                            )?;
                        }
                        // Under APPLY_LOCK: `ensure_fresh` is synchronous
                        // and never takes it, so this cannot deadlock; a
                        // slow reload only delays other card actions (the
                        // tick `try_lock`s and skips).
                        catalog::ensure_fresh_blocking(&self.store)
                            .await
                            .map_err(to_mcp_err)?;
                        let args = cs::apply::ApplyArgs {
                            id,
                            positions: p.positions,
                        };
                        cs::apply::apply_held(&busy, args, &self.store, &self.ssh).await
                    }
                    action => {
                        let all: Vec<&crate::store::ChangesetItemRow> = items.iter().collect();
                        self.check_card_grants(&caller, action, &all, false)?;
                        match action {
                            "undo" => cs::undo::undo_held(&busy, id, &self.store).await,
                            "dismiss" => cs::undo::dismiss_held(&busy, id, &self.store).await,
                            _ => {
                                let positions = p.positions.ok_or_else(|| {
                                    mcp_err(codes::E_INVALID, "reject_item needs positions", None)
                                })?;
                                cs::undo::reject_items_held(&busy, id, &positions, &self.store)
                                    .await
                            }
                        }
                    }
                };
                ok_json(&view.map_err(to_mcp_err)?)
            }
            other => Err(mcp_err(
                codes::E_INVALID,
                format!(
                    "unknown changesets action {other}: \
                     list|propose|propose_layer|apply|undo|dismiss|reject_item"
                ),
                None,
            )),
        }
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
        catalog::ensure_fresh_blocking(&self.store)
            .await
            .map_err(to_mcp_err)?;
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
        catalog::ensure_fresh_blocking(&self.store)
            .await
            .map_err(to_mcp_err)?;
        let res = catalog::resolve_preview(&p.host_alias, &self.store).map_err(to_mcp_err)?;
        // `Resolution` is a full `Catalog` (every asset body and resource's
        // base64 bytes), which blows past MCP token caps on a fleet-sized
        // catalog; answer the summary view (`ResolutionView`), as
        // `list_assets` does.
        ok_json(&catalog::resolve::ResolutionView::of(&res))
    }

    #[tool(description = "Propose a layer split from the last scan, grouping \
        assets by the exact set of hosts they are on: the largest group \
        becomes 'core'; single-host assets come back separately for triage. \
        Read-only.")]
    pub(super) async fn propose_layers(&self) -> Result<CallToolResult, McpError> {
        audit("propose_layers", "");
        catalog::ensure_fresh_blocking(&self.store)
            .await
            .map_err(to_mcp_err)?;
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
        // PF7: a host's layers wait for an apply in flight (a failed apply
        // restores its snapshot over them).
        let _busy = catalog::changesets::authoring_lock().await;
        catalog::ensure_fresh_blocking(&self.store)
            .await
            .map_err(to_mcp_err)?;
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

    /// The `changesets` pre-check that needs no card (fix round 1): the
    /// master, or a client holding a grant on some catalog (the personal
    /// one included, however it was made). Read live, without the lock.
    fn holds_any_catalog_grant(&self, caller: &Caller) -> Result<bool, McpError> {
        if caller.is_master() {
            return Ok(true);
        }
        let (None, Some(c)) = (&caller.host_alias, &caller.client) else {
            return Ok(false);
        };
        let s = self
            .store
            .lock()
            .map_err(|_| mcp_err(codes::E_LOCK, "store mutex poisoned", None))?;
        let any = s
            .client_is_assets_admin(c.id)
            .and_then(|personal| Ok(personal || s.client_has_any_catalog_grant(c.id)?))
            .map_err(|e| to_mcp_err(e.into()))?;
        Ok(any)
    }

    /// R25: a grant on every catalog `items` name — for an apply, exactly
    /// the items it runs; for undo / dismiss / reject_item, the whole card
    /// — and personal too when one of them names no catalog (a hide) or
    /// the apply writes hosts. A per-host token is refused at the central
    /// gate first (`NOT_FOR_HOST_TOKENS`) and would never pass here either
    /// (`may_admin_catalog_row`). R22: a catalog that no longer exists is
    /// `E_NOTFOUND` for the master and the ungranted refusal for a client.
    fn check_card_grants(
        &self,
        caller: &Caller,
        action: &str,
        items: &[&crate::store::ChangesetItemRow],
        writes_hosts: bool,
    ) -> Result<(), McpError> {
        let ids: std::collections::BTreeSet<i64> =
            items.iter().filter_map(|i| i.catalog_id).collect();
        let rows: Vec<(i64, Option<crate::store::CatalogRow>)> = {
            let s = self
                .store
                .lock()
                .map_err(|_| mcp_err(codes::E_LOCK, "store mutex poisoned", None))?;
            ids.iter()
                .map(|id| s.get_catalog(*id).map(|row| (*id, row)))
                .collect::<Result<_, _>>()
                .map_err(|e| to_mcp_err(e.into()))?
        };
        let personal = catalog::catalogs::PERSONAL;
        let fleet_wide = writes_hosts || items.iter().any(|i| i.catalog_id.is_none());
        if fleet_wide && !may_admin_catalog(caller, &self.store, personal)? {
            return Err(changesets_forbidden(action, Some(personal), caller));
        }
        for (id, row) in &rows {
            let Some(row) = row else {
                if caller.is_master() {
                    return Err(mcp_err(
                        codes::E_NOTFOUND,
                        format!("the card names catalog {id}, which no longer exists"),
                        None,
                    ));
                }
                return Err(changesets_forbidden(action, None, caller));
            };
            let name = if row.org_id.is_none() {
                personal
            } else {
                row.name.as_str()
            };
            if !may_admin_catalog_row(caller, &self.store, name, Some(row))? {
                return Err(changesets_forbidden(action, Some(name), caller));
            }
        }
        Ok(())
    }
}

/// Whether `host` takes org catalog `row`: bound to its org, or (a host
/// with no org) admitted to it.
fn host_serves_catalog(
    store: &std::sync::Mutex<Store>,
    host: &str,
    row: &crate::store::CatalogRow,
) -> Result<bool, McpError> {
    let s = store
        .lock()
        .map_err(|_| mcp_err(codes::E_LOCK, "store mutex poisoned", None))?;
    if row.org_id.is_some() && s.host_org(host).map_err(to_mcp_err)? == row.org_id {
        return Ok(true);
    }
    let admitted = s.host_admissions(host).map_err(|e| to_mcp_err(e.into()))?;
    Ok(admitted.contains(&row.id))
}

/// What a person approves for a host-writing card apply (fix round 1): the
/// card, its kind and each selected item with the first 12 characters of
/// its gap or content hash — e.g. `changeset 12 (drift): restore #1
/// skill/w on oci [3f2a9c01b7de]`. An approval is matched on this text, so
/// it covers this card's kind, positions, hosts, assets and content: a
/// card re-picked since, or refreshed to new content (even with the same
/// assets and hosts — final review, Task 9), asks again.
fn confirm_summary(
    card: &crate::store::ChangesetRow,
    selected: &[&crate::store::ChangesetItemRow],
) -> String {
    let items: Vec<String> = selected
        .iter()
        .map(|i| {
            let params = catalog::changesets::ItemParams::parse(i.params.as_deref());
            let hash = params
                .hash
                .as_deref()
                .map(|h| format!(" [{}]", h.get(..12).unwrap_or(h)))
                .unwrap_or_default();
            match i.action.as_str() {
                "sync" => format!(
                    "sync #{} {} to {} ({}){hash}",
                    i.position,
                    i.grp,
                    i.name,
                    params.assets.join(", ")
                ),
                action => {
                    let on = params.host.map(|h| format!(" on {h}")).unwrap_or_default();
                    format!("{action} #{} {}/{}{on}{hash}", i.position, i.kind, i.name)
                }
            }
        })
        .collect();
    format!(
        "changeset {} ({}): {}",
        card.id,
        card.kind,
        items.join("; ")
    )
}

/// The `E_FORBIDDEN` for a `changesets` action without the grant it needs,
/// naming the catalog and the operator's remedy on the hub. `None`: the
/// unnamed refusal — a caller that is not the master or a person's own
/// unbound full device, a client holding no grant at all, or a card or
/// catalog that does not exist (R22). It names no catalog and no card, so
/// it reads the same whatever id was asked for.
fn changesets_forbidden(action: &str, catalog: Option<&str>, caller: &Caller) -> McpError {
    let message = match catalog {
        Some(name) => {
            let grant = if name == catalog::catalogs::PERSONAL {
                "fleet-hub client grant <name> assets".to_string()
            } else {
                format!("fleet-hub client grant <name> assets --catalog {name}")
            };
            format!(
                "changesets {action} needs the master token or a paired client granted \
                 catalog {name} ({} refused); on the hub: {grant}",
                caller.label()
            )
        }
        None => format!(
            "changesets {action} needs the master token or a full paired client bound to no \
             org, granted every catalog the card names ({} refused)",
            caller.label()
        ),
    };
    mcp_err("E_FORBIDDEN", message, None)
}

/// Assets M5 (R11): every catalog's assets for the master and a person's own
/// unbound full device — the `list_catalogs` audience (`Touches::Nothing` in
/// `catalog_admin`); the personal catalog only, as before M5, for a per-host
/// token and an org-bound, readonly or single-purpose client, so none of
/// them learns an org catalog's name or assets from the listing.
pub(crate) fn listing_scope(caller: &Caller) -> catalog::ListingScope {
    if may_list_every_catalog(caller) {
        catalog::ListingScope::Every
    } else {
        catalog::ListingScope::Personal
    }
}

/// Who may see every catalog — its name, paths, remotes, grantees and
/// assets, across orgs: the master, or a person's own unbound full device.
/// The one rule behind both `list_catalogs` (`Touches::Nothing` in
/// `catalog_admin`) and [`listing_scope`], so the two audiences cannot
/// drift apart (final review, minor 3).
pub(crate) fn may_list_every_catalog(caller: &Caller) -> bool {
    caller.is_master() || (caller.is_person_device() && caller.mode == TokenMode::Full)
}

/// True when `caller` may touch the catalog `name` whose row the caller has
/// already read (`row`; `None` for personal or an unknown name) — spec:
/// `may_admin_catalog(caller, catalog_id)`. The master, or a live `full`
/// paired client, bound to no org, holding a grant on it (personal: the
/// assets grant, R2). Read from the store on every call, so an un-grant or
/// a revoke holds from the next call on. A per-host token never may: a
/// host's Claude editing what Sync then writes to every host is exactly
/// what the master gate exists to prevent. An unknown name is "no" for a
/// client, so the refusal does not tell it which catalogs exist.
fn may_admin_catalog_row(
    caller: &Caller,
    store: &std::sync::Mutex<Store>,
    name: &str,
    row: Option<&crate::store::CatalogRow>,
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
    let ok = if name == catalog::catalogs::PERSONAL {
        s.client_is_assets_admin(c.id)
    } else {
        match row {
            Some(r) => s.client_may_admin_catalog(c.id, r.id),
            None => Ok(false),
        }
    };
    ok.map_err(|e| to_mcp_err(e.into()))
}

/// [`may_admin_catalog_row`] by name, for a caller that has not read the row.
fn may_admin_catalog(
    caller: &Caller,
    store: &std::sync::Mutex<Store>,
    name: &str,
) -> Result<bool, McpError> {
    if caller.is_master() || name == catalog::catalogs::PERSONAL {
        return may_admin_catalog_row(caller, store, name, None);
    }
    let row = lookup_catalog(store, name)?;
    may_admin_catalog_row(caller, store, name, row.as_ref())
}

/// The catalog row named `name`, if any.
fn lookup_catalog(
    store: &std::sync::Mutex<Store>,
    name: &str,
) -> Result<Option<crate::store::CatalogRow>, McpError> {
    let s = store
        .lock()
        .map_err(|_| mcp_err(codes::E_LOCK, "store mutex poisoned", None))?;
    s.get_catalog_by_name(name)
        .map_err(|e| to_mcp_err(e.into()))
}

/// Whether no catalog is configured under `name` any more (`personal` is
/// never gone: its row cannot be removed).
fn catalog_is_gone(store: &std::sync::Mutex<Store>, name: &str) -> Result<bool, McpError> {
    if name == catalog::catalogs::PERSONAL {
        return Ok(false);
    }
    let s = store
        .lock()
        .map_err(|_| mcp_err(codes::E_LOCK, "store mutex poisoned", None))?;
    s.get_catalog_by_name(name)
        .map(|row| row.is_none())
        .map_err(|e| to_mcp_err(e.into()))
}

/// The `E_FORBIDDEN` for a call `may_admin_catalog` refused, naming the
/// operator's remedy on the hub.
fn forbidden(touches: &catalog::admin::Touches, caller: &Caller) -> McpError {
    use catalog::admin::Touches;
    let message = match touches {
        Touches::MasterOnly(None) => format!(
            "add_catalog needs the master token ({} refused); on the hub: fleet-hub catalog \
             add <name> <path> --org <org>",
            caller.label()
        ),
        Touches::MasterOnly(Some(name)) => format!(
            "remove_catalog needs the master token ({} refused): it drops every grant on \
             {name}, and only the master can add it back; on the hub: fleet-hub catalog \
             remove {name}",
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
/// (`skill`, `agent`, `hook`, `mcp_server`, `plugin_ref`, `command`).
fn parse_kind(s: &str) -> Result<catalog::model::Kind, McpError> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .map_err(|_| mcp_err(codes::E_INVALID, format!("unknown asset kind '{s}'"), None))
}

#[cfg(test)]
mod confirm_summary_tests {
    use super::confirm_summary;
    use crate::store::{ChangesetItemRow, ChangesetRow};

    fn card() -> ChangesetRow {
        ChangesetRow {
            id: 7,
            kind: "rollout".into(),
            summary: "Roll out core to oci".into(),
            state: "proposed".into(),
            created_at: 1,
            applied_at: None,
            commits: None,
            layers_snapshot: None,
            error: None,
            withdrawn_at: None,
        }
    }

    fn sync(hash: &str) -> ChangesetItemRow {
        ChangesetItemRow {
            changeset_id: 7,
            position: 0,
            grp: "core".into(),
            catalog_id: Some(1),
            kind: "host".into(),
            name: "oci".into(),
            action: "sync".into(),
            params: Some(format!(
                r#"{{"layer":"core","assets":["skill/w"],"hash":"{hash}"}}"#
            )),
            decider: "rule".into(),
            state: "pending".into(),
            decided_at: None,
            outcome: None,
        }
    }

    /// Final review (Task 9 park): a refresh that changes only an item's
    /// gap/content hash changes what the person approves, so an earlier
    /// approval never covers the new content.
    #[test]
    fn the_summary_is_bound_to_each_items_hash() {
        let a = sync("0123456789abcdef0123");
        let b = sync("fedcba9876543210fedc");
        let (sa, sb) = (
            confirm_summary(&card(), &[&a]),
            confirm_summary(&card(), &[&b]),
        );
        assert_ne!(sa, sb);
        assert_eq!(
            sa,
            "changeset 7 (rollout): sync #0 core to oci (skill/w) [0123456789ab]"
        );
    }
}
