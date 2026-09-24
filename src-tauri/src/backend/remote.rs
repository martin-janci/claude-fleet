//! `HubBackend`: every call this app makes to the hub — reads and mutations
//! alike — as the MCP tool the command's
//! [`VERDICTS`](super::verdicts::VERDICTS) row names. [`Self::route`] is the
//! way in; the tool name is never written twice.
//!
//! The hub's tools are built from `fleet-core`'s own service layer, so the
//! JSON they return *is* the row type a local command would have returned.
//! That makes this a deserialisation rather than a translation, and it is why
//! the backend can be swapped under a command without changing its
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

use super::connection::{ConnectionView, HubConnection};
use super::{RemoteConfig, UnavailableHub};
use fleet_core::ipc_error::{codes, IpcError};
use fleet_core::service::transcript::Conversation;
use fleet_core::service::{repair, sessions};
use fleet_core::store::{AccountRow, ConversationRow, HostRow, SessionEvent, SessionRow, TaskRow};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::sync::Arc;

// The transport moved to fleet-core (federation, cycle 3: the hub dials a
// peer hub with it). Re-exported so every `super::remote::…` path is unchanged.
pub use fleet_core::http_client::{
    connect, exchange, is_connect_failure, split_response, Duplex, Endpoint, HubResponse,
    HubStream, HubTransport, NoTransport, TcpTransport, CONNECT_TIMEOUT,
};

/// A client of one hub.
pub struct HubBackend {
    cfg: RemoteConfig,
    transport: Arc<dyn HubTransport>,
    /// Set for a hub that is configured but that this launch cannot use:
    /// every call is refused with this, before the transport is touched.
    unavailable: Option<UnavailableHub>,
    /// Where this window's live link to that hub stands, when something is
    /// watching it. `None` never gates — a test that is about something
    /// else, and nothing in production ([`Self::watching`] is called at both
    /// call sites, held there by a source test). See [`Self::contract_error`].
    link: Option<Arc<dyn ConnectionView>>,
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
            link: None,
        }
    }

    /// The transport, so the report flusher can post through the same
    /// `HubTransport` implementation as the tool calls (a test injects a
    /// recorded one).
    pub fn transport(&self) -> Arc<dyn HubTransport> {
        Arc::clone(&self.transport)
    }

    /// Consult `link` before every call, so that a hub whose wire contract
    /// this build does not read is refused rather than deserialised.
    ///
    /// The value handed in is the same [`super::connection::HubConnectionStatus`]
    /// the event bridge reports into and the `hub_connection` command answers
    /// from — one state, read here rather than mirrored.
    pub fn watching(mut self, link: Arc<dyn ConnectionView>) -> Self {
        self.link = Some(link);
        self
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
            link: None,
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

    /// The refusal for a call made while the last thing this window learned
    /// about the hub is that its wire contract is outside the range this
    /// build reads, or `None` when nothing has been learned or the last hub
    /// judged was in range. `what` names the tool.
    ///
    /// # Why a call is refused and not merely distrusted
    ///
    /// [`super::events::EventBridge::pump`] already applies no row event and
    /// runs no backfill from such a connection. A call made from a command
    /// walks past that: its answer is deserialised into the same row types,
    /// which carry `#[serde(default)]` on roughly forty-six optional fields
    /// (see [`super::contract`]), so a renamed column arrives as a default
    /// nobody can see rather than as a parse error. A mutation is no
    /// different — its return value is a row too, and the frontend merges it
    /// optimistically.
    ///
    /// # What it reads, and what it deliberately does not
    ///
    /// The last contract VERDICT
    /// ([`ConnectionView::contract_verdict`](super::connection::ConnectionView::contract_verdict)),
    /// not where the connection stands now. Only a `ready` frame judges a
    /// hub's wire contract, so only a `ready` frame may change the answer:
    ///
    /// - a window that has never completed a handshake on this launch
    ///   (`connecting`) calls, or every startup list would wait on
    ///   `GET /events`;
    /// - `reconnecting` / `offline` change nothing either way. Reading the
    ///   *state* would open the gate there, and since `GET /events` and
    ///   `POST /mcp` are separate sockets, a hub whose stream is merely down
    ///   would become readable again while still known to be incompatible;
    /// - a later `ready` frame in range makes the bridge report `Connected`,
    ///   which clears the verdict, and the gate opens without a restart.
    fn contract_error(&self, what: &str) -> Option<IpcError> {
        // The numbers come from the recorded verdict rather than from this
        // build's constants, so the message and `details` cannot disagree
        // with the banner that is on screen for the same connection.
        let (message, details) = match self.link.as_ref()?.contract_verdict()? {
            HubConnection::HubTooOld {
                hub_contract,
                min_contract,
            } => (
                format!(
                    "{what} was not run: {}'s wire contract is revision {hub_contract}, \
                     older than the {min_contract} this app requires. Update the hub.",
                    self.cfg.base_url
                ),
                json!({ "hub_contract": hub_contract, "min_contract": min_contract }),
            ),
            HubConnection::HubTooNew {
                hub_contract,
                max_contract,
            } => (
                format!(
                    "{what} was not run: {}'s wire contract is revision {hub_contract}, \
                     newer than the {max_contract} this app understands. Update this app.",
                    self.cfg.base_url
                ),
                json!({ "hub_contract": hub_contract, "max_contract": max_contract }),
            ),
            // Only a skew is ever recorded as a verdict; anything else here
            // would be a bug in `HubConnectionStatus::report`, and inventing
            // a refusal for it would be worse than letting the call run.
            _ => return None,
        };
        Some(IpcError::new(codes::E_HUB_CONTRACT, message).with_details(details))
    }

    /// Fail fast while the event bridge already knows the hub cannot be
    /// CONNECTED to: from the second consecutive failed connect on, a call is
    /// answered from that knowledge instead of waiting its own bound. Reads
    /// the STATE, unlike [`Self::contract_error`], because this gate only
    /// ever closes — it refuses a call, it never lets one through that the
    /// contract verdict would have stopped.
    ///
    /// # Only a connect-phase failure arms it
    ///
    /// `Offline` means the last `GET /events` attempt failed, which is not
    /// the same as "the hub is unreachable": `open_stream` reports the same
    /// state for a hub that ANSWERED — a 503 because events are disabled, a
    /// 504 from a proxy, a close after the head. `GET /events` and
    /// `POST /mcp` are separate sockets, so a hub whose event stream is
    /// unhappy can still serve every call; refusing them turns one broken
    /// stream into a window that can do nothing, and `E_HUB_UNREACHABLE`
    /// would not even be true. [`is_connect_failure`] is the difference, and
    /// it errs open.
    fn offline_error(&self, what: &str) -> Option<IpcError> {
        match self.link.as_ref()?.current() {
            HubConnection::Offline {
                attempt,
                retry_in_secs,
                reason,
            } if attempt >= 2 && is_connect_failure(&reason) => Some(IpcError::new(
                codes::E_HUB_UNREACHABLE,
                format!(
                    "{what} was not sent: {} has refused {attempt} connection attempts ({}); \
                     retrying in {retry_in_secs}s",
                    self.cfg.base_url,
                    self.redact(&reason)
                ),
            )),
            _ => None,
        }
    }

    /// Refuse a call that must only reach a hub whose wire contract the
    /// current connection has confirmed in range — `move_session`'s dry run
    /// or `when`, which a hub built before them ignores, performing a real
    /// move. `what` names the call; `reason` completes "and …" with what an
    /// older hub would do with it instead.
    ///
    /// The launch's own refusal comes first and the contract gate's second,
    /// exactly as in [`Self::call_text`], so a hub already judged out of
    /// range gets the refusal that names both revisions. Only then is "not
    /// yet confirmed" the answer: no `ready` frame judged in range (still
    /// connecting, or no link to consult at all).
    pub fn require_confirmed_contract(&self, what: &str, reason: &str) -> Result<(), IpcError> {
        if let Some(refused) = self.unavailable_error(what) {
            return Err(refused);
        }
        if let Some(refused) = self.contract_error(what) {
            return Err(refused);
        }
        let confirmed = self
            .link
            .as_ref()
            .is_some_and(|link| link.contract_confirmed());
        if confirmed {
            return Ok(());
        }
        Err(IpcError::new(
            codes::E_HUB_CONTRACT,
            format!(
                "{what} was not run: this app has not yet confirmed {}'s version, and \
                 {reason}. It becomes available once the desktop has confirmed the hub's \
                 version.",
                self.cfg.base_url
            ),
        ))
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
        // The three refusals that happen before a socket is opened, in this
        // order. "This launch cannot use the hub at all"
        // ([`Self::unavailable_error`]) comes first: it is about the
        // configuration rather than about the hub's version, its sentence is
        // the one the whole window is already showing, and such a client
        // never handshakes, so it has no skew to report. The contract gate
        // ([`Self::contract_error`]) is second: a hub whose wire revision
        // this build cannot read is refused whatever the connection is doing,
        // and that verdict outlives the socket that carried it. The breaker
        // ([`Self::offline_error`]) is LAST, deliberately: it is the only one
        // of the three that is merely a shortcut — the call would fail on its
        // own, just slower — so it must not pre-empt either of the refusals
        // that carry a specific, actionable sentence. These three are the
        // only ways a call ends before the transport is touched.
        if let Some(refused) = self.unavailable_error(tool) {
            return Err(refused);
        }
        if let Some(refused) = self.contract_error(tool) {
            return Err(refused);
        }
        if let Some(refused) = self.offline_error(tool) {
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
        // The exchange is bounded HERE rather than inside the transport
        // because this is the only layer that knows which tool is being
        // called, and one of them ([`call_timeout`]) legitimately runs for
        // minutes.
        let limit = call_timeout(tool);
        let response =
            tokio::time::timeout(limit, self.transport.post_json(&url, &self.cfg.token, body))
                .await
                .map_err(|_| {
                    IpcError::new(
                        codes::E_HUB_TIMEOUT,
                        format!(
                            "{} did not answer: no answer within {limit:.0?} — the request may \
                             still complete on the hub; refresh before retrying",
                            self.cfg.base_url
                        ),
                    )
                    // Whether the hub may have CHANGED something. A read that
                    // never answered changed nothing, and only this layer
                    // knows which tool was called — the frontend acts on this
                    // flag (`src/lib/result.ts` → `fleet:outcome-unknown`),
                    // and its reaction is a fleet-wide re-fetch built out of
                    // reads, so a read that broadcast would amplify: one
                    // timeout becomes two more calls that can time out in
                    // turn.
                    .with_details(json!({
                        "outcome_unknown": !fleet_core::mcp::guard::is_readonly_tool(tool),
                    }))
                })?
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
        // the hub about the contract, not something the user did, and it has
        // its own code so a caller built on a tool an older hub does not
        // serve can degrade to what it did before that tool existed.
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
                codes::E_HUB_PROTOCOL,
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

    /// `commands::sessions::session_activity` — one pane probe for the
    /// Conversation tab's live indicator, read over the hub's own ssh.
    pub async fn session_activity(
        &self,
        session_id: i64,
    ) -> Result<fleet_core::service::sessions::ActivityProbe, IpcError> {
        self.route("session_activity", &json!({ "session_id": session_id }))
            .await
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

    /// `commands::operator::operator_status`. No arguments: the tool reads
    /// the hub's own `mcp` settings and its own operator session.
    pub async fn operator_status(
        &self,
    ) -> Result<fleet_core::service::operator::OperatorStatus, IpcError> {
        self.route("operator_status", &json!({})).await
    }

    /// `commands::operator::ensure_operator`. No arguments: the hub births
    /// or finds ITS OWN operator session — the one whose `local` is the
    /// machine that actually serves the control API, which this desktop is
    /// not in hub-client mode.
    pub async fn ensure_operator(&self) -> Result<SessionRow, IpcError> {
        self.route("ensure_operator", &json!({})).await
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
                "resume_claude_session_id": args.resume_claude_session_id,
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

/// Added to the hub's own per-tool deadline ([`fleet_core::mcp::tool_deadline`])
/// so the client never gives up before the server: a call reported failed
/// while the hub completes it is how a `new_session` gets clicked twice.
const CALL_MARGIN: std::time::Duration = std::time::Duration::from_secs(10);

/// `move_session` copies a repository, a transcript and the Claude state
/// between hosts: minutes, not seconds. Its bound is the larger of the hub's
/// deadline and this floor.
const MOVE_CALL_FLOOR: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// How long `tool` may take to answer: the hub's deadline for it plus a
/// margin. Still bounded: a hub that stops answering mid-call must not leave
/// the window waiting forever.
fn call_timeout(tool: &str) -> std::time::Duration {
    let bound = fleet_core::mcp::tool_deadline(tool) + CALL_MARGIN;
    match tool {
        "move_session" => bound.max(MOVE_CALL_FLOOR),
        _ => bound,
    }
}

#[cfg(test)]
#[path = "tests_remote.rs"]
mod tests;
