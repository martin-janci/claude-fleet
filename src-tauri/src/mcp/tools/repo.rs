//! MCP tools: projects, worktrees and read-only repo browsing.

use super::*;

#[tool_router(router = repo_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "List discovered projects. Slim rows by default \
        (id, owner, repo, worktree_count, last_session_at); pass \
        summary=false for the full nested worktree tree.")]
    pub(super) async fn list_projects(
        &self,
        Parameters(p): Parameters<ListProjectsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("list_projects", &format!("summary={}", p.summary));
        let trees = projects::list_projects(&self.store).map_err(to_mcp_err)?;
        if p.summary {
            let slim: Vec<ProjectSummary> = trees.into_iter().map(ProjectSummary::from).collect();
            ok_json_compact(&slim)
        } else {
            ok_json_compact(&trees)
        }
    }

    #[tool(description = "Rescan the local projects directory for new or \
        removed repositories and worktrees. Returns the fresh project list.")]
    pub(super) async fn refresh_projects(&self) -> Result<CallToolResult, McpError> {
        audit("refresh_projects", "");
        ok_json(
            &projects::refresh_projects(&self.store)
                .await
                .map_err(to_mcp_err)?,
        )
    }

    // ---- sessions ----

    #[tool(description = "List git worktrees fleet knows about, each with \
        its alive-session occupants (empty = free to delete via \
        delete_worktree). Optional project filter.")]
    pub(super) async fn list_worktrees(
        &self,
        Parameters(p): Parameters<ListWorktreesParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("list_worktrees", &format!("project_id={:?}", p.project_id));
        let args = worktrees::ListWorktreesArgs {
            project_id: p.project_id,
        };
        let out = worktrees::list_worktrees(args, &self.store).map_err(to_mcp_err)?;
        ok_json(&out)
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
        Parameters(p): Parameters<SessionIdParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("repo_changes", &format!("session_id={}", p.session_id));
        let v = crate::commands::files::repo_changes_impl(
            crate::commands::files::SessionIdArgs {
                session_id: p.session_id,
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&v)
    }

    #[tool(description = "List a session's worktree files (tracked + untracked, \
        gitignore respected). Returns JSON {entries, truncated}.")]
    pub(super) async fn repo_tree(
        &self,
        Parameters(p): Parameters<SessionIdParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("repo_tree", &format!("session_id={}", p.session_id));
        let v = crate::commands::files::repo_tree_impl(
            crate::commands::files::SessionIdArgs {
                session_id: p.session_id,
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&v)
    }

    #[tool(description = "Read one worktree file's contents (capped). Returns \
        JSON {path, content, truncated, binary, size}.")]
    pub(super) async fn repo_file(
        &self,
        Parameters(p): Parameters<RepoPathParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "repo_file",
            &format!("session_id={} path={}", p.session_id, p.path),
        );
        let v = crate::commands::files::repo_file_impl(
            crate::commands::files::RepoFileArgs {
                session_id: p.session_id,
                path: p.path,
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&v)
    }

    #[tool(description = "Unified diff for one worktree file vs HEAD (untracked \
        files render as all-added). Returns JSON {path, diff, binary, truncated}.")]
    pub(super) async fn repo_diff(
        &self,
        Parameters(p): Parameters<RepoPathParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "repo_diff",
            &format!("session_id={} path={}", p.session_id, p.path),
        );
        let v = crate::commands::files::repo_diff_impl(
            crate::commands::files::RepoFileArgs {
                session_id: p.session_id,
                path: p.path,
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&v)
    }

    #[tool(description = "Commit log (branch graph) for a session's worktree. \
        all=true (default) includes every branch. Returns a JSON array of \
        commits with parents + ref decorations, newest first; `limit` defaults \
        to 50 and `skip` pages through older history.")]
    pub(super) async fn repo_log(
        &self,
        Parameters(p): Parameters<RepoLogParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("repo_log", &format!("session_id={}", p.session_id));
        let v = crate::commands::history::repo_log_impl(
            crate::commands::history::RepoLogArgs {
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
        ok_json(&v)
    }

    #[tool(description = "List local + remote branches for a session's worktree \
        with ahead/behind. Returns JSON array.")]
    pub(super) async fn repo_branches(
        &self,
        Parameters(p): Parameters<SessionIdParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("repo_branches", &format!("session_id={}", p.session_id));
        let v = crate::commands::history::repo_branches_impl(
            crate::commands::files::SessionIdArgs {
                session_id: p.session_id,
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&v)
    }

    #[tool(description = "One commit's metadata + changed files. Returns JSON \
        {hash, subject, body, author, date, files}.")]
    pub(super) async fn repo_commit(
        &self,
        Parameters(p): Parameters<RepoCommitParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "repo_commit",
            &format!("session_id={} hash={}", p.session_id, p.hash),
        );
        let v = crate::commands::history::repo_commit_impl(
            crate::commands::history::RepoCommitArgs {
                session_id: p.session_id,
                hash: p.hash,
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&v)
    }

    #[tool(description = "Diff of one file within a commit. Returns JSON \
        {path, diff, binary, truncated}.")]
    pub(super) async fn repo_commit_diff(
        &self,
        Parameters(p): Parameters<RepoCommitDiffParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "repo_commit_diff",
            &format!(
                "session_id={} hash={} path={}",
                p.session_id, p.hash, p.path
            ),
        );
        let v = crate::commands::history::repo_commit_diff_impl(
            crate::commands::history::RepoCommitDiffArgs {
                session_id: p.session_id,
                hash: p.hash,
                path: p.path,
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&v)
    }
}
