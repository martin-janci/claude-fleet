//! MCP tool: `search` (search phase 3, `service::search`). One query over
//! the hub's full-text index; every hit fenced for the caller.

use super::*;
use crate::service::search;

#[tool_router(router = search_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Search everything the hub indexes: tasks and \
        tickets (key, title, brief, description), sessions (name, host, \
        branch, tags, last prompt, notes), conversations (first prompt), \
        pull requests, the work journal, and conversation text when the hub \
        indexes transcripts (setting search.index_transcripts, off by \
        default). Every word must match, in any order; case and accents are \
        ignored and each word matches as a prefix. Only what you may see is \
        returned. Returns { hits: [{ kind, ref, title, title_marks?, \
        snippet, snippet_marks?, at, session_id?, session_name?, \
        host_alias?, claude_session_id?, task_id?, key? }], \
        transcripts_indexed }; marks are [start, end) in UTF-16 units.")]
    pub(super) async fn search(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<search::SearchArgs>,
    ) -> Result<CallToolResult, McpError> {
        let view = self.view_scope(&caller)?;
        let page = search::search(&self.store, &view, &args).map_err(to_mcp_err)?;
        ok_json_compact(&page)
    }
}
