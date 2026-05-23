//! MCP tool surface for claude-fleet.
//!
//! `FleetTools` holds the shared backend state (`Store`, `SshClient`,
//! `CancellationRegistry`) and exposes claude-fleet operations as MCP tools.
//! Every tool calls into the transport-agnostic `service` layer — the exact
//! same code path the Tauri IPC commands use; neither path is privileged.
//!
//! Tool arguments are MCP-specific structs (deriving `JsonSchema` so the AI
//! sees a typed schema). They deliberately omit the `call_id` cancellation
//! field the frontend uses — MCP tool calls run to completion.

use crate::cancel::CancellationRegistry;
use crate::ipc_error::IpcError;
use crate::service::{health, hosts, projects, sessions};
use crate::ssh::SshClient;
use crate::store::Store;
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler,
};
use std::sync::{Arc, Mutex};

/// The MCP server handler. Cloned per session by the streamable-HTTP service;
/// every clone shares the same backend state via the `Arc`s.
#[derive(Clone)]
pub struct FleetTools {
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    reg: Arc<CancellationRegistry>,
    tunnels: Arc<crate::service::tunnel::TunnelSupervisor>,
    // Consumed by the `#[tool_router]` / `#[tool_handler]` macro-generated
    // dispatch; the field itself reads as dead to the lint.
    #[allow(dead_code)]
    tool_router: ToolRouter<FleetTools>,
}

// --- shared helpers --------------------------------------------------------

/// Emit a one-line audit record for a tool call. A remote-control surface
/// that can mutate the fleet should be traceable; this logs the tool name and
/// the identifying (non-secret) arguments. Prompt *bodies* are never logged.
fn audit(tool: &str, detail: &str) {
    if detail.is_empty() {
        eprintln!("[mcp] tool call: {tool}");
    } else {
        eprintln!("[mcp] tool call: {tool} {detail}");
    }
}

/// Map a backend `IpcError` to an MCP tool error, preserving the `E_*` code.
fn to_mcp_err(e: IpcError) -> McpError {
    McpError::internal_error(format!("{}: {}", e.code, e.message), None)
}

/// Substituted for an otherwise-empty text block. The Anthropic API rejects
/// empty text content outright ("text content blocks must be non-empty"), and
/// when prompt caching tags such a block the request fails harder still
/// ("cache_control cannot be set for empty text blocks"). Tool results flow
/// into the calling session's conversation as `tool_result` blocks, so an
/// empty/whitespace-only result would surface there as an empty text block and
/// poison that session's next API call. We never emit one — this sentinel keeps
/// every block non-empty.
const EMPTY_RESULT_PLACEHOLDER: &str = "(no output)";

/// Build a text content block guaranteed to be non-empty. Empty or
/// whitespace-only text is replaced with [`EMPTY_RESULT_PLACEHOLDER`]. Every
/// tool result must go through here (directly or via [`ok_json`]) so the fleet
/// never hands a Claude session an empty text block to serialize.
fn text_content(text: impl Into<String>) -> Content {
    let text = text.into();
    if text.trim().is_empty() {
        Content::text(EMPTY_RESULT_PLACEHOLDER)
    } else {
        Content::text(text)
    }
}

/// Serialize a successful result to pretty JSON wrapped in a tool result.
fn ok_json<T: serde::Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))?;
    Ok(CallToolResult::success(vec![text_content(json)]))
}

/// Compact JSON with all `null` fields recursively removed. Used by
/// list-style tools whose rows carry many `Option<>` columns — pretty-printing
/// plus `"field": null` repetitions blows past MCP token caps on big fleets.
/// Stripping nulls at the MCP boundary (rather than via `#[serde(skip)]` on
/// the row struct) keeps the Tauri event bus's value→null clearing intact.
fn ok_json_compact<T: serde::Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let mut v = serde_json::to_value(value)
        .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))?;
    strip_nulls(&mut v);
    let json = serde_json::to_string(&v)
        .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))?;
    Ok(CallToolResult::success(vec![text_content(json)]))
}

fn strip_nulls(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            map.retain(|_, val| !val.is_null());
            for val in map.values_mut() {
                strip_nulls(val);
            }
        }
        serde_json::Value::Array(arr) => {
            for val in arr.iter_mut() {
                strip_nulls(val);
            }
        }
        _ => {}
    }
}

/// A `SessionRow` augmented with the controller flag for the `list_sessions`
/// MCP output. `#[serde(flatten)]` keeps every original SessionRow field at the
/// top level, so adding `is_controller` does not break existing consumers.
#[derive(serde::Serialize)]
struct SessionWithController {
    is_controller: bool,
    #[serde(flatten)]
    row: crate::store::SessionRow,
}

/// Slim row returned by `list_sessions` when `summary: true` (the default).
/// Trimmed to the fields a triage UI/agent actually needs to pick which session
/// to drill into; callers fetch full state via `peek_session` / `related_sessions`
/// or by re-calling with `summary: false`.
#[derive(serde::Serialize)]
struct SessionSummary {
    id: i64,
    host_alias: String,
    tmux_name: String,
    project_id: Option<i64>,
    worktree_id: Option<i64>,
    status: String,
    claude_status: Option<String>,
    stuck_kind: Option<String>,
    lost_at: Option<i64>,
    is_controller: bool,
}

impl From<SessionWithController> for SessionSummary {
    fn from(s: SessionWithController) -> Self {
        Self {
            id: s.row.id,
            host_alias: s.row.host_alias,
            tmux_name: s.row.tmux_name,
            project_id: s.row.project_id,
            worktree_id: s.row.worktree_id,
            status: s.row.status,
            claude_status: s.row.claude_status,
            stuck_kind: s.row.stuck_kind,
            lost_at: s.row.lost_at,
            is_controller: s.is_controller,
        }
    }
}

// --- tool parameter structs ------------------------------------------------

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct AddHostParams {
    /// claude-fleet alias to register the host under (must be a safe
    /// identifier — letters, digits, dashes).
    pub alias: String,
    /// SSH config alias used to reach the host (from `~/.ssh/config`).
    pub ssh_alias: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct HostAliasParams {
    /// The claude-fleet host alias (e.g. "local", "mefistos").
    pub alias: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct HideHostParams {
    /// The claude-fleet host alias.
    pub alias: String,
    /// `true` to hide the host (skipped during reconcile), `false` to show it.
    pub hidden: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RelatedSessionsParams {
    /// The session id to find siblings of (same project + worktree).
    pub session_id: i64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ListSessionsParams {
    /// Only return sessions on this host alias.
    #[serde(default)]
    pub host_alias: Option<String>,
    /// Only return sessions in this project id.
    #[serde(default)]
    pub project_id: Option<i64>,
    /// Only return sessions whose store-level `status` equals this (e.g.
    /// "alive", "dead").
    #[serde(default)]
    pub status: Option<String>,
    /// Only return sessions whose `claude_status` equals this (e.g. "idle",
    /// "working", "stuck", "awaiting_input").
    #[serde(default)]
    pub claude_status: Option<String>,
    /// Include lost sessions (those with a non-null `lost_at`). Default false.
    #[serde(default)]
    pub include_lost: bool,
    /// Return slim rows (id, host_alias, tmux_name, project_id, worktree_id,
    /// status, claude_status, stuck_kind, lost_at, is_controller). Default
    /// true to keep responses inside MCP token caps; set false for full rows.
    #[serde(default = "default_true")]
    pub summary: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct NewSessionParams {
    /// Host alias to create the session on.
    pub host_alias: String,
    /// Project id (see `list_projects`).
    pub project_id: i64,
    /// Optional worktree id; omit to use the project root.
    #[serde(default)]
    pub worktree_id: Option<i64>,
    /// tmux session name to create.
    pub name: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct KillSessionParams {
    /// Host alias the session lives on.
    pub host_alias: String,
    /// tmux session name to kill.
    pub name: String,
    /// Kill even if this is the registered fleet controller. Default false.
    #[serde(default)]
    pub force: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RenameSessionParams {
    /// Host alias the session lives on.
    pub host_alias: String,
    /// Current tmux session name.
    pub old_name: String,
    /// New tmux session name.
    pub new_name: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RestartSessionParams {
    /// Host alias the session lives on.
    pub host_alias: String,
    /// tmux session name to restart.
    pub name: String,
    /// Restart even if this is the registered fleet controller. Default false.
    #[serde(default)]
    pub force: bool,
}

fn default_true() -> bool {
    true
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SendPromptParams {
    /// Host alias the session lives on.
    pub host_alias: String,
    /// tmux session name to send the prompt to.
    pub tmux_name: String,
    /// The prompt text to deliver to the session's Claude REPL.
    pub prompt: String,
    /// Whether to submit the prompt (press Enter). Defaults to true. Set
    /// `submit: false` to stage the text in the REPL without submitting it.
    #[serde(default = "default_true")]
    pub submit: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct BroadcastPromptParams {
    /// Only target sessions on this host alias (omit for all hosts).
    pub host: Option<String>,
    /// Only target sessions in this project id (omit for all projects).
    pub project_id: Option<i64>,
    /// Only target sessions whose claude_status equals this (omit for any).
    pub status: Option<String>,
    /// The prompt text to deliver to every matching session.
    pub prompt: String,
    /// Press Enter to submit after the literal text. Defaults to true.
    pub submit: Option<bool>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SpawnReviewParams {
    /// Id of the session whose work should be reviewed.
    pub source_session_id: i64,
    /// The review prompt to seed the new review session with.
    pub prompt: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct CaptureSessionParams {
    /// Fleet session id (from list_sessions).
    pub session_id: i64,
    /// Rows of scrollback history to include; omit for just the visible pane.
    pub scrollback_lines: Option<u32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SessionIdParams {
    /// Fleet session id (from list_sessions).
    pub session_id: i64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SessionHistoryParams {
    /// Fleet session id (from list_sessions).
    pub session_id: i64,
    /// Maximum number of (newest-first) events to return. Defaults to 50.
    pub limit: Option<i64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SendMessageParams {
    /// Caller's fleet session id (the sender). Recorded on the inbox row and
    /// included in the pane-delivery header so the recipient can see who
    /// sent it.
    pub from_session_id: i64,
    /// Recipient's fleet session id.
    pub to_session_id: i64,
    /// Message body. Free text; the recipient sees it verbatim.
    pub body: String,
    /// Optional tag — `message` (default), `task`, `reply`, `alert`, …
    pub kind: Option<String>,
    /// When true, also type the message into the recipient's tmux pane with
    /// a `[msg #id from name@host]:` header. The inbox row is written
    /// regardless.
    #[serde(default)]
    pub deliver: bool,
    /// When `deliver`, whether to press Enter after the literal text.
    /// Defaults to true.
    #[serde(default = "default_true")]
    pub submit: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct InboxParams {
    /// Whose inbox to read (the caller's own session id).
    pub session_id: i64,
    /// Only return rows with `read_at IS NULL`. Defaults to false.
    #[serde(default)]
    pub unread_only: bool,
    /// Maximum messages to return, newest-first. Defaults to 50.
    pub limit: Option<i64>,
    /// Mark the returned unread rows as read. Defaults to true — typical
    /// "list and consume" pull. Pass false to peek.
    #[serde(default = "default_true")]
    pub mark_read: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct PeerStatusParams {
    /// Peer's fleet session id (from list_sessions).
    pub session_id: i64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RecreateSessionParams {
    /// Fleet session id (from list_sessions).
    pub session_id: i64,
    /// Recreate even if this is the registered fleet controller. Default false.
    #[serde(default)]
    pub force: bool,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RegisterSelfParams {
    /// Host alias of the calling (controller) session.
    pub host_alias: String,
    /// tmux session name of the calling (controller) session.
    pub tmux_name: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct NewBgSessionParams {
    /// Host alias to launch the background session on.
    pub host_alias: String,
    /// Display name for the session (also its tmux/agent name).
    pub name: String,
    /// Initial prompt for the headless Claude session.
    pub prompt: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RepoPathParams {
    /// Fleet session id (from list_sessions).
    pub session_id: i64,
    /// Worktree-relative file path.
    pub path: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RepoLogParams {
    /// Fleet session id.
    pub session_id: i64,
    /// Show all branches/refs (default true) instead of just HEAD.
    pub all: Option<bool>,
    /// Max commits to return (default 200).
    pub limit: Option<u32>,
    /// Commits to skip (pagination).
    pub skip: Option<u32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RepoCommitParams {
    /// Fleet session id.
    pub session_id: i64,
    /// Commit hash.
    pub hash: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct RepoCommitDiffParams {
    /// Fleet session id.
    pub session_id: i64,
    /// Commit hash.
    pub hash: String,
    /// Worktree-relative file path.
    pub path: String,
}

// --- tools -----------------------------------------------------------------

#[tool_router]
impl FleetTools {
    pub fn new(
        store: Arc<Mutex<Store>>,
        ssh: Arc<SshClient>,
        reg: Arc<CancellationRegistry>,
        tunnels: Arc<crate::service::tunnel::TunnelSupervisor>,
    ) -> Self {
        Self {
            store,
            ssh,
            reg,
            tunnels,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Report claude-fleet backend health: application version, SQLite schema version, and database readiness. Returns JSON."
    )]
    async fn fleet_health(&self) -> Result<CallToolResult, McpError> {
        audit("fleet_health", "");
        ok_json(&health::health_check(&self.store))
    }

    // ---- hosts ----

    #[tool(description = "List all registered hosts with their reachability, \
        claude/tmux versions, and linked account. Returns JSON.")]
    async fn list_hosts(&self) -> Result<CallToolResult, McpError> {
        audit("list_hosts", "");
        ok_json(&hosts::list_hosts(&self.store).map_err(to_mcp_err)?)
    }

    #[tool(description = "Discover SSH hosts from the user's ~/.ssh/config. \
        These are candidates for add_host. Returns JSON.")]
    async fn discover_hosts(&self) -> Result<CallToolResult, McpError> {
        audit("discover_hosts", "");
        ok_json(&hosts::discover_hosts().map_err(to_mcp_err)?)
    }

    #[tool(description = "List the cached Claude accounts seen across hosts. \
        Returns JSON.")]
    async fn list_accounts(&self) -> Result<CallToolResult, McpError> {
        audit("list_accounts", "");
        ok_json(&hosts::list_accounts(&self.store).map_err(to_mcp_err)?)
    }

    #[tool(description = "Register a new SSH host. Probes it first; only \
        persists the host if it is reachable. Returns the host row as JSON.")]
    async fn add_host(
        &self,
        Parameters(p): Parameters<AddHostParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "add_host",
            &format!("alias={} ssh_alias={}", p.alias, p.ssh_alias),
        );
        let args = hosts::AddHostArgs {
            alias: p.alias,
            ssh_alias: p.ssh_alias,
        };
        let row = hosts::add_host(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Re-probe a registered host's reachability and \
        versions. Returns the updated host row as JSON.")]
    async fn probe_host(
        &self,
        Parameters(p): Parameters<HostAliasParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("probe_host", &format!("alias={}", p.alias));
        let args = hosts::HostAliasArgs { alias: p.alias };
        let row = hosts::probe_host(args, &self.store, &self.ssh, &self.reg)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Remove a registered host. Its sessions are orphaned. \
        Returns the removed host row as JSON.")]
    async fn remove_host(
        &self,
        Parameters(p): Parameters<HostAliasParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("remove_host", &format!("alias={}", p.alias));
        let args = hosts::HostAliasArgs { alias: p.alias };
        ok_json(&hosts::remove_host(args, &self.store).map_err(to_mcp_err)?)
    }

    #[tool(description = "Hide or show a host. Hidden hosts are skipped during \
        reconcile. Returns the updated host row as JSON.")]
    async fn hide_host(
        &self,
        Parameters(p): Parameters<HideHostParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "hide_host",
            &format!("alias={} hidden={}", p.alias, p.hidden),
        );
        let args = hosts::HideHostArgs {
            alias: p.alias,
            hidden: p.hidden,
        };
        ok_json(&hosts::hide_host(args, &self.store).map_err(to_mcp_err)?)
    }

    // ---- projects ----

    #[tool(description = "List all discovered projects with their worktrees. \
        Returns JSON.")]
    async fn list_projects(&self) -> Result<CallToolResult, McpError> {
        audit("list_projects", "");
        ok_json(&projects::list_projects(&self.store).map_err(to_mcp_err)?)
    }

    #[tool(description = "Rescan the local projects directory for new or \
        removed repositories and worktrees. Returns the fresh project list.")]
    async fn refresh_projects(&self) -> Result<CallToolResult, McpError> {
        audit("refresh_projects", "");
        ok_json(
            &projects::refresh_projects(&self.store)
                .await
                .map_err(to_mcp_err)?,
        )
    }

    // ---- sessions ----

    #[tool(description = "Reconcile and list tmux sessions across reachable \
        hosts. Returns slim summary rows by default (id, host_alias, \
        tmux_name, project_id, worktree_id, status, claude_status, \
        stuck_kind, lost_at, is_controller) to fit MCP token caps on large \
        fleets — pass summary=false for full SessionRow. Optional filters: \
        host_alias, project_id, status, claude_status, include_lost (default \
        false drops sessions with a non-null lost_at). JSON.")]
    async fn list_sessions(
        &self,
        Parameters(p): Parameters<ListSessionsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "list_sessions",
            &format!(
                "host={:?} project={:?} status={:?} claude_status={:?} include_lost={} summary={}",
                p.host_alias, p.project_id, p.status, p.claude_status, p.include_lost, p.summary,
            ),
        );
        let rows = sessions::list_sessions(&self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        let controller = {
            let s = self
                .store
                .lock()
                .map_err(|_| McpError::internal_error("E_LOCK: store mutex poisoned", None))?;
            s.get_controller()
                .map_err(|e| to_mcp_err(IpcError::from(e)))?
        };
        let tagged = rows
            .into_iter()
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
                true
            })
            .map(|row| {
                let is_controller = controller
                    .as_ref()
                    .is_some_and(|(h, t)| *h == row.host_alias && *t == row.tmux_name);
                SessionWithController { is_controller, row }
            });
        if p.summary {
            let slim: Vec<SessionSummary> = tagged.map(SessionSummary::from).collect();
            ok_json_compact(&slim)
        } else {
            let full: Vec<SessionWithController> = tagged.collect();
            ok_json_compact(&full)
        }
    }

    #[tool(description = "List sessions related to a given session — those \
        sharing the same project and worktree. Returns JSON.")]
    async fn related_sessions(
        &self,
        Parameters(p): Parameters<RelatedSessionsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("related_sessions", &format!("session_id={}", p.session_id));
        let args = sessions::RelatedSessionsArgs {
            session_id: p.session_id,
        };
        ok_json(&sessions::related_sessions(args, &self.store).map_err(to_mcp_err)?)
    }

    #[tool(description = "Mark the calling session as the fleet controller; \
        kill/recreate/restart refuse to target it without force.")]
    async fn register_self(
        &self,
        Parameters(p): Parameters<RegisterSelfParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "register_self",
            &format!("host={} tmux={}", p.host_alias, p.tmux_name),
        );
        {
            let s = self
                .store
                .lock()
                .map_err(|_| McpError::internal_error("E_LOCK: store mutex poisoned", None))?;
            s.set_controller(&p.host_alias, &p.tmux_name)
                .map_err(|e| to_mcp_err(IpcError::from(e)))?;
        }
        ok_json(&serde_json::json!({
            "controller": { "host_alias": p.host_alias, "tmux_name": p.tmux_name }
        }))
    }

    #[tool(description = "Create a new Claude Code tmux session on a host, in \
        the given project (and optional worktree). Auto-clones the repo on \
        remote hosts if missing. Returns the new session row as JSON.")]
    async fn new_session(
        &self,
        Parameters(p): Parameters<NewSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "new_session",
            &format!("host={} name={}", p.host_alias, p.name),
        );
        let args = sessions::NewSessionArgs {
            host_alias: p.host_alias,
            project_id: p.project_id,
            worktree_id: p.worktree_id,
            name: p.name,
            call_id: None,
            // v1 of the MCP surface spawns a normal Claude session in an
            // existing project/worktree; new-branch and shell kinds are not
            // exposed yet.
            new_worktree: None,
            kind: None,
            start_command: None,
        };
        let row = sessions::new_session(args, &self.store, &self.ssh, &self.reg)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Kill a tmux session on a host. Returns the killed \
        session's id.")]
    async fn kill_session(
        &self,
        Parameters(p): Parameters<KillSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "kill_session",
            &format!("host={} name={}", p.host_alias, p.name),
        );
        let args = sessions::KillSessionArgs {
            host_alias: p.host_alias,
            name: p.name,
            force: p.force,
        };
        let id = sessions::kill_session(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&id)
    }

    #[tool(description = "Rename a tmux session on a host. Returns the updated \
        session row as JSON.")]
    async fn rename_session(
        &self,
        Parameters(p): Parameters<RenameSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "rename_session",
            &format!("host={} {} -> {}", p.host_alias, p.old_name, p.new_name),
        );
        let args = sessions::RenameSessionArgs {
            host_alias: p.host_alias,
            old_name: p.old_name,
            new_name: p.new_name,
        };
        let row = sessions::rename_session(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Restart a tmux session (kill and recreate it in the \
        same place). Returns the updated session row as JSON.")]
    async fn restart_session(
        &self,
        Parameters(p): Parameters<RestartSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "restart_session",
            &format!("host={} name={}", p.host_alias, p.name),
        );
        let args = sessions::RestartSessionArgs {
            host_alias: p.host_alias,
            name: p.name,
            force: p.force,
        };
        let row = sessions::restart_session(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Send and SUBMIT a prompt to a running Claude \
        session's REPL (literal text, then one Enter). This is how you steer a \
        session. Set submit=false to stage text in the REPL without submitting \
        it.")]
    async fn send_prompt(
        &self,
        Parameters(p): Parameters<SendPromptParams>,
    ) -> Result<CallToolResult, McpError> {
        // Prompt body intentionally not logged.
        audit(
            "send_prompt",
            &format!("host={} session={}", p.host_alias, p.tmux_name),
        );
        let args = sessions::SendPromptArgs {
            host_alias: p.host_alias,
            tmux_name: p.tmux_name,
            prompt: p.prompt,
            submit: p.submit,
        };
        sessions::send_prompt(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        Ok(CallToolResult::success(vec![text_content(
            "prompt delivered",
        )]))
    }

    #[tool(description = "Send the same prompt to every matching work session \
        (excludes the controller). Returns per-session results.")]
    async fn broadcast_prompt(
        &self,
        Parameters(p): Parameters<BroadcastPromptParams>,
    ) -> Result<CallToolResult, McpError> {
        // Prompt body intentionally not logged.
        audit(
            "broadcast_prompt",
            &format!(
                "host={:?} project_id={:?} status={:?}",
                p.host, p.project_id, p.status
            ),
        );
        let filter = sessions::BroadcastFilter {
            host: p.host,
            project_id: p.project_id,
            status: p.status,
        };
        let submit = p.submit.unwrap_or(true);
        let summary = sessions::broadcast_prompt(filter, p.prompt, submit, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&summary)
    }

    #[tool(description = "Spawn a review session: a new Claude session in the \
        source session's worktree, seeded with a review prompt. Returns the \
        new review session row as JSON.")]
    async fn spawn_review(
        &self,
        Parameters(p): Parameters<SpawnReviewParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "spawn_review",
            &format!("source_session_id={}", p.source_session_id),
        );
        let args = sessions::SpawnReviewArgs {
            source_session_id: p.source_session_id,
            prompt: p.prompt,
            call_id: None,
        };
        let row = sessions::spawn_review(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Capture a session's terminal output — the visible \
        tmux pane, or include scrollback history. Use after send_prompt to read \
        the session's reply. Returns the pane text.")]
    async fn capture_session(
        &self,
        Parameters(p): Parameters<CaptureSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("capture_session", &format!("session_id={}", p.session_id));
        let text = sessions::capture_session_output(
            p.session_id,
            &self.store,
            &self.ssh,
            p.scrollback_lines,
        )
        .await
        .map_err(to_mcp_err)?;
        // A blank pane (fresh/cleared session) yields empty output. Returning it
        // verbatim would put an empty text block into the caller's conversation;
        // say so explicitly instead. `text_content` is the backstop for any
        // residual whitespace-only capture.
        if text.trim().is_empty() {
            return Ok(CallToolResult::success(vec![text_content(
                "(session pane is empty — nothing to capture)",
            )]));
        }
        ok_json(&text)
    }

    #[tool(
        description = "Return the recorded event timeline for a session (status \
        changes, prompts, stuck, kills). Newest-first; pass `limit` to cap \
        (default 50). Returns the events as JSON."
    )]
    async fn session_history(
        &self,
        Parameters(p): Parameters<SessionHistoryParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("session_history", &format!("session_id={}", p.session_id));
        let limit = p.limit.unwrap_or(50);
        let events = {
            let s = self
                .store
                .lock()
                .map_err(|_| to_mcp_err(IpcError::new("E_LOCK", "store mutex poisoned")))?;
            s.list_session_events(p.session_id, limit)
                .map_err(to_mcp_err)?
        };
        ok_json(&events)
    }

    #[tool(
        description = "Send a peer-to-peer message from one session to another. \
        The message is persisted to the recipient's inbox (read with `inbox`); \
        set `deliver: true` to ALSO type the message into the recipient's tmux \
        pane with a `[msg #id from name@host]:` header. The inbox row is the \
        source of truth — it lands even if the pane delivery fails. Returns \
        JSON with the new message id and the delivery outcome."
    )]
    async fn send_message(
        &self,
        Parameters(p): Parameters<SendMessageParams>,
    ) -> Result<CallToolResult, McpError> {
        // Body intentionally not logged.
        audit(
            "send_message",
            &format!(
                "from={} to={} kind={:?} deliver={}",
                p.from_session_id, p.to_session_id, p.kind, p.deliver
            ),
        );
        let args = crate::service::messages::SendMessageArgs {
            from_session_id: p.from_session_id,
            to_session_id: p.to_session_id,
            body: p.body,
            kind: p.kind,
            deliver: p.deliver,
            submit: p.submit,
        };
        let result = crate::service::messages::send_message(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&result)
    }

    #[tool(description = "Read the caller session's inbox. Returns the messages \
        sent TO `session_id`, newest-first; `unread_only` filters and \
        `mark_read` (default true) flips the returned unread rows to read. \
        Pass `mark_read: false` to peek without consuming.")]
    async fn inbox(
        &self,
        Parameters(p): Parameters<InboxParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "inbox",
            &format!(
                "session_id={} unread_only={} mark_read={}",
                p.session_id, p.unread_only, p.mark_read
            ),
        );
        let limit = p.limit.unwrap_or(50);
        let msgs = crate::service::messages::list_inbox(
            p.session_id,
            p.unread_only,
            limit,
            p.mark_read,
            &self.store,
        )
        .map_err(to_mcp_err)?;
        ok_json(&msgs)
    }

    #[tool(description = "What is a peer session currently doing? Returns the \
        reconcile-derived `claude_status`, `current_activity`, `stuck_kind`, \
        and `context_pct` for one session, plus its host/name/status. Useful \
        before sending a message or broadcasting work.")]
    async fn peer_status(
        &self,
        Parameters(p): Parameters<PeerStatusParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("peer_status", &format!("session_id={}", p.session_id));
        let status =
            crate::service::messages::peer_status(p.session_id, &self.store).map_err(to_mcp_err)?;
        ok_json(&status)
    }

    #[tool(
        description = "Peek at a session's background Claude logs. Returns an \
        informational message for interactive sessions with no background job."
    )]
    async fn peek_session(
        &self,
        Parameters(p): Parameters<SessionIdParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("peek_session", &format!("session_id={}", p.session_id));
        let (host_alias, claude_id) = {
            let s = self
                .store
                .lock()
                .map_err(|_| to_mcp_err(IpcError::new("E_LOCK", "store mutex poisoned")))?;
            let row = s
                .get_session_by_id(p.session_id)
                .map_err(|e| to_mcp_err(IpcError::from(e)))?
                .ok_or_else(|| to_mcp_err(IpcError::new("E_NOTFOUND", "session not found")))?;
            (row.host_alias, row.claude_session_id)
        };
        let Some(claude_id) = claude_id else {
            return ok_json(
                &"This session has no Claude session id yet — nothing to peek.".to_string(),
            );
        };
        let logs = crate::service::bg_sessions::peek_session(
            crate::service::bg_sessions::PeekSessionArgs {
                host_alias,
                claude_session_id: claude_id,
            },
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&logs)
    }

    #[tool(description = "Recreate a session: kill its tmux session and rebuild \
        it fresh in the same worktree, resuming the same Claude conversation. \
        Works for running or ghost sessions. Returns the session row as JSON.")]
    async fn recreate_session(
        &self,
        Parameters(p): Parameters<RecreateSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("recreate_session", &format!("session_id={}", p.session_id));
        let row = sessions::recreate_session(
            sessions::RecreateSessionArgs {
                session_id: p.session_id,
                force: p.force,
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Dismiss a ghost session (lost from tmux): permanently \
        delete its row. Errors if the session is not a ghost.")]
    async fn dismiss_ghost_session(
        &self,
        Parameters(p): Parameters<SessionIdParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "dismiss_ghost_session",
            &format!("session_id={}", p.session_id),
        );
        sessions::dismiss_ghost_session(
            sessions::DismissGhostSessionArgs {
                session_id: p.session_id,
            },
            &self.store,
        )
        .map_err(to_mcp_err)?;
        ok_json(&serde_json::json!({ "dismissed": p.session_id }))
    }

    #[tool(description = "Launch a supervised headless (background) Claude \
        session on a host with an initial prompt. Returns the new Claude \
        session id as JSON; track progress with peek_session.")]
    async fn new_bg_session(
        &self,
        Parameters(p): Parameters<NewBgSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "new_bg_session",
            &format!("host={} name={}", p.host_alias, p.name),
        );
        let res = crate::service::bg_sessions::new_bg_session(
            crate::service::bg_sessions::NewBgSessionArgs {
                host_alias: p.host_alias,
                name: p.name,
                prompt: p.prompt,
            },
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&res)
    }

    #[tool(description = "List a session's changed files (git status) in its \
        worktree. Returns JSON array of changed files.")]
    async fn repo_changes(
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
    async fn repo_tree(
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
    async fn repo_file(
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
    async fn repo_diff(
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
        all=true (default) includes every branch. Returns JSON array of commits \
        with parents + ref decorations.")]
    async fn repo_log(
        &self,
        Parameters(p): Parameters<RepoLogParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("repo_log", &format!("session_id={}", p.session_id));
        let v = crate::commands::history::repo_log_impl(
            crate::commands::history::RepoLogArgs {
                session_id: p.session_id,
                all: p.all.unwrap_or(true),
                limit: p.limit.unwrap_or(0),
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
    async fn repo_branches(
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
    async fn repo_commit(
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
    async fn repo_commit_diff(
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

    #[tool(description = "Install the claude-fleet-control skill and register \
        this fleet's MCP server into every reachable host's Claude config \
        (~/.claude.json), with a reverse SSH tunnel for remote hosts. Returns a \
        per-host status list. Each host must restart Claude to load the server.")]
    async fn provision_hosts(&self) -> Result<CallToolResult, McpError> {
        audit("provision_hosts", "");
        let (port, token) = {
            let s = self
                .store
                .lock()
                .map_err(|_| to_mcp_err(IpcError::new("E_LOCK", "store mutex poisoned")))?;
            let port = s
                .get_setting(crate::mcp::SETTING_PORT)
                .map_err(|e| to_mcp_err(IpcError::from(e)))?
                .and_then(|p| p.parse().ok())
                .unwrap_or(crate::mcp::DEFAULT_PORT);
            let token = s
                .get_setting(crate::mcp::SETTING_TOKEN)
                .map_err(|e| to_mcp_err(IpcError::from(e)))?
                .unwrap_or_default();
            (port, token)
        };
        if token.is_empty() {
            return Err(to_mcp_err(IpcError::new(
                "E_PROVISION",
                "control API has no token yet",
            )));
        }
        let res = crate::service::provision::provision_hosts(
            &self.store,
            &self.ssh,
            &self.tunnels,
            port,
            &token,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&res)
    }
}

#[tool_handler]
impl ServerHandler for FleetTools {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions(
                "claude-fleet control API. Drives long-lived Claude Code sessions \
                 running in tmux across multiple hosts. Call list_sessions to see \
                 fleet state, new_session to spawn one, and send_prompt to steer it."
                    .to_string(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(c: &Content) -> &str {
        c.as_text().expect("text content").text.as_str()
    }

    #[test]
    fn text_content_substitutes_for_empty_and_whitespace() {
        assert_eq!(text_of(&text_content("")), EMPTY_RESULT_PLACEHOLDER);
        assert_eq!(text_of(&text_content("   ")), EMPTY_RESULT_PLACEHOLDER);
        assert_eq!(text_of(&text_content("\n\t  \n")), EMPTY_RESULT_PLACEHOLDER);
    }

    #[test]
    fn text_content_preserves_real_text() {
        assert_eq!(text_of(&text_content("hello")), "hello");
        // Surrounding whitespace is kept once there is real content.
        assert_eq!(text_of(&text_content("  hi  ")), "  hi  ");
    }

    #[test]
    fn strip_nulls_drops_nulls_recursively() {
        let mut v = serde_json::json!({
            "a": 1,
            "b": null,
            "nested": { "x": null, "y": "keep" },
            "arr": [{ "k": null, "v": 2 }, { "k": "kept", "v": null }],
        });
        strip_nulls(&mut v);
        assert_eq!(
            v,
            serde_json::json!({
                "a": 1,
                "nested": { "y": "keep" },
                "arr": [{ "v": 2 }, { "k": "kept" }],
            })
        );
    }

    #[test]
    fn ok_json_compact_is_compact_and_strips_nulls() {
        let v = serde_json::json!({ "a": 1, "b": null, "c": [1, 2] });
        let r = ok_json_compact(&v).unwrap();
        let text = text_of(&r.content[0]);
        assert!(!text.contains('\n'), "expected compact JSON, got: {text}");
        assert!(!text.contains("null"), "null fields must be stripped: {text}");
        assert!(text.contains("\"a\":1"));
    }

    #[test]
    fn ok_json_never_emits_an_empty_text_block() {
        // Even degenerate values must serialize to a non-empty text block, so a
        // tool result can never poison the caller's conversation with an empty
        // block (which the Anthropic API rejects, fatally so under caching).
        for r in [
            ok_json(&"").unwrap(),
            ok_json(&String::new()).unwrap(),
            ok_json(&serde_json::json!(null)).unwrap(),
            ok_json(&Vec::<i32>::new()).unwrap(),
        ] {
            let block = &r.content[0];
            assert!(
                !text_of(block).trim().is_empty(),
                "ok_json produced an empty text block: {:?}",
                text_of(block)
            );
        }
    }
}
