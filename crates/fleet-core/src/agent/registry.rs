//! Alias → live agent connection.
//!
//! The registry owns nothing about *how* a connection is carried: an entry is
//! a channel some owner drains into a socket, plus a table of requests waiting
//! for their answer. Task 5's `/agent` endpoint supplies both ends; these
//! tests supply a fake one.
//!
//! Two invariants earn their own generation counter ([`ConnId`]):
//!
//! - a second connection for an alias **replaces** the first, so a restarted
//!   agent recovers without waiting for a timeout;
//! - the replaced connection's reader and its eventual `disconnect` must not
//!   touch the replacement. Both are checked against the id they were given.

use crate::ipc_error::{codes, IpcError};
use dashmap::DashMap;
use fleet_proto::{AgentFrame, HubFrame};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Identifies one connection *attempt* for an alias. Monotonic per registry,
/// so a stale owner can always be told apart from the live one.
pub type ConnId = u64;

/// What an agent reported in its `hello` frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentHello {
    pub agent_version: String,
    pub host_name: String,
    pub os: String,
    /// The agent's `proto`. `/agent` (`ws.rs`) judges this against
    /// [`fleet_proto::judge_proto`] BEFORE this hello ever reaches
    /// [`AgentRegistry::connect_bound`] — an out-of-range one is refused, not
    /// registered — so by the time it is here it has already cleared that
    /// check. Kept on the connection as [`Connection::proto`], the
    /// negotiated version [`AgentRegistry::negotiated_proto`] reads back.
    pub proto: u32,
    /// The loopback port the agent relays hooks on (`fleet-agent`'s
    /// `hook_relay`), when it bound one. Stamped on the host row
    /// (`Store::set_host_agent_hook_port`) so provisioning points the host's
    /// hooks at it (`HubBase::for_host`).
    pub hook_port: Option<u16>,
}

/// One row of [`AgentRegistry::snapshot`] — what `agent_status` (Task 8) and
/// the reconcile pass need to know about a connected agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AgentStatus {
    pub alias: String,
    /// Unix seconds when this connection was registered.
    pub connected_at: i64,
    pub agent_version: String,
    pub host_name: String,
    pub os: String,
}

struct Connection {
    id: ConnId,
    hello: AgentHello,
    /// The negotiated protocol version — `hello.proto`, copied out here so
    /// `AgentRegistry::negotiated_proto` does not have to expose the whole
    /// `hello`. `transport.rs` has nothing to gate on it yet: see the doc
    /// comment where its frames are built.
    proto: u32,
    connected_at: i64,
    /// Frames to write to the socket. Unbounded: the writer is a dedicated
    /// task and a blocked `send` here would block a service call.
    outbound: mpsc::UnboundedSender<HubFrame>,
    /// Request id → the caller waiting for its `result`/`pong`.
    pending: DashMap<String, oneshot::Sender<AgentFrame>>,
    /// What this connection authenticated with — the SHA-256 of the host
    /// token it presented — so a later change to that host's token can be
    /// recognised as revoking THIS connection. `None` for an owner that
    /// authenticated some other way (the in-process fake).
    credential: Option<String>,
    /// Cancelled the moment this connection stops being the live one —
    /// replaced, disconnected or evicted — so its owner can tear the socket
    /// down even while its writer is stuck mid-send.
    gone: tokio_util::sync::CancellationToken,
    /// Cancelled, before `gone`, when a newer connection for the same alias
    /// replaced this one: its owner then closes the socket with
    /// [`fleet_proto::SUPERSEDED_CLOSE_CODE`] rather than bare.
    superseded: tokio_util::sync::CancellationToken,
}

impl Connection {
    /// Wake every waiting caller with "the connection is gone". Dropping the
    /// senders is what the caller's `recv` sees; it maps to
    /// `E_AGENT_OFFLINE`, not a hang until the wall clock. Also tells the
    /// owner, through `gone`.
    fn abort_pending(&self) {
        self.gone.cancel();
        self.pending.clear();
    }
}

/// How far back takeovers are counted for [`AgentRegistry::duplicate`].
pub const DUPLICATE_WINDOW_SECS: i64 = 300;

/// Takeovers within [`DUPLICATE_WINDOW_SECS`] that mean two agents share one
/// host's token (review w14-1). A restarted agent replaces its old
/// connection once; two agents on one token keep taking it from each other
/// (#965 only slows that down), whether or not their host names differ: a
/// cloned VM keeps its hostname, and a leftover service beside a reinstall
/// runs on the same machine.
pub const DUPLICATE_TAKEOVERS: usize = 2;

/// Two agents are taking one host's connection from each other.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DuplicateAgent {
    /// Unix seconds of the first takeover still in the window.
    pub since: i64,
    /// Takeovers in the window.
    pub takeovers: usize,
    /// The agents that said hello in those takeovers, as
    /// `"<host_name> (fleet-agent <version>)"`, each once.
    pub peers: Vec<String>,
}

/// One takeover: when, and the two connections' labels (replaced, new).
type Takeover = (i64, String, String);

/// Is `credential` still current for `alias`? See [`AgentRegistry::verify_with`].
type Verifier = Box<dyn Fn(&str, &str) -> bool + Send + Sync>;

/// Alias → the one live agent connection for that host.
pub struct AgentRegistry {
    conns: DashMap<String, Arc<Connection>>,
    next_id: AtomicU64,
    verifier: std::sync::OnceLock<Verifier>,
    /// Alias → its recent takeovers, oldest first, pruned to
    /// [`DUPLICATE_WINDOW_SECS`].
    takeovers: DashMap<String, Vec<Takeover>>,
}

impl AgentRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            conns: DashMap::new(),
            next_id: AtomicU64::new(1),
            verifier: std::sync::OnceLock::new(),
            takeovers: DashMap::new(),
        })
    }

    /// Judge every connection's credential with `current(alias, credential)`
    /// before anything is sent over it. The hub installs this once, with the
    /// store behind it (`SshClient::with_agents`); a later call is ignored.
    ///
    /// **Why here, and not only in the router.** The router used to check
    /// the live connection and then `request` looked the connection up AGAIN
    /// to send on, so a stale connection registering between the two (a
    /// pre-rotation upgrade saying hello late) received a frame whose check
    /// it never passed (the re-review's NEW-1). Checked here, the connection
    /// that was judged is the very `Arc` the frame is written to.
    pub fn verify_with(&self, current: impl Fn(&str, &str) -> bool + Send + Sync + 'static) {
        let _ = self.verifier.set(Box::new(current));
    }

    /// Register a live connection for `alias`, replacing (and aborting) any
    /// previous one. `outbound` is drained by the connection's owner.
    pub fn connect(
        &self,
        alias: &str,
        hello: AgentHello,
        outbound: mpsc::UnboundedSender<HubFrame>,
    ) -> ConnId {
        self.register(alias, hello, outbound, None)
    }

    /// [`AgentRegistry::connect`] for a connection that authenticated with a
    /// host token: `credential` is that token's SHA-256, which the verifier
    /// ([`AgentRegistry::verify_with`]) checks before every send.
    pub fn connect_bound(
        &self,
        alias: &str,
        hello: AgentHello,
        outbound: mpsc::UnboundedSender<HubFrame>,
        credential: String,
    ) -> ConnId {
        self.register(alias, hello, outbound, Some(credential))
    }

    fn register(
        &self,
        alias: &str,
        hello: AgentHello,
        outbound: mpsc::UnboundedSender<HubFrame>,
        credential: Option<String>,
    ) -> ConnId {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let conn = Arc::new(Connection {
            id,
            proto: hello.proto,
            hello,
            connected_at: now_unix(),
            outbound,
            pending: DashMap::new(),
            credential,
            gone: tokio_util::sync::CancellationToken::new(),
            superseded: tokio_util::sync::CancellationToken::new(),
        });
        let host_name = conn.hello.host_name.clone();
        let label = peer_label(&conn.hello);
        if let Some(previous) = self.conns.insert(alias.to_string(), conn) {
            let replaced = peer_label(&previous.hello);
            let duplicate = self.note_takeover(alias, now_unix(), replaced.clone(), label.clone());
            if previous.hello.host_name != host_name || duplicate.is_some() {
                let peers = duplicate
                    .map(|d| d.peers.join(", "))
                    .unwrap_or_else(|| format!("{replaced}, {label}"));
                tracing::warn!(
                    host = %alias,
                    "[agent] {} replaced the agent on {}: two agents are using this host's token \
                     ({}). Stop one, or rotate the token",
                    fleet_proto::sanitize_for_log(&host_name, 64),
                    fleet_proto::sanitize_for_log(&previous.hello.host_name, 64),
                    fleet_proto::sanitize_for_log(&peers, 256),
                );
            }
            previous.superseded.cancel();
            previous.abort_pending();
        }
        id
    }

    /// Record a takeover of `alias` at `at` and answer the duplicate it now
    /// amounts to, if any.
    fn note_takeover(
        &self,
        alias: &str,
        at: i64,
        replaced: String,
        new: String,
    ) -> Option<DuplicateAgent> {
        let mut entry = self.takeovers.entry(alias.to_string()).or_default();
        entry.push((at, replaced, new));
        entry.retain(|(t, _, _)| at - *t <= DUPLICATE_WINDOW_SECS);
        duplicate_of(&entry)
    }

    /// Two agents are taking `alias`'s connection from each other: at least
    /// [`DUPLICATE_TAKEOVERS`] takeovers in the last
    /// [`DUPLICATE_WINDOW_SECS`]. `agent_status` reports it per host.
    pub fn duplicate(&self, alias: &str) -> Option<DuplicateAgent> {
        self.duplicate_at(alias, now_unix())
    }

    fn duplicate_at(&self, alias: &str, now: i64) -> Option<DuplicateAgent> {
        let entry = self.takeovers.get(alias)?;
        let recent: Vec<Takeover> = entry
            .iter()
            .filter(|(t, _, _)| now - *t <= DUPLICATE_WINDOW_SECS)
            .cloned()
            .collect();
        duplicate_of(&recent)
    }

    /// Deregister `conn`, if it is still the live connection for `alias`. A
    /// late call from a connection that was already replaced does nothing.
    pub fn disconnect(&self, alias: &str, conn: ConnId) {
        let gone = self.conns.remove_if(alias, |_, live| live.id == conn);
        if let Some((_, live)) = gone {
            live.abort_pending();
        }
    }

    /// The live connection for `alias`, if its credential is still current.
    /// One whose credential is stale is dropped on the spot, so nothing is
    /// ever sent over it; one with no credential (the in-process fake), or a
    /// registry with no verifier installed, is taken as it is.
    ///
    /// This is what makes revocation reach a connection that is already
    /// open: the token is checked at the upgrade, and the upgrade is long
    /// past by the time an operator rotates, narrows or removes it.
    fn live_current(&self, alias: &str) -> Option<Arc<Connection>> {
        let live = self.live(alias)?;
        let (Some(credential), Some(current)) = (live.credential.as_deref(), self.verifier.get())
        else {
            return Some(live);
        };
        if current(alias, credential) {
            return Some(live);
        }
        self.disconnect(alias, live.id);
        None
    }

    /// Fires when connection `conn` for `alias` stops being the live one. An
    /// already-cancelled token when it has stopped already, so an owner that
    /// asks late is told at once.
    pub fn gone(&self, alias: &str, conn: ConnId) -> tokio_util::sync::CancellationToken {
        match self.live(alias) {
            Some(live) if live.id == conn => live.gone.clone(),
            _ => {
                let gone = tokio_util::sync::CancellationToken::new();
                gone.cancel();
                gone
            }
        }
    }

    /// Fires when connection `conn` for `alias` was replaced by a newer one
    /// (and not merely disconnected). Read it while `conn` is live: asked
    /// later, it never fires.
    pub fn superseded(&self, alias: &str, conn: ConnId) -> tokio_util::sync::CancellationToken {
        match self.live(alias) {
            Some(live) if live.id == conn => live.superseded.clone(),
            _ => tokio_util::sync::CancellationToken::new(),
        }
    }

    /// Is an agent connected for this host?
    pub fn connected(&self, alias: &str) -> bool {
        self.conns.contains_key(alias)
    }

    /// The protocol version negotiated with the live connection for `alias`,
    /// if any. Nothing built on top of this needs it yet — no frame this
    /// crate sends requires an agent above `proto` 1 to understand it — but
    /// `AgentTransport` (`transport.rs`) is where a future one would gate a
    /// new frame kind on it, and this is what it would read.
    pub fn negotiated_proto(&self, alias: &str) -> Option<u32> {
        self.live(alias).map(|c| c.proto)
    }

    /// Every connected agent, ordered by alias.
    pub fn snapshot(&self) -> Vec<AgentStatus> {
        let mut rows: Vec<AgentStatus> = self
            .conns
            .iter()
            .map(|e| AgentStatus {
                alias: e.key().clone(),
                connected_at: e.connected_at,
                agent_version: e.hello.agent_version.clone(),
                host_name: e.hello.host_name.clone(),
                os: e.hello.os.clone(),
            })
            .collect();
        rows.sort_by(|a, b| a.alias.cmp(&b.alias));
        rows
    }

    /// Send `frame` and wait for the answer carrying the same id.
    ///
    /// With no live agent this returns **immediately** — an agent host that is
    /// not connected must report unreachable the way a down SSH host does, not
    /// spend the caller's whole wall clock finding out.
    pub async fn request(
        &self,
        alias: &str,
        frame: HubFrame,
        timeout: Duration,
    ) -> Result<AgentFrame, IpcError> {
        self.request_after(alias, Vec::new(), frame, timeout).await
    }

    /// [`Self::request`], with `before` sent first down the SAME connection —
    /// an upload's chunks ahead of its final `upload`. Queued together on one
    /// channel, so nothing routed meanwhile lands between them and a replaced
    /// connection cannot get the final frame without its chunks (the agent
    /// would refuse it anyway: the final frame counts them).
    pub async fn request_after(
        &self,
        alias: &str,
        before: Vec<HubFrame>,
        frame: HubFrame,
        timeout: Duration,
    ) -> Result<AgentFrame, IpcError> {
        let id = frame_id(&frame).to_string();
        let conn = self.live_current(alias).ok_or_else(|| offline(alias))?;
        let (tx, rx) = oneshot::channel();
        // Claim the slot, or refuse. A plain `insert` evicted the sitting
        // tenant, and `PendingGuard::drop` then removed the *replacement's*
        // slot by id — so both callers were told `E_AGENT_OFFLINE` while the
        // agent was connected and answering, and the answer that did arrive
        // was dropped. `AgentTransport` cannot reach this (uuid v4 ids), but
        // `request` is public and the `/agent` endpoint drives it, so it
        // fails loudly instead of corrupting the caller that was here first.
        match conn.pending.entry(id.clone()) {
            dashmap::mapref::entry::Entry::Occupied(_) => {
                return Err(IpcError::new(
                    codes::E_AGENT_PROTOCOL,
                    format!("request id {id} is already in flight on {alias}"),
                ));
            }
            dashmap::mapref::entry::Entry::Vacant(slot) => {
                slot.insert(tx);
            }
        }
        // Only now: the guard must not remove a slot this call did not claim.
        let _guard = PendingGuard {
            conn: Arc::clone(&conn),
            id: id.clone(),
        };
        for piece in before {
            if conn.outbound.send(piece).is_err() {
                return Err(offline(alias));
            }
        }
        if conn.outbound.send(frame).is_err() {
            // The owner dropped the receiver: the socket is already gone.
            return Err(offline(alias));
        }
        // …and it could have been replaced between the lookup and the insert,
        // in which case nobody will ever answer this slot. From here on the
        // frame is queued on that connection and may have reached the agent,
        // which runs it whether or not the answer gets back: not "offline"
        // (nothing ran, retry elsewhere) but lost mid-request, a code
        // `codes::may_have_run` counts, so a billed usage call or a
        // `gh repo create` is not fired a second time.
        if self.live(alias).map(|c| c.id) != Some(conn.id) {
            return Err(lost_mid_request(alias));
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(answer)) => Ok(answer),
            // The sender was dropped: the connection was replaced or closed.
            Ok(Err(_)) => Err(lost_mid_request(alias)),
            // `E_SSH_TIMEOUT`, not `E_TIMEOUT`, even though no ssh is
            // involved: it is the code EVERY `SshExec` returns for a blown
            // wall clock (`ssh::wall_clock_error`, shared by `SshClient`,
            // `LocalExec` and `ssh_fake`), and the service layer above the
            // seam branches on it. A code of its own looked harmless and was
            // not — `account_usage::classify_run` reads an unrecognised code
            // as "nothing ran on that host" and re-fires the Anthropic API
            // request against the next one, the exact double-billing that
            // classification exists to prevent. The message says agent, so
            // nothing in the text lies; only the identifier is shared.
            Err(_) => Err(IpcError::new(
                codes::E_SSH_TIMEOUT,
                format!(
                    "agent on {alias} did not answer within {}s",
                    timeout.as_secs_f32()
                ),
            )),
        }
    }

    /// Send a frame that has no answer (`cancel`). Fire and forget: the agent
    /// may already have finished, and an id it does not know is a no-op there.
    pub fn send(&self, alias: &str, frame: HubFrame) -> Result<(), IpcError> {
        let conn = self.live_current(alias).ok_or_else(|| offline(alias))?;
        conn.outbound.send(frame).map_err(|_| offline(alias))
    }

    /// Hand the registry a frame that arrived from `conn`. Returns whether it
    /// resolved a waiting request: an unknown id, a frame from a connection
    /// that has been replaced, and a `hello` are all dropped — never a panic,
    /// because every one of them is reachable from the network.
    pub fn deliver(&self, alias: &str, conn: ConnId, frame: AgentFrame) -> bool {
        let Some(live) = self.live(alias) else {
            return false;
        };
        if live.id != conn {
            return false;
        }
        let Some(id) = agent_frame_id(&frame).map(str::to_string) else {
            return false;
        };
        match live.pending.remove(&id) {
            Some((_, tx)) => tx.send(frame).is_ok(),
            None => false,
        }
    }

    fn live(&self, alias: &str) -> Option<Arc<Connection>> {
        self.conns.get(alias).map(|e| Arc::clone(e.value()))
    }

    /// Which connection is live for `alias`. Test-only (`testkit` for the
    /// `fleet-agent-e2e` crate): it is how an end-to-end test knows a
    /// reconnect has REPLACED the old connection, which `connected(alias)`
    /// cannot tell it.
    #[cfg(all(unix, any(test, feature = "testkit")))]
    pub fn live_conn(&self, alias: &str) -> Option<ConnId> {
        self.live(alias).map(|c| c.id)
    }

    /// How many requests are waiting on this host's connection. Test-only: it
    /// is how a leaked slot (a timed-out or abandoned call) becomes visible.
    #[cfg(test)]
    pub(crate) fn pending_len(&self, alias: &str) -> usize {
        self.live(alias).map(|c| c.pending.len()).unwrap_or(0)
    }
}

/// Removes a request's slot when its caller goes away, and — unlike plain
/// SSH, where a dropped caller or a wall-clock timeout kills the child via
/// `kill_on_drop`/`kill_and_reap` (`ssh.rs`) — tells the agent to stop it
/// too.
struct PendingGuard {
    conn: Arc<Connection>,
    id: String,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        // `Some`: this id was never answered — a dropped caller future or
        // this call's own timeout, both of which reach here the same way —
        // so tell the agent to stop the child too, or it runs on until its
        // `timeout_ms`. May duplicate a `Cancel` that `AgentTransport::exec`'s
        // own token-cancellation path already sent for this id; harmless,
        // since `fleet-agent` no-ops on an id it no longer has in flight.
        // `None`: `deliver` or `abort_pending` already took the slot, so
        // there is nothing to cancel.
        //
        // `outbound` is unbounded so `send` never blocks (`Drop` cannot
        // await); a failure just means the connection is already gone, and
        // the agent's own `timeout_ms` still bounds the child either way.
        if self.conn.pending.remove(&self.id).is_some() {
            let _ = self.conn.outbound.send(HubFrame::Cancel {
                id: self.id.clone(),
            });
        }
    }
}

/// The connection ended after the request was sent: its outcome is unknown.
/// `details.connection_lost` lets a reader that only cares whether the agent
/// is there (the reconcile's redial grace) treat it like `E_AGENT_OFFLINE`.
pub(crate) fn lost_mid_request(alias: &str) -> IpcError {
    IpcError {
        details: Some(serde_json::json!({ "connection_lost": true })),
        ..IpcError::new(
            codes::E_AGENT_PROTOCOL,
            format!(
                "the connection to {alias}'s agent ended mid-request; the command may have run"
            ),
        )
    }
}

/// Whether `e` says the host's agent connection is gone: not connected, or
/// lost while a request was in flight.
pub fn connection_gone(e: &IpcError) -> bool {
    e.code == codes::E_AGENT_OFFLINE
        || e.details
            .as_ref()
            .and_then(|d| d.get("connection_lost"))
            .and_then(serde_json::Value::as_bool)
            == Some(true)
}

fn offline(alias: &str) -> IpcError {
    IpcError::new(
        codes::E_AGENT_OFFLINE,
        format!("no fleet-agent is connected for host {alias}"),
    )
}

/// The request id a hub frame carries. Every variant has one, `welcome`
/// excepted.
pub(crate) fn frame_id(frame: &HubFrame) -> &str {
    match frame {
        HubFrame::Exec { id, .. }
        | HubFrame::Upload { id, .. }
        | HubFrame::UploadChunk { id, .. }
        | HubFrame::Cancel { id }
        | HubFrame::Ping { id } => id,
        // `welcome` carries no id: it is a one-way broadcast `ws.rs` writes
        // straight onto a connection's outbound channel, never through
        // `request`/`send`, and the match must stay exhaustive regardless.
        // The debug assertion is the actual guard — release keeps today's
        // graceful (if meaningless) "" rather than a hard panic in front of
        // a real caller.
        HubFrame::Welcome { .. } => {
            debug_assert!(
                false,
                "a welcome frame must never be routed through frame_id \
                 (request/send) — it is queued directly onto the \
                 connection's outbound channel before registration"
            );
            ""
        }
    }
}

/// The request id an agent frame answers — `None` for `hello`, which answers
/// nothing.
fn agent_frame_id(frame: &AgentFrame) -> Option<&str> {
    match frame {
        AgentFrame::Result { id, .. } | AgentFrame::Pong { id } => Some(id),
        // A piece of an answer, reassembled by `ws.rs` before anything is
        // delivered: never an answer on its own.
        AgentFrame::ResultChunk { .. } | AgentFrame::Hello { .. } | AgentFrame::Report { .. } => {
            None
        }
    }
}

/// How a connection reads in a duplicate warning.
fn peer_label(hello: &AgentHello) -> String {
    format!("{} (fleet-agent {})", hello.host_name, hello.agent_version)
}

fn duplicate_of(takeovers: &[Takeover]) -> Option<DuplicateAgent> {
    if takeovers.len() < DUPLICATE_TAKEOVERS {
        return None;
    }
    let mut peers: Vec<String> = Vec::new();
    for (_, a, b) in takeovers {
        for p in [a, b] {
            if !peers.contains(p) {
                peers.push(p.clone());
            }
        }
    }
    Some(DuplicateAgent {
        since: takeovers[0].0,
        takeovers: takeovers.len(),
        peers,
    })
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::fake::{answer_exit, custom, hello, silent, FakeAgent};
    use crate::ipc_error::codes;
    use fleet_proto::{encode_b64, AgentFrame, HubFrame};
    use std::time::{Duration, Instant};

    fn exec(id: &str) -> HubFrame {
        HubFrame::Exec {
            id: id.into(),
            argv: vec!["true".into()],
            stdin: None,
            timeout_ms: 1_000,
            cap_bytes: None,
        }
    }

    fn ok_result(id: &str) -> AgentFrame {
        AgentFrame::Result {
            chunks: 0,
            id: id.into(),
            exit_code: 0,
            stdout_b64: encode_b64(b""),
            stderr_b64: encode_b64(b""),
            truncated: false,
        }
    }

    // ── offline ─────────────────────────────────────────────────────────────

    /// The headline requirement: no live agent fails *now*, not after the
    /// timeout. The bound only has to separate "returned immediately" from
    /// "errored after the 60 s budget" — it is not a latency measurement, so
    /// it is loose enough that a loaded machine cannot fail it. A tight bound
    /// here used to redden the whole suite, which then skipped every target
    /// after this one.
    #[tokio::test]
    async fn a_request_with_no_connection_is_offline_immediately() {
        let reg = AgentRegistry::new();
        let started = Instant::now();
        let err = reg
            .request("ghost", exec("1"), Duration::from_secs(60))
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_AGENT_OFFLINE);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "waited {:?} before reporting an offline agent (the budget was 60s)",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn send_with_no_connection_is_offline() {
        let reg = AgentRegistry::new();
        let err = reg
            .send("ghost", HubFrame::Cancel { id: "1".into() })
            .unwrap_err();
        assert_eq!(err.code, codes::E_AGENT_OFFLINE);
    }

    // ── connect / snapshot ──────────────────────────────────────────────────

    #[tokio::test]
    async fn connect_records_the_hello_and_shows_up_in_the_snapshot() {
        let reg = AgentRegistry::new();
        assert!(!reg.connected("laptop"));
        let _agent = FakeAgent::connect(&reg, "laptop", answer_exit(0));
        assert!(reg.connected("laptop"));

        let snap = reg.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].alias, "laptop");
        assert_eq!(snap[0].agent_version, hello().agent_version);
        assert_eq!(snap[0].host_name, hello().host_name);
        assert_eq!(snap[0].os, hello().os);
        assert!(snap[0].connected_at > 0, "connect time is recorded");
    }

    #[tokio::test]
    async fn the_snapshot_is_ordered_by_alias() {
        let reg = AgentRegistry::new();
        let _c = FakeAgent::connect(&reg, "charlie", answer_exit(0));
        let _a = FakeAgent::connect(&reg, "alpha", answer_exit(0));
        let _b = FakeAgent::connect(&reg, "bravo", answer_exit(0));
        let aliases: Vec<_> = reg.snapshot().into_iter().map(|s| s.alias).collect();
        assert_eq!(aliases, ["alpha", "bravo", "charlie"]);
    }

    #[tokio::test]
    async fn a_request_reaches_the_agent_and_comes_back() {
        let reg = AgentRegistry::new();
        let agent = FakeAgent::connect(&reg, "laptop", answer_exit(7));
        let answer = reg
            .request("laptop", exec("req-1"), Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(answer, ok_result_with_code("req-1", 7));
        assert_eq!(agent.only_frame(), exec("req-1"));
    }

    fn ok_result_with_code(id: &str, code: i32) -> AgentFrame {
        match ok_result(id) {
            AgentFrame::Result {
                id,
                stdout_b64,
                stderr_b64,
                truncated,
                ..
            } => AgentFrame::Result {
                chunks: 0,
                id,
                exit_code: code,
                stdout_b64,
                stderr_b64,
                truncated,
            },
            other => other,
        }
    }

    // ── reconnection ────────────────────────────────────────────────────────

    /// Review w14-1: one takeover is a restart; two within the window are
    /// two agents on one token, flagged even when both say the same host
    /// name (a cloned VM), naming each agent once. Older takeovers age out.
    #[tokio::test]
    async fn repeated_takeovers_flag_two_agents_on_one_token() {
        let reg = AgentRegistry::new();
        let _a = FakeAgent::connect(&reg, "laptop", answer_exit(0));
        assert_eq!(reg.duplicate("laptop"), None);
        let (tx, _rx) = mpsc::unbounded_channel();
        let other = AgentHello {
            agent_version: "1.0.0".into(),
            ..hello()
        };
        reg.connect("laptop", other, tx);
        assert_eq!(reg.duplicate("laptop"), None, "one takeover is a restart");
        let _c = FakeAgent::connect(&reg, "laptop", answer_exit(0));
        let dup = reg
            .duplicate("laptop")
            .expect("two takeovers in the window");
        assert_eq!(dup.takeovers, 2);
        assert_eq!(
            dup.peers,
            vec![
                "fake-host (fleet-agent 9.9.9)".to_string(),
                "fake-host (fleet-agent 1.0.0)".to_string()
            ]
        );
        assert_eq!(reg.duplicate("desk"), None);
        // Past the window, the same takeovers no longer count.
        assert_eq!(
            reg.duplicate_at("laptop", dup.since + DUPLICATE_WINDOW_SECS + 1),
            None
        );
        let label = || "x (fleet-agent 1)".to_string();
        assert!(reg.note_takeover("desk", 1_000, label(), label()).is_none());
        assert!(reg
            .note_takeover("desk", 1_000 + DUPLICATE_WINDOW_SECS + 1, label(), label())
            .is_none());
    }

    /// A restarted agent must take over without waiting for a timeout.
    #[tokio::test]
    async fn a_second_connection_replaces_the_first() {
        let reg = AgentRegistry::new();
        let first = FakeAgent::connect(&reg, "laptop", answer_exit(1));
        let second = FakeAgent::connect(&reg, "laptop", answer_exit(2));
        assert_ne!(first.conn_id, second.conn_id);

        let answer = reg
            .request("laptop", exec("r"), Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(
            answer,
            ok_result_with_code("r", 2),
            "the new agent answered"
        );
        assert!(first.sent().is_empty(), "the replaced agent got nothing");
        assert_eq!(reg.snapshot().len(), 1, "one row per alias, not two");
    }

    /// Only a takeover marks the old connection superseded; a plain
    /// disconnect, and the replacement itself, are not.
    #[tokio::test]
    async fn only_a_replaced_connection_is_marked_superseded() {
        let reg = AgentRegistry::new();
        let first = FakeAgent::connect(&reg, "laptop", answer_exit(1));
        let replaced = reg.superseded("laptop", first.conn_id);
        let second = FakeAgent::connect(&reg, "laptop", answer_exit(2));
        assert!(replaced.is_cancelled(), "the first was replaced");
        let current = reg.superseded("laptop", second.conn_id);
        reg.disconnect("laptop", second.conn_id);
        assert!(!current.is_cancelled(), "a disconnect is not a takeover");
    }

    /// The replaced connection's reader task may still be draining. Its
    /// frames must not resolve the *new* connection's requests.
    #[tokio::test]
    async fn deliver_from_a_replaced_connection_is_ignored() {
        let reg = AgentRegistry::new();
        let stale = FakeAgent::connect(&reg, "laptop", silent());
        let stale_id = stale.conn_id;
        let _fresh = FakeAgent::connect(&reg, "laptop", silent());

        let reg2 = Arc::clone(&reg);
        let call = tokio::spawn(async move {
            reg2.request("laptop", exec("r"), Duration::from_millis(100))
                .await
        });
        wait_pending(&reg, "laptop", 1).await;
        assert!(
            !reg.deliver("laptop", stale_id, ok_result("r")),
            "a frame from the replaced connection is dropped"
        );
        let err = call.await.unwrap().unwrap_err();
        assert_eq!(err.code, codes::E_SSH_TIMEOUT);
    }

    /// Spin until `n` requests are waiting on `alias`'s connection. Yields
    /// instead of sleeping: the request is a task on this runtime, so this
    /// costs two polls rather than a slice of the real clock. Other tests in
    /// this binary are timing-sensitive; none of mine may hold the machine.
    async fn wait_pending(reg: &AgentRegistry, alias: &str, n: usize) {
        for _ in 0..100_000 {
            if reg.pending_len(alias) == n {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("{alias} never had {n} request(s) pending");
    }

    /// A late `disconnect` from the connection that was replaced must not
    /// tear down the replacement — otherwise a reconnect would leave the
    /// host offline once the old socket finally noticed it was dead.
    #[tokio::test]
    async fn a_stale_disconnect_does_not_evict_the_replacement() {
        let reg = AgentRegistry::new();
        let first = FakeAgent::connect(&reg, "laptop", answer_exit(0));
        let _second = FakeAgent::connect(&reg, "laptop", answer_exit(0));
        first.disconnect();
        assert!(reg.connected("laptop"), "the replacement is still live");
    }

    #[tokio::test]
    async fn disconnect_removes_the_connection() {
        let reg = AgentRegistry::new();
        let agent = FakeAgent::connect(&reg, "laptop", answer_exit(0));
        agent.disconnect();
        assert!(!reg.connected("laptop"));
        assert!(reg.snapshot().is_empty());
        let err = reg
            .request("laptop", exec("r"), Duration::from_secs(60))
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_AGENT_OFFLINE);
    }

    /// The spec's "a call racing a disconnect": the caller must be told the
    /// agent is gone, not left waiting out the whole wall clock. The frame
    /// was already sent, so the code is one `may_have_run` counts.
    #[tokio::test]
    async fn an_in_flight_request_fails_offline_when_the_connection_drops() {
        let reg = AgentRegistry::new();
        let agent = FakeAgent::connect(&reg, "laptop", silent());
        let reg2 = Arc::clone(&reg);
        let call = tokio::spawn(async move {
            reg2.request("laptop", exec("r"), Duration::from_secs(60))
                .await
        });
        wait_pending(&reg, "laptop", 1).await;
        let started = Instant::now();
        agent.disconnect();
        let err = call.await.unwrap().unwrap_err();
        assert_eq!(err.code, codes::E_AGENT_PROTOCOL);
        assert!(codes::may_have_run(&err.code));
        // …yet it still reads as "the agent is gone" for the redial grace.
        assert!(connection_gone(&err));
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "waited {:?} after the disconnect",
            started.elapsed()
        );
    }

    /// A second request under an id that is already in flight used to evict
    /// the first one's slot silently: **both** callers were then told
    /// `E_AGENT_OFFLINE` while the agent was connected and answering, and the
    /// answer that did arrive was dropped. `AgentTransport` cannot reach this
    /// (uuid v4 ids) but `AgentRegistry::request` is `pub` and the `/agent`
    /// endpoint drives it. The duplicate is refused; the original is
    /// untouched and still gets its answer.
    #[tokio::test]
    async fn a_duplicate_request_id_is_refused_and_leaves_the_first_alone() {
        let reg = AgentRegistry::new();
        let _agent = FakeAgent::connect(&reg, "laptop", silent());

        let reg2 = Arc::clone(&reg);
        let first = tokio::spawn(async move {
            reg2.request("laptop", exec("same"), Duration::from_secs(60))
                .await
        });
        wait_pending(&reg, "laptop", 1).await;

        let dup = reg
            .request("laptop", exec("same"), Duration::from_secs(60))
            .await
            .unwrap_err();
        assert_eq!(dup.code, codes::E_AGENT_PROTOCOL);
        assert!(
            dup.message.contains("same"),
            "the message names the id: {}",
            dup.message
        );
        assert_eq!(
            reg.pending_len("laptop"),
            1,
            "the refusal must not disturb the slot it collided with"
        );

        assert!(
            reg.deliver("laptop", _agent.conn_id, ok_result("same")),
            "the original request is still waiting and takes the answer"
        );
        first.await.unwrap().expect("the first caller is answered");
    }

    // ── bookkeeping ─────────────────────────────────────────────────────────

    // `start_paused`: the deadline is the point, and virtual time reaches it
    // without this test occupying the machine for 60 real milliseconds.
    #[tokio::test(start_paused = true)]
    async fn a_timed_out_request_leaves_no_pending_entry_behind() {
        let reg = AgentRegistry::new();
        let _agent = FakeAgent::connect(&reg, "laptop", silent());
        let err = reg
            .request("laptop", exec("r"), Duration::from_millis(60))
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_SSH_TIMEOUT);
        assert_eq!(
            reg.pending_len("laptop"),
            0,
            "a timed-out request must not leak its slot"
        );
    }

    /// Dropping the future (what `tokio::select!` does when a cancellation
    /// token wins the race) must release the slot too.
    #[tokio::test]
    async fn an_abandoned_request_leaves_no_pending_entry_behind() {
        let reg = AgentRegistry::new();
        let _agent = FakeAgent::connect(&reg, "laptop", silent());
        {
            let call = reg.request("laptop", exec("r"), Duration::from_secs(60));
            tokio::pin!(call);
            // One poll dispatches the request and registers its slot; then the
            // future is dropped with the answer still outstanding.
            tokio::select! {
                biased;
                _ = &mut call => panic!("the silent agent cannot have answered"),
                _ = tokio::task::yield_now() => {}
            }
            assert_eq!(reg.pending_len("laptop"), 1, "the slot was taken");
        }
        assert_eq!(reg.pending_len("laptop"), 0);
    }

    // ── cancel on drop (#145) ───────────────────────────────────────────────
    //
    // Over SSH, dropping the caller's future kills the child (`kill_on_drop`).
    // Over the agent transport the child is on another host, so the only way
    // to reach it is a `Cancel` frame — `PendingGuard::drop` best-effort sends
    // one when the request it guarded never got an answer.

    /// (a) A caller that drops its future mid-request — same trigger as
    /// `an_abandoned_request_leaves_no_pending_entry_behind` — must make the
    /// agent receive a `Cancel` naming that request's id, or the remote child
    /// runs on until its own `timeout_ms`.
    #[tokio::test]
    async fn an_abandoned_request_sends_the_agent_a_cancel_for_its_id() {
        let reg = AgentRegistry::new();
        let agent = FakeAgent::connect(&reg, "laptop", silent());
        {
            let call = reg.request("laptop", exec("r"), Duration::from_secs(60));
            tokio::pin!(call);
            // One poll dispatches the request; then the future is dropped
            // with the answer still outstanding — no cancellation token
            // involved, just the caller walking away.
            tokio::select! {
                biased;
                _ = &mut call => panic!("the silent agent cannot have answered"),
                _ = tokio::task::yield_now() => {}
            }
        }
        agent.wait_until_sent(2).await;
        let sent = agent.sent();
        assert_eq!(sent[0], exec("r"), "the exec was dispatched first");
        match &sent[1] {
            HubFrame::Cancel { id } => {
                assert_eq!(id, "r", "the cancel names the abandoned request")
            }
            other => panic!("expected a cancel frame, got {other:?}"),
        }
    }

    /// (b) A request that is answered before the caller drops it must not
    /// produce a `Cancel` — the slot was already emptied by `deliver`, so
    /// `PendingGuard::drop` has nothing to signal.
    #[tokio::test]
    async fn a_completed_request_sends_no_cancel() {
        let reg = AgentRegistry::new();
        let agent = FakeAgent::connect(&reg, "laptop", answer_exit(0));
        reg.request("laptop", exec("r"), Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(
            agent.only_frame(),
            exec("r"),
            "no cancel follows a request that was answered"
        );
    }

    /// (c) The connection's outbound channel can already be closed — its
    /// owner (the socket writer) gone — by the time a caller drops a request
    /// on it. The guard's best-effort send must swallow that, not panic.
    #[tokio::test]
    async fn a_dropped_caller_on_a_connection_whose_outbound_is_already_closed_does_not_panic() {
        let reg = AgentRegistry::new();
        let (tx, rx) = mpsc::unbounded_channel();
        drop(rx); // nothing will ever read a frame sent on `tx`
        reg.connect("laptop", hello(), tx);
        // `request` itself fails while dispatching (the send fails), but not
        // before claiming the pending slot — so its `PendingGuard` drops with
        // the slot still occupied and attempts a `Cancel` on the very same
        // dead channel. That must not panic.
        let err = reg
            .request("laptop", exec("r"), Duration::from_secs(60))
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_AGENT_OFFLINE);
    }

    /// (d) `tokio::time::timeout` giving up drops the inner future the same
    /// way a caller walking away does, so a hub-side timeout must also send
    /// a `Cancel` — parity with `ssh.rs`'s wall-clock arm, which kills the
    /// child itself (`kill_and_reap`) when *its* deadline wins. The caller
    /// must still see the same timeout error as before.
    // `start_paused`, like `a_timed_out_request_leaves_no_pending_entry_
    // behind`: virtual time reaches the deadline without holding a thread.
    #[tokio::test(start_paused = true)]
    async fn a_hub_side_timeout_sends_the_agent_a_cancel_for_its_id() {
        let reg = AgentRegistry::new();
        let agent = FakeAgent::connect(&reg, "laptop", silent());
        let err = reg
            .request("laptop", exec("r"), Duration::from_millis(60))
            .await
            .unwrap_err();
        assert_eq!(
            err.code,
            codes::E_SSH_TIMEOUT,
            "the caller still sees the same timeout as before this fix"
        );
        agent.wait_until_sent(2).await;
        let sent = agent.sent();
        assert_eq!(sent[0], exec("r"), "the exec was dispatched first");
        match &sent[1] {
            HubFrame::Cancel { id } => {
                assert_eq!(id, "r", "the cancel names the timed-out request")
            }
            other => panic!("expected a cancel frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn deliver_of_an_unknown_id_is_dropped_not_a_panic() {
        let reg = AgentRegistry::new();
        let agent = FakeAgent::connect(&reg, "laptop", silent());
        assert!(!reg.deliver("laptop", agent.conn_id, ok_result("nobody-waits")));
        assert!(!reg.deliver("ghost", agent.conn_id, ok_result("no-such-host")));
    }

    #[tokio::test]
    async fn two_requests_in_flight_get_their_own_answers() {
        let reg = AgentRegistry::new();
        // Echo the request id back in stdout so a crossed answer is visible.
        let _agent = FakeAgent::connect(
            &reg,
            "laptop",
            custom(|f: &HubFrame| {
                let id = match f {
                    HubFrame::Exec { id, .. } => id.clone(),
                    _ => return None,
                };
                Some(AgentFrame::Result {
                    chunks: 0,
                    id: id.clone(),
                    exit_code: 0,
                    stdout_b64: encode_b64(id.as_bytes()),
                    stderr_b64: encode_b64(b""),
                    truncated: false,
                })
            }),
        );
        let a = reg.request("laptop", exec("aaa"), Duration::from_secs(5));
        let b = reg.request("laptop", exec("bbb"), Duration::from_secs(5));
        let (a, b) = tokio::join!(a, b);
        assert_eq!(a.unwrap(), stdout_of("aaa"));
        assert_eq!(b.unwrap(), stdout_of("bbb"));
    }

    fn stdout_of(id: &str) -> AgentFrame {
        AgentFrame::Result {
            chunks: 0,
            id: id.into(),
            exit_code: 0,
            stdout_b64: encode_b64(id.as_bytes()),
            stderr_b64: encode_b64(b""),
            truncated: false,
        }
    }
}
