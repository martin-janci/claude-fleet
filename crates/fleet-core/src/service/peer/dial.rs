//! The dialer's exchange loop: one task per live dialer link. Sends the
//! outbox and pending rejections, long-polls when it has nothing, drops a
//! parked poll the moment the outbox gets a row, backs off on transport
//! failure and stops on a refusal.
//!
//! The link's token lives in [`HttpPeerCall`] and in `PeerLinkRow` (whose
//! `Debug` redacts it). Neither this module nor the supervisor puts it in a
//! log field or an error, and `HttpPeerCall` deliberately has no `Debug`.

use super::apply::{apply_inbound, apply_results, outbox_to_wire};
use super::backoff::{is_terminal, Backoff};
use super::validate::{check_batch, check_fleet_id};
use super::wire::{
    cap_page, ExchangeRequest, ExchangeResponse, ResultStatus, WireResult, PEER_BATCH_MAX,
    PEER_WAIT_MAX_MS, PROTO,
};
use crate::http_client::HubTransport;
use crate::ipc_error::{codes, lock, IpcError};
use crate::mcp::guard;
use crate::service::messages::timeline_detail;
use crate::ssh::SshClient;
use crate::store::{
    Adopted, PeerLinkRow, Store, LINK_CONNECTED, LINK_INCOMPATIBLE, LINK_REFUSED, LINK_RETRYING,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Past the long-poll budget before a call counts as a transport timeout.
const CALL_MARGIN: Duration = Duration::from_secs(10);
/// How often a parked dialer re-reads its outbox (and its link row) when no
/// notify arrives — a row written by another process (the CLI) has none.
const POLL_FLOOR: Duration = Duration::from_millis(500);
/// A parked call answered empty sooner than this was not a long-poll: a
/// peer or proxy answering at once, or a second dialer on the same link
/// releasing this one. The next call waits out the backoff instead (G3).
const EMPTY_POLL_FLOOR: Duration = Duration::from_secs(1);

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug)]
pub enum CallError {
    /// Retry with backoff: the peer was not reached, or answered with
    /// anything that is not a refusal.
    Transport(String),
    /// Terminal: the link stops (`refused`, or `incompatible` for
    /// `E_UNSUPPORTED`).
    Refused { code: String, message: String },
}

/// The one classifier of a peer's coded error, shared by [`HttpPeerCall`]
/// and the two-hub test fake: a refusal only for [`is_terminal`] codes,
/// every other code — `E_RATE_LIMITED` included — a transport failure. The
/// peer's words are scrubbed onto one capped line: they land in
/// `last_error`, which an operator reads.
pub(crate) fn classify(code: &str, message: String) -> CallError {
    let message = guard::scrub_line(&timeline_detail(&message));
    let code = guard::scrub_line(&timeline_detail(code));
    if is_terminal(&code) {
        CallError::Refused { code, message }
    } else {
        CallError::Transport(format!("{code}: {message}"))
    }
}

#[async_trait::async_trait]
pub trait PeerCall: Send + Sync {
    async fn exchange(
        &self,
        req: &ExchangeRequest,
        timeout: Duration,
    ) -> Result<ExchangeResponse, CallError>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum LinkExit {
    Cancelled,
    Revoked,
    Refused,
    Incompatible,
    /// The handshake merged this row into an older link for the same fleet.
    Rebound(i64),
    /// The row changed under this loop — new credentials from a re-pair, or
    /// revoked — so its writes no longer apply. The supervisor starts the
    /// row again on its current credentials.
    Superseded,
}

/// `peer_exchange` over HTTP(S). Holds the link's token: no `Debug`.
pub struct HttpPeerCall {
    pub url: String,
    pub token: String,
    pub transport: Arc<dyn HubTransport>,
}

#[async_trait::async_trait]
impl PeerCall for HttpPeerCall {
    async fn exchange(
        &self,
        req: &ExchangeRequest,
        timeout: Duration,
    ) -> Result<ExchangeResponse, CallError> {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "peer_exchange", "arguments": req },
        })
        .to_string();
        let url = format!("{}/mcp", self.url.trim_end_matches('/'));
        let resp = tokio::time::timeout(timeout, self.transport.post_json(&url, &self.token, body))
            .await
            .map_err(|_| CallError::Transport(format!("no answer within {timeout:?}")))?
            .map_err(|e| CallError::Transport(self.redact(&e)))?;
        read_answer(resp.status, &resp.body)
    }
}

impl HttpPeerCall {
    /// A transport error names the URL at most; scrubbed of the token anyway.
    /// A token too short to be a real one (a test's `"t"`) is left alone:
    /// replacing it would mangle every word that contains it.
    fn redact(&self, s: &str) -> String {
        let one = guard::scrub_line(s);
        if self.token.len() < 8 {
            one
        } else {
            one.replace(&self.token, "[token]")
        }
    }
}

/// PURE: one HTTP answer to a `peer_exchange` call.
fn read_answer(status: u16, body: &str) -> Result<ExchangeResponse, CallError> {
    match status {
        200 => {}
        // The hub's auth layer answers 401 for a revoked or unknown token:
        // terminal.
        401 => {
            return Err(CallError::Refused {
                code: codes::E_UNAUTHORIZED.into(),
                message: "the peer refused this token".into(),
            })
        }
        // A bare 403 included (G20): the hub answers it only for an
        // Origin/Host mismatch, and a proxy in between may answer it for
        // anything. The hub refuses a LINK with a structured tool error
        // (below), never a bare status, so this is retried.
        s => return Err(CallError::Transport(format!("HTTP {s}"))),
    }
    let payload = crate::mcp::wire::last_event_payload(body);
    let envelope: serde_json::Value = serde_json::from_str(&payload)
        .map_err(|e| CallError::Transport(format!("unreadable answer: {e}")))?;
    if let Some(err) = envelope.get("error") {
        let message = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        // Only rmcp's "no such method / no such tool" (rmcp 1.7 answers an
        // unknown tool with -32602 "tool not found") means an older hub.
        // Any other protocol error may be transient: retried.
        let rpc = err.get("code").and_then(|c| c.as_i64());
        let unknown =
            rpc == Some(-32601) || (rpc == Some(-32602) && message.contains("tool not found"));
        return Err(if unknown {
            classify(codes::E_UNSUPPORTED, message)
        } else {
            classify(codes::E_HUB_PROTOCOL, message)
        });
    }
    let result = envelope
        .get("result")
        .ok_or_else(|| CallError::Transport("no result".into()))?;
    let text = result
        .pointer("/content/0/text")
        .and_then(|t| t.as_str())
        .unwrap_or_default();
    if result.get("isError").and_then(|v| v.as_bool()) == Some(true) {
        // The structured form first (`tool_error_result`), the text block's
        // "CODE: message" as the fallback.
        let structured = result
            .get("structuredContent")
            .and_then(|sc| Some((sc.get("code")?.as_str()?, sc.get("message")?.as_str()?)));
        let (code, message) = match structured {
            Some((c, m)) => (c.to_string(), m.to_string()),
            None => match text.split_once(": ") {
                Some((c, m)) if c.starts_with("E_") => (c.to_string(), m.to_string()),
                _ => (codes::E_INTERNAL.to_string(), text.to_string()),
            },
        };
        return Err(classify(&code, message));
    }
    serde_json::from_str(text)
        .map_err(|e| CallError::Transport(format!("unreadable exchange: {e}")))
}

/// What one successful exchange came to.
enum Settled {
    /// Applied; reset the backoff.
    Ok,
    /// Applied, but the peer answered no result for this many of the
    /// messages sent: back off rather than resend at full speed.
    Short(usize),
    /// Not applied: the handshake's fleet still has a running link. Stay
    /// `retrying` with this reason and handshake again after a backoff.
    Wait(String),
    Exit(LinkExit),
}

/// One loop per dialer link. `token` is the credential the loop was
/// started with (the one `call` sends): every state or progress write the
/// loop makes is fenced on it, so a loop a re-pair has outlived cannot
/// write over the row, and it stops as `Superseded` once it sees the row
/// carry another token.
pub async fn run_link(
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    link_id: i64,
    token: String,
    call: Arc<dyn PeerCall>,
    cancel: CancellationToken,
) -> LinkExit {
    let mut backoff = Backoff::new();
    let Ok(notify) = lock(&store).map(|s| s.message_notify()) else {
        return LinkExit::Cancelled;
    };
    let fence = Fence {
        store: &store,
        link_id,
        token: &token,
    };
    // The first exchange of a loop never parks: it is the handshake (or the
    // resume after a restart), and its answer settles the link's state now
    // rather than a long-poll later.
    let mut first = true;
    loop {
        let snapshot = read_link(&store, link_id);
        let (own, link, send, rejects) = match snapshot {
            Ok(Some(x)) => x,
            Ok(None) => return LinkExit::Revoked,
            Err(e) => {
                tracing::warn!(link_id, error = %e.message, "[peer] cannot read the link; retrying");
                if !fence.state(LINK_RETRYING, &format!("{}: {}", e.code, e.message)) {
                    return LinkExit::Superseded;
                }
                if sleep_or_cancel(&cancel, backoff.next()).await {
                    return LinkExit::Cancelled;
                }
                continue;
            }
        };
        if link.revoked_at.is_some() {
            return LinkExit::Revoked;
        }
        if link.token.as_deref() != Some(token.as_str()) {
            return LinkExit::Superseded;
        }
        let idle = send.is_empty() && rejects.is_empty();
        let parks = idle && !first && link.fleet_id.is_some();
        let wait_ms = if parks { PEER_WAIT_MAX_MS } else { 0 };
        let req = ExchangeRequest {
            proto: PROTO,
            fleet_id: own.clone(),
            send,
            after: link.after,
            results: rejects,
            wait_ms,
        };
        let timeout = Duration::from_millis(wait_ms) + CALL_MARGIN;
        let started = tokio::time::Instant::now();
        let outcome = tokio::select! {
            biased;
            _ = cancel.cancelled() => return LinkExit::Cancelled,
            r = call.exchange(&req, timeout) => Some(r),
            _ = wake_parked(&store, &notify, link_id, &token), if parks => None,
        };
        // `None`: the parked poll was dropped to send now (D2, D7 — `after`
        // did not move, so whatever it would have carried comes again).
        let Some(result) = outcome else { continue };
        match result {
            Ok(resp) => {
                // G3: a parked call that came back empty before the floor did
                // not wait at all — whoever answered it will answer the next
                // one the same way. Only the backoff breaks that spin.
                let empty_at_once = parks
                    && resp.messages.is_empty()
                    && resp.results.is_empty()
                    && started.elapsed() < EMPTY_POLL_FLOOR;
                match settle(&fence, &ssh, &link, &own, &req, resp).await {
                    Ok(Settled::Ok) if empty_at_once => {
                        first = false;
                        tracing::debug!(
                            link_id,
                            "[peer] a parked poll came back empty at once; backing off"
                        );
                        if sleep_or_cancel(&cancel, backoff.next()).await {
                            return LinkExit::Cancelled;
                        }
                    }
                    Ok(Settled::Ok) => {
                        first = false;
                        backoff.reset();
                    }
                    Ok(Settled::Short(unanswered)) => {
                        first = false;
                        tracing::warn!(
                            link_id,
                            unanswered,
                            "[peer] the peer answered no result for a sent message; backing off"
                        );
                        // G14(b): still exchanging, so still `connected` —
                        // but the operator sees why nothing is settling.
                        let why =
                            format!("the peer did not answer for {unanswered} sent message(s)");
                        if !fence.state(LINK_CONNECTED, &why) {
                            return LinkExit::Superseded;
                        }
                        if sleep_or_cancel(&cancel, backoff.next()).await {
                            return LinkExit::Cancelled;
                        }
                    }
                    Ok(Settled::Wait(why)) => {
                        tracing::info!(
                            link_id,
                            "[peer] the peer's fleet is still linked; waiting for that link to stop"
                        );
                        if !fence.state(LINK_RETRYING, &why) {
                            return LinkExit::Superseded;
                        }
                        if sleep_or_cancel(&cancel, backoff.next()).await {
                            return LinkExit::Cancelled;
                        }
                    }
                    Ok(Settled::Exit(exit)) => return exit,
                    Err(e) => {
                        tracing::warn!(link_id, error = %e.message, "[peer] applying an exchange failed; retrying");
                        if !fence.state(LINK_RETRYING, &format!("{}: {}", e.code, e.message)) {
                            return LinkExit::Superseded;
                        }
                        if sleep_or_cancel(&cancel, backoff.next()).await {
                            return LinkExit::Cancelled;
                        }
                    }
                }
            }
            Err(CallError::Transport(why)) => {
                tracing::debug!(link_id, error = %why, "[peer] exchange failed; backing off");
                if !fence.state(LINK_RETRYING, &why) {
                    return LinkExit::Superseded;
                }
                if sleep_or_cancel(&cancel, backoff.next()).await {
                    return LinkExit::Cancelled;
                }
            }
            Err(CallError::Refused { code, message }) => {
                let (state, exit) = if code == codes::E_UNSUPPORTED {
                    (LINK_INCOMPATIBLE, LinkExit::Incompatible)
                } else {
                    (LINK_REFUSED, LinkExit::Refused)
                };
                tracing::warn!(link_id, code = %code, "[peer] the peer refused the link; stopping");
                return fence.terminal(state, &format!("{code}: {message}"), exit);
            }
        }
    }
}

/// The loop's writes to its own row, fenced on the token it was started
/// with. Holds the token: no `Debug`.
struct Fence<'a> {
    store: &'a Mutex<Store>,
    link_id: i64,
    token: &'a str,
}

impl Fence<'_> {
    /// `false` only when the row no longer carries this loop's token (a
    /// re-pair) or was revoked: the loop is stale and must stop. A store
    /// error cannot tell, so it counts as written and the loop goes on.
    fn state(&self, state: &str, why: &str) -> bool {
        lock(self.store)
            .and_then(|s| {
                s.set_dialer_link_state(self.link_id, self.token, state, Some(why), now_unix())
            })
            .unwrap_or(true)
    }

    /// Write a terminal state and end with `exit` — or `Superseded` when the
    /// row has moved on, so the supervisor starts it on its new credentials.
    fn terminal(&self, state: &str, why: &str, exit: LinkExit) -> LinkExit {
        if self.state(state, why) {
            exit
        } else {
            LinkExit::Superseded
        }
    }
}

type Snapshot = (
    String,
    PeerLinkRow,
    Vec<super::wire::WireMessage>,
    Vec<WireResult>,
);

/// The link row, our fleet id, the outbox page and the pending rejections.
/// `ensure_local_fleet_id` locks the store itself, so it runs first, outside
/// the guard below.
fn read_link(store: &Mutex<Store>, link_id: i64) -> Result<Option<Snapshot>, IpcError> {
    let own = crate::service::address::ensure_local_fleet_id(store)?;
    let s = lock(store)?;
    let Some(link) = s.peer_link(link_id)? else {
        return Ok(None);
    };
    let rows = s.pending_outbox(link_id, 0, PEER_BATCH_MAX as i64)?;
    // Cut by size too: the rest goes on the next exchange, which follows at
    // once (a non-empty send never parks).
    let (send, _) = cap_page(outbox_to_wire(&s, &own, rows)?);
    let mut rejects: Vec<WireResult> = link
        .pending_rejects
        .as_deref()
        .map(|j| serde_json::from_str(j).unwrap_or_default())
        .unwrap_or_default();
    // A response is at most PEER_BATCH_MAX items, so this never cuts; it
    // keeps a corrupt column from turning every request into a refusal.
    rejects.truncate(PEER_BATCH_MAX);
    Ok(Some((own, link, send, rejects)))
}

/// Apply one successful exchange. `Err` leaves `after` and
/// `pending_rejects` as they were, so the same page comes again.
async fn settle(
    fence: &Fence<'_>,
    ssh: &Arc<SshClient>,
    link: &PeerLinkRow,
    own: &str,
    req: &ExchangeRequest,
    resp: ExchangeResponse,
) -> Result<Settled, IpcError> {
    let store = fence.store;
    if resp.proto != PROTO {
        let why = format!("the peer speaks proto {}, not {PROTO}", resp.proto);
        return Ok(Settled::Exit(fence.terminal(
            LINK_INCOMPATIBLE,
            &why,
            LinkExit::Incompatible,
        )));
    }
    // The peer's fleet id is checked before it is stored, compared, or put
    // in any message: it is the peer's word, shown in `peer list`.
    if check_fleet_id(&resp.fleet_id).is_err() {
        return Ok(Settled::Exit(fence.terminal(
            LINK_INCOMPATIBLE,
            "the peer sent an invalid fleet id",
            LinkExit::Incompatible,
        )));
    }
    if resp.fleet_id == own {
        return Ok(Settled::Exit(fence.terminal(
            LINK_REFUSED,
            "the peer answered with this hub's own fleet id",
            LinkExit::Refused,
        )));
    }
    let mut link = link.clone();
    match link.fleet_id.as_deref() {
        None => {
            let adopted = lock(store)?.adopt_dialer_fleet(link.id, &resp.fleet_id);
            let kept = match adopted {
                Ok(Adopted::Link(kept)) => kept,
                // That fleet's dialer link is still running. Most often it
                // is the old loop of a re-pair, not yet refused on its
                // revoked token (parked, or backing off); it may also be a
                // working link this handshake must never take over. Either
                // way nothing the peer answered applies — no results, no
                // messages, `after` unmoved — and this row asks again after
                // a backoff: it merges once the other row has stopped.
                Ok(Adopted::Waiting(live)) => {
                    return Ok(Settled::Wait(format!(
                        "fleet {} is still linked (link {live}); waiting for it to stop",
                        resp.fleet_id
                    )));
                }
                // That fleet already has a listener row (it dials us). One
                // link per fleet, and retrying cannot change that: terminal
                // on THIS row; the other is untouched.
                Err(e) if e.code == codes::E_EXISTS => {
                    return Ok(Settled::Exit(fence.terminal(
                        LINK_REFUSED,
                        &format!("E_EXISTS: {}", e.message),
                        LinkExit::Refused,
                    )));
                }
                Err(e) => return Err(e),
            };
            if kept != link.id {
                return Ok(Settled::Exit(LinkExit::Rebound(kept)));
            }
            link.fleet_id = Some(resp.fleet_id.clone());
        }
        Some(f) if f != resp.fleet_id => {
            return Ok(Settled::Exit(fence.terminal(
                LINK_REFUSED,
                "E_FORBIDDEN: the peer answered as another fleet",
                LinkExit::Refused,
            )));
        }
        Some(_) => {}
    }
    // The peer's limits hold both ways: an oversized answer is not applied.
    check_batch(resp.messages.len(), resp.results.len())
        .map_err(|r| IpcError::new(r.code, format!("the peer's answer: {}", r.message)))?;
    // Only results for what this request sent: a peer settles the rows it
    // was handed, not the rest of this link's outbox. An answer for an id
    // this request did not carry is the peer's mistake (or its malice) and
    // is dropped here, never applied.
    let sent: Vec<i64> = req.send.iter().map(|m| m.id).collect();
    let (results, unsent): (Vec<WireResult>, Vec<WireResult>) = resp
        .results
        .iter()
        .cloned()
        .partition(|r| sent.contains(&r.id));
    if !unsent.is_empty() {
        tracing::debug!(
            link_id = link.id,
            ids = ?unsent.iter().map(|r| r.id).collect::<Vec<_>>(),
            "[peer] ignoring results for ids this request did not send"
        );
    }
    let unanswered = sent
        .iter()
        .filter(|id| !results.iter().any(|r| r.id == **id))
        .count();
    apply_results(store, link.id, &results)?;
    // A store fault here is `Err`: nothing below runs, `after` stays put and
    // the page is handed over again.
    let rejects: Vec<WireResult> = apply_inbound(store, ssh, &link, own, &resp.messages)
        .await?
        .into_iter()
        .filter(|r| r.status == ResultStatus::Rejected)
        .collect();
    // The watermark is the peer's word: the ids are ITS outbox rows, so
    // nothing here can bound them (a hostile peer answering `i64::MAX`
    // plants a watermark that marks all it later queues accepted without
    // delivery — its own loss). What must not happen is that watermark
    // outliving the peer: a re-pair merge starts from 0 again
    // (`Store::adopt_dialer_fleet`), so an honest peer on the same fleet id
    // gets its outbox delivered.
    let after = resp
        .messages
        .iter()
        .map(|m| m.id)
        .max()
        .unwrap_or(req.after)
        .max(req.after);
    // Overwrites `pending_rejects`: the ones this request carried were
    // delivered by its success, so only the new ones remain.
    let rejects_json = if rejects.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&rejects).unwrap_or_default())
    };
    // The items above were stored one transaction each, and the watermark
    // is written only now, separately (the plan's shape, not one big
    // transaction). That is safe because every inbound insert is idempotent
    // on (remote_fleet_id, remote_message_id): a crash between the two
    // leaves `after` behind, the peer hands the page over again, and each
    // item already stored comes back as a duplicate — accepted, not stored
    // twice (D7; crash-table row 2).
    let wrote = lock(store)?.set_dialer_link_progress(
        link.id,
        fence.token,
        after,
        rejects_json.as_deref(),
        now_unix(),
    )?;
    if !wrote {
        return Ok(Settled::Exit(LinkExit::Superseded));
    }
    Ok(if unanswered > 0 {
        Settled::Short(unanswered)
    } else {
        Settled::Ok
    })
}

/// Resolves when a parked poll should be dropped: the outbox got a row, or
/// the link was revoked, removed or re-paired onto other credentials (the
/// loop then exits on its next read).
async fn wake_parked(
    store: &Mutex<Store>,
    notify: &tokio::sync::Notify,
    link_id: i64,
    token: &str,
) {
    loop {
        // Registered BEFORE the read, so a row inserted between the read
        // and the wait still wakes it.
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let wake = lock(store)
            .and_then(|s| {
                Ok(s.has_pending_outbox(link_id, 0)?
                    || s.peer_link(link_id)?.is_none_or(|l| {
                        l.revoked_at.is_some() || l.token.as_deref() != Some(token)
                    }))
            })
            .unwrap_or(false);
        if wake {
            return;
        }
        let _ = tokio::time::timeout(POLL_FLOOR, notified).await;
    }
}

/// True when cancelled.
async fn sleep_or_cancel(cancel: &CancellationToken, d: Duration) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => true,
        _ = tokio::time::sleep(d) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::peer::testkit::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn sse(envelope: serde_json::Value) -> String {
        format!("event: message\ndata: {envelope}\n\n")
    }

    fn tool_error(code: &str, message: &str) -> String {
        sse(serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {
                "isError": true,
                "content": [{"type": "text", "text": format!("{code}: {message}")}],
                "structuredContent": {"code": code, "message": message, "details": null},
            },
        }))
    }

    #[test]
    fn an_answer_is_read_and_every_failure_is_classified() {
        let ok = ExchangeResponse {
            proto: PROTO,
            fleet_id: "fleet-b".into(),
            results: vec![WireResult::accepted(3)],
            messages: vec![],
            more: false,
        };
        let body = sse(serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {"content": [{"type": "text", "text": serde_json::to_string(&ok).unwrap()}]},
        }));
        assert_eq!(read_answer(200, &body).unwrap(), ok);

        let refused = |r: Result<ExchangeResponse, CallError>| match r {
            Err(CallError::Refused { code, .. }) => code,
            other => panic!("not a refusal: {:?}", other.map(|_| ())),
        };
        let transport = |r: Result<ExchangeResponse, CallError>| match r {
            Err(CallError::Transport(why)) => why,
            other => panic!("not a transport failure: {:?}", other.map(|_| ())),
        };
        assert_eq!(refused(read_answer(401, "")), "E_UNAUTHORIZED");
        assert_eq!(
            refused(read_answer(200, &tool_error("E_FORBIDDEN", "removed"))),
            "E_FORBIDDEN"
        );
        assert_eq!(
            refused(read_answer(200, &tool_error("E_UNSUPPORTED", "proto 2"))),
            "E_UNSUPPORTED"
        );
        let unknown_tool = sse(serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "error": {"code": -32602, "message": "tool not found"},
        }));
        assert_eq!(refused(read_answer(200, &unknown_tool)), "E_UNSUPPORTED");
        // Not refusals: the link retries.
        assert!(
            transport(read_answer(200, &tool_error("E_RATE_LIMITED", "busy")))
                .starts_with("E_RATE_LIMITED: ")
        );
        assert!(
            transport(read_answer(200, &tool_error("E_INTERNAL", "db"))).starts_with("E_INTERNAL")
        );
        assert_eq!(transport(read_answer(502, "bad gateway")), "HTTP 502");
        assert_eq!(transport(read_answer(429, "")), "HTTP 429");
        assert!(transport(read_answer(200, "not json")).starts_with("unreadable answer"));
        // The fallback when a hub sends no structured form.
        let text_only = sse(serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {"isError": true, "content": [{"type": "text", "text": "E_FORBIDDEN: no"}]},
        }));
        assert_eq!(refused(read_answer(200, &text_only)), "E_FORBIDDEN");
    }

    /// G20: HTTP 401 is the hub's own answer for a revoked or unknown token —
    /// terminal. A bare HTTP 403 is not a refusal the hub makes of a link
    /// (it answers 403 only for an Origin/Host mismatch, and a proxy may
    /// answer it for anything): a transport failure, retried. The hub's own
    /// refusal is the structured tool error, which stays terminal.
    #[test]
    fn a_401_is_terminal_a_bare_403_is_retried_and_a_structured_refusal_is_terminal() {
        match read_answer(401, "") {
            Err(CallError::Refused { code, .. }) => assert_eq!(code, "E_UNAUTHORIZED"),
            other => panic!("401: not a refusal: {:?}", other.map(|_| ())),
        }
        for body in ["", "<html>Forbidden</html>"] {
            match read_answer(403, body) {
                Err(CallError::Transport(why)) => assert_eq!(why, "HTTP 403"),
                other => panic!("403 {body:?}: not retried: {:?}", other.map(|_| ())),
            }
        }
        for code in ["E_FORBIDDEN", "E_UNAUTHORIZED"] {
            match read_answer(200, &tool_error(code, "no")) {
                Err(CallError::Refused { code: got, .. }) => assert_eq!(got, code),
                other => panic!("{code}: not a refusal: {:?}", other.map(|_| ())),
            }
        }
    }

    /// M2: only rmcp's unknown-method / unknown-tool shapes mean an older
    /// hub (incompatible); every other JSON-RPC error is retried.
    #[test]
    fn only_an_unknown_tool_is_incompatible() {
        let rpc = |code: i64, message: &str| {
            sse(serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "error": {"code": code, "message": message},
            }))
        };
        for (code, message) in [(-32601, "Method not found"), (-32602, "tool not found")] {
            match read_answer(200, &rpc(code, message)) {
                Err(CallError::Refused { code, .. }) => assert_eq!(code, "E_UNSUPPORTED"),
                other => panic!("{code}: not incompatible: {:?}", other.map(|_| ())),
            }
        }
        for (code, message) in [
            (
                -32602,
                "failed to deserialize parameters: missing field `proto`",
            ),
            (-32603, "internal error"),
            (-32000, "server busy"),
        ] {
            match read_answer(200, &rpc(code, message)) {
                Err(CallError::Transport(_)) => {}
                other => panic!("{code} {message}: not retried: {:?}", other.map(|_| ())),
            }
        }
    }

    #[test]
    fn a_peers_words_become_one_capped_line() {
        let noisy = "a\nb\r\n\u{2028}".repeat(200);
        match classify("E_FORBIDDEN", noisy.clone()) {
            CallError::Refused { message, .. } => {
                assert!(
                    !message.chars().any(crate::store::breaks_a_line),
                    "{message:?}"
                );
                assert!(message.chars().count() <= 200);
            }
            CallError::Transport(_) => panic!("a refusal"),
        }
        match classify("E_INTERNAL", noisy) {
            CallError::Transport(why) => {
                assert!(!why.chars().any(crate::store::breaks_a_line), "{why:?}")
            }
            CallError::Refused { .. } => panic!("not a refusal"),
        }
    }

    struct Failing;

    #[async_trait::async_trait]
    impl HubTransport for Failing {
        async fn post_json(
            &self,
            _url: &str,
            bearer: &str,
            _body: String,
        ) -> Result<crate::http_client::HubResponse, String> {
            Err(format!("refused, and a careless transport echoed {bearer}"))
        }
    }

    #[tokio::test]
    async fn a_transport_error_never_carries_the_token() {
        let call = HttpPeerCall {
            url: "https://b.example".into(),
            token: "sekrit-token-value".into(),
            transport: Arc::new(Failing),
        };
        let req = ExchangeRequest {
            proto: PROTO,
            fleet_id: "fleet-a".into(),
            send: vec![],
            after: 0,
            results: vec![],
            wait_ms: 0,
        };
        match call.exchange(&req, Duration::from_secs(1)).await {
            Err(CallError::Transport(why)) => assert!(!why.contains("sekrit"), "{why}"),
            _ => panic!("a transport failure"),
        }
    }

    /// A peer that answers nothing for what it was sent is backed off, not
    /// hammered: the row stays pending and is resent at the backoff's pace.
    struct Mute {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl PeerCall for Mute {
        async fn exchange(
            &self,
            _req: &ExchangeRequest,
            _timeout: Duration,
        ) -> Result<ExchangeResponse, CallError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ExchangeResponse {
                proto: PROTO,
                fleet_id: "fleet-b".into(),
                results: vec![],
                messages: vec![],
                more: false,
            })
        }
    }

    #[tokio::test]
    async fn a_peer_answering_no_result_is_backed_off() {
        let (store, ssh) = hub("fleet-a");
        let a1 = session(&store, "a1");
        let link = {
            let s = store.lock().unwrap();
            let link = s.insert_dialer_link("https://b.example", "t").unwrap();
            s.adopt_dialer_fleet(link, "fleet-b").unwrap();
            let to = s
                .ensure_remote_participant(link, "fleet-b/session/h/b1")
                .unwrap();
            s.insert_outbound_remote(
                a1,
                "fleet-a/session/local/a1",
                to,
                "x",
                "message",
                None,
                false,
            )
            .unwrap();
            link
        };
        let mute = Arc::new(Mute {
            calls: AtomicUsize::new(0),
        });
        let cancel = CancellationToken::new();
        let h = tokio::spawn(run_link(
            store.clone(),
            ssh,
            link,
            "t".into(),
            mute.clone(),
            cancel.clone(),
        ));
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let calls = mute.calls.load(Ordering::SeqCst);
        assert!((1..=3).contains(&calls), "{calls} calls in 1.5 s");
        assert_eq!(
            store
                .lock()
                .unwrap()
                .pending_outbox(link, 0, 50)
                .unwrap()
                .len(),
            1
        );
        // G14(b): the operator sees it. The link still exchanges, so it
        // stays `connected`, but not silently.
        let row = store.lock().unwrap().peer_link(link).unwrap().unwrap();
        assert_eq!(row.state, "connected");
        assert_eq!(
            row.last_error.as_deref(),
            Some("the peer did not answer for 1 sent message(s)")
        );
        cancel.cancel();
        assert_eq!(h.await.unwrap(), LinkExit::Cancelled);
    }

    /// A peer that answers every call as `fleet_id`, with nothing else.
    struct AnswersAs(String);

    #[async_trait::async_trait]
    impl PeerCall for AnswersAs {
        async fn exchange(
            &self,
            _req: &ExchangeRequest,
            _timeout: Duration,
        ) -> Result<ExchangeResponse, CallError> {
            Ok(ExchangeResponse {
                proto: PROTO,
                fleet_id: self.0.clone(),
                results: vec![],
                messages: vec![],
                more: false,
            })
        }
    }

    /// Run a fresh dialer row against a peer answering as `fleet`; the exit
    /// (or `None` if the loop is still running after 3 s) and the row.
    async fn handshake_as(fleet: &str) -> (Option<LinkExit>, PeerLinkRow) {
        let (store, ssh) = hub("fleet-a");
        let link = store
            .lock()
            .unwrap()
            .insert_dialer_link("https://b.example", "t")
            .unwrap();
        let cancel = CancellationToken::new();
        let h = tokio::spawn(run_link(
            store.clone(),
            ssh,
            link,
            "t".into(),
            Arc::new(AnswersAs(fleet.to_string())),
            cancel.clone(),
        ));
        let exit = match tokio::time::timeout(Duration::from_secs(3), h).await {
            Ok(done) => Some(done.unwrap()),
            Err(_) => {
                cancel.cancel();
                None
            }
        };
        let row = store.lock().unwrap().peer_link(link).unwrap().unwrap();
        (exit, row)
    }

    /// I1: the fleet id a peer answers with is checked before it is stored:
    /// a malformed one (a line break, any length) is `incompatible`, and the
    /// row pins nothing.
    // multi_thread: a peer that answers instantly must not starve the
    // timeout that ends the test if the loop never stops.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_peer_answering_an_invalid_fleet_id_is_incompatible() {
        for bad in ["fleet\nb".to_string(), "f".repeat(5000), String::new()] {
            let (exit, row) = handshake_as(&bad).await;
            assert_eq!(exit, Some(LinkExit::Incompatible), "{:?}", bad.len());
            assert_eq!(row.state, LINK_INCOMPATIBLE);
            assert!(
                row.fleet_id.is_none(),
                "{:?}",
                row.fleet_id.map(|f| f.len())
            );
            let why = row.last_error.unwrap_or_default();
            assert!(why.contains("the peer sent an invalid fleet id"), "{why}");
            assert!(why.len() < 200, "the bad id is not echoed");
        }
    }

    /// I1: a peer answering with THIS hub's fleet id is refused.
    // multi_thread: a peer that answers instantly must not starve the
    // timeout that ends the test if the loop never stops.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_peer_answering_with_our_own_fleet_id_is_refused() {
        let (exit, row) = handshake_as("fleet-a").await;
        assert_eq!(exit, Some(LinkExit::Refused));
        assert_eq!(row.state, LINK_REFUSED);
        assert!(row.fleet_id.is_none());
        let why = row.last_error.unwrap_or_default();
        assert!(why.contains("this hub's own fleet id"), "{why}");
    }

    /// I4: the dialer's `send` page is cut by size — fifty worst-case
    /// bodies are far over the cap — and never to nothing.
    #[test]
    fn the_dialers_send_page_is_capped_by_size() {
        use crate::service::peer::wire::{PEER_BODY_MAX, PEER_PAGE_MAX_BYTES};
        let (store, _ssh) = hub("fleet-a");
        let a1 = session(&store, "a1");
        let link = {
            let s = store.lock().unwrap();
            let link = s.insert_dialer_link("https://b.example", "t").unwrap();
            s.adopt_dialer_fleet(link, "fleet-b").unwrap();
            let to = s
                .ensure_remote_participant(link, "fleet-b/session/h/b1")
                .unwrap();
            let body = "\u{1}".repeat(PEER_BODY_MAX);
            for _ in 0..PEER_BATCH_MAX {
                s.insert_outbound_remote(
                    a1,
                    "fleet-a/session/local/a1",
                    to,
                    &body,
                    "message",
                    None,
                    false,
                )
                .unwrap();
            }
            link
        };
        let (_, _, send, _) = read_link(&store, link).unwrap().unwrap();
        let bytes: usize = send
            .iter()
            .map(|m| serde_json::to_string(m).unwrap().len())
            .sum();
        assert!(!send.is_empty());
        assert!(bytes <= PEER_PAGE_MAX_BYTES, "{bytes} bytes in one send");
    }
}
