//! Redeeming a pairing code for a client token.
//!
//! This is the one exchange with a hub that carries **no** bearer token,
//! because it is how the desktop gets its first one. The operator runs
//! `fleet-hub pair --name laptop`, which mints an 8-character Crockford code
//! (`fleet_core::mcp::pairing`), and the code is pasted into Settings; this
//! module posts it to `POST /pair` and the hub answers once, with a freshly
//! generated client token.
//!
//! Two credentials pass through here and neither may ever be printed:
//!
//! - **the code**, which is good for exactly one token until it expires, and
//! - **the token**, which is good for the whole fleet until an operator
//!   revokes it — and [`super::token_store::TokenStore::clear`] does not
//!   revoke, so a leaked one stays valid until somebody who does not know it
//!   leaked goes and revokes it.
//!
//! So [`PairedClient`] has a hand-written `Debug` like [`super::RemoteConfig`],
//! carries no `Serialize`, and every error built here is checked against the
//! code it was given.
//!
//! The transport is a trait for the same reason [`super::remote::HubTransport`]
//! is: the whole answer-reading path is then a pure function over a recorded
//! response, and none of these tests need a network.

use super::remote::{split_response, Endpoint, HubResponse};
use fleet_core::ipc_error::{codes, IpcError};

/// What `POST /pair` hands back: the client token plus what the hub calls
/// this client. See `fleet_core::mcp::pairing::handle_pair`.
pub struct PairedClient {
    /// The bearer token for every later call. Goes straight to the
    /// [`super::token_store::TokenStore`] and nowhere else.
    pub token: String,
    /// The name the operator paired under (`fleet-hub pair --name laptop`).
    pub name: String,
    /// `full` or `readonly`, as the hub recorded it.
    pub mode: String,
    /// The hub's own idea of its base URL. Only used for display; the URL the
    /// app talks to is the one the user typed, normalised.
    pub hub: String,
}

/// Hand-written so a `{:?}` — a `tracing` field, a panic, a test failure —
/// cannot spill a token that was minted seconds ago.
impl std::fmt::Debug for PairedClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairedClient")
            .field("token", &"<redacted>")
            .field("name", &self.name)
            .field("mode", &self.mode)
            .field("hub", &self.hub)
            .finish()
    }
}

/// What Settings shows for a hub that answered without naming the client.
const DEFAULT_NAME: &str = "desktop";
/// …and for a hub that did not name the access mode. Deliberately not `full`:
/// claiming more access than was granted turns a refusal into a mystery.
const UNKNOWN_MODE: &str = "unknown";

/// One unauthenticated `POST` to the hub's pairing route.
///
/// Separate from [`super::remote::HubTransport`] precisely because it has no
/// `bearer` parameter: this request must not carry one, and a trait that
/// cannot express a token cannot leak one.
#[async_trait::async_trait]
pub trait PairTransport: Send + Sync {
    async fn post_pair(&self, url: &str, body: String) -> Result<HubResponse, String>;
}

/// Longest slice of an error body worth repeating. A proxy can answer with a
/// whole HTML page; the first line of it is the useful part.
const MAX_BODY_ECHO: usize = 200;

/// The hub's own words from an error body, one line, capped.
fn summarise(body: &str) -> Option<String> {
    let line = body.trim().lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    // A JSON `{"error": "..."}` is what this route actually sends; unwrap it
    // rather than showing the user braces and quotes.
    let said = serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| line.to_string());
    Some(match said.char_indices().nth(MAX_BODY_ECHO) {
        Some((cut, _)) => format!("{}…", &said[..cut]),
        None => said,
    })
}

/// Turn one answered `POST /pair` into a [`PairedClient`] or an [`IpcError`].
/// Pure, so every branch is a unit test.
pub fn read_pair_response(base_url: &str, response: HubResponse) -> Result<PairedClient, IpcError> {
    match response.status {
        200 => {}
        // The hub answers ONE thing for unknown, already-used and expired
        // (`invalid_code`), and so must this: guessing which would be a lie,
        // and the action is the same for all three.
        404 => {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "{base_url} did not accept that pairing code — it is unknown, \
                     already used, or expired. Mint a fresh one on the hub with \
                     `fleet-hub pair --name <this machine>` and paste it here."
                ),
            ))
        }
        // The per-address attempt budget (one every 6 s). Its own code so the
        // dialog can say "wait" rather than "wrong code", which would send
        // someone to mint codes they cannot spend.
        429 => {
            return Err(IpcError::new(
                codes::E_RATE_LIMITED,
                format!(
                    "{base_url} is refusing pairing attempts for now (it allows about \
                     ten a minute from one address) — wait a few seconds and try again"
                ),
            ))
        }
        400 => {
            return Err(IpcError::new(
                codes::E_INVALID,
                match summarise(&response.body) {
                    Some(s) => format!("{base_url} could not read the pairing request: {s}"),
                    None => format!(
                        "{base_url} could not read the pairing request (400) — check that \
                         the URL points at a fleet-hub and not at something else"
                    ),
                },
            ))
        }
        // A 5xx, a proxy's 502, a 404 from an entirely wrong path: this call
        // never reached a working pairing route. Calling it a bad code would
        // send the user round a loop of minting codes that cannot work.
        other => {
            return Err(IpcError::new(
                codes::E_HUB_UNREACHABLE,
                match summarise(&response.body) {
                    Some(s) => format!("{base_url} answered {other} to the pairing request: {s}"),
                    None => format!("{base_url} answered {other} to the pairing request"),
                },
            ))
        }
    }

    let body: serde_json::Value = serde_json::from_str(&response.body).map_err(|e| {
        IpcError::new(
            codes::E_PARSE,
            format!("{base_url} answered the pairing request with something unreadable: {e}"),
        )
    })?;
    let token = body
        .get("token")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if token.is_empty() {
        // Storing an empty token would "succeed" and then 401 on every later
        // call, with nothing anywhere explaining why.
        return Err(IpcError::new(
            codes::E_PARSE,
            format!(
                "{base_url} accepted the code but sent no client token — this does not \
                 look like a fleet-hub pairing route"
            ),
        ));
    }
    Ok(PairedClient {
        token,
        name: non_empty(body.get("name")).unwrap_or_else(|| DEFAULT_NAME.to_string()),
        mode: non_empty(body.get("mode")).unwrap_or_else(|| UNKNOWN_MODE.to_string()),
        hub: non_empty(body.get("hub")).unwrap_or_else(|| base_url.to_string()),
    })
}

fn non_empty(v: Option<&serde_json::Value>) -> Option<String> {
    let s = v?.as_str()?.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Redeem `code` at `base_url` for a client token.
///
/// `base_url` is expected normalised (see `super::normalise_base_url`) — the
/// trailing slash is stripped here anyway, because `//pair` is a 404 nobody
/// would diagnose from the message.
pub async fn redeem(
    transport: &dyn PairTransport,
    base_url: &str,
    code: &str,
) -> Result<PairedClient, IpcError> {
    let base = base_url.trim_end_matches('/');
    // Pasted off a terminal: whitespace comes with it, and the hub's alphabet
    // is upper-case Crockford, so a lower-cased paste is a typo we can fix
    // rather than a failure to report.
    let code = code.trim().to_ascii_uppercase();
    let body = serde_json::json!({ "code": code }).to_string();
    let url = format!("{base}/pair");
    let response = transport.post_pair(&url, body).await.map_err(|e| {
        // The transport was handed the code inside the body, so its own error
        // text could quote it back. Scrub before it reaches a toast or a log.
        let scrubbed = e.replace(&code, "<code>");
        IpcError::new(
            codes::E_HUB_UNREACHABLE,
            format!("{base} did not answer the pairing request: {scrubbed}"),
        )
    })?;
    read_pair_response(base, response).map_err(|e| scrub_code(e, &code))
}

/// No error text may echo the pairing code: these reach a toast and the log,
/// and a code in a log is a token anyone reading it can mint.
fn scrub_code(e: IpcError, code: &str) -> IpcError {
    if code.is_empty() || !e.message.contains(code) {
        return e;
    }
    IpcError::new(&e.code, e.message.replace(code, "<code>"))
}

/// One whole pairing exchange. Short: the hub answers from memory.
const PAIR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// The real transport: the same hand-written HTTP the tool calls use, minus
/// the `Authorization` header.
pub struct TcpPairTransport;

#[async_trait::async_trait]
impl PairTransport for TcpPairTransport {
    async fn post_pair(&self, url: &str, body: String) -> Result<HubResponse, String> {
        let at = Endpoint::parse(url)?;
        // No `Authorization` header, by design — this route is
        // unauthenticated because the caller has no credential yet
        // (`mcp::build_app` mounts it outside the `authorize` layer).
        let request = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\
             Accept: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            at.target(),
            at.authority(),
            body.len()
        );
        let raw = tokio::time::timeout(PAIR_TIMEOUT, super::remote::exchange(&at, &request))
            .await
            .map_err(|_| format!("no answer within {PAIR_TIMEOUT:.0?}"))??;
        split_response(&raw)
    }
}

#[cfg(test)]
#[path = "tests_pairing.rs"]
mod tests;
