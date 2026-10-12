//! MCP tools: projects, worktrees and read-only repo browsing.

use super::*;
use crate::ipc_error::lock;
use crate::service::{add_project, project_picks, repo, repo_read};

/// `repo_diff`'s snapshot cursor key: one file in one session's worktree.
/// Two different files (or the same file across two sessions) never share a
/// cursor.
pub(super) fn repo_diff_resource_key(session_id: i64, path: &str) -> String {
    format!("{session_id}:{path}")
}

/// What `add_project`'s audit line says about its source. A clone URL is
/// logged only as the `owner/repo` the service's own parser reads from it —
/// never raw, so a `https://user:TOKEN@github.com/o/r` never reaches the
/// hub log — and `<invalid>` when it does not parse.
pub(super) fn add_project_audit_target(source: &add_project::AddProjectSource) -> String {
    match source {
        add_project::AddProjectSource::Clone { url, existing } => {
            match crate::repo_url::parse_repo_url(url) {
                Some((owner, repo)) if *existing => {
                    format!("kind=clone repo={owner}/{repo} existing=true")
                }
                Some((owner, repo)) => format!("kind=clone repo={owner}/{repo}"),
                None => "kind=clone repo=<invalid>".to_string(),
            }
        }
        add_project::AddProjectSource::Folder { path } => format!("kind=folder path={path}"),
        add_project::AddProjectSource::New {
            owner,
            repo,
            create_remote,
            ..
        } => format!("kind=new repo={owner}/{repo} create_remote={create_remote}"),
    }
}

#[tool_router(router = repo_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Discovered projects (repos fleet can spawn \
        sessions in).")]
    pub(super) async fn list_projects(
        &self,
        Parameters(p): Parameters<ListProjectsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "list_projects",
            &format!(
                "summary={} limit={:?} has_sessions={}",
                p.summary, p.limit, p.has_sessions
            ),
        );
        let mut trees = if p.has_sessions {
            projects::list_projects_with_sessions(self.reader())
        } else {
            projects::list_projects(self.reader())
        }
        .map_err(to_mcp_err)?;
        // After the filter, so a capped page is a page of matches — the same
        // order `list_sessions` applies its own `limit` in.
        if let Some(n) = p.limit {
            trees.truncate(n);
        }
        if p.summary {
            let slim: Vec<ProjectSummary> = trees.into_iter().map(ProjectSummary::from).collect();
            ok_json_compact(&slim)
        } else {
            ok_json_compact(&trees)
        }
    }

    #[tool(description = "Rescan the local projects directory for new or \
        removed repositories and worktrees; returns the project list. \
        On a hub with hub.local_host off there is no local projects \
        directory: nothing is scanned and the stored list is returned.")]
    pub(super) async fn refresh_projects(&self) -> Result<CallToolResult, McpError> {
        audit("refresh_projects", "");
        ok_json_compact(
            &projects::refresh_projects(&self.store)
                .await
                .map_err(to_mcp_err)?,
        )
    }

    #[tool(description = "Drop a project row and its worktrees (ghost \
        sessions go with it); E_INVALID_STATE while a live session \
        references it. Master only.")]
    pub(super) async fn forget_project(
        &self,
        Parameters(p): Parameters<ForgetProjectParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("forget_project", &format!("project_id={}", p.project_id));
        ok_json(&projects::forget_project(&self.store, p.project_id).map_err(to_mcp_err)?)
    }

    #[tool(description = "The New session picker's choices per project: \
        pinned, vis (hide|keep), group.")]
    pub(super) async fn project_picks(&self) -> Result<CallToolResult, McpError> {
        audit("project_picks", "");
        ok_json_compact(&project_picks::list(&self.store).map_err(to_mcp_err)?)
    }

    #[tool(description = "Replace one project's picker choices: pinned, \
        vis hide|keep|null, group or null. The empty state clears.")]
    pub(super) async fn set_project_pick(
        &self,
        Parameters(args): Parameters<project_picks::SetProjectPickArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "set_project_pick",
            &format!(
                "project={:?}/{:?} pinned={} vis={:?} grp={:?}",
                args.owner, args.repo, args.pinned, args.vis, args.grp
            ),
        );
        ok_json_compact(&project_picks::set(&self.store, &args).map_err(to_mcp_err)?)
    }

    #[tool(description = "Add a project on a host: clone a GitHub URL, adopt a \
        folder (the hub's local host only) or create a new repository \
        (create_remote is refused once with a confirm token to send back). \
        git and gh run on the host with its own credentials. Returns the \
        project row.")]
    pub(super) async fn add_project(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(AddProjectParams {
            mut args,
            confirm_nonce,
        }): Parameters<AddProjectParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "add_project",
            &format!(
                "host={} {}",
                args.host_alias,
                add_project_audit_target(&args.source)
            ),
        );
        // The same fences as `new_session`: a per-host token acts on its own
        // host only (B1), and an org-bound client only on its org's hosts.
        require_host(&caller, &args.host_alias, "the project's host")?;
        {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            require_bound_client_sees_host(&s, &caller, &args.host_alias)?;
        }
        // Publishing a repository on GitHub needs a person. A paired
        // client (a phone, a paired desktop) is a person at a UI, and the
        // service's single-use token is their confirmation. Every other
        // caller — the operator (D12, see `guard::OPERATOR_CONFIRMS`), a
        // per-host token, the master token an agent may hold — could send
        // that token straight back, so it waits for a person's approval
        // (refused outright where there is no approver, a hub). Only the
        // call that can actually create — `create_remote` WITH the token —
        // is gated: the call without one is refused by the service before
        // anything runs, and gating it too would spend the person's
        // approval on a call that could never create.
        if let add_project::AddProjectSource::New {
            owner,
            repo,
            create_remote: true,
            confirm: Some(_),
        } = &args.source
        {
            let person = !caller.is_client() || caller.is_operator();
            self.confirm_gate_with(
                "add_project",
                confirm_nonce.as_deref(),
                &format!(
                    "host={} repo={}/{} create_remote=true",
                    bound_text(Some(&args.host_alias)),
                    bound_text(Some(owner)),
                    bound_text(Some(repo))
                ),
                &caller,
                person,
            )?;
        }
        // `call_id` is the desktop dialog's Cancel handle, never an MCP
        // caller's: `#[schemars(skip)]` only hides it from the schema, and
        // `#[serde(default)]` still reads one a caller sends. Bound, it would
        // replace a desktop call's token of the same id in the shared
        // registry, and its `CancelGuard` would then remove that slot. So it
        // is dropped here: the registry mints an anonymous token (ids from
        // `ANONYMOUS_ID_BASE`, never a frontend's) and `CancelGuard` releases
        // it.
        args.call_id = None;
        let row = add_project::add_project(args, &self.store, &*self.ssh, &self.reg)
            .await
            .map_err(to_mcp_err)?;
        ok_json_compact(&row)
    }

    #[tool(description = "Repositories gh on the host can see, for choosing \
        what to clone with add_project.")]
    pub(super) async fn list_github_repos(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<ListGithubReposParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "list_github_repos",
            &format!(
                "host={} owner={}",
                p.host_alias,
                p.owner.as_deref().unwrap_or("-")
            ),
        );
        // `gh repo list` runs with the host's own GitHub login: a per-host
        // token reads only its own host's, an org-bound client only its
        // org's hosts'.
        require_host(&caller, &p.host_alias, "the host to list")?;
        {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            require_bound_client_sees_host(&s, &caller, &p.host_alias)?;
        }
        let repos = add_project::list_github_repos_with(
            &p.host_alias,
            p.owner.as_deref(),
            &self.store,
            &*self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&repos)
    }

    // ---- sessions ----

    #[tool(description = "Git worktrees fleet knows about, with their \
        alive-session occupants (0 = free to delete via delete_worktree). \
        Returns {total, worktrees}: total counts every match. Narrow with \
        project_id / host_alias: a fleet-wide call answers hundreds of rows.")]
    pub(super) async fn list_worktrees(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<ListWorktreesParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "list_worktrees",
            &format!(
                "project_id={:?} host={:?} summary={} limit={:?}",
                p.project_id, p.host_alias, p.summary, p.limit
            ),
        );
        let args = worktrees::ListWorktreesArgs {
            project_id: p.project_id,
        };
        let mut out = worktrees::list_worktrees(args, self.reader()).map_err(to_mcp_err)?;
        if let Some(host) = p.host_alias.as_deref() {
            out.retain(|w| w.worktree.host_alias == host);
        }
        // Multi-user M1 (T8d): the OCCUPANTS are fenced by person, the
        // worktrees are not.
        //
        // This tool took no `Caller` at all and answered
        // `WorktreeOccupant { host_alias, tmux_name }` for every alive session
        // in the fleet — a private session's machine and its tmux name, which
        // in this fleet is a branch or a ticket key, i.e. §4.3 content. The
        // coverage gate could not see it (it is addressed by `worktree_id`
        // only in `delete_worktree`'s sense; here the arguments are
        // `project_id` / `host_alias`, both deliberately outside
        // `SESSION_KEY_NAMES`), and the result gate could not either until
        // `WorktreeOccupant` started carrying the id.
        //
        // The OCCUPANT goes and the worktree stays: a worktree is a checkout,
        // not a session, and a caller that may start work in it has to be able
        // to see it. The occupancy COUNT the `summary` view prints is taken
        // after this, so it counts what the caller may see rather than
        // answering "two sessions you cannot name are in here".
        {
            let s = lock(self.reader()).map_err(to_mcp_err)?;
            let view = caller.view_scope(&s).map_err(to_mcp_err)?;
            if !view.is_internal() {
                for w in out.iter_mut() {
                    let mut keep = Vec::with_capacity(w.occupants.len());
                    for o in w.occupants.drain(..) {
                        let visible = match s
                            .get_session_by_id(o.session_id)
                            .map_err(|e| to_mcp_err(crate::ipc_error::IpcError::from(e)))?
                        {
                            Some(row) => view.sees_session_row(&row).is_visible(),
                            // An occupant whose row cannot be resolved is not
                            // served: the gate is generous in the safe
                            // direction, exactly as `looks_like_session_row` is.
                            None => false,
                        };
                        if visible {
                            keep.push(o);
                        }
                    }
                    w.occupants = keep;
                }
            }
        }
        // `total` before the cap: a caller that gets 100 of 249 rows must be
        // able to see that it is holding a slice, or it will reason about the
        // fleet from a silent truncation.
        let total = out.len();
        match p.limit {
            // 0 = no cap: the desktop in hub-client mode needs the whole list
            // to render the project tree, where an agent wants a page.
            Some(0) => {}
            Some(n) => out.truncate(n),
            None => out.truncate(WORKTREES_DEFAULT_LIMIT),
        }
        let worktrees = if p.summary {
            serde_json::to_value(
                out.into_iter()
                    .map(WorktreeSummary::from)
                    .collect::<Vec<_>>(),
            )
        } else {
            serde_json::to_value(&out)
        }
        .map_err(|e| McpError::internal_error(format!("serialize worktrees: {e}"), None))?;
        ok_json_compact(&serde_json::json!({ "total": total, "worktrees": worktrees }))
    }

    #[tool(description = "Scan one host over SSH for a project's git \
        worktrees and cache them. Returns {host_alias, project_id, cloned, \
        worktrees}; cloned=false: not checked out there yet. Prefer \
        list_worktrees (a store read) unless you need a REMOTE host's \
        worktrees: stored rows cover the local host only. Errors: \
        E_NOTFOUND, E_GIT_SETUP, E_SSH.")]
    pub(super) async fn list_host_worktrees(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<ListHostWorktreesParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "list_host_worktrees",
            &format!("host={} project_id={}", p.host_alias, p.project_id),
        );
        // Multi-user M1 (T10): the HOST fence, which this tool had none of.
        //
        // It took no `Caller` at all and is `Access::Client`, so a per-host
        // token could make the hub ssh into ANY host and scan it. That is
        // the org/host boundary rather than §4.3's person fence — a
        // `HostWorktrees` carries `WorktreeRow`s (project, host, name, path,
        // branch) and no session field at all, which is why T10's
        // "filter the occupants" instruction has nothing to bite on here and
        // why the worktrees themselves stay whole for everyone (a caller
        // who may start work in a checkout has to be able to see it, the
        // same argument `list_worktrees` makes one function up). The master
        // token and every paired client are unbound and pass.
        require_host(&caller, &p.host_alias, "the worktrees to scan")?;
        let args = worktrees::ListHostWorktreesArgs {
            host_alias: p.host_alias,
            project_id: p.project_id,
        };
        let out = worktrees::list_host_worktrees(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json_compact(&out)
    }

    #[tool(description = "Delete a git worktree on its host (no --force) and \
        drop fleet's row. Refuses if an alive session points at it, unless \
        force. Errors: E_WORKTREE_BUSY, E_NOTFOUND, E_GIT, \
        E_CONFIRM_REQUIRED (desktop confirmation on).")]
    pub(super) async fn delete_worktree(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<DeleteWorktreeParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "delete_worktree",
            &format!("worktree_id={} force={}", p.worktree_id, p.force),
        );
        // Multi-user M1 (T7): the session gate, by worktree id.
        //
        // This tool is session-addressed without naming a session — it
        // removes the checkout a session is RUNNING IN, leaving the owner's
        // pane in a deleted directory and dropping fleet's row, which
        // `force: true` does even while the session is alive
        // (`service::worktrees::delete_worktree`). That is destruction of
        // somebody's work, so it takes the tier §4.3 invariant 5 gives
        // destruction: `Reach::Own` on every alive occupant, the same level
        // `safe_kill_session` takes for strictly less. Nobody is granted
        // `own`, so no share ever reaches it.
        //
        // The occupants are resolved HERE rather than inside the service,
        // because the service is also the desktop's own path, where there is
        // one person at the keyboard and no `Caller` to ask.
        //
        // `occupant_session_ids_for_worktree`, not the ALIVE set: that query
        // is `status='running' AND lost_at IS NULL`, and a host reboot leaves
        // the row `ghost` with `lost_at` set while the checkout and its
        // uncommitted work sit on disk waiting for `restore_host_sessions`.
        // Over the alive set this loop iterated nothing for exactly those
        // rows, so the gate below silently did not run — a fix that looks
        // right and never fires (T8d).
        let occupants: Vec<i64> = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            s.occupant_session_ids_for_worktree(p.worktree_id)
                .map_err(|e| to_mcp_err(crate::ipc_error::IpcError::from(e)))?
        };
        //
        // The refusal is the WORKTREE's, never the occupant's.
        // `require_person_sees` phrases its own as `session {id} not found`,
        // and that id came from the STORE rather than from the caller: served
        // verbatim, walking worktree ids would tell anybody which trees hold a
        // private session and what its id is. The sibling path was hardened
        // against exactly this — the `E_WORKTREE_BUSY` prose prints a COUNT
        // now, never host/tmux_name (`service::worktrees::delete_worktree`) —
        // so neither sentence below names a session. An occupant the caller
        // cannot see answers `E_WORKTREE_BUSY`, indistinguishable in shape
        // from a tree that is merely busy; one they CAN see (a grantee) keeps
        // an `E_FORBIDDEN` that says the tier, since the row is theirs to see
        // anyway and silence there would only be confusing.
        for sid in occupants {
            let refusal = {
                let s = lock(&self.store).map_err(to_mcp_err)?;
                match crate::service::sessions::resolve_session_target(&s, Some(sid), None, None) {
                    // The row went between the two reads: nothing left to
                    // protect, and the delete's own checks still run.
                    Err(_) => None,
                    Ok(row) => {
                        if reaches_row(&s, &caller, &row, Reach::Own)? {
                            None
                        } else if caller
                            .view_scope(&s)
                            .map_err(to_mcp_err)?
                            .sees_session_row(&row)
                            .is_visible()
                        {
                            // Visible, below the tier: a grantee. Asked
                            // directly rather than through a second
                            // `Reach::Read`, which would make this tool's row
                            // in `SESSION_REACH` read as if a watcher could
                            // delete a worktree.
                            Some((
                                codes::E_FORBIDDEN,
                                "removing this worktree destroys the work of a session in it: \
                                 that needs `own`, which no grant reaches",
                            ))
                        } else {
                            Some((
                                codes::E_WORKTREE_BUSY,
                                "this worktree is occupied by a session that is not yours",
                            ))
                        }
                    }
                }
            };
            if let Some((code, why)) = refusal {
                return Err(mcp_err(code, why, None));
            }
        }
        self.confirm_gate(
            "delete_worktree",
            p.confirm_nonce.as_deref(),
            &format!("worktree_id={} force={}", p.worktree_id, p.force),
            &caller,
        )?;
        let args = worktrees::DeleteWorktreeArgs {
            worktree_id: p.worktree_id,
            force: p.force,
        };
        worktrees::delete_worktree(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        Ok(CallToolResult::success(vec![text_content(
            "worktree deleted",
        )]))
    }

    #[tool(description = "A session's changed files (git status) in its \
        worktree, each with added/removed line counts against HEAD when git \
        diffs it.")]
    pub(super) async fn repo_changes(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<repo::SessionIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit("repo_changes", &format!("session_id={}", args.session_id));
        // `watch`, for every `repo_*` read in this file: a session's
        // worktree is its work, and the four reads the spec calls the
        // substance of a watch grant include it. The gate was
        // `require_visible_session` until multi-user M1's T7 — a function
        // whose first line (`if !caller.is_scoped()`) returned `Ok(())` for
        // the master AND for every unbound paired client, which is every
        // person's device. `repo_file` under it returned arbitrary file
        // contents from anybody's worktree to anybody's phone.
        //
        // Person-gated without the host fence in front, which is what the
        // function it replaces did: a session this caller may not reach —
        // another person's, another org's, another host's — answers exactly
        // as an id that does not exist.
        self.resolve_row_person_gated(
            &caller,
            args.session_id,
            Reach::Read,
            "the session whose worktree to read",
        )?;
        let v = repo_read::repo_changes(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json_compact(&v)
    }

    #[tool(description = "A session's worktree files (tracked + untracked, \
        gitignore respected): {entries, truncated}.")]
    pub(super) async fn repo_tree(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<repo::SessionIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit("repo_tree", &format!("session_id={}", args.session_id));
        self.resolve_row_person_gated(
            &caller,
            args.session_id,
            Reach::Read,
            "the session whose worktree to read",
        )?;
        let v = repo_read::repo_tree(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json_compact(&v)
    }

    #[tool(description = "One worktree file's contents (capped): {path, \
        content, truncated, binary, size}.")]
    pub(super) async fn repo_file(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<repo_read::RepoFileArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "repo_file",
            &format!("session_id={} path={}", args.session_id, args.path),
        );
        self.resolve_row_person_gated(
            &caller,
            args.session_id,
            Reach::Read,
            "the session whose worktree to read",
        )?;
        let v = repo_read::repo_file(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&v)
    }

    #[tool(description = "Blame of one worktree file as runs of lines: \
        {path, hunks: [{start, lines, hash, author, time, summary, \
        uncommitted}], truncated}.")]
    pub(super) async fn repo_blame(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<repo_read::RepoFileArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "repo_blame",
            &format!("session_id={} path={}", args.session_id, args.path),
        );
        self.resolve_row_person_gated(
            &caller,
            args.session_id,
            Reach::Read,
            "the session whose worktree to read",
        )?;
        let v = repo_read::repo_blame(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&v)
    }

    #[tool(description = "Unified diff of one worktree file vs HEAD \
        (untracked: all added): {path, diff, binary, truncated}.")]
    pub(super) async fn repo_diff(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<RepoDiffParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "repo_diff",
            &format!(
                "session_id={} path={} fresh_for={:?}",
                p.session_id, p.path, p.fresh_for
            ),
        );
        self.resolve_row_person_gated(
            &caller,
            p.session_id,
            Reach::Read,
            "the session whose worktree to read",
        )?;

        // fresh_for absent: today's default, byte-identical, no cursor
        // touched — kept as a literal early return so the two paths can
        // never drift apart.
        let Some(reader) = p.fresh_for else {
            let v = repo_read::repo_diff((&p).into(), &self.store, &self.ssh)
                .await
                .map_err(to_mcp_err)?;
            return ok_json(&v);
        };

        // Scope 1: reader + stored hash, resolved BEFORE the (async,
        // SSH-backed) diff below — never held across an `.await`.
        let resource_key = repo_diff_resource_key(p.session_id, &p.path);
        let (reader_exists, stored_hash) = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            let reader_exists = resolve_reader(&s, &caller, reader)?;
            let stored_hash = s
                .get_read_cursor(reader, "repo_diff", &resource_key)
                .map_err(to_mcp_err)?
                .and_then(|c| c.content_hash);
            (reader_exists, stored_hash)
        };

        // The diff is still computed here in order to hash it — repo_diff
        // has no cheaper "did it change" signal than the diff itself (no
        // watermark, no generation to compare first), so `unchanged` saves
        // the CALLER context and transfer, never the server-side git work.
        //
        // `data` keeps every field `ok_json` would send (never
        // `compact_json_value`, which strips nulls) — hashing a narrower
        // shape than what is actually returned would make `unchanged` a
        // lie. `snapshot_decision` hashes this exact `Value`.
        let v = repo_read::repo_diff((&p).into(), &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        let data = serde_json::to_value(&v)
            .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))?;
        let decision = snapshot_decision(reader_exists, stored_hash.as_deref(), data)?;

        // Scope 2: written only now that the diff was built and hashed
        // successfully — a failure above must not mark this as delivered,
        // and `snapshot_decision` never returns a hash to store for an
        // unknown reader or an unchanged read.
        if let Some(hash) = &decision.new_hash {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            s.put_snapshot_cursor(reader, "repo_diff", &resource_key, Some(p.session_id), hash)
                .map_err(to_mcp_err)?;
        }
        ok_json(&decision.envelope)
    }

    #[tool(description = "Commit log (branch graph) of a session's worktree, \
        newest first, with parents + ref decorations; `skip` pages back.")]
    pub(super) async fn repo_log(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<RepoLogParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("repo_log", &format!("session_id={}", p.session_id));
        self.resolve_row_person_gated(
            &caller,
            p.session_id,
            Reach::Read,
            "the session whose worktree to read",
        )?;
        let v = repo_read::repo_log(
            repo_read::RepoLogArgs {
                session_id: p.session_id,
                all: p.all.unwrap_or(true),
                limit: p.limit.unwrap_or(REPO_LOG_DEFAULT_LIMIT),
                skip: p.skip.unwrap_or(0),
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json_compact(&v)
    }

    #[tool(description = "Local + remote branches of a session's worktree, \
        with ahead/behind and `merged` (the base branch contains the tip).")]
    pub(super) async fn repo_branches(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<repo::SessionIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit("repo_branches", &format!("session_id={}", args.session_id));
        self.resolve_row_person_gated(
            &caller,
            args.session_id,
            Reach::Read,
            "the session whose worktree to read",
        )?;
        let v = repo_read::repo_branches(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json_compact(&v)
    }

    #[tool(description = "One commit's metadata + changed files with \
        added/removed line counts: {hash, subject, body, author, date, files, \
        pushed}.")]
    pub(super) async fn repo_commit(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<repo_read::RepoCommitArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "repo_commit",
            &format!("session_id={} hash={}", args.session_id, args.hash),
        );
        self.resolve_row_person_gated(
            &caller,
            args.session_id,
            Reach::Read,
            "the session whose worktree to read",
        )?;
        let v = repo_read::repo_commit(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&v)
    }

    #[tool(description = "One file's diff within a commit: {path, diff, \
        binary, truncated}.")]
    pub(super) async fn repo_commit_diff(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<repo_read::RepoCommitDiffArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "repo_commit_diff",
            &format!(
                "session_id={} hash={} path={}",
                args.session_id, args.hash, args.path
            ),
        );
        self.resolve_row_person_gated(
            &caller,
            args.session_id,
            Reach::Read,
            "the session whose worktree to read",
        )?;
        let v = repo_read::repo_commit_diff(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&v)
    }

    #[tool(description = "What a session's branch carries: the commits no \
        remote has and the files they change, and the files it changes \
        against the base branch: {branch, upstream, unpushed, unpushedFiles, \
        truncated, base, aheadOfBase, baseFiles, behindBase}.")]
    pub(super) async fn repo_branch_diff(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<repo::SessionIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "repo_branch_diff",
            &format!("session_id={}", args.session_id),
        );
        self.resolve_row_person_gated(
            &caller,
            args.session_id,
            Reach::Read,
            "the session whose worktree to read",
        )?;
        let v = repo_read::repo_branch_diff(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&v)
    }

    #[tool(description = "One file's diff over a session's unpushed commits \
        (range `unpushed`) or against the base branch (range `base`): {path, \
        diff, binary, truncated}.")]
    pub(super) async fn repo_range_diff(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<repo_read::RepoRangeDiffArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "repo_range_diff",
            &format!(
                "session_id={} path={} range={:?}",
                args.session_id, args.path, args.range
            ),
        );
        self.resolve_row_person_gated(
            &caller,
            args.session_id,
            Reach::Read,
            "the session whose worktree to read",
        )?;
        let v = repo_read::repo_range_diff(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&v)
    }
}
