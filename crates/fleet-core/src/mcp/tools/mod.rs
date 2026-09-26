//! MCP tool surface for claude-fleet.
//!
//! `FleetTools` holds the shared backend state (`Store`, `SshClient`,
//! `CancellationRegistry`) and exposes claude-fleet operations as MCP tools.
//! Every tool calls into the transport-agnostic `service` layer — the exact
//! same code path the Tauri IPC commands use; neither path is privileged.
//!
//! Tool arguments derive `JsonSchema` so the AI sees a typed schema. Where a
//! tool's parameters are exactly a `service::*Args` struct it takes that
//! struct directly; the MCP-specific structs in `params` cover the rest
//! (optional `session_id` OR host+name addressing, `confirm_nonce` gates,
//! MCP-side defaults). The frontend's `call_id` cancellation field is never
//! exposed — MCP tool calls run to completion.

use super::auth::{Caller, TokenMode};
use super::guard::{self, ConfirmState};
use super::McpGuards;
use crate::cancel::CancellationRegistry;
use crate::ipc_error::{codes, IpcError};
use crate::service::pane_intel::{ClaudeStatus, StuckKind};
use crate::service::{
    catalog, fresh, health, hosts, projects, quick_replies, safe_kill, sessions, tasks, transcript,
    usage, worktrees,
};
use crate::ssh::SshClient;
use crate::store::Store;
use rmcp::{
    handler::server::{
        router::tool::ToolRouter,
        tool::{Extension, ToolCallContext},
        wrapper::Parameters,
    },
    model::*,
    schemars,
    service::RequestContext,
    tool, tool_router, ErrorData as McpError, RoleServer, ServerHandler,
};
use std::sync::{Arc, Mutex};

mod assets;
mod fleet;
mod lifecycle;
mod list_changed;
mod messaging;
mod orchestration;
mod params;
mod peer;
mod present;
mod repo;
mod session_ops;
mod support;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_isolation;
#[cfg(test)]
mod tests_read_pool;
mod views;

// `rmcp::model::*` also exports a `CancelTaskParams`. Name ours explicitly:
// an explicit import outranks both globs, here and in every child module
// that reaches it through `use super::*`.
use params::CancelTaskParams;
use params::*;
use support::*;
use views::*;

pub use support::tool_deadline;

/// The tools that read only through [`FleetTools::reader`] — the hub's read
/// pool — and write nothing: their per-host result gate reads there too.
/// (`list_sessions` with `force` or `fresh_for` still writes: a forced pass,
/// a read cursor. Its gate then reads the pool after those commits, which a
/// new read transaction sees.)
const POOLED_READ_TOOLS: [&str; 6] = [
    "list_hosts",
    "whoami",
    "list_sessions",
    "list_worktrees",
    "list_projects",
    "fleet_health",
];

/// The MCP server handler. Cloned per session by the streamable-HTTP service;
/// every clone shares the same backend state via the `Arc`s.
#[derive(Clone)]
pub struct FleetTools {
    store: Arc<Mutex<Store>>,
    /// The hub's read-only connections on the same file (see
    /// [`FleetTools::reader`]). `None` on the desktop and in tests.
    read_pool: Option<Arc<crate::store::ReadPool>>,
    ssh: Arc<SshClient>,
    reg: Arc<CancellationRegistry>,
    tunnels: Arc<crate::service::tunnel::TunnelSupervisor>,
    /// Rate limiter, pending confirmations and the desktop notifier.
    guards: McpGuards,
    /// Per-caller cap on concurrent bounded waits (S4). Created once in
    /// `new`; every per-MCP-session clone shares it.
    long_polls: Arc<guard::LongPollLimiter>,
    /// `send_prompt` results by `(caller label, client_msg_id)`, so a
    /// retried call delivers once. Created once in `new`; every per-MCP-
    /// session clone shares it.
    recent_sends: Arc<std::sync::Mutex<RecentSends>>,
    /// The tool list each caller is known to hold, for
    /// `notifications/tools/list_changed`. Created once in `new`; both mounts
    /// and every per-request clone share it.
    list_changed: Arc<list_changed::ToolListTracker>,
    /// True on the SSE mount only: `/mcp/json` answers with the FIRST message
    /// the handler sends, so a notification there would replace the result.
    push_list_changed: bool,
    tool_router: ToolRouter<FleetTools>,
}

/// Where one `client_msg_id` stands.
///
/// The `Pending` arm is the whole point: a dedupe cache that is only written
/// AFTER the send has returned cannot dedupe the window a retry actually
/// lands in. A prompt takes an SSH round trip plus up to 1.5 s of ack wait,
/// and the caller that gives up and retries does so DURING that, not after —
/// so a write-on-success cache delivers twice and then reports one result.
#[derive(Debug)]
pub(super) enum SendOutcome {
    /// Reserved by a send that has not finished. Nothing was returned yet.
    Pending,
    /// The result the first send returned; every repeat gets this back.
    Done(serde_json::Value),
}

/// What [`RecentSends::reserve`] found for a key.
#[derive(Debug)]
pub(super) enum Reservation {
    /// Nobody holds this key: the caller now does, and MUST end it with
    /// [`RecentSends::complete`] or [`RecentSends::release`].
    Fresh,
    /// A send with this key is still running.
    Pending,
    /// It already ran; this is what it returned.
    Done(serde_json::Value),
}

/// Bounded, TTL'd memory of `send_prompt` keys and results (S-dedupe).
#[derive(Default)]
pub(super) struct RecentSends {
    entries: std::collections::HashMap<(String, String), (std::time::Instant, SendOutcome)>,
}

/// How long a completed result is replayed to a repeat of its id.
pub(super) const RECENT_SENDS_TTL: std::time::Duration = std::time::Duration::from_secs(600);
/// How long a reservation may be held before the sweep takes it back. A send
/// is bounded by its SSH timeout plus the ack wait, well under this; anything
/// still `Pending` after it is a task that died between `reserve` and its
/// `complete`/`release`, and it must not pin the key for the result TTL.
pub(super) const PENDING_TTL: std::time::Duration = std::time::Duration::from_secs(60);
pub(super) const RECENT_SENDS_MAX: usize = 1024;

impl RecentSends {
    /// Claim `id` for `caller`, or say who already has it.
    pub(super) fn reserve(&mut self, caller: &str, id: &str) -> Reservation {
        self.sweep();
        let key = (caller.to_string(), id.to_string());
        match self.entries.get(&key) {
            Some((_, SendOutcome::Done(v))) => return Reservation::Done(v.clone()),
            Some((_, SendOutcome::Pending)) => return Reservation::Pending,
            None => {}
        }
        self.make_room();
        self.entries
            .insert(key, (std::time::Instant::now(), SendOutcome::Pending));
        Reservation::Fresh
    }

    /// The reserved send returned `value`: replay it to every repeat.
    pub(super) fn complete(&mut self, caller: &str, id: &str, value: serde_json::Value) {
        self.entries.insert(
            (caller.to_string(), id.to_string()),
            (std::time::Instant::now(), SendOutcome::Done(value)),
        );
    }

    /// The reserved send failed, so nothing happened under this key and a
    /// retry with it must be allowed to deliver — which is the one thing
    /// `client_msg_id` is for.
    pub(super) fn release(&mut self, caller: &str, id: &str) {
        self.entries.remove(&(caller.to_string(), id.to_string()));
    }

    /// Keep the map bounded under a chatty client: drop the oldest COMPLETED
    /// entry, and only when there is none, the oldest entry of any kind.
    /// Evicting a live reservation costs the dedupe guarantee for that key,
    /// so it is the last resort rather than the first.
    fn make_room(&mut self) {
        if self.entries.len() < RECENT_SENDS_MAX {
            return;
        }
        let oldest_done = self
            .entries
            .iter()
            .filter(|(_, (_, out))| matches!(out, SendOutcome::Done(_)))
            .min_by_key(|(_, (at, _))| *at)
            .map(|(k, _)| k.clone());
        let victim = oldest_done.or_else(|| {
            self.entries
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone())
        });
        if let Some(k) = victim {
            self.entries.remove(&k);
        }
    }

    fn sweep(&mut self) {
        let now = std::time::Instant::now();
        self.entries.retain(|_, (at, out)| {
            let ttl = match out {
                SendOutcome::Done(_) => RECENT_SENDS_TTL,
                SendOutcome::Pending => PENDING_TTL,
            };
            now.duration_since(*at) < ttl
        });
    }

    /// Age an entry, for the sweep's tests: `Instant` cannot be constructed
    /// in the past and this map is not on a clock a test can pause.
    #[cfg(test)]
    pub(super) fn backdate(&mut self, caller: &str, id: &str, by: std::time::Duration) {
        if let Some((at, _)) = self.entries.get_mut(&(caller.to_string(), id.to_string())) {
            *at = at.checked_sub(by).expect("test durations are small");
        }
    }
}

/// Lock `recent_sends` tolerating poison.
///
/// The map is a cache, not a consistency boundary: a panic somewhere between
/// `reserve` and `complete` leaves one key `Pending`, which the sweep takes
/// back within [`PENDING_TTL`]. Refusing every later send because of that —
/// which is what `.unwrap()` would do, by panicking again — is strictly worse
/// than carrying on with a slightly stale cache.
pub(super) fn lock_sends(
    m: &std::sync::Mutex<RecentSends>,
) -> std::sync::MutexGuard<'_, RecentSends> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Server-level instructions handed to every MCP client on `initialize`.
///
/// The status vocabularies are rendered from `ClaudeStatus::vocabulary_doc()`
/// / `StuckKind::vocabulary_doc()` so they cannot drift from the enums. Keep
/// the surrounding wording stable — every edit invalidates connected clients'
/// cached tool definitions.
fn server_instructions() -> String {
    format!(
        "claude-fleet control API. Drives long-lived Claude Code sessions \
         running in tmux across multiple hosts. Call list_sessions to see \
         fleet state, new_session to spawn one, and send_prompt to steer it. \
         Session rows carry two status fields: claude_status is one of {} \
         (null when unknown), and stuck_kind is one of {} (null when not stuck).",
        ClaudeStatus::vocabulary_doc(),
        StuckKind::vocabulary_doc(),
    )
}

// --- tools -----------------------------------------------------------------

impl FleetTools {
    pub fn new(
        store: Arc<Mutex<Store>>,
        ssh: Arc<SshClient>,
        reg: Arc<CancellationRegistry>,
        tunnels: Arc<crate::service::tunnel::TunnelSupervisor>,
        guards: McpGuards,
    ) -> Self {
        Self {
            store,
            read_pool: None,
            ssh,
            reg,
            tunnels,
            guards,
            long_polls: guard::LongPollLimiter::new(guard::MAX_LONG_POLLS_PER_CALLER),
            recent_sends: Arc::new(std::sync::Mutex::new(RecentSends::default())),
            list_changed: Arc::default(),
            push_list_changed: false,
            tool_router: Self::tool_router(),
        }
    }

    /// Read the plain listing tools through `pool` instead of the writer
    /// (the hub). `None` leaves them on the writer.
    pub fn with_read_pool(mut self, pool: Option<Arc<crate::store::ReadPool>>) -> Self {
        self.read_pool = pool;
        self
    }

    /// Where a tool reads when it has no write of its own to see: a pooled
    /// read-only connection on the hub — never waiting on a reconcile pass
    /// or a hook transaction holding the writer — else the writer. A tool
    /// that writes and then reads in the same call keeps reading through
    /// `self.store`, so it sees its own write under the same lock order.
    pub(super) fn reader(&self) -> &Mutex<Store> {
        crate::store::read_via(self.read_pool.as_deref(), &self.store)
    }

    /// This handler as mounted with `framing`: only an SSE response can carry
    /// a notification ahead of the result, so only there is `listChanged`
    /// advertised and sent.
    pub(crate) fn for_framing(mut self, framing: super::Framing) -> Self {
        self.push_list_changed = framing == super::Framing::Sse;
        self
    }

    /// Fingerprint of the tool names `caller` may see — the same filter
    /// `list_tools` applies, over names only so a `tools/call` does not clone
    /// every schema.
    fn visible_fingerprint(&self, caller: &Caller) -> u64 {
        list_changed::fingerprint(
            self.tool_router
                .map
                .keys()
                .map(|k| k.as_ref())
                .filter(|name| present::visible_to(caller, name)),
        )
    }

    /// Tell `caller` its cached tool list is stale, on this call's own SSE
    /// stream, ahead of the result — see `list_changed`. A send failure only
    /// means the client went away; the call itself goes on.
    async fn notice_list_changed(&self, caller: &Caller, context: &RequestContext<RoleServer>) {
        if !self.push_list_changed || !list_changed::wants_notification(caller) {
            return;
        }
        let current = self.visible_fingerprint(caller);
        if self.list_changed.needs_notice(&caller.label(), current) {
            let _ = context.peer.notify_tool_list_changed().await;
        }
    }

    /// Every tool, summed from the per-domain `#[tool_router]` blocks. Both
    /// `new()` and `tool_router_for_doc()` call this, so the tools served and
    /// the generated reference cannot drift apart.
    fn tool_router() -> ToolRouter<FleetTools> {
        Self::fleet_router()
            + Self::session_ops_router()
            + Self::lifecycle_router()
            + Self::messaging_router()
            + Self::orchestration_router()
            + Self::repo_router()
            + Self::assets_router()
            + Self::peer_router()
    }
}

/// Hand-written (not `#[tool_handler]`) so every call passes through one
/// gate: readonly-mode enforcement and the persisted audit row happen here,
/// before the router dispatches to the tool. The [`Caller`] is then made
/// available to tools as an `Extension<Caller>` extractor.
impl ServerHandler for FleetTools {
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        mut context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        // Fail closed: a request that somehow bypassed the auth middleware
        // has no caller and gets nothing.
        let caller = match caller_from_context(&context) {
            Some(c) => c,
            None => {
                return tool_error_result(mcp_err(
                    "E_FORBIDDEN",
                    "request carries no caller identity",
                    None,
                ))
            }
        };
        let tool = request.name.to_string();
        let label = caller.label();
        // Audit first so refused calls are on the timeline too.
        persist_audit(&self.store, &tool, request.arguments.as_ref(), &caller);
        // Before the gates: a call refused for a tool the caller cannot see is
        // the likeliest sign that its list is stale.
        self.notice_list_changed(&caller, &context).await;
        if let Err(e) = enforce_mode(&caller, &tool).and_then(|()| enforce_admin(&caller, &tool)) {
            return tool_error_result(e);
        }
        context.extensions.insert(caller.clone());
        let tcc = ToolCallContext::new(self, request, context);
        // Tool-execution failures travel as `is_error` results; only rmcp's
        // own protocol errors (unknown tool, bad arguments) stay JSON-RPC.
        let answer = bounded(&tool, tool_deadline(&tool), self.tool_router.call(tcc)).await;
        // One counter write per call, on the same dimension the rate limiter
        // and the stream cap key on — so a number on `/metrics` lines up with
        // a refusal in the log. A tool failure travels as an `is_error`
        // RESULT rather than an `Err`, so both shapes are read here or the
        // error count would only ever see rmcp's own protocol faults.
        let failed = match &answer {
            Ok(r) => r.is_error.unwrap_or(false),
            Err(_) => true,
        };
        self.guards.metrics.record_call(&label, failed);
        let mut out = match answer {
            Ok(result) => Ok(result),
            Err(e) => tool_error_result(e),
        };
        // Work graph M5: whatever tool answered — a result or an error's
        // details — a per-host token never receives session rows' work of
        // another org.
        if let (Ok(result), true) = (out.as_mut(), caller.host_alias.is_some()) {
            // A tool that wrote nothing has no write of its own to see, so
            // its gate reads the pool too; every other tool's reads the
            // writer, after its own writes.
            let orgs = if POOLED_READ_TOOLS.contains(&tool.as_str()) {
                self.reader()
            } else {
                &self.store
            };
            self.redact_work_via(orgs, &caller, result);
        }
        out
    }

    /// The tools this caller may actually call, slimmed and annotated —
    /// see `present`. Fails closed like `call_tool`: a request that reached
    /// here without a caller identity is served nothing.
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let Some(caller) = caller_from_context(&context) else {
            return Ok(ListToolsResult {
                tools: Vec::new(),
                meta: None,
                next_cursor: None,
            });
        };
        let tools: Vec<Tool> = self
            .tool_router
            .list_all()
            .into_iter()
            .filter(|t| present::visible_to(&caller, &t.name))
            .map(present::present)
            .collect();
        self.list_changed.listed(
            &caller.label(),
            list_changed::fingerprint(tools.iter().map(|t| t.name.as_ref())),
        );
        Ok(ListToolsResult {
            tools,
            meta: None,
            next_cursor: None,
        })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router.get(name).cloned().map(present::present)
    }

    fn get_info(&self) -> ServerInfo {
        let tools = ServerCapabilities::builder().enable_tools();
        let capabilities = if self.push_list_changed {
            tools.enable_tool_list_changed().build()
        } else {
            tools.build()
        };
        ServerInfo::new(capabilities)
            .with_server_info(Implementation::from_build_env())
            // 2025-11-25; rmcp negotiates down for a client that asks for an
            // older known revision.
            .with_protocol_version(ProtocolVersion::LATEST)
            .with_instructions(server_instructions())
    }
}

/// Test-only: expose the summed `tool_router()` so `doc_gen` can call
/// `FleetTools::tool_router_for_doc()` without needing access to the private
/// associated function.
#[cfg(test)]
impl FleetTools {
    pub(crate) fn tool_router_for_doc(
    ) -> rmcp::handler::server::router::tool::ToolRouter<FleetTools> {
        Self::tool_router()
    }
}
