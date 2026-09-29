//! MCP tools: fleet health, usage, hosts, accounts and provisioning.

use super::*;
use crate::ipc_error::lock;
use crate::service::orgs::OrgScope;

#[tool_router(router = fleet_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Backend health: app and schema version, database \
        readiness, the cached fleet roll-up, per-host reverse-tunnel health \
        (tunnels_flapping: supervised but crash-looping, so the Control API \
        is unreachable from that host), and ESTIMATED token usage and cost \
        (micro-USD) per host and UTC day for 7 days, and trackers (each \
        ok/degraded/failing, failures in a row, last error and success; \
        detection_backlog: suggestions undecided for detection_backlog_days). \
        A per-host token sees its own host's usage and its org's trackers. \
        hub: uptime and last reconcile pass; tunnels_mode none|reverse; \
        peer_links_total. \
        hosts[]: per host disk_home_pct/disk_low, claude_behind, \
        agent_behind, hooks_silent. decide (master only): Jev's last \
        hour, degraded if its breaker is open or >20% failed.")]
    pub(super) async fn fleet_health(
        &self,
        Extension(caller): Extension<Caller>,
    ) -> Result<CallToolResult, McpError> {
        audit("fleet_health", "");
        let mut h = health::health_check(self.reader());
        // On the hub the roll-up comes from a pooled connection, which says
        // nothing about the writer. A poisoned writer fails every write tool
        // (E_LOCK), and `db_ready: false` is how health reports that — so
        // check it here too, without taking the lock.
        if self.store.is_poisoned() {
            h.db_ready = false;
        }
        h.set_tunnels(self.tunnels.health());
        // Host identity & health, task 2: the agents connected right now
        // outrank the stored hello for `agent_version` / `agent_behind`.
        if let Some(reg) = self.ssh.agent_registry() {
            let live: Vec<(String, String)> = reg
                .snapshot()
                .into_iter()
                .map(|a| (a.alias, a.agent_version))
                .collect();
            health::overlay_agents(&mut h.hosts, &live, crate::app_version::get());
        }
        if let Some(host) = caller.host_alias.as_deref() {
            match self.reader().lock() {
                Ok(s) => {
                    health::scope_usage_to_host(&mut h, &s, host);
                    // Work graph M12.4: its org's trackers, its host's backlog.
                    // A scope that cannot be read shows no tracker at all.
                    match caller.org_scope(&s) {
                        Ok(scope) => health::scope_trackers(&mut h, &s, &scope),
                        Err(_) => h.trackers = Default::default(),
                    }
                }
                Err(_) => h.trackers = Default::default(),
            }
        } else if caller.is_scoped() {
            // Work graph M14: a client bound to an org sees its org's
            // trackers only, as a host token does, and every roll-up that
            // sums across hosts is re-derived over the hosts it sees.
            match self.reader().lock() {
                Ok(s) => match caller.org_scope(&s) {
                    Ok(scope) => {
                        health::scope_to_org(&mut h, &s, &scope);
                        health::scope_trackers(&mut h, &s, &scope)
                    }
                    Err(_) => health::blank_rollups(&mut h),
                },
                Err(_) => health::blank_rollups(&mut h),
            }
        }
        // The last reconcile error is the hub's own text about any host —
        // another org's too; a scoped caller gets that it failed, not why.
        if caller.is_scoped() {
            if let Some(r) = h.hub.as_mut().map(|hub| &mut hub.reconcile) {
                if r.last_error.is_some() {
                    r.last_error = Some("reconcile failed (details on the hub)".into());
                }
            }
        }
        // Update design §9: the channel and the targets that need a person,
        // scoped as `update_status` is. A read that fails leaves it out.
        h.updates = self.reader().lock().ok().and_then(|s| {
            crate::service::update::health(
                &s,
                &caller,
                &crate::service::update::trusted_keys(),
                crate::store::now_unix(),
            )
            .ok()
        });
        // The decision envelope is the hub's own business: a per-host token
        // or an org-bound client gets none of it.
        if caller.host_alias.is_some() || caller.is_scoped() {
            h.decide = None;
        }
        // An agent reads it: a tracker's error is the tracker's text.
        h.trackers.fence_errors();
        ok_json_compact(&h)
    }

    #[tool(description = "ESTIMATED token usage and cost per session, host \
        and UTC day, from each session's transcript (collected every \
        usage.interval_secs). Costs are micro-USD from a built-in price \
        table (usage.prices_json), not a bill. total and by_host sum live \
        rows over their lifetime; by_day is the durable daily roll-up \
        (killed sessions included). Sessions sorted by cost, at most 200. A \
        per-host token only sees its own host. by_day.backfill_cost_micros: \
        history a first read booked, apart from live cost.")]
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
            // Work graph M14: a client bound to an org sees its org's sessions
            // and hosts only, as in `fleet_health`; a host outside them
            // answers as an unknown one. (A per-host token is pinned to its
            // host by `usage_scope`.)
            let scope = if caller.is_scoped() && caller.host_alias.is_none() {
                caller.org_scope(&s).map_err(to_mcp_err)?
            } else {
                OrgScope::All
            };
            if let (Some(h), false) = (host.as_deref(), scope.is_all()) {
                if !health::hosts_in_scope(&s, &scope)
                    .iter()
                    .any(|x| x.alias == h)
                {
                    return Err(mcp_err("E_NOTFOUND", format!("no host {h:?}"), None));
                }
            }
            usage::report_on(&s, host.as_deref(), p.since_secs, now, &scope).map_err(to_mcp_err)?
        };
        ok_json_compact(&report)
    }

    // ---- hosts ----

    #[tool(description = "Registered hosts: reachability, claude/tmux \
        versions, linked account.")]
    pub(super) async fn list_hosts(&self) -> Result<CallToolResult, McpError> {
        audit("list_hosts", "");
        ok_json_compact(&hosts::list_hosts(self.reader()).map_err(to_mcp_err)?)
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

    #[tool(description = "Read or replace the fleet's quick replies: the \
        chip row the desktop and phone composers draw above the prompt box, \
        as [{label, text, auto_send}] in order. No arguments reads; `set` \
        replaces the whole list (max 24, [] restores the defaults; not a \
        host token or the operator). \
        Errors: E_INVALID, E_CONFLICT, E_FORBIDDEN.")]
    pub(super) async fn quick_replies(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<QuickRepliesParams>,
    ) -> Result<CallToolResult, McpError> {
        // Chip TEXT is a prompt the operator wrote; the count is the whole
        // audit line, same rule as set_clipboard's body.
        audit(
            "quick_replies",
            &match &p.set {
                Some(entries) => format!("set={}", entries.len()),
                None => "read".to_string(),
            },
        );
        // The chips are a PERSON's buttons, on every screen of the fleet, and
        // an auto-send chip is a prompt one tap away. A per-host token is a
        // host's own Claude and the operator is the UX agent: letting either
        // rewrite the list would let an agent plant a prompt the person then
        // sends without reading. Reads stay open to both.
        if p.set.is_some() && (caller.host_alias.is_some() || caller.is_operator()) {
            return Err(to_mcp_err(IpcError::new(
                codes::E_FORBIDDEN,
                "quick replies are the person's: an agent token may read them, not replace them",
            )));
        }
        let entries = match p.set {
            Some(entries) => {
                quick_replies::replace(&self.store, entries, p.expected).map_err(to_mcp_err)?
            }
            None => quick_replies::list(&self.store).map_err(to_mcp_err)?,
        };
        ok_json_compact(&entries)
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

    #[tool(description = "Fold host `from` into `into` in one transaction: \
        worktrees, fingerprints, dismissals, layers and daily usage move \
        (usage sums), sessions move unless `into` already has the same \
        claude_session_id or tmux_name (those are dropped), then `from` is \
        deleted. For a renamed host (`local` -> `mac`). Master only; may \
        return E_CONFIRM_REQUIRED.")]
    pub(super) async fn merge_host(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<hosts::MergeHostArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "merge_host",
            &format!("from={} into={}", args.from, args.into),
        );
        self.confirm_gate(
            "merge_host",
            args.confirm_nonce.as_deref(),
            &format!("from={} into={}", args.from, args.into),
            &caller,
        )?;
        ok_json(&hosts::merge_host(args, &self.store).map_err(to_mcp_err)?)
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
        per-host status; each host must restart Claude to load it. host: \
        one alias; content_only: skills, CLAUDE.md and hooks only, no token.")]
    pub(super) async fn provision_hosts(
        &self,
        Parameters(p): Parameters<ProvisionHostsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "provision_hosts",
            &format!(
                "rotate={} host={:?} content_only={}",
                p.rotate, p.host, p.content_only
            ),
        );
        let base = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            crate::service::hub::HubBase::read(&s).map_err(to_mcp_err)?
        };
        let res = crate::service::provision::provision_hosts(
            &self.store,
            &self.ssh,
            &self.tunnels,
            &base,
            crate::service::provision::ProvisionScope {
                rotate: p.rotate,
                only_host: p.host,
                content_only: p.content_only,
            },
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
        hub's link (see peer_exchange), updater is fleet-updater's (/update \
        only); fleet-admin tools stay out of a \
        client's reach. Codes are in memory only: a hub restart voids them. \
        org_id binds it to one org (its work and sessions only). Master token \
        only. Returns { url, code, expires_in_s, name, mode, trusted, org_id }.")]
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
        if mode == "peer" && p.org_id.is_some() {
            return Err(mcp_err(
                codes::E_VALIDATE,
                "a peer hub link is never bound to an org; drop org_id",
                None,
            ));
        }
        // `fleet-updater` sends no prompts and belongs to no org.
        if mode == "updater" && (p.trusted || p.org_id.is_some()) {
            return Err(mcp_err(
                codes::E_VALIDATE,
                "an updater token is never trusted or bound to an org; drop trusted / org_id",
                None,
            ));
        }
        audit(
            "pair_client",
            &format!(
                "name={name} mode={mode} trusted={} ttl_s={:?} org_id={:?}",
                p.trusted, p.ttl_s, p.org_id
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
            if let Some(org) = p.org_id {
                if s.get_org(org).map_err(to_mcp_err)?.is_none() {
                    return Err(mcp_err(
                        codes::E_NOTFOUND,
                        format!("org {org} not found"),
                        None,
                    ));
                }
            }
            crate::service::hub::HubBase::read(&s).map_err(to_mcp_err)?
        };
        let req = self
            .guards
            .pairings
            .mint_bound(&name, &mode, p.trusted, p.org_id, ttl);
        ok_json(&serde_json::json!({
            "url": crate::mcp::pair_url(&base.url, &req.code),
            "code": req.code,
            "expires_in_s": ttl.as_secs(),
            "name": req.name,
            "mode": req.mode,
            "trusted": req.trusted,
            "org_id": req.org_id,
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

    // ---- operator settings ----

    #[tool(description = "Operator settings (ticks, GC, playbooks, projects \
        roots, move, usage, reports, work graph), each key's effective value. \
        Master token or a paired device bound to no org.")]
    pub(super) async fn get_settings(
        &self,
        Parameters(p): Parameters<GetSettingsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("get_settings", "");
        let s = lock(&self.store).map_err(to_mcp_err)?;
        if p.describe == Some(true) {
            let described = crate::service::settings::describe(&s);
            drop(s);
            return ok_json_compact(&described);
        }
        let all = crate::service::settings::read_all(&s);
        drop(s);
        ok_json_compact(&all)
    }

    #[tool(description = "Change one get_settings key, validated; E_INVALID \
        otherwise. mcp.*, hub.* and controller.* are refused. Master, or a \
        trusted device. Returns the settings, or with propose the proposal.")]
    pub(super) async fn set_setting(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SetSettingParams>,
    ) -> Result<CallToolResult, McpError> {
        // The key only: a value is not a secret here, but the audit trail
        // keeps values out of every tool alike.
        audit("set_setting", &format!("key={}", p.key.escape_debug()));
        let value = match p.value {
            serde_json::Value::String(v) => v,
            serde_json::Value::Null => return Err(mcp_err(
                codes::E_INVALID,
                "value is required: to go back to the default, set the default get_settings shows",
                None,
            )),
            other => other.to_string(),
        };
        let who = settings_actor(&caller);
        let actor = who.actor();
        if p.propose {
            let row = {
                let s = lock(&self.store).map_err(to_mcp_err)?;
                crate::service::settings_review::propose(
                    &s,
                    &p.key,
                    &value,
                    p.why.as_deref(),
                    actor,
                )
                .map_err(to_mcp_err)?
            };
            tracing::info!(key = %p.key.escape_debug(), id = row.id, "[mcp] proposed a setting");
            return ok_json_compact(&row);
        }
        settings_writer(&caller, &who)?;
        let all = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            crate::service::settings::set_by(&s, &p.key, &value, actor, None)
                .map_err(to_mcp_err)?;
            crate::service::settings::read_all(&s)
        };
        tracing::info!(key = %p.key.escape_debug(), "[mcp] changed a setting");
        ok_json_compact(&all)
    }

    // ---- settings review on a paired device (declarative pages P6) ----

    #[tool(description = "Settings proposals waiting for review, each with \
        the key's value now, and can_write: whether this device may decide.")]
    pub(super) async fn setting_proposals(
        &self,
        Extension(caller): Extension<Caller>,
    ) -> Result<CallToolResult, McpError> {
        audit("setting_proposals", "");
        let who = settings_actor(&caller);
        let proposals = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            crate::service::settings_review::pending(&s).map_err(to_mcp_err)?
        };
        ok_json_compact(&crate::service::settings_review::Pending {
            can_write: settings_writer(&caller, &who).is_ok(),
            proposals,
        })
    }

    #[tool(description = "One setting's writes, newest first: who, before \
        and after, the proposal applied.")]
    pub(super) async fn setting_history(
        &self,
        Parameters(p): Parameters<SettingHistoryParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("setting_history", &format!("key={}", p.key.escape_debug()));
        let rows = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            crate::service::settings_review::history(&s, &p.key, p.limit).map_err(to_mcp_err)?
        };
        ok_json_compact(&rows)
    }

    #[tool(description = "Apply or reject settings proposals by id, each on \
        its own; a trusted device only.")]
    pub(super) async fn decide_setting_proposals(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<DecideSettingProposalsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "decide_setting_proposals",
            &format!("accept={:?} reject={:?}", p.accept, p.reject),
        );
        let who = settings_actor(&caller);
        settings_writer(&caller, &who)?;
        let out = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            crate::service::settings_review::decide_as(&s, &p.accept, &p.reject, who.actor())
                .map_err(to_mcp_err)?
        };
        ok_json_compact(&out)
    }

    #[tool(description = "The settings page specs, data source shapes, \
        resources and page actions a device renders.")]
    pub(super) async fn list_pages(&self) -> Result<CallToolResult, McpError> {
        audit("list_pages", "");
        ok_json_compact(&crate::pages::bundle())
    }

    // ---- workspace repair ----
}

/// Who a settings write or proposal is, as the audit trail records it: the
/// master is an agent over the control API; a paired device is the person
/// holding it — unless it is the UX agent's operator client, an agent.
pub(super) enum SettingsWho {
    ControlApi,
    /// An agent on a paired client (the operator) or a host's own token:
    /// it proposes, never writes. `Access::Person` keeps a host's token away
    /// from these tools; this is the second line.
    Agent(String),
    Device(String),
}

impl SettingsWho {
    pub(super) fn actor(&self) -> crate::service::settings::Actor<'_> {
        use crate::service::settings::Actor;
        match self {
            SettingsWho::ControlApi => Actor::Agent("control API"),
            SettingsWho::Agent(name) => Actor::Agent(name),
            SettingsWho::Device(name) => Actor::PersonVia(name),
        }
    }
}

pub(super) fn settings_actor(caller: &Caller) -> SettingsWho {
    if caller.is_master() {
        return SettingsWho::ControlApi;
    }
    match (&caller.host_alias, &caller.client) {
        (None, Some(c)) if !caller.is_operator() => {
            SettingsWho::Device(format!("client {}", c.name))
        }
        _ => SettingsWho::Agent(caller.label()),
    }
}

/// A write of the fleet's settings (a direct `set_setting`, or deciding a
/// proposal) is the master's, or a trusted device's (declarative pages P6):
/// trust is the operator vouching that the device is a person's own
/// (`fleet-hub client trust`). An agent's client proposes instead.
pub(super) fn settings_writer(caller: &Caller, who: &SettingsWho) -> Result<(), McpError> {
    match who {
        SettingsWho::ControlApi => Ok(()),
        SettingsWho::Agent(_) => Err(mcp_err(
            "E_FORBIDDEN",
            "an agent proposes a settings change (set_setting with propose: true); a person applies it",
            None,
        )),
        SettingsWho::Device(_) if caller.is_trusted_client() && caller.mode == TokenMode::Full => {
            Ok(())
        }
        SettingsWho::Device(_) => Err(mcp_err(
            "E_FORBIDDEN",
            format!(
                "this device may read the fleet's settings and propose a change; to change them, \
                 the hub's operator trusts it: fleet-hub client trust {}",
                caller.client.as_ref().map_or("<name>", |c| c.name.as_str())
            ),
            None,
        )),
    }
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
