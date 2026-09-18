//! `HubBackend`: every read this app does, expressed as a call to the MCP
//! tool the hub already serves.
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

use super::RemoteConfig;
use fleet_core::ipc_error::{codes, IpcError};
use fleet_core::service::transcript::Conversation;
use fleet_core::store::{AccountRow, HostRow, SessionEvent, SessionRow, TaskRow};
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
/// A trait, not a concrete client, for two reasons: the whole tool mapping is
/// then testable against recorded responses with no network (which is what
/// the design calls for), and the TLS decision — see [`TcpTransport`] — can be
/// made later without touching a line of the mapping.
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
        Self { cfg, transport }
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

    /// Call one tool and deserialise its result into `T`.
    pub async fn call<T: DeserializeOwned>(&self, tool: &str, args: Value) -> Result<T, IpcError> {
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
    pub async fn call_text(&self, tool: &str, args: Value) -> Result<String, IpcError> {
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
            let message = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("no message");
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
                    Some(d) => err.with_details(d.clone()),
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

// --- the typed calls ---------------------------------------------------------
//
// One method per read command that has a remote counterpart. Each names the
// tool and the arguments explicitly rather than forwarding the command's own
// argument struct: the two vocabularies are close but not identical (the
// desktop's `force` is the tool's `force`, but the desktop always wants
// `summary: false`), and a silent mismatch here would be invisible.

impl HubBackend {
    /// `commands::sessions::list_sessions`. `summary: false` because the
    /// desktop renders full rows; the tool's slim default exists for token
    /// caps an IPC caller does not have.
    pub async fn list_sessions(&self, force: bool) -> Result<Vec<SessionRow>, IpcError> {
        self.call(
            "list_sessions",
            json!({ "summary": false, "force": force, "include_lost": true }),
        )
        .await
    }

    /// `commands::sessions::related_sessions`.
    pub async fn related_sessions(&self, session_id: i64) -> Result<Vec<SessionRow>, IpcError> {
        self.call("related_sessions", json!({ "session_id": session_id }))
            .await
    }

    /// `commands::hosts::list_hosts`.
    pub async fn list_hosts(&self) -> Result<Vec<HostRow>, IpcError> {
        self.call("list_hosts", json!({})).await
    }

    /// `commands::hosts::list_accounts`.
    pub async fn list_accounts(&self) -> Result<Vec<AccountRow>, IpcError> {
        self.call("list_accounts", json!({})).await
    }

    /// `commands::projects::list_projects`.
    pub async fn list_projects(
        &self,
    ) -> Result<Vec<fleet_core::service::projects::ProjectTreeRow>, IpcError> {
        self.call("list_projects", json!({ "summary": false }))
            .await
    }

    /// `commands::worktrees::list_worktrees`.
    pub async fn list_worktrees(
        &self,
        project_id: Option<i64>,
    ) -> Result<Vec<fleet_core::service::worktrees::WorktreeOccupancy>, IpcError> {
        self.call("list_worktrees", json!({ "project_id": project_id }))
            .await
    }

    /// `commands::tasks::list_tasks`.
    pub async fn list_tasks(
        &self,
        requester_session_id: Option<i64>,
        state: Option<String>,
        limit: Option<i64>,
    ) -> Result<Vec<TaskRow>, IpcError> {
        self.call(
            "list_tasks",
            json!({
                "requester_session_id": requester_session_id,
                "state": state,
                "limit": limit,
            }),
        )
        .await
    }

    /// `commands::sessions::session_history`.
    pub async fn session_history(
        &self,
        session_id: i64,
        limit: Option<i64>,
    ) -> Result<Vec<SessionEvent>, IpcError> {
        self.call(
            "session_history",
            json!({ "session_id": session_id, "limit": limit }),
        )
        .await
    }

    /// `commands::sessions::session_conversation`.
    pub async fn session_conversation(
        &self,
        session_id: i64,
        turns: Option<usize>,
    ) -> Result<Conversation, IpcError> {
        self.call(
            "session_conversation",
            json!({ "session_id": session_id, "turns": turns }),
        )
        .await
    }

    /// `commands::health::health_check`.
    pub async fn fleet_health(&self) -> Result<fleet_core::service::health::Health, IpcError> {
        self.call("fleet_health", json!({})).await
    }
}

// --- the real transport ------------------------------------------------------

/// The hub over plain HTTP, written by hand onto a `TcpStream` — the same way
/// `fleet-hub`'s own CLI talks to `/mcp`, and for the same reason: nothing in
/// the dependency tree can currently do better.
///
/// **This cannot reach an `https://` hub, which is how `docs/hub.md` says to
/// run one.** There is no HTTP client in this workspace's graph (`reqwest` is
/// in `Cargo.lock` but not in `claude-fleet`'s tree, and enabling a TLS
/// feature pulls in crates the lockfile does not have). `rustls`,
/// `tokio-rustls` and `ring` *are* already there via `fleet-hub`'s server
/// side; only a root-certificate store (`webpki-roots` or
/// `rustls-native-certs`) is missing. Which of those to add is a dependency
/// decision for the repository owner, so this refuses `https://` with a
/// message that says so rather than guessing.
pub struct TcpTransport;

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
        let parsed = url::Url::parse(url).map_err(|e| format!("{url}: {e}"))?;
        if parsed.scheme() != "http" {
            return Err(format!(
                "this build can only reach an http:// hub, not {}:// — TLS needs a \
                 certificate-store dependency that has not been chosen yet",
                parsed.scheme()
            ));
        }
        let host = parsed.host_str().ok_or("no host in the hub URL")?;
        let port = parsed.port_or_known_default().unwrap_or(80);
        // `authority` is what the Host header must carry: the port is part of
        // it unless it is the scheme's default, and the hub's allowlist is
        // matched against exactly this string.
        let authority = match parsed.port() {
            Some(p) => format!("{host}:{p}"),
            None => host.to_string(),
        };
        let path = parsed.path();
        // `Accept` carries both types because the transport answers
        // SSE-framed; rmcp refuses a request that does not accept
        // `text/event-stream`.
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: {authority}\r\nAuthorization: Bearer {bearer}\r\n\
             Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let raw = tokio::time::timeout(CALL_TIMEOUT, exchange(host, port, &request))
            .await
            .map_err(|_| format!("no answer within {CALL_TIMEOUT:.0?}"))??;
        split_response(&raw)
    }
}

/// Write the request and read the whole answer back.
async fn exchange(host: &str, port: u16, request: &str) -> Result<String, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut conn = tokio::net::TcpStream::connect((host, port))
        .await
        .map_err(|e| format!("connect {host}:{port}: {e}"))?;
    conn.write_all(request.as_bytes())
        .await
        .map_err(|e| format!("send to {host}:{port}: {e}"))?;
    let mut raw = Vec::new();
    conn.take(MAX_RESPONSE)
        .read_to_end(&mut raw)
        .await
        .map_err(|e| format!("read from {host}:{port}: {e}"))?;
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

/// Split a raw HTTP response into its status code and body.
pub fn split_response(raw: &str) -> Result<HubResponse, String> {
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .ok_or("the hub sent a malformed HTTP response")?;
    let status_line = head.lines().next().unwrap_or_default();
    // The status token, not a substring: a `contains(" 200")` would match the
    // reason phrase and any header that happened to carry " 200" too.
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| format!("unreadable status line: {status_line:?}"))?;
    Ok(HubResponse {
        status,
        body: body.to_string(),
    })
}

#[cfg(test)]
#[path = "tests_remote.rs"]
mod tests;
