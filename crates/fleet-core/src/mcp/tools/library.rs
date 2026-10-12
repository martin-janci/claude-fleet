//! MCP tool: Control's Library (Orbit Fleet redesign step 9.7,
//! `service::library`). The files a person put on a host; a per-host token is
//! not served it (`NOT_FOR_HOST_TOKENS`), the same as `list_downloads`.

use super::*;
use crate::ipc_error::lock;
use crate::service::library;

#[tool_router(router = library_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Library: files a person put on a host (Upload or \
        a prompt's attachment). list {session_id?, host_alias?, limit?}: \
        {items}; add {kind: upload|attachment, session_id, files: [{path, \
        name?, size?}]} records files already on the session's host; remove \
        {id} drops a row, never the file. Downloads: list_downloads. \
        E_NOTFOUND, E_INVALID.")]
    pub(super) async fn library(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<LibraryParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "library",
            &format!("action={} id={:?}", p.action.escape_debug(), p.id),
        );
        let s = lock(&self.store).map_err(to_mcp_err)?;
        let scope = caller.view_scope(&s).map_err(to_mcp_err)?;
        match p.action.as_str() {
            "list" => {
                let args = library::ListArgs {
                    session_id: p.session_id,
                    host_alias: p.host_alias.clone(),
                    limit: p.limit,
                };
                ok_json_compact(&library::list(&s, &scope, &args).map_err(to_mcp_err)?)
            }
            "add" => {
                let args = library::AddArgs {
                    kind: p.kind.clone().unwrap_or_default(),
                    session_id: p
                        .session_id
                        .ok_or_else(|| mcp_err(codes::E_INVALID, "add needs session_id", None))?,
                    files: p.files.clone().unwrap_or_default(),
                };
                let items = library::add(&s, &scope, &args).map_err(to_mcp_err)?;
                ok_json_compact(&library::LibraryList { items })
            }
            "remove" => {
                let id =
                    p.id.ok_or_else(|| mcp_err(codes::E_INVALID, "remove needs id", None))?;
                let removed = library::remove(&s, &scope, id).map_err(to_mcp_err)?;
                ok_json_compact(&serde_json::json!({ "removed": removed }))
            }
            other => Err(mcp_err(
                codes::E_INVALID,
                format!("action must be list | add | remove, got {other:?}"),
                None,
            )),
        }
    }
}
