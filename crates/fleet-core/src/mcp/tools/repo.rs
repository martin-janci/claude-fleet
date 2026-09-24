//! MCP tools: projects, worktrees and read-only repo browsing.

use super::*;
use crate::ipc_error::lock;
use crate::service::{repo, repo_read};

/// `repo_diff`'s snapshot cursor key: one file in one session's worktree.
/// Two different files (or the same file across two sessions) never share a
/// cursor.
pub(super) fn repo_diff_resource_key(session_id: i64, path: &str) -> String {
    format!("{session_id}:{path}")
}

#[tool_router(router = repo_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "List discovered projects (repos fleet can spawn \
        sessions in). Slim rows by default (id, owner, repo, worktree_count, \
        last_session_at); summary=false returns the full nested worktree \
        tree, which is large — pair it with limit.")]
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
            projects::list_projects_with_sessions(&self.store)
        } else {
            projects::list_projects(&self.store)
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
        removed repositories and worktrees. Returns the fresh project list. \
        On a hub with hub.local_host off it returns E_NOTFOUND: that hub has \
        no local projects directory to scan.")]
    pub(super) async fn refresh_projects(&self) -> Result<CallToolResult, McpError> {
        audit("refresh_projects", "");
        ok_json_compact(
            &projects::refresh_projects(&self.store)
                .await
                .map_err(to_mcp_err)?,
        )
    }

    // ---- sessions ----

    #[tool(description = "List git worktrees fleet knows about, with their \
        alive-session occupants (0 = free to delete via delete_worktree). \
        Returns {total, worktrees}: total counts every match, the array holds \
        at most `limit` (default 100; 0 = no cap). Slim rows by default; summary=false \
        adds the worktree path and the occupant sessions. Narrow with \
        project_id / host_alias — a fleet-wide call answers hundreds of rows.")]
    pub(super) async fn list_worktrees(
        &self,
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
        let mut out = worktrees::list_worktrees(args, &self.store).map_err(to_mcp_err)?;
        if let Some(host) = p.host_alias.as_deref() {
            out.retain(|w| w.worktree.host_alias == host);
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

    #[tool(description = "Scan one host over SSH for a project's git worktrees \
        and cache them as that host's rows. Returns {host_alias, project_id, \
        cloned, worktrees}; cloned=false means the repo is not checked out \
        there yet. Prefer list_worktrees, a store read, unless you need a \
        REMOTE host's worktrees — the stored rows cover the local host only. \
        Errors: E_NOTFOUND (no such project), E_GIT_SETUP, E_SSH.")]
    pub(super) async fn list_host_worktrees(
        &self,
        Parameters(p): Parameters<ListHostWorktreesParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "list_host_worktrees",
            &format!("host={} project_id={}", p.host_alias, p.project_id),
        );
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
        drop fleet's row. Refuses if an alive session points at it (override \
        with force=true). Errors: E_WORKTREE_BUSY, E_NOTFOUND, E_GIT, \
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

    #[tool(description = "List a session's changed files (git status) in its \
        worktree. Returns JSON array of changed files.")]
    pub(super) async fn repo_changes(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<repo::SessionIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit("repo_changes", &format!("session_id={}", args.session_id));
        self.require_visible_session(&caller, args.session_id)?;
        let v = repo_read::repo_changes(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json_compact(&v)
    }

    #[tool(description = "List a session's worktree files (tracked + untracked, \
        gitignore respected). Returns JSON {entries, truncated}.")]
    pub(super) async fn repo_tree(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<repo::SessionIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit("repo_tree", &format!("session_id={}", args.session_id));
        self.require_visible_session(&caller, args.session_id)?;
        let v = repo_read::repo_tree(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json_compact(&v)
    }

    #[tool(description = "Read one worktree file's contents (capped). Returns \
        JSON {path, content, truncated, binary, size}.")]
    pub(super) async fn repo_file(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<repo_read::RepoFileArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "repo_file",
            &format!("session_id={} path={}", args.session_id, args.path),
        );
        self.require_visible_session(&caller, args.session_id)?;
        let v = repo_read::repo_file(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&v)
    }

    #[tool(description = "Unified diff for one worktree file vs HEAD (untracked \
        files render as all-added). Returns JSON {path, diff, binary, truncated}. \
        fresh_for returns only what is new since your last read.")]
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
        self.require_visible_session(&caller, p.session_id)?;

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
            let reader_exists = s
                .get_session_by_id(reader)
                .map_err(|e| to_mcp_err(e.into()))?
                .is_some();
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

    #[tool(description = "Commit log (branch graph) for a session's worktree. \
        all=true (default) includes every branch. Returns a JSON array of \
        commits with parents + ref decorations, newest first; `limit` defaults \
        to 50 and `skip` pages through older history.")]
    pub(super) async fn repo_log(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<RepoLogParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("repo_log", &format!("session_id={}", p.session_id));
        self.require_visible_session(&caller, p.session_id)?;
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

    #[tool(description = "List local + remote branches for a session's worktree \
        with ahead/behind. Returns JSON array.")]
    pub(super) async fn repo_branches(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<repo::SessionIdArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit("repo_branches", &format!("session_id={}", args.session_id));
        self.require_visible_session(&caller, args.session_id)?;
        let v = repo_read::repo_branches(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json_compact(&v)
    }

    #[tool(description = "One commit's metadata + changed files. Returns JSON \
        {hash, subject, body, author, date, files}.")]
    pub(super) async fn repo_commit(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<repo_read::RepoCommitArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "repo_commit",
            &format!("session_id={} hash={}", args.session_id, args.hash),
        );
        self.require_visible_session(&caller, args.session_id)?;
        let v = repo_read::repo_commit(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&v)
    }

    #[tool(description = "Diff of one file within a commit. Returns JSON \
        {path, diff, binary, truncated}.")]
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
        self.require_visible_session(&caller, args.session_id)?;
        let v = repo_read::repo_commit_diff(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&v)
    }
}
