//! MCP tools: listing, spawning, inspecting and addressing sessions.

use super::*;
use crate::ipc_error::lock;

#[tool_router(router = session_ops_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "List tmux sessions across reachable hosts: slim \
        rows unless summary=false; the filters combine. claude_status is one \
        of working | blocked | completed | failed | stopped | idle; \
        stuck_kind is one of auth_menu | reconnect | trust_prompt | oom | \
        press_enter; ci_status (full rows) is one of passing | failing | \
        pending (null without a PR or checks).")]
    pub(super) async fn list_sessions(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<ListSessionsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "list_sessions",
            &format!(
                "host={:?} project={:?} status={:?} claude_status={:?} include_lost={} summary={} view={:?} limit={:?} force={} tag={:?} fresh_for={:?}",
                p.host_alias,
                p.project_id,
                p.status,
                p.claude_status,
                p.include_lost,
                p.summary,
                p.view,
                p.limit,
                p.force,
                p.tag,
                p.fresh_for,
            ),
        );
        // Parsed before the listing runs: an unknown view must cost the
        // caller a refusal, not a reconcile pass whose result it then
        // cannot have.
        let view = p.view.as_deref().map(SessionView::parse).transpose()?;
        // A forced pass reads its rows back through the writer that wrote
        // them; a listing served from the store reads through the pool.
        let rows = if p.force {
            sessions::refresh_sessions(&self.store, &self.ssh).await
        } else {
            sessions::list_sessions_reading(&self.store, self.reader(), &self.ssh).await
        }
        .map_err(to_mcp_err)?;
        let reader = if p.force { &*self.store } else { self.reader() };
        // Choke point 1 (multi-user M1, plan T6). The caller becomes a
        // `ViewScope` — who they are, what they were granted, which host
        // they speak for, which pane they can prove — and the page is cut
        // against it HERE, before `fresh_for` hashes it. Two properties,
        // both deliberate:
        //
        // * the scope is read off **the same handle the rows came from**
        //   (`p.force` → the writer, else the read pool). A scope read
        //   through the other handle races a grant created between the two,
        //   which is a revoked share still being served;
        // * an invisible row — another person's private session, an
        //   `unclaimed` row on a hub with more than one person — is
        //   DROPPED, never blanked. A session's metadata is its content
        //   (spec §4.3), so there is no shape of this row that an
        //   out-of-scope caller may hold.
        //
        // The org half is unchanged and still composed: a per-host token or
        // a bound client reads each surviving row without the work of other
        // orgs (work graph M5, D7).
        //
        // **Nothing per-caller is stamped on a row** (R6-j). The rows carry
        // `owner_person_id` and `visibility` — facts about the row,
        // identical for every caller — and a client derives watch / drive /
        // own from those plus its own person and grants. A per-caller
        // access field could not survive the bus (`BroadcastEventBus::emit`
        // serialises a bare `SessionRow` with no caller in scope), could not
        // be read as absent (`strip_nulls` removes an absent key), and
        // would be erased by the frontend's wholesale row merge — which,
        // fail-closed, would shut the OWNER's own terminal on the next
        // routine update.
        let (controller, scope, context_red_pct, facts) = {
            let s = lock(reader).map_err(to_mcp_err)?;
            let controller = s
                .get_controller()
                .map_err(|e| to_mcp_err(IpcError::from(e)))?;
            (
                controller,
                caller.view_scope(&s).map_err(to_mcp_err)?,
                crate::service::health::context_red_pct(&s),
                s.attention_facts(),
            )
        };
        let tagged = rows
            .into_iter()
            .filter(|row| scope.sees_session_row(row).is_visible())
            .map(|mut row| {
                scope.org.redact_row_org_only(&mut row);
                row
            })
            .filter(|row| {
                if !p.include_lost && row.lost_at.is_some() {
                    return false;
                }
                if let Some(h) = &p.host_alias {
                    if &row.host_alias != h {
                        return false;
                    }
                }
                if let Some(pid) = p.project_id {
                    if row.project_id != Some(pid) {
                        return false;
                    }
                }
                if let Some(st) = &p.status {
                    if &row.status != st {
                        return false;
                    }
                }
                if let Some(cs) = &p.claude_status {
                    if row.claude_status.as_deref() != Some(cs.as_str()) {
                        return false;
                    }
                }
                if let Some(tag) = &p.tag {
                    if !row.tags.iter().any(|t| t == tag) {
                        return false;
                    }
                }
                // Free: the rows are already in hand under one store lock, so
                // this is one predicate more over what was read anyway — and
                // it is the difference between a phone fetching 44 rows to
                // find 3 and fetching 3.
                if let Some(want) = p.needs_attention {
                    if crate::service::attention::needs_attention_in(row, context_red_pct, &facts)
                        .is_some()
                        != want
                    {
                        return false;
                    }
                }
                true
            })
            .map(|row| {
                let is_controller = controller
                    .as_ref()
                    .is_some_and(|(h, t)| *h == row.host_alias && *t == row.tmux_name);
                SessionWithController::with_threshold(is_controller, row, context_red_pct, &facts)
            })
            // `limit` applies AFTER the filters so a filtered page is a real
            // page of matches, not the first N rows of the whole fleet.
            .take(p.limit.unwrap_or(usize::MAX));
        // A view is a projection OF the full row, so it outranks `summary`
        // rather than composing with it: the slim shape drops
        // `friendly_name`, `current_activity` and `last_activity_at`, which
        // are three of the columns a phone draws — the exact reason that
        // client asks for full rows today. `summary` keeps its default of
        // true, so a caller that names a view need not also say
        // `summary: false` to be understood.
        //
        // fresh_for absent: today's default, byte-identical, no cursor
        // touched — kept as a literal early return (not folded into the
        // hashing branch below) so the two paths can never drift apart.
        let Some(reader) = p.fresh_for else {
            return match (view, p.summary) {
                (Some(v), _) => {
                    let full: Vec<SessionWithController> = tagged.collect();
                    ok_json_compact_view(&full, Some(v.fields()))
                }
                (None, true) => {
                    let slim: Vec<SessionSummary> = tagged.map(SessionSummary::from).collect();
                    ok_json_compact(&slim)
                }
                (None, false) => {
                    let full: Vec<SessionWithController> = tagged.collect();
                    ok_json_compact(&full)
                }
            };
        };

        // `fresh_for` present: build the EXACT same fields the branch above
        // would have sent — `compact_json_value` is what `compact_json_string`
        // (in turn what `ok_json_compact` / `ok_json_compact_view` call)
        // builds internally — and hash THAT `Value`, never a curated subset.
        // This is why the default slim shape (which drops `last_activity_at`
        // and `current_activity`, the two fields a reconcile pass bumps
        // constantly) makes `unchanged` fire usefully: a `summary: false`
        // caller hashes those two churny fields too, so it will rarely see
        // `unchanged`.
        //
        // Sorted by session id first, on this path only: `list_all_sessions`
        // (the store's underlying query) orders by `last_activity_at DESC` —
        // the very field the slim shape just dropped because reconcile bumps
        // it constantly. Two sessions trading activity would otherwise
        // reorder this array with no field actually differing, changing the
        // hash for a caller who could not possibly see why. The DEFAULT path
        // above keeps today's order untouched — only the opt-in envelope's
        // row order changes here.
        let data = match (view, p.summary) {
            (Some(v), _) => {
                let mut full: Vec<SessionWithController> = tagged.collect();
                full.sort_by_key(|s| s.row.id);
                compact_json_value(&full, Some(v.fields()))?
            }
            (None, true) => {
                let mut slim: Vec<SessionSummary> = tagged.map(SessionSummary::from).collect();
                slim.sort_by_key(|s| s.id);
                compact_json_value(&slim, None)?
            }
            (None, false) => {
                let mut full: Vec<SessionWithController> = tagged.collect();
                full.sort_by_key(|s| s.row.id);
                compact_json_value(&full, None)?
            }
        };

        let resource_key = list_sessions_resource_key(&p);
        // Read-then-write: validate the reader and read the stored hash
        // before deciding anything, so `unchanged` can never be answered to
        // a reader session that no longer exists.
        let (reader_exists, stored_hash) = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            let reader_exists = resolve_reader(&s, &caller, reader)?;
            let stored_hash = s
                .get_read_cursor(reader, "list_sessions", &resource_key)
                .map_err(to_mcp_err)?
                .and_then(|c| c.content_hash);
            (reader_exists, stored_hash)
        };

        let decision = snapshot_decision(reader_exists, stored_hash.as_deref(), data)?;
        // Written only now that the payload was built successfully;
        // `snapshot_decision` never returns a hash for an unknown reader or
        // an unchanged read.
        if let Some(hash) = &decision.new_hash {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            s.put_snapshot_cursor(reader, "list_sessions", &resource_key, None, hash)
                .map_err(to_mcp_err)?;
        }
        ok_json(&decision.envelope)
    }

    #[tool(description = "Sessions sharing this session's project and \
        worktree.")]
    pub(super) async fn related_sessions(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<sessions::RelatedSessionsArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "related_sessions",
            &format!("session_id={}", args.session_id),
        );
        // No per-row `Reach` here, deliberately (multi-user M1, T7): both
        // halves of this tool are a FILTER rather than a gate on one named
        // row. `related_sessions_scoped` now takes the caller's whole
        // `ViewScope` and applies it to the anchor (answering exactly as a
        // missing anchor does) and to every row it returns; the result gate
        // (T8) is the net under that, not the fence. Writing a second filter
        // here would be a second definition of the rule, and the one nobody
        // updates.
        let view = self.view_scope(&caller)?;
        ok_json_compact(
            &sessions::related_sessions_scoped(args, &self.store, &view).map_err(to_mcp_err)?,
        )
    }

    #[tool(description = "Mark the calling session as the fleet controller; \
        kill/recreate/restart refuse to target it without force. A per-host \
        token may only register a session on its own host (E_FORBIDDEN).")]
    pub(super) async fn register_self(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<RegisterSelfParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "register_self",
            &format!(
                "session_id={:?} host={:?} tmux={:?}",
                p.session_id, p.host_alias, p.tmux_name
            ),
        );
        let (host_alias, tmux_name) = self.resolve_target(
            &caller,
            p.session_id,
            p.host_alias.as_deref(),
            p.tmux_name.as_deref(),
            // `drive`: `register_self` writes the fleet's controller record, which
            // makes kill / recreate / restart refuse that session without `force`.
            // A row write, and the agent inside the session is the caller that
            // makes it — §4.4's pane proof is what lets it through.
            Reach::Drive,
            "the session to register",
        )?;
        {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            crate::service::operator::refuse_operator_as_controller(&s, &host_alias, &tmux_name)
                .map_err(to_mcp_err)?;
            s.set_controller(&host_alias, &tmux_name)
                .map_err(|e| to_mcp_err(IpcError::from(e)))?;
        }
        ok_json(&serde_json::json!({
            "controller": { "host_alias": host_alias, "tmux_name": tmux_name }
        }))
    }

    #[tool(description = "Find your own fleet row from your tmux session \
        name (`tmux display-message -p '#S'`). E_NOTFOUND until fleet has \
        reconciled it; E_AMBIGUOUS when the name exists on several hosts: \
        pick yours from the error's {session_id, host_alias} candidates and \
        use session_id from then on.")]
    pub(super) async fn whoami(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<WhoamiParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("whoami", &format!("tmux={}", p.tmux_name));
        // Scoped so the store guard is released before
        // `stored_local_fleet_id` takes it again below — `Store` is a plain
        // (non-reentrant) `std::sync::Mutex`.
        let (row, is_controller, context_red_pct, facts) = {
            let s = lock(self.reader()).map_err(to_mcp_err)?;
            // The WHOLE scope, org and person (multi-user M1): a row this
            // caller may not see must not match, and must not appear among
            // the `E_AMBIGUOUS` candidates either — those carry
            // `(session_id, host_alias)`, which is metadata of somebody's
            // private session (rules 1 and 6).
            let scope = caller.view_scope(&s).map_err(to_mcp_err)?;
            let row = sessions::find_session_by_tmux_name_scoped(&s, &p.tmux_name, &scope)
                .map_err(to_mcp_err)?;
            let controller = s
                .get_controller()
                .map_err(|e| to_mcp_err(IpcError::from(e)))?;
            let is_controller = controller
                .as_ref()
                .is_some_and(|(h, t)| *h == row.host_alias && *t == row.tmux_name);
            let context_red_pct = crate::service::health::context_red_pct(&s);
            (row, is_controller, context_red_pct, s.attention_facts())
        };
        // Reported, never minted: `whoami` is a read, and a readonly token
        // can call it — a read must not write (final review, Minor 7). Null
        // means no address has yet needed a fleet comparison, which is the
        // only thing that mints one (`ensure_local_fleet_id`).
        let fleet_id =
            crate::service::address::stored_local_fleet_id(self.reader()).map_err(to_mcp_err)?;
        let mut payload = serde_json::to_value(SessionWithController::with_threshold(
            is_controller,
            row,
            context_red_pct,
            &facts,
        ))
        .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))?;
        if let serde_json::Value::Object(map) = &mut payload {
            map.insert(
                "fleet_id".to_string(),
                match fleet_id {
                    Some(id) => serde_json::Value::String(id),
                    None => serde_json::Value::Null,
                },
            );
        }
        ok_json(&payload)
    }

    #[tool(description = "Create a Claude Code tmux session on a host, in a \
        project (and optional worktree, or a fresh one with new_worktree). \
        Auto-clones the repo on remote hosts.")]
    pub(super) async fn new_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<NewSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "new_session",
            &format!("host={} name={}", p.host_alias, p.name),
        );
        // A per-host token may only spawn on its own host (B1).
        require_host(&caller, &p.host_alias, "the new session")?;
        {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            require_bound_client_may_create(&s, &caller, &p.host_alias, p.project_id)?;
        }
        // Multi-user M1 (T8d): and only into a checkout it may drive. The two
        // fences above are both no-ops for a person's own device — see
        // `require_may_land_in_worktree`, which is where the reasoning lives.
        require_may_land_in_worktree(&self.store, &caller, p.worktree_id, Reach::Drive)?;
        // Multi-user M1 (T8d): and `resume_claude_session_id` goes through the
        // SAME predicate `work_link { resume }` uses.
        //
        // The service's own `reject_foreign_conversation` asks T3's durable
        // `conversation_owners` record and nothing else, and that record is
        // written only `WHEN NEW.owner_person_id IS NOT NULL` (migration 099's
        // triggers) — so for every reconcile-discovered conversation it
        // answers `None => true` while `ViewScope::sees_past_conversation`,
        // which asks the SURVIVING row first, refuses. Two mechanisms on one
        // question, and the weaker one was on the path that replays a whole
        // transcript into a pane this caller owns. The service check stays: it
        // is the fence on the desktop's own path, where there is no `Caller`.
        self.require_conversation_person(&caller, p.resume_claude_session_id.as_deref(), || {
            crate::ipc_error::IpcError::new(
                codes::E_NOTFOUND,
                "no resumable conversation with that id",
            )
        })?;
        // Step 4.4: a Claude start on a login past `accounts.pause_at` asks
        // first, as the desktop's dialog does: refused unless the caller
        // says the person chose it (`over_limit_ok`).
        let over_limit_ok = p.over_limit_ok.unwrap_or(false);
        let runs_claude = p.kind.as_deref().unwrap_or("work") != "shell"
            && matches!(
                p.agent.as_deref().map(str::trim).filter(|a| !a.is_empty()),
                None | Some(crate::store::AGENT_CLAUDE)
            );
        if runs_claude && !over_limit_ok {
            let s = lock(self.reader()).map_err(to_mcp_err)?;
            crate::service::account_limits::refuse_over_limit(
                &s,
                &p.host_alias,
                p.profile.as_deref(),
                crate::store::now_unix(),
            )
            .map_err(to_mcp_err)?;
        }
        self.confirm_gate(
            "new_session",
            p.confirm_nonce.as_deref(),
            &new_session_summary(&p),
            &caller,
        )?;
        let args = sessions::NewSessionArgs {
            host_alias: p.host_alias,
            project_id: p.project_id,
            worktree_id: p.worktree_id,
            name: p.name,
            call_id: None,
            new_worktree: p.new_worktree,
            base_branch: p.base_branch,
            kind: p.kind,
            start_command: p.start_command,
            // Omit / empty -> the service derives one from the branch via
            // `humanize::humanize_branch`.
            friendly_name: p.friendly_name,
            resume_claude_session_id: p.resume_claude_session_id,
            model: p.model,
            effort: p.effort,
            profile: p.profile,
            agent: p.agent,
            over_limit_ok,
            // Who started it (migration 124), from the connection, as the
            // owner is.
            origin: Some({
                let s = lock(self.reader()).map_err(to_mcp_err)?;
                super::fleet::origin_for(&caller, &s)
            }),
            // Whose the new session is (multi-user M1, T5): the person behind
            // THIS connection, resolved by `owner_for` — a paired device's own
            // person, the hub's personal owner for the master token, and
            // `None` for a per-host token, which is a machine and not a
            // person, so its sessions land `unclaimed` rather than being
            // attributed to whoever runs the hub. It is never read from the
            // request: `NewSessionArgs::owner_person_id` is
            // `skip_deserializing` precisely so a caller cannot name an owner.
            owner_person_id: {
                let s = lock(self.reader()).map_err(to_mcp_err)?;
                super::fleet::owner_for(&caller, &s)
            },
            start_token: p.start_token,
        };
        let row = sessions::new_session(args, &self.store, &self.ssh, &self.reg)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Create a plain-shell tmux session (an interactive \
        login shell, no Claude Code): new_session's project/worktree \
        plumbing plus a start_command. Steer it with send_prompt and read it \
        with capture_session.")]
    pub(super) async fn new_shell_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<NewShellSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "new_shell_session",
            &format!("host={} name={}", p.host_alias, p.name),
        );
        // A per-host token may only spawn on its own host (B1).
        require_host(&caller, &p.host_alias, "the new session")?;
        {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            require_bound_client_may_create(&s, &caller, &p.host_alias, p.project_id)?;
        }
        // Multi-user M1 (T8d): and only into a checkout it may drive. The two
        // fences above are both no-ops for a person's own device — see
        // `require_may_land_in_worktree`, which is where the reasoning lives.
        require_may_land_in_worktree(&self.store, &caller, p.worktree_id, Reach::Drive)?;
        self.confirm_gate(
            "new_shell_session",
            p.confirm_nonce.as_deref(),
            &new_shell_session_summary(&p),
            &caller,
        )?;
        let args = sessions::NewSessionArgs {
            host_alias: p.host_alias,
            project_id: p.project_id,
            worktree_id: p.worktree_id,
            name: p.name,
            call_id: None,
            new_worktree: p.new_worktree,
            base_branch: p.base_branch,
            kind: Some("shell".to_string()),
            start_command: p.start_command,
            // Let the service derive a humanised label from the branch.
            friendly_name: None,
            resume_claude_session_id: None,
            model: None,
            effort: None,
            profile: None,
            agent: None,
            // Who started it (migration 124), from the connection, as the
            // owner is.
            origin: Some({
                let s = lock(self.reader()).map_err(to_mcp_err)?;
                super::fleet::origin_for(&caller, &s)
            }),
            // The caller's own person, as in `new_session` above — a shell
            // session is as private as any other (its pane sees the same
            // checkout and the same credentials).
            over_limit_ok: false,
            owner_person_id: {
                let s = lock(self.reader()).map_err(to_mcp_err)?;
                super::fleet::owner_for(&caller, &s)
            },
            start_token: None,
        };
        let row = sessions::new_session(args, &self.store, &self.ssh, &self.reg)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Capture a session's terminal: the visible tmux \
        pane, plus scrollback_lines of history. Use after send_prompt to \
        read the reply. Returns plain text, not JSON.")]
    pub(super) async fn capture_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<CaptureSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "capture_session",
            &format!(
                "session_id={} scrollback_lines={:?} max_lines={:?}",
                p.session_id, p.scrollback_lines, p.max_lines
            ),
        );
        // A pane can show secrets: a per-host token reads only its own host.
        self.resolve_target(
            &caller,
            Some(p.session_id),
            None,
            None,
            // `watch`: a pane dump is the substance of a watch grant (spec §2.4,
            // correction 4 — the read-only pane view is what M1 gives back after
            // taking the live terminal away).
            Reach::Read,
            "the session to capture",
        )?;
        let text = sessions::capture_session_output(
            p.session_id,
            &self.store,
            &self.ssh,
            p.scrollback_lines,
        )
        .await
        .map_err(to_mcp_err)?;
        // A blank pane (fresh/cleared session) yields empty output, and a
        // capture longer than `max_lines` is cut with a note — both in
        // `sessions::shape_capture`, which the desktop's routed
        // `capture_session` command calls too, so one watcher's pane does not
        // read differently from another's.
        //
        // Plain text, not `ok_json`: a JSON-encoded string turns every newline
        // into `\n` and doubles the token cost of a pane dump for no benefit.
        Ok(CallToolResult::success(vec![text_content(
            sessions::shape_capture(&text, p.max_lines),
        )]))
    }

    #[tool(description = "What the session's pane shows now: claude_status, \
        stuck_kind, current_activity, waiting_for and the spinner line. One \
        capture, nothing stored: the cheap read behind a live indicator \
        (capture_session is the whole pane). E_INVALID_STATE outside tmux.")]
    pub(super) async fn session_activity(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SessionActivityParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("session_activity", &format!("session_id={}", p.session_id));
        // The probe carries a slice of pane intel, so it is gated exactly
        // like `capture_session`: a per-host token reads only its own host.
        self.resolve_target(
            &caller,
            Some(p.session_id),
            None,
            None,
            // `watch`: one capture's worth of pane intel, gated exactly like
            // `capture_session` above.
            Reach::Read,
            "the session to probe",
        )?;
        let probe = sessions::session_activity(&self.store, &self.ssh, p.session_id)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&probe)
    }

    #[tool(description = "Recreate a session: kill its tmux session and \
        rebuild it in the same worktree, resuming the same Claude \
        conversation. For a frozen, OOM-killed or out-of-context session, or \
        to revive a ghost: the conversation survives, the process does not. \
        Returns the row.")]
    pub(super) async fn recreate_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(RecreateSessionParams {
            args,
            confirm_nonce,
        }): Parameters<RecreateSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "recreate_session",
            &format!("session_id={}", args.session_id),
        );
        // Gate on the stored row's host: a per-host token must not kill and
        // rebuild a session on another host by naming its fleet id.
        let (host, name) = self.resolve_target(
            &caller,
            Some(args.session_id),
            None,
            None,
            // `own`: it re-creates the session, and it is also the primitive
            // `restore_host_sessions` batches over — gating one and not the other
            // would gate nothing (spec §4.3, invariant 5).
            Reach::Own,
            "the session to recreate",
        )?;
        // The operator's recreates (a kill and a start) need a person (D12).
        self.confirm_gate(
            "recreate_session",
            confirm_nonce.as_deref(),
            &format!(
                "session_id={} host={} name={} force={}",
                args.session_id,
                bound_text(Some(&host)),
                bound_text(Some(&name)),
                args.force
            ),
            &caller,
        )?;
        let row = sessions::recreate_session(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Restore sessions a host lost to a reboot or tmux \
        restart: resume each one's conversation in its original worktree \
        under its original name. Call dry_run first, and again after a \
        timeout to see what is still lost. One failing session never fails \
        the others; every outcome is listed. One restore per host at a time \
        (E_INVALID_STATE otherwise); a lost fleet controller is skipped \
        (recreate_session force=true). Paced by restore.batch_size / \
        restore.stagger_ms.")]
    pub(super) async fn restore_host_sessions(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(RestoreHostSessionsParams {
            args,
            confirm_nonce,
        }): Parameters<RestoreHostSessionsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "restore_host_sessions",
            &format!("host={} dry_run={}", args.host_alias, args.dry_run),
        );
        require_host(&caller, &args.host_alias, "the lost sessions")?;
        let (host_alias, dry_run) = (args.host_alias.clone(), args.dry_run);
        // Multi-user M1 (T7): the batch is gated like the primitive it
        // batches over.
        //
        // `restore_host_sessions` reaches `recreate_session` at the SERVICE
        // layer, so `Reach::Own` on the `recreate_session` TOOL above covers
        // nothing here — and the spec's §4.3 invariant 5 names both ("gating
        // one and not the other gates nothing"). Each planned session takes
        // the same `Reach::Own`, per item, as the single recreate does.
        //
        // The DRY RUN is gated too: its plan is `tmux_name`, `cwd`,
        // `claude_session_id` and `friendly_name` per session — §4.3 content,
        // and handing it over for free would be the whole leak with the
        // restart left out.
        //
        // How a refused item answers follows the plan's own vocabulary. An
        // id the CALLER named keeps an entry, as every unrestorable id
        // already does, and reads exactly like one that names no session
        // (`not found on this host`, every other field `null`) — no
        // existence oracle. An id the caller did NOT name (the whole-host
        // plan) is simply absent: a plan nobody asked a question about must
        // not answer one.
        let (args, refused) = match self.gate_restore_plan(&caller, args, Reach::Own)? {
            GatedRestore::Run(args, refused) => (args, refused),
            GatedRestore::Nothing(plan) => {
                return ok_json(&sessions::RestoreReport {
                    host_alias,
                    dry_run,
                    plan,
                    results: Vec::new(),
                })
            }
        };
        // A real restore starts sessions: the operator's needs a person
        // (D12). A dry run only reads the plan.
        if !args.dry_run {
            self.confirm_gate(
                "restore_host_sessions",
                confirm_nonce.as_deref(),
                &format!(
                    "host={} session_ids={:?}",
                    bound_text(Some(&args.host_alias)),
                    args.session_ids
                ),
                &caller,
            )?;
        }
        let mut report = sessions::restore_host_sessions(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        report.plan.extend(refused);
        ok_json(&report)
    }

    #[tool(description = "Read-only: scan ~/.claude/projects on a host for \
        conversations fleet has no live pane for, ranked against the host's \
        boot (rank_hint: before_boot | after_boot | stale | unknown), with \
        project_id, worktree_id and existing_session_id (set when a fleet \
        row, live or lost, holds it: restore_host_sessions handles that \
        one). resumable is true only when the pane would start in exactly \
        the transcript's cwd; anywhere else claude --resume silently starts \
        an empty conversation, so do not resume it. Resume with new_session \
        { host_alias, project_id, worktree_id, name: derived_tmux_name (a \
        hint), resume_claude_session_id }.")]
    pub(super) async fn discover_lost_sessions(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<sessions::DiscoverLostSessionsArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "discover_lost_sessions",
            &format!("host={} limit={:?}", args.host_alias, args.limit),
        );
        require_host(&caller, &args.host_alias, "the lost sessions")?;
        let candidates = sessions::discover_lost_sessions(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        // Multi-user M1 (T8d): `require_host` above is the host fence and
        // NOTHING ELSE — its own doc says the master token and a paired client
        // both pass it, so for every caller with no host binding it is a
        // no-op. The scan reads `~/.claude/projects` on the named host, which
        // on a shared box is every person's transcripts, and a
        // `LostCandidate` carries `cwd`, `git_branch`, `derived_tmux_name` and
        // `claude_session_id` — §4.3 content, and the handle
        // `new_session { resume_claude_session_id }` replays a whole
        // conversation from. The dry-run argument already written down for
        // `restore_host_sessions` ("handing it over for free would be the
        // whole leak with the restart left out") is this argument; that tool
        // got `gate_restore_plan` and this one got nothing.
        let candidates = self.fence_lost_candidates(&caller, candidates)?;
        ok_json(&candidates)
    }

    /// The person fence on a transcript scan (multi-user M1, T8d): which of
    /// `candidates` this caller may be told about at all.
    ///
    /// Two arms, because a candidate comes in two shapes and only one of them
    /// has a row to judge:
    ///
    /// * **A candidate fleet already holds** (`existing_session_id`, live or
    ///   lost) is judged by that ROW, through `sees_session_row` — the same
    ///   answer `list_sessions` gives for it, so one conversation cannot be
    ///   private through one tool and public through another.
    /// * **A candidate with no row at all** is judged by the conversation,
    ///   through `ViewScope::sees_past_conversation`: T3's durable
    ///   `conversation_owners` record decides, and a transcript nobody is
    ///   recorded against passes (rule 7 — the pre-M1 world, and the reason a
    ///   per-host token can still discover the unclaimed work on its own box).
    ///
    /// A refused candidate is simply ABSENT, never an entry saying something
    /// was withheld: the caller asked a question about a host, not about a
    /// conversation id, so there is nothing here to answer without becoming
    /// the existence oracle the fence exists to remove.
    pub(super) fn fence_lost_candidates(
        &self,
        caller: &Caller,
        candidates: Vec<sessions::LostCandidate>,
    ) -> Result<Vec<sessions::LostCandidate>, McpError> {
        let s = lock(self.reader()).map_err(to_mcp_err)?;
        let view = caller.view_scope(&s).map_err(to_mcp_err)?;
        if view.is_internal() {
            return Ok(candidates);
        }
        let mut out = Vec::with_capacity(candidates.len());
        for c in candidates {
            let keep = match c.existing_session_id {
                Some(id) => match s
                    .get_session_by_id(id)
                    .map_err(|e| to_mcp_err(crate::ipc_error::IpcError::from(e)))?
                {
                    Some(row) => view.sees_session_row(&row).is_visible(),
                    // The row the scan matched has gone between the two
                    // reads: nothing left to judge it by, and a candidate
                    // whose owner cannot be established is not served.
                    None => false,
                },
                None => view
                    .sees_past_conversation(&s, &c.claude_session_id)
                    .map_err(to_mcp_err)?,
            };
            if keep {
                out.push(c);
            }
        }
        Ok(out)
    }

    #[tool(description = "Permanently delete a ghost session's row (lost \
        from tmux, not worth reviving). Errors if it is not a ghost.")]
    pub(super) async fn dismiss_ghost_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<sessions::DismissGhostSessionArgs>,
    ) -> Result<CallToolResult, McpError> {
        let session_id = args.session_id;
        audit("dismiss_ghost_session", &format!("session_id={session_id}"));
        self.resolve_target(
            &caller,
            Some(session_id),
            None,
            None,
            // `drive`: it deletes a GHOST's row — a session tmux has already lost.
            // There is no live work to destroy and nothing is copied or
            // relocated, so it is not in the spec's `own` list; the desktop's
            // `SESSION_TIER` reads it the same way.
            Reach::Drive,
            "the ghost to dismiss",
        )?;
        sessions::dismiss_ghost_session(args, &self.store).map_err(to_mcp_err)?;
        ok_json(&serde_json::json!({ "dismissed": session_id }))
    }

    #[tool(description = "Adopt a live tmux session fleet did not start \
        (started_at null: someone ran tmux by hand on the host). Fleet runs \
        it from now on: started_at is set and the caller becomes its owner \
        when it has none. project_id puts it in that project (lost_target \
        proposes one). The pane is untouched. Errors with \
        E_INVALID_STATE for a row fleet already runs, a lost one (use \
        restore_host_sessions) or one with no pane (bg, external).")]
    pub(super) async fn adopt_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(mut args): Parameters<sessions::AdoptSessionArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit("adopt_session", &format!("session_id={}", args.session_id));
        self.resolve_target(
            &caller,
            Some(args.session_id),
            None,
            None,
            // `own`: it takes ownership of the row, which is the claim's
            // reach, and `resolve_target` keeps an unclaimed row this caller
            // may not see answering exactly like a missing one.
            Reach::Own,
            "the session to adopt",
        )?;
        // Whose it becomes follows from the connection, never the request
        // (the field is `skip_deserializing`), as in `new_session`.
        args.owner_person_id = {
            let s = lock(self.reader()).map_err(to_mcp_err)?;
            super::fleet::owner_for(&caller, &s)
        };
        args.decider = caller.work_decider();
        let row = sessions::adopt_session(args, &self.store).map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Read-only: the project a Lost and found entry \
        would go into, to prefill Adopt or Restore. Pass session_id (a pane \
        fleet did not start) or host_alias, claude_session_id, cwd and \
        git_branch (a conversation discover_lost_sessions found). A \
        directory inside a fleet project answers source rule; otherwise, \
        with decide.jev.adopt_target / restore_target at assist, Jev may \
        answer source jev with a confidence, or unsure (leave the form \
        blank). Never adopts or restores anything.")]
    pub(super) async fn lost_target(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<sessions::LostTargetArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "lost_target",
            &format!(
                "session_id={:?} host={:?}",
                args.session_id, args.host_alias
            ),
        );
        match (args.session_id, args.host_alias.as_deref()) {
            (Some(id), _) => {
                self.resolve_target(
                    &caller,
                    Some(id),
                    None,
                    None,
                    Reach::Own,
                    "the pane to adopt",
                )?;
            }
            (None, Some(host)) => {
                require_host(&caller, host, "the lost conversations")?;
                self.fence_lost_conversation(&caller, args.claude_session_id.as_deref())?;
            }
            (None, None) => {}
        }
        let ctx = crate::service::decide::DecideCtx::jev(std::sync::Arc::clone(&self.store));
        let target = sessions::lost_target_over_ssh(args, &self.store, &self.ssh, &ctx)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&target)
    }

    #[tool(description = "Restore into a project: copy a conversation \
        discover_lost_sessions found (not resumable where it ran) under the \
        directory Claude Code keys that project's root by, on the same \
        host, so claude --resume finds it there. Never moves or overwrites \
        a transcript. Then resume with new_session { host_alias, \
        project_id, name: tmux_name, resume_claude_session_id }.")]
    pub(super) async fn place_transcript(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<sessions::PlaceTranscriptArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "place_transcript",
            &format!("host={} project_id={}", args.host_alias, args.project_id),
        );
        require_host(&caller, &args.host_alias, "the lost conversations")?;
        self.fence_lost_conversation(&caller, Some(&args.claude_session_id))?;
        let by_person = caller.work_decider() == crate::store::Decider::Person;
        let placed = sessions::place_transcript_over_ssh(args, &self.store, &self.ssh, by_person)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&placed)
    }

    /// The person fence on one found conversation: the same answer
    /// [`Self::fence_lost_candidates`] gives a conversation with no row, as
    /// `E_NOTFOUND` (a refused one is indistinguishable from none).
    fn fence_lost_conversation(&self, caller: &Caller, id: Option<&str>) -> Result<(), McpError> {
        let Some(id) = id else {
            return Ok(());
        };
        let s = lock(self.reader()).map_err(to_mcp_err)?;
        let view = caller.view_scope(&s).map_err(to_mcp_err)?;
        if view.is_internal() || view.sees_past_conversation(&s, id).map_err(to_mcp_err)? {
            return Ok(());
        }
        Err(to_mcp_err(crate::ipc_error::IpcError::new(
            crate::ipc_error::codes::E_NOTFOUND,
            format!("no conversation {id} on this host"),
        )))
    }

    #[tool(description = "Launch a supervised headless (background) Claude \
        session on a host with an initial prompt, which becomes its default \
        friendly name. Returns the claude_session_id AND the fleet row \
        (`session`; absent until reconcile matches it, on the next tick) for \
        session_transcript { session_id }.")]
    pub(super) async fn new_bg_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(NewBgSessionParams {
            args,
            confirm_nonce,
        }): Parameters<NewBgSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "new_bg_session",
            &format!("host={} name={}", args.host_alias, args.name),
        );
        require_host(&caller, &args.host_alias, "the new background session")?;
        if let Some(pid) = args.project_id {
            let s = lock(self.reader()).map_err(to_mcp_err)?;
            require_bound_client_may_create(&s, &caller, &args.host_alias, pid)?;
        }
        self.confirm_gate(
            "new_bg_session",
            confirm_nonce.as_deref(),
            &new_bg_session_summary(&args),
            &caller,
        )?;
        // The requester (when given) must exist and, for a per-host caller,
        // live on that host — otherwise any agent could parent a background
        // session onto somebody else's conversation. Same gate as
        // `dispatch_task`; `parent_session_id` has no foreign key to catch it
        // later.
        if let Some(req) = args.requester_session_id {
            // `drive` on the REQUESTER, the same answer `dispatch_task`
            // gives for the same argument (multi-user M1, T7). Naming a
            // session as the parent does write about it: the new row is
            // stamped `parent_session_id = req`, so it appears in that
            // session's Conversations panel, and `inherit_worker_work` copies
            // the requester's work links onto it — a change to what the
            // OWNER sees on their own row, which is T7's mechanical rule for
            // `Drive`. The background session itself is the caller's own,
            // claimed below.
            self.resolve_target_row(
                &caller,
                Some(req),
                None,
                None,
                Reach::Drive,
                "requester_session_id",
            )?;
        }
        // Whose the background agent's row is (multi-user M1, T5): this
        // connection's person, as for `new_session`. A `bg:<id>` row carries
        // the launch prompt and the conversation, both of which are content.
        // It is claimed after the fact (`stamp_bg_row`): the reservation the
        // tmux paths use keys on a tmux NAME, and a background agent's row is
        // keyed on a claude session id the caller does not know yet.
        let owner = {
            let s = lock(self.reader()).map_err(to_mcp_err)?;
            super::fleet::owner_for(&caller, &s)
        };
        let res = crate::service::bg_sessions::new_bg_session_tracked(
            args,
            &self.store,
            &self.ssh,
            owner,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&res)
    }

    #[tool(
        description = "Ensure the UX agent's operator session exists on a reachable host; returns its row."
    )]
    pub(super) async fn ensure_operator(&self) -> Result<CallToolResult, McpError> {
        audit("ensure_operator", "");
        let row = crate::service::operator::ensure_operator(&self.store, &self.ssh, &self.reg)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(
        description = "Whether the UX agent can work, and why not: absent|lost|no_mcp|token_revoked|no_host|host_down; fallback: where ensure_operator moves it."
    )]
    pub(super) async fn operator_status(&self) -> Result<CallToolResult, McpError> {
        audit("operator_status", "");
        let status = crate::service::operator::operator_status(&self.store).map_err(to_mcp_err)?;
        ok_json(&status)
    }

    // ── Orchestration (Wave 3 Track E) ───────────────────────────────────

    /// The person gate on a batch restore (multi-user M1, T7): plan it, put
    /// every planned session through `reach` — which the caller passes as the
    /// same level the single `recreate_session` takes, so the literal sits in
    /// the handler where the coverage test reads it — and answer what is left.
    ///
    /// See the comment at the call site for why the batch is gated at all and
    /// how a refused item reads. The mechanics here:
    ///
    /// * **Nothing refused: the request is untouched.** The whole-host plan
    ///   keeps its own shape (its order, and its fleet-controller skip
    ///   entry), which a narrowed id list would quietly rewrite.
    /// * **Something refused:** the request becomes the explicit list of the
    ///   ids that passed, in plan order, so the service re-plans exactly
    ///   those — entry for entry what it would have planned anyway.
    /// * **Nothing left:** no restore runs at all. The report is the
    ///   refusals, and a request that can only be refused is never put to a
    ///   person for confirmation (the ordering `tidy_apply` uses).
    fn gate_restore_plan(
        &self,
        caller: &Caller,
        mut args: sessions::RestoreHostSessionsArgs,
        reach: Reach,
    ) -> Result<GatedRestore, McpError> {
        let plan = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            sessions::plan_restore(&s, &args).map_err(to_mcp_err)?
        };
        let named = args.session_ids.is_some();
        let mut allowed: Vec<i64> = Vec::new();
        let mut refused: Vec<sessions::RestorePlanEntry> = Vec::new();
        for entry in &plan {
            match self.resolve_row_person_gated(
                caller,
                entry.session_id,
                reach,
                "a session to restore",
            ) {
                Ok(_) => allowed.push(entry.session_id),
                // An id this caller never named is dropped; one they did
                // name answers as an id that names nothing.
                Err(_) if !named => {}
                Err(_) => refused.push(sessions::RestorePlanEntry {
                    session_id: entry.session_id,
                    tmux_name: None,
                    cwd: None,
                    claude_session_id: None,
                    friendly_name: None,
                    action: "skip".into(),
                    reason: Some(sessions::NOT_ON_THIS_HOST.to_string()),
                }),
            }
        }
        if allowed.len() == plan.len() {
            return Ok(GatedRestore::Run(args, refused));
        }
        if allowed.is_empty() {
            return Ok(GatedRestore::Nothing(refused));
        }
        args.session_ids = Some(allowed);
        Ok(GatedRestore::Run(args, refused))
    }
}

/// What [`FleetTools::gate_restore_plan`] decided.
enum GatedRestore {
    /// Run this request, then append these refusals to the report's plan.
    Run(
        sessions::RestoreHostSessionsArgs,
        Vec<sessions::RestorePlanEntry>,
    ),
    /// There is nothing this caller may restore: the refusals are the report.
    Nothing(Vec<sessions::RestorePlanEntry>),
}

/// `list_sessions`'s snapshot cursor key: a fingerprint of every filter
/// param that SHAPES the output — `host_alias`, `project_id`, `status`,
/// `claude_status`, `include_lost`, `summary`, `limit`, `tag`, `view`,
/// `needs_attention`. `force` and `fresh_for` are excluded: neither changes
/// what "the same filters" means, so including them would split one caller's
/// repeated reads across needless cursors. A plain JSON encoding of a
/// fixed-field-order struct is already deterministic; it is hashed only to
/// keep the stored key short, reusing `fresh::snapshot_hash` rather than a
/// second hasher. Two calls with identical filters share a cursor; two
/// different filters never do.
#[derive(serde::Serialize)]
struct ListSessionsFilterFingerprint<'a> {
    host_alias: &'a Option<String>,
    project_id: Option<i64>,
    status: &'a Option<String>,
    claude_status: &'a Option<String>,
    include_lost: bool,
    summary: bool,
    limit: Option<usize>,
    tag: &'a Option<String>,
    view: &'a Option<String>,
    needs_attention: Option<bool>,
}

fn list_sessions_resource_key(p: &ListSessionsParams) -> String {
    let fp = ListSessionsFilterFingerprint {
        host_alias: &p.host_alias,
        project_id: p.project_id,
        status: &p.status,
        claude_status: &p.claude_status,
        include_lost: p.include_lost,
        summary: p.summary,
        limit: p.limit,
        tag: &p.tag,
        view: &p.view,
        needs_attention: p.needs_attention,
    };
    let json = serde_json::to_string(&fp).expect("fixed-shape fingerprint always serializes");
    fresh::snapshot_hash(&json)
}
