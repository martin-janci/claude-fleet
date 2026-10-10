//! MCP tools: fleet health, usage, hosts, accounts and provisioning.

use super::*;
use crate::ipc_error::lock;

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
        hour, degraded if its breaker is open or >20% failed. loops[]: \
        each background job's last and next run and result \
        (ok/error/paused); automation_paused.")]
    pub(super) async fn fleet_health(
        &self,
        Extension(caller): Extension<Caller>,
    ) -> Result<CallToolResult, McpError> {
        audit("fleet_health", "");
        // One read, one pass: the roll-up is built for the caller's scope
        // under a single lock, not fleet-wide and then re-derived.
        let tunnels = self.tunnels.health();
        let mut h = match self.reader().lock() {
            Ok(s) => {
                let view = if let Some(host) = caller.host_alias.as_deref() {
                    // Its own host's telemetry and spend; the host counts
                    // stay fleet-wide, as the usage note says. Work graph
                    // M12.4: its org's trackers, its host's backlog — none
                    // when the scope cannot be read.
                    health::HealthView::Host {
                        alias: host.to_string(),
                        trackers: caller.org_scope(&s).ok(),
                    }
                // This is the org boundary, not a privacy fence: it asks which org a caller is
                // BOUND to, so as to pick that org's HealthView. The person's own view is the
                // next arm (`HealthView::Person`).
                } else if caller.is_scoped() {
                    // Work graph M14: a client bound to an org sees its org's
                    // trackers only, as a host token does, and every roll-up
                    // that sums across hosts is over the hosts it sees.
                    //
                    // Multi-user M1 (T10): with the caller's WHOLE scope, not
                    // its org half. An org-bound client is somebody's device
                    // too, and `HealthView::Org`'s session roll-ups were summed
                    // over every session in the org — a colleague's private
                    // ones included. The org half it still asks is
                    // `ViewScope::org`, which `org_scope` built here before.
                    match caller.view_scope(&s) {
                        Ok(view) => health::HealthView::Org(view),
                        Err(_) => health::HealthView::Blank,
                    }
                } else if caller.client.is_some() {
                    // Multi-user M1 (T8d): a paired client bound to no org is
                    // a PERSON's own device, and it used to fall through to
                    // `Fleet` because `is_scoped()` is false for it — the
                    // predicate spec §3.6 names as a leak class. Its session
                    // roll-ups and per-host spend are now its own.
                    match caller.view_scope(&s) {
                        Ok(view) => health::HealthView::Person(view),
                        Err(_) => health::HealthView::Blank,
                    }
                } else {
                    // The master token and the standalone desktop: §4.5's hub
                    // operator, deliberately not fenced.
                    health::HealthView::Fleet
                };
                health::health_for(&s, &view, tunnels)
            }
            Err(_) => {
                let mut h = health::unready_health();
                // An org-bound client is told nothing of another org's
                // hosts, tunnels included.
                // This is the org boundary, not a privacy fence: an org-bound client is told
                // nothing of another org's hosts, tunnels included.
                if caller.host_alias.is_some() || !caller.is_scoped() {
                    h.set_tunnels(tunnels);
                }
                h
            }
        };
        // On the hub the roll-up comes from a pooled connection, which says
        // nothing about the writer. A poisoned writer fails every write tool
        // (E_LOCK), and `db_ready: false` is how health reports that — so
        // check it here too, without taking the lock.
        if self.store.is_poisoned() {
            h.db_ready = false;
        }
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
        // The last reconcile error is the hub's own text about any host —
        // another org's too; a scoped caller gets that it failed, not why.
        // This is the org boundary, not a privacy fence: the reconcile error is the hub's own
        // text about any host, another ORG's included.
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
        // This is the org boundary, not a privacy fence: whether this caller is inside an org
        // boundary at all, and the decision envelope is outside every one of them.
        // So is a background loop's error (redesign 8.1), the hub's own
        // text about any host or tracker: such a caller gets that it failed.
        if caller.host_alias.is_some() || caller.is_scoped() {
            h.decide = None;
            for l in &mut h.loops {
                if l.last_error.is_some() {
                    l.last_error = Some("failed (details on the hub)".into());
                }
            }
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
            //
            // Multi-user M1 (T7): and WHOSE sessions, which `is_scoped()`
            // cannot ask — it is false for the master and for every paired
            // client bound to no org, so the old `if caller.is_scoped()` here
            // handed a second person `OrgScope::All` and `report_on` returned
            // every private session's tmux name, friendly name, model and
            // per-session spend. `Caller::view_scope` is the predicate that
            // means "restricted"; its `.org` is the org half this host check
            // still asks.
            let view = caller.view_scope(&s).map_err(to_mcp_err)?;
            let scope = view.org.clone();
            // This is the org boundary, not a privacy fence: whether a named HOST exists for
            // this caller, and a host belongs to an org. The report's own session rows are
            // fenced by `view` (the whole scope), inside `report_on`.
            if let (Some(h), false) = (host.as_deref(), scope.is_all()) {
                if !health::hosts_in_scope(&s, &scope)
                    .iter()
                    .any(|x| x.alias == h)
                {
                    return Err(mcp_err("E_NOTFOUND", format!("no host {h:?}"), None));
                }
            }
            usage::report_on(&s, host.as_deref(), p.since_secs, now, &view).map_err(to_mcp_err)?
        };
        ok_json_compact(&report)
    }

    // ---- hosts ----

    #[tool(description = "Registered hosts: reachability, claude/tmux \
        versions, linked account. unclaimed_sessions is how many sessions \
        on that host nobody owns, served to a one-person fleet and to who \
        administers the host: null means not told, 0 means none.")]
    pub(super) async fn list_hosts(
        &self,
        Extension(caller): Extension<Caller>,
    ) -> Result<CallToolResult, McpError> {
        audit("list_hosts", "");
        // Scope and rows off the same handle: `list_hosts` takes the lock
        // itself, so the scope is built here against the same reader.
        let scope = {
            let s = lock(self.reader()).map_err(to_mcp_err)?;
            caller.view_scope(&s).map_err(to_mcp_err)?
        };
        ok_json_compact(&hosts::list_hosts(self.reader(), &scope).map_err(to_mcp_err)?)
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

    #[tool(description = "Each Claude account's latest plan usage: 5-hour \
        and weekly utilization with reset times, status, fetched_at. Never \
        fetches.")]
    pub(super) async fn account_usage(&self) -> Result<CallToolResult, McpError> {
        audit("account_usage", "");
        ok_json_compact(
            &crate::service::account_usage_poll::served_account_usage(self.reader())
                .map_err(to_mcp_err)?,
        )
    }

    #[tool(description = "Whether starting or switching a session on \
        host_alias under login `profile` (a profile name; omitted or \"\" for \
        the host's own) crosses accounts.pause_at, and the login on that host \
        with the most headroom: {pause_at_pct, chosen?, over, suggestion?, \
        logins:[{profile?, account_uuid, used_pct?}]}. From the usage \
        account_usage serves; never fetches. Errors: E_NOTFOUND (no such \
        host), E_INVALID.")]
    pub(super) async fn check_account_headroom(
        &self,
        Parameters(p): Parameters<crate::service::account_limits::CheckAccountHeadroomArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit("check_account_headroom", &p.host_alias);
        let now = crate::store::now_unix();
        ok_json_compact(
            &crate::service::account_limits::served_check_account_headroom(&p, self.reader(), now)
                .map_err(to_mcp_err)?,
        )
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
        // The list is fleet-wide (`ui.quick_replies`), so it is the hub
        // owner's, like every fleet-wide setting since M1: a second
        // person's device or an org-bound one reads it, never replaces it
        // (review r04 F4).
        if p.set.is_some()
            && !(caller.is_master() || (caller.is_personal_owner && caller.is_person_device()))
        {
            return Err(to_mcp_err(IpcError::new(
                codes::E_FORBIDDEN,
                "quick replies are the hub owner's: another person's or an org's device \
                 may read them, not replace them",
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
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<hosts::AddHostArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit("add_host", &add_host_audit_detail(&args));
        owner_device_admin(&caller, "adding a host")?;
        let row = hosts::add_host(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Install fleet-agent on a host this hub reaches \
        over SSH and move the host onto it. Returns the job at once (see \
        agent_installs): target, download (SHA256SUMS-checked), start, \
        connect (no hello in 120 s: back on SSH). Hub only.")]
    pub(super) async fn install_agent(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<agent_install::InstallAgentArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit("install_agent", &format!("alias={}", args.alias));
        owner_device_admin(&caller, "installing fleet-agent")?;
        let ssh: Arc<dyn crate::ssh::SshExec> = self.ssh.clone();
        let row = agent_install::start(
            Arc::clone(&self.store),
            ssh,
            self.ssh.agent_registry().cloned(),
            args,
        )
        .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "fleet-agent install jobs, newest first: state, \
        step, detail.")]
    pub(super) async fn agent_installs(
        &self,
        Parameters(args): Parameters<agent_install::AgentInstallsArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit("agent_installs", "");
        ok_json_compact(
            &agent_install::list(&self.store, args.alias.as_deref()).map_err(to_mcp_err)?,
        )
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
        org_id binds it to one org (its work and sessions only). person names \
        whose device it is; the default is this hub's owner, and its sessions \
        are private to that person. Master token \
        only. Returns { url, code, expires_in_s, name, mode, trusted, org_id, \
        person }.")]
    // The master-only gate is `enforce_admin` in `call_tool` (`pair_client`
    // is in `guard::ADMIN_TOOLS`), so no caller extractor is needed here.
    pub(super) async fn pair_client(
        &self,
        Parameters(p): Parameters<PairClientParams>,
    ) -> Result<CallToolResult, McpError> {
        ok_json(&self.mint_pairing(p, "pair_client")?)
    }

    /// Mint a pairing code: `pair_client`'s body, shared with `org_admin {
    /// pair_device }` (org administration phase B), which narrows the modes
    /// before it gets here. `tool` names the audit line.
    pub(super) fn mint_pairing(
        &self,
        p: PairClientParams,
        tool: &str,
    ) -> Result<serde_json::Value, McpError> {
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
        // A hub link and an updater token are nobody's device: binding
        // either to a person would make it a reader of that person's private
        // sessions. `Store::set_client_person` refuses both at the write, and
        // this is the same rule at the mint, where the operator sees it.
        if p.person.is_some() && (mode == "peer" || mode == "updater") {
            return Err(mcp_err(
                codes::E_VALIDATE,
                format!("a {mode} token is not a person's device; drop person"),
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
        // The person's NAME is validated here, at the mint, rather than at
        // redemption: the row is created when the phone redeems the code
        // (see `pairing::MintRequest::person`), and an operator must read a
        // bad name at their own terminal, not minutes later on a device.
        let person = p
            .person
            .as_deref()
            .map(crate::store::validate_person_name)
            .transpose()
            .map_err(to_mcp_err)?;
        audit(
            tool,
            &format!(
                "name={name} mode={mode} trusted={} ttl_s={:?} org_id={:?} person={}",
                p.trusted,
                p.ttl_s,
                p.org_id,
                person.as_deref().unwrap_or("-").escape_debug()
            ),
        );
        let ttl = pair_ttl(p.ttl_s);
        // Every read under one lock, released before the mint.
        let (base, person) = {
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
            // A device belongs to somebody or it is not minted at all
            // (multi-user M1): with no `person` named, the code is for THIS
            // HUB'S OWNER, by the owner's current name — the flag, not the
            // name, is what `people` keys the owner on, so a renamed owner
            // still pairs their own devices. A hub that cannot say whose it
            // is refuses rather than minting a person-less token, which is
            // the privilege level the milestone exists to remove.
            //
            // A `peer` / `updater` code is the exception and stays `None`:
            // neither is anybody's device, and both were refused a `person`
            // above.
            let person = match (person, mode.as_str()) {
                (some @ Some(_), _) => some,
                (None, "peer" | "updater") => None,
                (None, _) => Some(owner_name(&s).ok_or_else(|| {
                    mcp_err(
                        codes::E_PROVISION,
                        "this hub has no personal owner, so a device cannot be paired to \
                         anybody; run fleet-hub init (or name a person explicitly)",
                        None,
                    )
                })?),
            };
            (
                crate::service::hub::HubBase::read(&s).map_err(to_mcp_err)?,
                person,
            )
        };
        let req = self.guards.pairings.mint(crate::mcp::pairing::MintRequest {
            name: &name,
            mode: &mode,
            trusted: p.trusted,
            org_id: p.org_id,
            person: person.as_deref(),
            ttl,
        });
        Ok(serde_json::json!({
            "url": crate::mcp::pair_url(&base.url, &req.code),
            "code": req.code,
            "expires_in_s": ttl.as_secs(),
            "name": req.name,
            "mode": req.mode,
            "trusted": req.trusted,
            "org_id": req.org_id,
            "person": req.person,
        }))
    }

    #[tool(description = "Paired client devices and what each token may do. \
        The token digest is never returned: a token exists in plaintext only \
        in the /pair response that minted it. Read-only but master token \
        only (it names every paired device). Rows: { id, name, mode, \
        created_at, last_seen_at, revoked_at, trusted_at, org_id, person_id }.")]
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

    // ---- confirmations on the owner's device (redesign step 9.2) ----
    //
    // A call that must wait for a person (the operator's starts and kills,
    // M9.7) parks a nonce in `guards.confirms`. On a desktop its own dialog
    // and Control's cards answer it; on a hub nobody was there to, so these
    // two let the owner's paired device list and answer the hub's queue.
    // The operator never answers its own request.

    #[tool(description = "Calls waiting for your OK (the agent's starts \
        and kills), oldest first.")]
    pub(super) async fn mcp_confirms(
        &self,
        Extension(caller): Extension<Caller>,
    ) -> Result<CallToolResult, McpError> {
        audit("mcp_confirms", "");
        refuse_operator_answer(&caller)?;
        ok_json_compact(&self.guards.confirms.pending())
    }

    #[tool(description = "Approve or deny one waiting call by its nonce; \
        false when it was already answered or expired.")]
    pub(super) async fn answer_mcp_confirm(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<AnswerMcpConfirmParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("answer_mcp_confirm", &format!("approved={}", p.approved));
        refuse_operator_answer(&caller)?;
        let answered = self.guards.confirms.resolve(&p.nonce, p.approved);
        if answered {
            if let Ok(s) = lock(&self.store) {
                s.bus_confirm_changed();
            }
        }
        ok_json_compact(&answered)
    }

    #[tool(description = "What the agent handed on, newest first: \
        prompts and tasks sent to sessions, new sessions, missions, tasks \
        and proposed trees, each with its target's state now.")]
    pub(super) async fn control_handoffs(
        &self,
        Parameters(p): Parameters<ControlHandoffsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("control_handoffs", "");
        ok_json_compact(
            &crate::service::control_handoffs::list(&self.store, p.limit).map_err(to_mcp_err)?,
        )
    }

    // ---- Control routing (redesign step 9.9, Jev K2) ----

    #[tool(description = "Where a message just sent in Control goes \
        (propose {text}), or record the person's pick (follow {run_id, \
        chosen}).")]
    pub(super) async fn control_route(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<ControlRouteParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::service::decide::{control_route, DecideCtx};
        audit(
            "control_route",
            &format!("action={}", p.action.escape_debug()),
        );
        refuse_operator_answer(&caller)?;
        match p.action.as_str() {
            "propose" => {
                let text = p.text.unwrap_or_default();
                let scope = {
                    let s = lock(&self.store).map_err(to_mcp_err)?;
                    caller.view_scope(&s).map_err(to_mcp_err)?
                };
                let ctx = DecideCtx::jev(std::sync::Arc::clone(&self.store));
                ok_json_compact(&control_route::propose(&ctx, &scope, &text).await)
            }
            "follow" => {
                let (Some(run_id), Some(chosen)) = (p.run_id, p.chosen) else {
                    return Err(mcp_err("E_INVALID", "follow needs run_id and chosen", None));
                };
                let s = lock(&self.store).map_err(to_mcp_err)?;
                let scope = caller.view_scope(&s).map_err(to_mcp_err)?;
                let marked =
                    control_route::follow(&s, &scope, run_id, &chosen, crate::store::now_unix())
                        .map_err(to_mcp_err)?;
                ok_json_compact(&marked)
            }
            other => Err(mcp_err(
                "E_INVALID",
                format!("control_route's action is propose or follow, not {other:?}"),
                None,
            )),
        }
    }

    #[tool(description = "The settings page specs, data source shapes, \
        resources and page actions a device renders.")]
    pub(super) async fn list_pages(&self) -> Result<CallToolResult, McpError> {
        audit("list_pages", "");
        ok_json_compact(&crate::pages::bundle())
    }

    // ---- organisation administration (phase B) ----

    #[tool(description = "Administer the company: orgs (work_admin's org \
        actions), devices (list, pair_device → code + QR, revoke, trust, bind, \
        hand over, grant a catalog), people and members (roles, a member's \
        grants). Hub owner's device: all; an org admin's: their org. Changes \
        need a trusted full device, never locking out the one in use.")]
    pub(super) async fn org_admin(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<crate::service::org_admin::OrgAdminArgs>,
    ) -> Result<CallToolResult, McpError> {
        use crate::service::org_admin::{self as oa, Action, Me};
        audit("org_admin", &args.audit_summary());
        let action = Action::parse(&args.action).map_err(to_mcp_err)?;
        if !action.is_read() {
            org_admin_writer(&caller)?;
        }
        // Phase D: whose authority this device carries — the hub owner's
        // unbound device for the fleet, an org admin's for their org.
        let authority = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            oa::authority_for(
                &s,
                caller.is_personal_owner && caller.is_person_device(),
                caller.person(),
                caller.client.as_ref().and_then(|c| c.org_id),
            )
            .map_err(to_mcp_err)?
        }
        .ok_or_else(|| {
            mcp_err(
                "E_FORBIDDEN",
                "org_admin is for the hub owner's own device, or an org admin's for \
                 their org; this device administers no org",
                None,
            )
        })?;
        let me = Me {
            device: caller.client.as_ref().map(|c| c.name.as_str()),
            person: caller.person(),
            authority,
        };
        if action == Action::PairDevice {
            return ok_json(&self.pair_device(args, authority)?);
        }
        let out = oa::run(&args, &self.store, me).map_err(to_mcp_err)?;
        ok_json_compact(&out)
    }

    /// `org_admin { pair_device }`: a person's device only — never a peer
    /// link or an updater token, which stay `fleet-hub pair --mode …` — and
    /// the answer carries the URL's QR as rows of `1` (dark) and `0`, so the
    /// desktop draws it without a QR library of its own.
    fn pair_device(
        &self,
        args: crate::service::org_admin::OrgAdminArgs,
        authority: crate::service::org_admin::Authority,
    ) -> Result<serde_json::Value, McpError> {
        let mode = args.mode.clone().unwrap_or_else(|| "full".into());
        if !matches!(mode.as_str(), "full" | "answer" | "readonly") {
            return Err(mcp_err(
                codes::E_VALIDATE,
                format!(
                    "pair_device pairs a person's device (full, answer or readonly), not {mode:?}; \
                     a peer link or an updater token is paired on the hub (fleet-hub pair --mode)"
                ),
                None,
            ));
        }
        let org_id = match (args.org_id, args.org.as_deref()) {
            (Some(id), _) => Some(id),
            (None, Some(name)) if !name.trim().is_empty() => {
                let s = lock(&self.store).map_err(to_mcp_err)?;
                Some(
                    s.list_orgs()
                        .map_err(to_mcp_err)?
                        .into_iter()
                        .find(|o| o.name == name.trim())
                        .ok_or_else(|| {
                            mcp_err(codes::E_NOTFOUND, format!("no org named {name:?}"), None)
                        })?
                        .id,
                )
            }
            _ => None,
        };
        // Phase D: an org admin pairs a device for a member of their org, and
        // it is fenced to that org.
        let org_id = match authority {
            crate::service::org_admin::Authority::Fleet => org_id,
            crate::service::org_admin::Authority::Org { org, .. } => {
                if org_id.is_some_and(|o| o != org) {
                    return Err(mcp_err(
                        "E_FORBIDDEN",
                        "an org admin pairs devices for their own org only",
                        None,
                    ));
                }
                let person = args
                    .person
                    .as_deref()
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .ok_or_else(|| {
                        mcp_err(
                            codes::E_INVALID,
                            "pair_device needs person: whose device, a member of your org",
                            None,
                        )
                    })?;
                let s = lock(&self.store).map_err(to_mcp_err)?;
                let found = s.get_person_by_name(person).map_err(to_mcp_err)?;
                let member = match &found {
                    Some(p) => {
                        s.personal_owner_id().map_err(to_mcp_err)? != Some(p.id)
                            && s.org_role(org, p.id).map_err(to_mcp_err)?.is_some()
                    }
                    None => false,
                };
                // An org admin pairs a member's first device. A second
                // device of someone who already has one would be a token
                // that is that person, their private sessions included;
                // that is the hub owner's (review r04 F1).
                if let Some(p) = found.filter(|_| member) {
                    if crate::service::org_admin::has_identity_beyond(&s, p.id, org)
                        .map_err(to_mcp_err)?
                    {
                        return Err(mcp_err(
                            "E_FORBIDDEN",
                            format!(
                                "{person:?} already has a device or belongs to another org; \
                                 the hub owner pairs their next device"
                            ),
                            None,
                        ));
                    }
                }
                if !member {
                    return Err(mcp_err(
                        "E_FORBIDDEN",
                        format!(
                            "{person:?} is not a member of your org; add them first (set_member)"
                        ),
                        None,
                    ));
                }
                Some(org)
            }
        };
        let name = args
            .device
            .clone()
            .or(args.name.clone())
            .ok_or_else(|| mcp_err(codes::E_INVALID, "pair_device needs device", None))?;
        let mut out = self.mint_pairing(
            PairClientParams {
                name,
                mode: Some(mode),
                ttl_s: args.ttl_s,
                trusted: args.trusted.unwrap_or(false),
                org_id,
                person: args.person.clone().filter(|p| !p.trim().is_empty()),
            },
            "org_admin",
        )?;
        let url = out["url"].as_str().unwrap_or_default().to_string();
        out["qr"] = qr_rows(&url)?;
        Ok(out)
    }

    // ---- guides (declarative pages, layout guide) ----

    #[tool(description = "Settings guides. catalog: what one may name; \
        validate / propose a spec (a person approves); list; decide / remove: \
        master or trusted device.")]
    pub(super) async fn guide(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<GuideParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::service::guides;
        let spec = || {
            p.spec.as_ref().ok_or_else(|| {
                mcp_err(codes::E_INVALID, "spec is required: the guide's JSON", None)
            })
        };
        let who = settings_actor(&caller);
        match p.action {
            GuideAction::Catalog => {
                audit("guide", "action=catalog");
                let s = lock(&self.store).map_err(to_mcp_err)?;
                ok_json_compact(&guides::authoring_catalog(&s))
            }
            GuideAction::Validate => {
                audit("guide", "action=validate");
                let spec = spec()?;
                let s = lock(&self.store).map_err(to_mcp_err)?;
                ok_json_compact(&guides::check(&s, spec))
            }
            GuideAction::Propose => {
                audit("guide", "action=propose");
                let spec = spec()?;
                let row = {
                    let s = lock(&self.store).map_err(to_mcp_err)?;
                    guides::propose(&s, spec, p.why.as_deref(), who.actor()).map_err(to_mcp_err)?
                };
                tracing::info!(guide = %row.page_id, id = row.id, "[mcp] proposed a guide");
                ok_json_compact(&serde_json::json!({
                    "id": row.id,
                    "page_id": row.page_id,
                    "state": row.state,
                    "next": "a person approves or rejects it in Settings → Guides (on a hub: fleet-hub guides)",
                }))
            }
            GuideAction::List => {
                audit("guide", "action=list");
                let s = lock(&self.store).map_err(to_mcp_err)?;
                let view =
                    guides::view(&s, guide_decider(&caller, &who).is_ok()).map_err(to_mcp_err)?;
                ok_json_compact(&view)
            }
            GuideAction::Decide => {
                let id =
                    p.id.ok_or_else(|| mcp_err(codes::E_INVALID, "id is required", None))?;
                let approve = p
                    .approve
                    .ok_or_else(|| mcp_err(codes::E_INVALID, "approve is required", None))?;
                audit("guide", &format!("action=decide id={id} approve={approve}"));
                guide_decider(&caller, &who)?;
                let s = lock(&self.store).map_err(to_mcp_err)?;
                let view = guides::decide(&s, id, approve, who.actor()).map_err(to_mcp_err)?;
                ok_json_compact(&view)
            }
            GuideAction::Remove => {
                let page_id = p
                    .page_id
                    .as_deref()
                    .ok_or_else(|| mcp_err(codes::E_INVALID, "page_id is required", None))?;
                audit(
                    "guide",
                    &format!("action=remove page_id={}", page_id.escape_debug()),
                );
                guide_decider(&caller, &who)?;
                let s = lock(&self.store).map_err(to_mcp_err)?;
                let view = guides::remove(&s, page_id, who.actor()).map_err(to_mcp_err)?;
                ok_json_compact(&view)
            }
        }
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

/// WHOSE a thing this caller creates is (multi-user M1): the `people` row id
/// to stamp on a session it starts, a grant it makes, a claim it files.
///
/// A paired device carries its person on the connection
/// ([`Caller::person`]); the MASTER does not, because the master token is
/// not a device — its person is whoever owns this hub, which is a store
/// read. This is the one place that mapping is made, so "the master is the
/// owner" is written down once rather than re-derived per call site.
///
/// `None` has exactly one meaning everywhere it is returned — **nobody** —
/// and it is never a substitute for somebody:
///
/// - a per-host token is an agent on a machine, not a person;
/// - a device the migration's backfill did not reach is bound to nobody;
/// - a hub whose `people` row is missing cannot say who its owner is
///   (`Store::personal_owner_id`'s fail-closed rule, T1).
///
/// The create paths leave the row UNCLAIMED on `None` rather than
/// attributing it to a guess: an unowned row leaks no metadata and can be
/// claimed later, while a row attributed to the wrong person cannot be taken
/// back.
/// Callers: the create paths (`new_session`, `new_shell_session`,
/// `new_bg_session`, `dispatch_task`'s fallback worker owner, `work_link`'s
/// `start` and `resume`) and `orchestration::require_conversation_person`,
/// the gate that stands in front of `work_link { resume | summarize }`.
pub(super) fn owner_for(caller: &Caller, s: &Store) -> Option<i64> {
    match caller.person() {
        some @ Some(_) => some,
        // Not `is_master()` alone: a per-host token also carries no person,
        // and it must stay `None` rather than inherit the hub's owner.
        // A named token (G2.8) is the owner's too, whatever its scope.
        None if caller.speaks_for_owner() => s.personal_owner_id().ok().flatten(),
        None => None,
    }
}

/// Who or what a start over this connection records as the session's
/// origin (migration 124): the operator's own client, a per-host token
/// (with the session its pane proves, when it proves one), or else the
/// person behind the connection, as [`owner_for`] resolves them.
///
/// A store read that fails degrades the REFERENCE, never the kind: the
/// origin of an operator start is `operator` even when its row cannot be
/// read.
pub(super) fn origin_for(caller: &Caller, s: &Store) -> crate::store::SessionOrigin {
    use crate::store::SessionOrigin;
    if caller.is_operator() {
        let id = crate::service::operator::operator_ref(s).and_then(|r| {
            s.get_session(&r.tmux_name, &r.host_alias)
                .ok()
                .flatten()
                .map(|row| row.id)
        });
        return SessionOrigin::operator(id);
    }
    if let Some(alias) = caller.host_alias.as_deref() {
        let proven = caller.pane.as_deref().and_then(|pane| {
            s.find_session_by_pane(alias, pane)
                .ok()
                .flatten()
                .map(|row| row.id)
        });
        return SessionOrigin::token(proven);
    }
    SessionOrigin::person(owner_for(caller, s))
}

/// The hub's personal owner by NAME, or `None` when it has none.
///
/// `pair_client` needs the name rather than the id because a pairing code
/// carries a person's name (the row is created at redemption — see
/// `pairing::MintRequest::person`), and it must be the owner's CURRENT name:
/// `people.is_personal_owner` is the flag the owner is keyed on, and the
/// name beside it is explicitly renameable.
fn owner_name(s: &Store) -> Option<String> {
    let id = s.personal_owner_id().ok().flatten()?;
    Some(s.get_person(id).ok().flatten()?.name)
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
/// The operator asks; a person answers. `Access::PersonDevice` already
/// keeps the operator's token out (it has no person), and this says so
/// where the rule matters, so a later access change cannot let the agent
/// approve itself.
fn refuse_operator_answer(caller: &Caller) -> Result<(), McpError> {
    if caller.is_operator() {
        return Err(mcp_err(
            "E_FORBIDDEN",
            "the agent asks for confirmation; a person answers it",
            None,
        ));
    }
    Ok(())
}

pub(super) fn settings_writer(caller: &Caller, who: &SettingsWho) -> Result<(), McpError> {
    match who {
        SettingsWho::ControlApi => Ok(()),
        SettingsWho::Agent(_) => Err(mcp_err(
            "E_FORBIDDEN",
            "an agent proposes a settings change (set_setting with propose: true); a person applies it",
            None,
        )),
        // An ORG-BOUND device never writes the fleet's settings, trusted or
        // not: the settings surface is fleet-wide, so a client bound to one
        // org has no business over it. `get_settings` / `set_setting` are
        // `Access::Person`, which already excludes such a client at the gate
        // — but an `Access::Client` tool that writes through here (the
        // `guide` tool, so a host's own token can reach catalog/validate/
        // propose) reaches this arm instead, and without this check a
        // trusted org-bound client could approve and remove fleet-wide
        // guides. `is_person_device` is the one rule for "a person's own
        // device"; defer to it rather than restating it.
        SettingsWho::Device(_) if !caller.is_person_device() => Err(mcp_err(
            "E_FORBIDDEN",
            "a client bound to an organisation does not change the fleet's settings; \
             an unbound device of the hub's operator does",
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

/// An `org_admin` change: the master, or a trusted `full` device — the
/// tool's `Access::PersonDevice` row already made it the hub owner's own
/// device bound to no org. A readonly or untrusted device lists.
pub(super) fn org_admin_writer(caller: &Caller) -> Result<(), McpError> {
    if caller.is_master() || (caller.is_trusted_client() && caller.mode == TokenMode::Full) {
        return Ok(());
    }
    Err(mcp_err(
        "E_FORBIDDEN",
        format!(
            "this device may read the company's orgs, devices and people; to change them, \
             the hub's operator trusts it (a full device): fleet-hub client trust {}",
            caller.client.as_ref().map_or("<name>", |c| c.name.as_str())
        ),
        None,
    ))
}

/// Fleet administration from the owner's phone (Martin, 2026-10-08:
/// "Owner's phone"; trackers "Allow on phone"): `add_host`,
/// `install_agent` and `work_admin`'s tracker actions. Their
/// `Access::Person` row already made the caller the master or the hub
/// owner's own device bound to no org; a device must also be trusted and
/// `full`, as for `org_admin`. Credentials it sends are stored on the hub
/// and never answered back.
pub(super) fn owner_device_admin(caller: &Caller, what: &str) -> Result<(), McpError> {
    if caller.is_master() || (caller.is_trusted_client() && caller.mode == TokenMode::Full) {
        return Ok(());
    }
    Err(mcp_err(
        "E_FORBIDDEN",
        format!(
            "{what} from a device needs the hub's operator to trust it (a full device): \
             fleet-hub client trust {}",
            caller.client.as_ref().map_or("<name>", |c| c.name.as_str())
        ),
        None,
    ))
}

/// A QR code of `text` as rows of `1` (dark) / `0`, no quiet zone: the
/// desktop draws it as an SVG of squares.
fn qr_rows(text: &str) -> Result<serde_json::Value, McpError> {
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(|e| {
        mcp_err(
            codes::E_INTERNAL,
            format!("could not encode the pairing URL as a QR code: {e}"),
            None,
        )
    })?;
    let width = code.width();
    let rows: Vec<String> = code
        .to_colors()
        .chunks(width)
        .map(|row| {
            row.iter()
                .map(|c| if *c == qrcode::Color::Dark { '1' } else { '0' })
                .collect()
        })
        .collect();
    Ok(serde_json::json!(rows))
}

/// Deciding or removing a guide is a person's, as a settings write is:
/// the master or a trusted device. An agent — a host's own session with
/// the fleet-guides skill — is told what it may do instead.
fn guide_decider(caller: &Caller, who: &SettingsWho) -> Result<(), McpError> {
    match who {
        SettingsWho::Agent(_) => Err(mcp_err(
            "E_FORBIDDEN",
            "an agent proposes a guide (guide with action propose); a person approves it in \
             Settings → Guides or with fleet-hub guides",
            None,
        )),
        _ => settings_writer(caller, who),
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
/// alongside `alias`/`ssh_alias` — but only when the caller named one: an
/// unset transport writes none (a new row takes the column's `ssh`, a re-add
/// keeps the row's own, possibly `agent`), so naming `ssh` there would log a
/// change that did not happen (r18).
fn add_host_audit_detail(args: &hosts::AddHostArgs) -> String {
    let mut detail = format!("alias={} ssh_alias={}", args.alias, args.ssh_alias);
    if let Some(t) = args.transport.as_deref() {
        detail.push_str(&format!(" transport={t}"));
    }
    detail
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
    fn add_host_audit_detail_names_no_transport_when_unset() {
        // Unset keeps the row's transport (a re-add of an agent host stays
        // agent), so the line must not claim `transport=ssh`.
        let args = hosts::AddHostArgs {
            alias: "h".into(),
            ssh_alias: "h.example".into(),
            transport: None,
        };
        assert_eq!(add_host_audit_detail(&args), "alias=h ssh_alias=h.example");
    }
}
