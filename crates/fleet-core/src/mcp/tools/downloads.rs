//! MCP tools: file downloads (`service::downloads`). A session's Claude, or
//! a person, sends a file from the session's host; it is copied to this
//! machine and fetched with `GET /downloads/<id>` (`mcp::downloads_route`).

use super::*;
use crate::ipc_error::lock;
use crate::service::downloads;
use crate::ssh::SshExec;

#[tool_router(router = downloads_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Send a file from a session's host to the user's \
        phone and desktop (copied in the background, ≤ downloads.max_file_mb; \
        zip a folder first). Your session_id: whoami. path: absolute or \
        relative to the session's worktree root. E_NOTFOUND, E_LIMIT.")]
    pub(super) async fn send_file(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<downloads::SendFileArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "send_file",
            &format!("session_id={} path={}", p.session_id, p.path.escape_debug()),
        );
        let scope = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            caller.view_scope(&s).map_err(to_mcp_err)?
        };
        let source = if caller.is_client() {
            downloads::SOURCE_PERSON
        } else {
            downloads::SOURCE_AGENT
        };
        let row = downloads::send(&self.store, &*self.ssh, &scope, source, &p)
            .await
            .map_err(to_mcp_err)?;
        let ssh: Arc<dyn SshExec> = self.ssh.clone();
        downloads::spawn_fetch(self.store.clone(), ssh, row.clone());
        ok_json_compact(&row)
    }

    #[tool(description = "Files sent to the user's devices, newest first: \
        {downloads, total_bytes, max_total_bytes, max_file_bytes}. Bytes: \
        GET /downloads/<id>.")]
    pub(super) async fn list_downloads(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<downloads::ListDownloadsArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit("list_downloads", &format!("session_id={:?}", p.session_id));
        let s = lock(&self.store).map_err(to_mcp_err)?;
        let scope = caller.view_scope(&s).map_err(to_mcp_err)?;
        let out = downloads::list(&s, &scope, &p).map_err(to_mcp_err)?;
        ok_json_compact(&out)
    }

    #[tool(description = "Pause (paused: true) or resume a sent file's copy \
        in flight; it stops between slices. Answers the row. E_INVALID_STATE \
        when it is not being copied.")]
    pub(super) async fn pause_download(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<downloads::PauseDownloadArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "pause_download",
            &format!("id={} paused={}", p.id, p.paused),
        );
        let s = lock(&self.store).map_err(to_mcp_err)?;
        let scope = caller.view_scope(&s).map_err(to_mcp_err)?;
        let row = downloads::pause(&s, &scope, &p).map_err(to_mcp_err)?;
        ok_json_compact(&row)
    }

    #[tool(description = "Remove a sent file and its copy: {removed}.")]
    pub(super) async fn remove_download(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<RemoveDownloadParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("remove_download", &format!("id={}", p.id));
        let s = lock(&self.store).map_err(to_mcp_err)?;
        let scope = caller.view_scope(&s).map_err(to_mcp_err)?;
        let removed = downloads::remove(&s, &scope, p.id).map_err(to_mcp_err)?;
        ok_json_compact(&serde_json::json!({ "removed": removed }))
    }
}
