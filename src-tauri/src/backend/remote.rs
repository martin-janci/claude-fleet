//! `HubBackend`: every call this app makes to the hub — reads and mutations
//! alike — as the MCP tool the command's
//! [`VERDICTS`](super::verdicts::VERDICTS) row names. [`Self::route`] is the
//! way in; the tool name is never written twice.
//!
//! The hub's tools are built from `fleet-core`'s own service layer, so the
//! JSON they return *is* the row type a local command would have returned.
//! That makes this a deserialisation rather than a translation, and it is why
//! Task 3 can swap the backend under a command without changing its
//! signature.
//!
//! Three things about the wire are easy to get wrong and are handled here
//! rather than at each call site:
//!
//! 1. **The answer is SSE-framed.** `POST /mcp` keeps the streamable-HTTP
//!    framing even for a one-shot call, so the JSON-RPC envelope arrives on
//!    `data:` lines ([`fleet_core::mcp::wire::last_event_payload`]).
//! 2. **The payload is double-encoded.** The envelope's
//!    `result.content[0].text` is a *string* holding the tool's JSON.
//! 3. **List tools strip nulls.** `ok_json_compact` removes every null key
//!    recursively, so absent is the normal encoding of `None` and the row
//!    types tolerate a missing key (`#[serde(default)]`).
//!
//! The bearer token reaches [`HubTransport::post_json`] and nowhere else: it
//! is never logged, never formatted into an error, and [`RemoteConfig`]'s
//! hand-written `Debug` redacts it.

use super::{RemoteConfig, UnavailableHub};
use fleet_core::ipc_error::{codes, IpcError};
use fleet_core::service::transcript::Conversation;
use fleet_core::service::{repair, sessions};
use fleet_core::store::{AccountRow, ConversationRow, HostRow, SessionEvent, SessionRow, TaskRow};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::sync::Arc;

/// What the hub answered: the HTTP status and the body exactly as received,
/// SSE framing still intact. Interpreting it is [`HubBackend`]'s job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubResponse {
    pub status: u16,
    pub body: String,
}

/// One HTTP exchange with the hub.
///
/// A trait, not a concrete client, so the whole tool mapping is testable
/// against recorded responses with no network — which is what the design
/// calls for, and which is why [`TcpTransport`] could grow TLS without a line
/// of the mapping changing.
///
/// `bearer` is the client token. An implementation must put it in the
/// `Authorization` header and must not log it.
#[async_trait::async_trait]
pub trait HubTransport: Send + Sync {
    async fn post_json(&self, url: &str, bearer: &str, body: String)
        -> Result<HubResponse, String>;
}

/// A client of one hub.
pub struct HubBackend {
    cfg: RemoteConfig,
    transport: Arc<dyn HubTransport>,
    /// Set for a hub that is configured but that this launch cannot use:
    /// every call is refused with this, before the transport is touched.
    unavailable: Option<UnavailableHub>,
}

/// Redacting by construction: [`RemoteConfig`]'s own `Debug` hides the token,
/// and the transport is not printed at all.
impl std::fmt::Debug for HubBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubBackend")
            .field("cfg", &self.cfg)
            .finish_non_exhaustive()
    }
}

impl HubBackend {
    /// The real backend, over [`TcpTransport`].
    pub fn new(cfg: RemoteConfig) -> Self {
        Self::with_transport(cfg, Arc::new(TcpTransport))
    }

    pub fn with_transport(cfg: RemoteConfig, transport: Arc<dyn HubTransport>) -> Self {
        Self {
            cfg,
            transport,
            unavailable: None,
        }
    }

    /// A hub that is configured but that this launch cannot use.
    ///
    /// Why a `HubBackend` at all, rather than no hub: every routed command is
    /// `match backend.hub() { Some(h) => …, None => <the standalone call> }`,
    /// and the standalone arm is exactly what must not run — it would SSH
    /// into the hub's hosts with this machine's keys, and `list_sessions`
    /// would reconcile on its own. Presenting a hub that refuses every call
    /// sends all of them down the hub arm, where they fail with the reason,
    /// without one command changing.
    ///
    /// It holds no token and its transport cannot send anything, so there is
    /// nothing to leak and no request to make even if the refusal in
    /// [`Self::call_text`] were ever bypassed.
    pub fn unavailable(hub: UnavailableHub) -> Self {
        Self {
            cfg: RemoteConfig {
                base_url: hub
                    .url
                    .clone()
                    .unwrap_or_else(|| "the configured hub".to_string()),
                token: String::new(),
                client_name: String::new(),
            },
            transport: Arc::new(NoTransport),
            unavailable: Some(hub),
        }
    }

    /// The refusal for a command run while the configured hub cannot be
    /// used, or `None` for a working client. `what` names the command.
    pub fn unavailable_error(&self, what: &str) -> Option<IpcError> {
        self.unavailable.as_ref().map(|hub| {
            IpcError::new(
                codes::E_HUB_UNAVAILABLE,
                format!("{what} was not run: {}", hub.explain()),
            )
        })
    }

    pub fn config(&self) -> &RemoteConfig {
        &self.cfg
    }

    /// Blank the token out of anything that came from outside this process
    /// before it can reach an error message (and from there a log, a panic or
    /// the UI). A transport that names the failing request, or a proxy that
    /// echoes the `Authorization` header back in an error page, would
    /// otherwise publish it — neither is hypothetical, and neither is this
    /// module's to control.
    fn redact(&self, text: &str) -> String {
        if self.cfg.token.is_empty() {
            return text.to_string();
        }
        text.replace(&self.cfg.token, "<redacted>")
    }

    /// [`Self::redact`] over a whole JSON tree — every string value and every
    /// object key, at any depth.
    ///
    /// Recursing over the parsed value rather than redacting its serialised
    /// text is deliberate: `to_string` escapes a token containing a quote or a
    /// backslash, and a search for the raw token would then miss it. Walking
    /// the tree compares against the unescaped strings, so no token spelling
    /// can slip past.
    fn redact_value(&self, value: &Value) -> Value {
        if self.cfg.token.is_empty() {
            return value.clone();
        }
        match value {
            Value::String(s) => Value::String(self.redact(s)),
            Value::Array(items) => {
                Value::Array(items.iter().map(|v| self.redact_value(v)).collect())
            }
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (self.redact(k), self.redact_value(v)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// Call one tool and deserialise its result into `T`.
    ///
    /// Not `pub`: [`Self::route`] is the only way in from outside this
    /// module, so a command can never reach the hub with a hand-written tool
    /// literal behind [`VERDICTS`](super::verdicts::VERDICTS)'s back — the
    /// tool always comes from the row.
    pub(super) async fn call<T: DeserializeOwned>(
        &self,
        tool: &str,
        args: Value,
    ) -> Result<T, IpcError> {
        let text = self.call_text(tool, args).await?;
        serde_json::from_str(&text).map_err(|e| {
            IpcError::new(
                codes::E_PARSE,
                format!(
                    "{tool} on {} returned unreadable JSON: {e}",
                    self.cfg.base_url
                ),
            )
        })
    }

    /// Call one tool and return its result text unparsed — for the tools that
    /// answer prose rather than JSON (`session_transcript`, `capture_session`).
    ///
    /// Not `pub`, for the same reason as [`Self::call`]: only
    /// [`Self::route_text`] calls it from outside this module.
    pub(super) async fn call_text(&self, tool: &str, args: Value) -> Result<String, IpcError> {
        if let Some(refused) = self.unavailable_error(tool) {
            return Err(refused);
        }
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": tool, "arguments": args },
        })
        .to_string();
        let url = format!("{}/mcp", self.cfg.base_url);
        let response = self
            .transport
            .post_json(&url, &self.cfg.token, body)
            .await
            .map_err(|e| {
                IpcError::new(
                    codes::E_HUB_UNREACHABLE,
                    format!("{} did not answer: {}", self.cfg.base_url, self.redact(&e)),
                )
            })?;
        self.read_response(tool, response)
    }

    /// Turn one answered request into the tool's result text or an
    /// [`IpcError`]. Pure, so every branch is a unit test.
    fn read_response(&self, tool: &str, response: HubResponse) -> Result<String, IpcError> {
        let body = response.body;
        match response.status {
            200 => {}
            // The token is no longer accepted: revoked by the operator, or
            // the hub was re-inited. Retrying cannot help, so this is the one
            // error that must send the user back to the Hub settings.
            401 => {
                return Err(IpcError::new(
                    codes::E_UNAUTHORIZED,
                    format!(
                        "the hub revoked this client ({} answered 401) — pair again in Settings",
                        self.cfg.base_url
                    ),
                ))
            }
            // The hub's Host/Origin allowlist, or something in front of it.
            // Carry its own words: the fix (add the host to the allowlist, or
            // reach the hub by the name it expects) is in them, not in ours.
            403 => {
                let said = summarise_body(&body).map(|s| self.redact(&s));
                return Err(IpcError::new(
                    codes::E_FORBIDDEN,
                    match said {
                        Some(s) => format!("{} refused this request (403): {s}", self.cfg.base_url),
                        None => format!(
                            "{} refused this request (403) — its Host or Origin allowlist \
                             does not admit this client",
                            self.cfg.base_url
                        ),
                    },
                ));
            }
            // A proxy's 502, a 404 on the wrong path, a 500. Every one of
            // them means this call did not reach a working hub, which is what
            // the UI's banner is for; the status is in the message so nothing
            // is hidden behind the code.
            other => {
                let said = summarise_body(&body).map(|s| self.redact(&s));
                return Err(IpcError::new(
                    codes::E_HUB_UNREACHABLE,
                    match said {
                        Some(s) => format!("{} answered {other}: {s}", self.cfg.base_url),
                        None => format!("{} answered {other}", self.cfg.base_url),
                    },
                ));
            }
        }

        let payload = fleet_core::mcp::wire::last_event_payload(&body);
        let envelope: Value = serde_json::from_str(&payload).map_err(|e| {
            IpcError::new(
                codes::E_PARSE,
                format!(
                    "{} sent an unreadable answer to {tool}: {e}",
                    self.cfg.base_url
                ),
            )
        })?;

        // A JSON-RPC `error` is a *protocol* failure — an unknown tool, or
        // arguments rmcp could not bind. That is this client disagreeing with
        // the hub about the contract, not something the user did.
        if let Some(err) = envelope.get("error") {
            // Scrubbed like every other scrap of text that came from outside
            // this process. rmcp builds this message from its own dispatch and
            // has no reason to echo a header — but "has no reason to" is a
            // claim about code on the other side of a network, which is
            // exactly the reasoning this module refuses to rely on elsewhere.
            let message = self.redact(
                err.get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("no message"),
            );
            return Err(IpcError::new(
                codes::E_INTERNAL,
                format!("the hub refused the {tool} call: {message}"),
            ));
        }

        let result = envelope.get("result").ok_or_else(|| {
            IpcError::new(
                codes::E_PARSE,
                format!("{}'s answer to {tool} carried no result", self.cfg.base_url),
            )
        })?;

        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            return Err(self.tool_error(tool, result));
        }

        Ok(result
            .get("content")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }

    /// Rebuild the `IpcError` the service layer raised on the other side.
    ///
    /// The hub puts it in `structured_content` as `{code, message, details}`
    /// (`mcp::tools::support::tool_error_result`), so the code, the prose and
    /// any structured `details` — `E_AMBIGUOUS`'s candidate list, `E_LINT`'s
    /// report — survive the round trip intact. The text block is the same
    /// error rendered as `"CODE: message"`, and is the fallback for a hub too
    /// old to send the structured form.
    ///
    /// The hub builds these from service-layer messages, never from request
    /// headers, so the token cannot appear here — it is scrubbed anyway,
    /// because "cannot" is a claim about code on the other side of a network.
    fn tool_error(&self, tool: &str, result: &Value) -> IpcError {
        let text = &self.redact(
            result
                .get("content")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );

        if let Some(sc) = result
            .get("structuredContent")
            .or_else(|| result.get("structured_content"))
        {
            if let Some(code) = sc.get("code").and_then(Value::as_str) {
                let message = sc
                    .get("message")
                    .and_then(Value::as_str)
                    .map(|m| self.redact(m))
                    .unwrap_or_else(|| text.clone());
                let err = IpcError::new(code, message);
                return match sc.get("details") {
                    Some(Value::Null) | None => err,
                    // `details` is the one piece of a tool error that crosses
                    // the IPC boundary structurally — `IpcError` derives
                    // `Serialize`, so this reaches the frontend rather than
                    // only a `Debug` line. It gets the same scrubbing its
                    // `code` and `message` siblings already had.
                    Some(d) => err.with_details(self.redact_value(d)),
                };
            }
        }

        // Fallback: split the text block's `"CODE: message"`. Only a leading
        // `E_`-shaped token counts, so ordinary prose containing a colon is
        // left whole rather than being mangled into a bogus code.
        match text.split_once(": ") {
            Some((code, message)) if is_error_code(code) => IpcError::new(code, message),
            _ => IpcError::new(
                codes::E_INTERNAL,
                format!("{tool} failed on the hub: {text}"),
            ),
        }
    }
}

// --- routing one command -----------------------------------------------------
//
// [`HubBackend::call`] takes a tool name and a `Value`; a command has neither.
// These two turn the one into the other, and they are how a routed command
// reaches the hub: the tool comes from the command's own row in
// [`VERDICTS`](super::verdicts::VERDICTS) — the table the frontend list and
// the docs are generated from — rather than from a second literal written
// here, and the arguments are whatever `args` serialises to.

impl HubBackend {
    /// Call the tool `command`'s verdict names, with `args` as its arguments,
    /// and deserialise the answer into `T`.
    ///
    /// `args` is the command's own argument struct wherever the wire is that
    /// struct field for field, and a `json!` literal wherever it is not — a
    /// defaulted value, a clamped one, a key sent only when it is set. The
    /// difference is the thing to get right, so it stays written down at the
    /// call site rather than being inferred here.
    pub async fn route<A: serde::Serialize, T: DeserializeOwned>(
        &self,
        command: &str,
        args: &A,
    ) -> Result<T, IpcError> {
        let tool = self.tool_for(command)?;
        self.call(tool, arguments(command, args)?).await
    }

    /// [`Self::route`] for the tools that answer prose rather than JSON.
    pub async fn route_text<A: serde::Serialize>(
        &self,
        command: &str,
        args: &A,
    ) -> Result<String, IpcError> {
        let tool = self.tool_for(command)?;
        self.call_text(tool, arguments(command, args)?).await
    }

    /// The hub tool `command` routes to.
    ///
    /// **It fails closed**, for the same reason
    /// [`FleetBackend::refuse_local_only`](super::routing::FleetBackend::refuse_local_only)
    /// does: a command with no row, or with a row that names no tool, is a
    /// bug — `every_route_names_a_command_the_table_can_route` makes shipping
    /// one impossible — but if one ever got out, the call must fail rather
    /// than guess at a tool. Loud in a debug build and in every test, an
    /// `IpcError` in release, never a panic on a user path.
    fn tool_for(&self, command: &str) -> Result<&'static str, IpcError> {
        let tool = super::verdicts::verdict(command).and_then(super::verdicts::Verdict::tool);
        debug_assert!(
            tool.is_some(),
            "{command} routes to the hub but VERDICTS names no tool for it"
        );
        tool.ok_or_else(|| {
            IpcError::new(
                codes::E_INTERNAL,
                format!(
                    "{command} was not run: this build has no hub tool recorded for it, \
                     which is a bug in the app"
                ),
            )
        })
    }
}

/// `args` as the tool's `arguments` object. A value that cannot serialise is
/// this app's bug rather than the hub's, so it is reported as one instead of
/// being sent as whatever survived.
fn arguments<A: serde::Serialize>(command: &str, args: &A) -> Result<Value, IpcError> {
    serde_json::to_value(args).map_err(|e| {
        IpcError::new(
            codes::E_INTERNAL,
            format!("{command}'s arguments could not be encoded for the hub: {e}"),
        )
    })
}

/// `E_` followed by upper-case ASCII, digits or `_` — the shape every code in
/// `fleet_core::ipc_error::codes` has.
fn is_error_code(s: &str) -> bool {
    s.starts_with("E_")
        && s.len() > 2
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

/// Longest slice of a non-200 body worth repeating to the user. A proxy can
/// answer with a whole HTML page; the first line of it is the useful part.
const MAX_BODY_ECHO: usize = 200;

/// The hub's own words from an error body, trimmed to one line and capped, or
/// `None` when it said nothing (an axum `StatusCode` reply has an empty body).
fn summarise_body(body: &str) -> Option<String> {
    let line = body.trim().lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    Some(match line.char_indices().nth(MAX_BODY_ECHO) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_string(),
    })
}

// --- the calls that are more than their command's arguments ------------------
//
// [`HubBackend::route`] covers the ordinary case: the command's own argument
// struct, serialised whole. What stays here is everything that is not that —
// a value the desktop insists on rather than letting the tool default, a key
// sent only when it is set, a clamp applied on this side, an answer that
// needs shaping before the UI sees it — and the four reads the event bridge's
// resync (`super::events`) shares with their commands, which therefore need a
// name of their own.
//
// The two vocabularies are close but not identical (the desktop's `force` is
// the tool's `force`, but the desktop always wants `summary: false`), and a
// silent mismatch would be invisible. So these spell the ARGUMENTS out; the
// tool is still the one the command's `VERDICTS` row names.

impl HubBackend {
    /// `commands::sessions::list_sessions`. `summary: false` because the
    /// desktop renders full rows; the tool's slim default exists for token
    /// caps an IPC caller does not have.
    pub async fn list_sessions(&self, force: bool) -> Result<Vec<SessionRow>, IpcError> {
        self.route(
            "list_sessions",
            &json!({ "summary": false, "force": force, "include_lost": true }),
        )
        .await
    }

    /// `commands::hosts::list_hosts`.
    pub async fn list_hosts(&self) -> Result<Vec<HostRow>, IpcError> {
        self.route("list_hosts", &json!({})).await
    }

    /// `commands::hosts::list_accounts`.
    pub async fn list_accounts(&self) -> Result<Vec<AccountRow>, IpcError> {
        self.route("list_accounts", &json!({})).await
    }

    /// `commands::projects::list_projects`.
    pub async fn list_projects(
        &self,
    ) -> Result<Vec<fleet_core::service::projects::ProjectTreeRow>, IpcError> {
        self.route("list_projects", &json!({ "summary": false }))
            .await
    }

    /// `commands::projects::refresh_projects`.
    pub async fn refresh_projects(
        &self,
    ) -> Result<Vec<fleet_core::service::projects::ProjectTreeRow>, IpcError> {
        self.route("refresh_projects", &json!({})).await
    }

    /// `commands::worktrees::list_worktrees`.
    ///
    /// The tool is shaped for agents: slim rows, capped at a page, wrapped in
    /// `{total, worktrees}`. The desktop renders the whole project tree, so it
    /// asks for full rows and `limit: 0` (no cap) and unwraps the envelope.
    pub async fn list_worktrees(
        &self,
        project_id: Option<i64>,
    ) -> Result<Vec<fleet_core::service::worktrees::WorktreeOccupancy>, IpcError> {
        #[derive(serde::Deserialize)]
        struct Page {
            worktrees: Vec<fleet_core::service::worktrees::WorktreeOccupancy>,
        }
        let page: Page = self
            .route(
                "list_worktrees",
                &json!({ "project_id": project_id, "summary": false, "limit": 0 }),
            )
            .await?;
        Ok(page.worktrees)
    }

    /// `commands::tasks::list_tasks`.
    pub async fn list_tasks(
        &self,
        requester_session_id: Option<i64>,
        state: Option<String>,
        limit: Option<i64>,
    ) -> Result<Vec<TaskRow>, IpcError> {
        let mut rows: Vec<TaskRow> = self
            .route(
                "list_tasks",
                &json!({
                    "requester_session_id": requester_session_id,
                    "state": state,
                    "limit": limit,
                }),
            )
            .await?;
        // The tool prefixes every result with the untrusted-content marker
        // (`tasks::mark_task_result`), which is for an agent reading a tool
        // answer. The local path hands the Tasks panel the worker's words
        // as stored, so the hub's line comes off here — here rather than in
        // the command, because the event bridge's resync reads through this
        // too. `strip_marker` removes only a genuine first marker line.
        for row in &mut rows {
            if let Some(r) = row.result.as_mut() {
                *r = fleet_core::mcp::guard::strip_marker(r).to_string();
            }
        }
        Ok(rows)
    }

    /// `commands::sessions::session_history`. `limit` is the clamp the
    /// command applied, not the number the frontend asked for.
    pub async fn session_history(
        &self,
        session_id: i64,
        limit: Option<i64>,
    ) -> Result<Vec<SessionEvent>, IpcError> {
        self.route(
            "session_history",
            &json!({ "session_id": session_id, "limit": limit }),
        )
        .await
    }

    /// `commands::sessions::session_conversation`.
    /// `claude_session_id` is sent only when set, so a read of the current
    /// conversation asks exactly what it did before conversations existed.
    /// `events_limit` asks for the UI's larger event window; a hub that
    /// predates the parameter ignores it and answers its own default.
    pub async fn session_conversation(
        &self,
        session_id: i64,
        turns: Option<usize>,
        claude_session_id: Option<&str>,
        events_limit: i64,
    ) -> Result<Conversation, IpcError> {
        let mut args = json!({
            "session_id": session_id,
            "turns": turns,
            "events_limit": events_limit,
        });
        if let Some(id) = claude_session_id {
            args["claude_session_id"] = json!(id);
        }
        self.route("session_conversation", &args).await
    }

    /// `commands::sessions::session_conversations`. `limit` is the clamp the
    /// command applied.
    pub async fn session_conversations(
        &self,
        session_id: i64,
        limit: i64,
    ) -> Result<Vec<ConversationRow>, IpcError> {
        self.route(
            "session_conversations",
            &json!({ "session_id": session_id, "limit": limit }),
        )
        .await
    }

    /// `commands::health::health_check` — the second place the two
    /// vocabularies differ, and the row is what resolves it: the command is
    /// `health_check`, the tool is `fleet_health`.
    pub async fn fleet_health(&self) -> Result<fleet_core::service::health::Health, IpcError> {
        self.route("health_check", &json!({})).await
    }

    /// `commands::tasks::cancel_task`, whose one argument arrives as a bare
    /// `i64` rather than a struct.
    pub async fn cancel_task(&self, task_id: i64) -> Result<TaskRow, IpcError> {
        self.route("cancel_task", &json!({ "task_id": task_id }))
            .await
    }
}

// --- the mutations that are more than their arguments ------------------------
//
// A mutation routes only where the desktop's arguments map **one to one**
// onto the tool's parameters; where they do not, the command refuses with
// `E_LOCAL_ONLY` rather than calling a tool that would drop a field or mean
// something else. A silent argument mismatch on a *mutation* is the worst
// failure this module can have, so the rule is parity or refusal.
//
// The two below are the ones whose argument struct carries a field the tool
// must NOT see, so they cannot go over whole: the rest route through
// [`HubBackend::route`] from their `routed::` function. Every one of these
// tools answers with `ok_json` of the same type the local service call
// returns, so the mapping stays a deserialisation.

impl HubBackend {
    /// `commands::sessions::new_session`. `call_id` is this process's own
    /// cancellation-registry key (`cancel_command`) and has no hub
    /// counterpart, so it is never sent — which is why this spells the
    /// arguments out rather than serialising `NewSessionArgs`.
    pub async fn new_session(
        &self,
        args: &sessions::NewSessionArgs,
    ) -> Result<SessionRow, IpcError> {
        self.route(
            "new_session",
            &json!({
                "host_alias": args.host_alias,
                "project_id": args.project_id,
                "worktree_id": args.worktree_id,
                "name": args.name,
                "new_worktree": args.new_worktree,
                "base_branch": args.base_branch,
                "kind": args.kind,
                "start_command": args.start_command,
                "friendly_name": args.friendly_name,
            }),
        )
        .await
    }

    /// `commands::sessions::repair_session` with `explicit: true` only — the
    /// tool's own repair is always explicit, so `explicit` itself is not a
    /// parameter (`routed::repair_session` guards `explicit: false` before
    /// this is ever called).
    pub async fn repair_session(&self, session_id: i64) -> Result<repair::RepairReport, IpcError> {
        self.route("repair_session", &json!({ "session_id": session_id }))
            .await
    }
}

/// The transport of a hub this launch cannot use: it sends nothing. See
/// [`HubBackend::unavailable`].
struct NoTransport;

#[async_trait::async_trait]
impl HubTransport for NoTransport {
    async fn post_json(
        &self,
        _url: &str,
        _bearer: &str,
        _body: String,
    ) -> Result<HubResponse, String> {
        Err("no request was sent: the configured hub cannot be used by this launch".into())
    }
}

// --- the real transport ------------------------------------------------------

/// The hub over HTTP or HTTPS, written by hand onto a `TcpStream` — the same
/// way `fleet-hub`'s own CLI talks to `/mcp`. One request, one response,
/// `Connection: close`; there is no connection pool because a desktop makes a
/// handful of calls a second at worst, and no *usable outbound HTTP client*
/// crate is in this workspace's graph to borrow one from: `hyper` is present
/// only as `axum`'s server side (via `fleet-core`'s embedded MCP server), and
/// `reqwest` appears in `Cargo.lock` only through a target-specific `tauri`
/// dependency that is not compiled here (`cargo tree -i reqwest` prints
/// nothing on this platform).
///
/// `https://` is the case that matters: `docs/hub.md` refuses to serve a
/// public hub in plaintext, so a real hub is always TLS. `http://` stays for a
/// loopback or tunnelled hub.
///
/// Certificates are verified against the **platform trust store**
/// (`rustls-native-certs`), not a bundled root set. That is deliberate: a
/// desktop is exactly the place where a corporate CA or a root the operator
/// installed themselves has to work, and a bundled set would silently reject
/// both. (`webpki-roots` would bundle them, and its CDLA-Permissive-2.0
/// licence is not on `deny.toml`'s allow list.)
pub struct TcpTransport;

/// Build the TLS client config: read the platform trust store and turn it
/// into a connector. Blocking (file I/O, and on macOS a keychain read), so
/// every caller goes through [`tls_connector`], which runs it on the blocking
/// pool.
fn build_tls_connector() -> Result<tokio_rustls::TlsConnector, String> {
    // Only the `ring` provider is compiled in, so rustls would pick it
    // anyway; installing it explicitly means a future second provider
    // cannot silently change which one is used. Same reasoning, and
    // the same line, as `fleet_hub::tls`.
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    let found = rustls_native_certs::load_native_certs();
    for cert in found.certs {
        // A single unparseable root is not fatal: the store is a bag
        // of certificates from the OS and one bad entry must not stop
        // the app trusting the rest.
        let _ = roots.add(cert);
    }
    if roots.is_empty() {
        // Failing closed. An empty root store would reject every hub
        // with an opaque certificate error; saying so once, here, is
        // the difference between a diagnosable problem and a mystery.
        return Err(format!(
            "no usable certificates in this machine's trust store \
             ({} error(s) while reading it); an https:// hub cannot be verified",
            found.errors.len()
        ));
    }
    let config = tokio_rustls::rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(tokio_rustls::TlsConnector::from(std::sync::Arc::new(
        config,
    )))
}

/// The TLS client config, built once and cached.
///
/// Two things this deliberately does NOT do:
///
/// - It does not load the trust store on a runtime worker.
///   `load_native_certs` reads (and on macOS unlocks and queries) the
///   platform store; on a locked-down or network-mounted box that is slow
///   blocking I/O, and a runtime worker parked in it is a worker not running
///   anyone else's future. It goes to [`tokio::task::spawn_blocking`].
/// - It does not cache a FAILURE. A trust store that was momentarily
///   unreadable — a keychain still locked at login, a profile not yet
///   mounted — used to poison every https call for the lifetime of the
///   process, so the app had to be restarted to recover from a condition that
///   had already cleared. Only a success is remembered; a failure is retried
///   on the next call.
///
/// Note that "the platform trust store" is really "the platform trust store,
/// unless the environment says otherwise": `load_native_certs` honours
/// `SSL_CERT_FILE` and `SSL_CERT_DIR` when they are set
/// (`rustls-native-certs`'s `CertPaths::from_env`).
async fn tls_connector() -> Result<&'static tokio_rustls::TlsConnector, String> {
    static CONNECTOR: std::sync::OnceLock<tokio_rustls::TlsConnector> = std::sync::OnceLock::new();
    if let Some(ready) = CONNECTOR.get() {
        return Ok(ready);
    }
    let built = tokio::task::spawn_blocking(build_tls_connector)
        .await
        .map_err(|e| format!("reading this machine's trust store failed: {e}"))??;
    // Two callers racing both build one; `get_or_init` keeps whichever
    // arrived first and drops the other. Both are equivalent.
    Ok(CONNECTOR.get_or_init(|| built))
}

/// Where a hub URL points, split into the pieces a hand-written request
/// needs. One implementation for every request this app makes — `POST /mcp`
/// and the `GET /events` stream alike — so a fix to the parsing cannot land
/// in one and not the other.
///
/// The parsing itself is [`fleet_proto::net::Endpoint`]'s, shared with
/// `fleet-agent` and `fleet-hub`. What stays here is this transport's own
/// policy — a WebSocket URL is not a hub address for a request this app
/// writes by hand — and the request-line target that policy needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    at: fleet_proto::net::Endpoint,
}

impl Endpoint {
    /// Parse a hub URL: scheme, authority and a path prefix, with `/mcp`,
    /// `/events` or `/pair` already appended by the caller.
    pub fn parse(url: &str) -> Result<Self, String> {
        let at = fleet_proto::net::Endpoint::parse(url).map_err(|e| format!("{url}: {e}"))?;
        if at.scheme().is_websocket() {
            // The agent dials a WebSocket; everything this app sends is HTTP
            // it writes itself.
            return Err(format!("{}:// is not a hub address", at.scheme()));
        }
        Ok(Self { at })
    }

    /// The host to connect to and to check the certificate against.
    /// **Unbracketed**, so an IPv6 literal works: the bracketed `[::1]`
    /// neither resolves nor parses as a `ServerName`, and an IPv6 hub was
    /// simply unreachable before that was fixed.
    pub fn host(&self) -> &str {
        self.at.host()
    }

    pub fn port(&self) -> u16 {
        self.at.port()
    }

    pub fn is_tls(&self) -> bool {
        self.at.is_tls()
    }

    /// What the `Host` header must carry. Here the brackets are REQUIRED
    /// (`[::1]:8787`), and the port is part of it unless it is the scheme's
    /// default — the hub's allowlist is matched against exactly this string.
    pub fn authority(&self) -> &str {
        self.at.authority()
    }

    /// The path, ready to go on the request line. A query would have been
    /// refused by the parser: this value is built by concatenation
    /// (`{base_url}/mcp`), which a query silently breaks.
    pub fn target(&self) -> String {
        self.at.request_target()
    }
}

/// A connected stream to the hub, TLS-wrapped when the URL said `https`.
/// Boxed because the two arms are different types and both the one-shot and
/// the streaming caller want one name for them.
pub type HubStream = Box<dyn Duplex>;

/// `AsyncRead + AsyncWrite`, object-safe.
pub trait Duplex: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin> Duplex for T {}

/// Connect to `at`, wrapping in TLS when it says so. The one place a socket
/// to a hub is opened.
pub async fn connect(at: &Endpoint) -> Result<HubStream, String> {
    let tcp = tokio::net::TcpStream::connect((at.host(), at.port()))
        .await
        .map_err(|e| format!("connect {}:{}: {e}", at.host(), at.port()))?;
    if !at.is_tls() {
        return Ok(Box::new(tcp));
    }
    let connector = tls_connector().await?;
    // The name the certificate is checked against. An IP literal is accepted
    // by `ServerName` and matched as an IP SAN, which is what a hub reached
    // at `https://10.0.0.5` needs.
    let server_name = tokio_rustls::rustls::pki_types::ServerName::try_from(at.host().to_string())
        .map_err(|e| format!("{} is not a valid certificate name: {e}", at.host()))?;
    let stream = connector
        .connect(server_name, tcp)
        .await
        // The usual causes are an expired or self-signed certificate and a
        // name that does not match; rustls says which, and the operator needs
        // to hear it verbatim.
        .map_err(|e| format!("TLS handshake with {}:{} failed: {e}", at.host(), at.port()))?;
    Ok(Box::new(stream))
}

/// Largest response read from the hub, so a stray listener cannot make the
/// app buffer without bound. A full `list_sessions` on a large fleet is a few
/// hundred kilobytes.
const MAX_RESPONSE: u64 = 8 * 1024 * 1024;

/// One whole request/response exchange.
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[async_trait::async_trait]
impl HubTransport for TcpTransport {
    async fn post_json(
        &self,
        url: &str,
        bearer: &str,
        body: String,
    ) -> Result<HubResponse, String> {
        let at = Endpoint::parse(url)?;
        // `Accept` carries both types because the transport answers
        // SSE-framed; rmcp refuses a request that does not accept
        // `text/event-stream`.
        let request = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {bearer}\r\n\
             Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            at.target(),
            at.authority(),
            body.len()
        );
        let raw = tokio::time::timeout(CALL_TIMEOUT, exchange(&at, &request))
            .await
            .map_err(|_| format!("no answer within {CALL_TIMEOUT:.0?}"))??;
        split_response(&raw)
    }
}

/// Connect, write the request, read the whole answer back — as bytes, since
/// the body may be chunked and only [`split_response`] may decode it.
pub(crate) async fn exchange(at: &Endpoint, request: &str) -> Result<Vec<u8>, String> {
    let conn = connect(at).await?;
    speak(conn, at.host(), at.port(), request).await
}

/// Write `request` and read until the peer closes. Generic over the stream so
/// the plain and TLS paths share one implementation and cannot drift.
///
/// Returns raw bytes, NOT text: decoding here, before [`split_response`]
/// de-chunks, corrupted any character a chunk boundary split — the same order
/// the event stream already gets right (de-chunk bytes, then decode).
async fn speak<S>(mut conn: S, host: &str, port: u16, request: &str) -> Result<Vec<u8>, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    conn.write_all(request.as_bytes())
        .await
        .map_err(|e| format!("send to {host}:{port}: {e}"))?;
    // TLS needs an explicit flush: the record is buffered until one.
    conn.flush()
        .await
        .map_err(|e| format!("send to {host}:{port}: {e}"))?;
    let mut raw = Vec::new();
    // One byte MORE than the cap, so "the answer was too big" is
    // distinguishable from "the answer was exactly the cap". Truncating
    // silently at the cap used to surface as `E_PARSE … returned unreadable
    // JSON`, which sends whoever reads it looking for a bug in the hub's
    // encoder rather than at a size limit.
    if let Err(e) = conn.take(MAX_RESPONSE + 1).read_to_end(&mut raw).await {
        // A peer that closes the TCP connection without sending `close_notify`
        // makes rustls 0.23 return `UnexpectedEof` (`rustls/src/conn.rs`,
        // `(false, true) => Err(UnexpectedEof)`), and `?` here used to throw
        // away a body that had already arrived in full. That is a spurious
        // failure against any hub or proxy that half-closes, and it is a
        // regression the plain-HTTP path does not have.
        //
        // This is the standard shape for HTTP-over-TLS with
        // `Connection: close`: the response is framed by the connection
        // ending, so bytes already read ARE the response. An empty buffer is
        // still a real failure — nothing arrived. And only `UnexpectedEof` is
        // forgiven: a reset mid-body leaves a TRUNCATED response, which must
        // not be parsed as if it were whole.
        if e.kind() != std::io::ErrorKind::UnexpectedEof || raw.is_empty() {
            return Err(format!("read from {host}:{port}: {e}"));
        }
        tracing::debug!(
            "{host}:{port} closed without close_notify after {} byte(s); \
             treating the response as complete",
            raw.len()
        );
    }
    if raw.len() as u64 > MAX_RESPONSE {
        return Err(format!(
            "{host}:{port} sent more than {} MiB; refusing to buffer it \
             (silently truncating at the cap surfaced as unreadable JSON, \
             which names the wrong problem)",
            MAX_RESPONSE / (1024 * 1024)
        ));
    }
    Ok(raw)
}

/// Split a raw HTTP response into its status code and body, undoing
/// `Transfer-Encoding: chunked` when the head declares it.
///
/// The de-chunking is not decoration. Without it this worked only by
/// accident: a chunk-size line is not a `data:` line, so `last_event_payload`
/// skipped it, and the boundaries happened to align because rmcp writes one
/// SSE frame per body frame. Nothing guarantees either, and the day a frame is
/// split across two chunks the size line lands in the middle of a `data:`
/// line and the JSON is quietly corrupt. (`fleet-hub/src/pair.rs` still takes
/// the shortcut; it is the same latent bug, not a different one.)
///
/// Bytes in, text out, decoded ONCE at the end: a chunk size is a byte count,
/// and a chunk boundary may fall inside a multi-byte character. The head
/// parsing and de-chunking themselves are [`super::http1`]'s, shared with
/// `events.rs`'s streaming reader; this is the one-shot assembly on top.
pub fn split_response(raw: impl AsRef<[u8]>) -> Result<HubResponse, String> {
    let raw = raw.as_ref();
    let split =
        super::http1::find(raw, b"\r\n\r\n").ok_or("the hub sent a malformed HTTP response")?;
    // The head is ASCII by the grammar; a stray byte in it is not worth
    // failing over.
    let head = String::from_utf8_lossy(&raw[..split]);
    let head = head.as_ref();
    let body = &raw[split + 4..];
    let status = super::http1::parse_status(head)?;
    let body = if super::http1::head_is_chunked(head) {
        String::from_utf8_lossy(&super::http1::dechunk(body)?).into_owned()
    } else {
        String::from_utf8_lossy(body).into_owned()
    };
    Ok(HubResponse { status, body })
}

#[cfg(test)]
#[path = "tests_remote.rs"]
mod tests;
