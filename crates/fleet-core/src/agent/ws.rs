//! `GET /agent` — the WebSocket a `fleet-agent` dials in on.
//!
//! The route sits behind the SAME [`authorize`](crate::mcp) layer as `/mcp`
//! and `/events`, and then asks one more question the other two do not: the
//! caller must be a **host**. An agent is a host — the design's second settled
//! question — so its credential is the per-host bearer token the hub already
//! mints at provisioning, and the alias it registers under is the one that
//! token names. Nothing the client sends chooses it. A missing or unknown
//! token is the layer's own `401`; the master token and a paired client's
//! token both get `403`, because neither names a host and everything the hub
//! sends down this socket is a command to run. A per-host token whose mode is
//! `readonly` gets `403` too ([`READONLY_HOST`]): an agent receives every
//! command the hub runs on its host, which is more than "readonly" promises.
//!
//! What the handler owns after the upgrade:
//!
//! ```text
//! socket ──split──┬── write task ── drains AgentRegistry's outbound channel
//!                 └── read  loop ── decodes, AgentRegistry::deliver, heartbeat
//! ```
//!
//! **Two tasks, not one loop.** A single task that read and wrote in turn would
//! deadlock against an agent built the same way the moment both sides had a
//! large frame to send: each blocked writing into a socket the other has
//! stopped reading. The registry's channel is what decouples them.
//!
//! **The inbound budget is per request, not the global ceiling** — but read
//! what that buys before relying on it. A frame may legitimately be
//! [`MAX_FRAME_BYTES`] (~267 MiB, because the transport has to carry a
//! 200 MiB transcript), and every frame is decoded against the largest budget
//! among the requests in flight ([`decode_agent_frame_within`]), refused if it
//! is over, and the connection closed. See `Budgets`. **In practice that
//! budget is usually the whole ceiling**: `AgentTransport::run` and
//! `run_bounded` send no `cap_bytes` (only the transcript read needs the
//! ceiling, but nothing tells them apart), and any uncapped call in flight —
//! every reconcile tick has some — raises the allowance for every frame on the
//! connection, a `pong` included. The budget stops a peer from spending the
//! ceiling while only capped calls are in flight; it does not stop a connected
//! agent that waits for an uncapped one.
//!
//! **How many connections, and for how long.** A connection is only accepted
//! for a host on the agent transport, holding its current `full` token, and
//! re-checked against the store on every heartbeat. At most
//! [`MAX_CONNECTIONS_PER_HOST`] per host and [`MAX_CONNECTIONS`] in all, taken
//! before the upgrade: each one can hold a ceiling-sized buffer, and
//! tungstenite reserves the whole declared size as soon as it reads a frame's
//! header. A write is bounded by `SEND_TIMEOUT` and a close by
//! `CLOSE_TIMEOUT`, and a connection the registry lets go of is torn down
//! at once even while its writer is stuck mid-send, so a peer that stops
//! reading (or advertises a zero window and keeps acknowledging the probes)
//! cannot hold a socket or its buffer indefinitely.
//!
//! Two layers, and what each one stops:
//!
//! - **Over the ceiling:** refused from the frame HEADER, before any payload
//!   is buffered (`max_frame_size`, see `Limits::frame_cap`).
//! - **Under the ceiling, over the budget:** tungstenite has already buffered
//!   the frame by the time the budget can be applied — the socket's limits
//!   are fixed at the upgrade, and neither axum nor tokio-tungstenite exposes
//!   tungstenite's `set_config` to follow the budget per read. What the budget
//!   stops is everything after the buffer: the JSON parse and the base64
//!   decode, each another copy the size of the frame, and the connection
//!   staying up to do it again. So an agent holding a valid per-host token can
//!   still make the hub buffer up to one ceiling-sized frame per connection it
//!   opens; it cannot make the hub parse one it did not ask for.

use super::registry::{AgentHello, AgentRegistry, ConnId};
use crate::mcp::auth::TokenMode;
use crate::mcp::Caller;
use crate::store::Store;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use fleet_proto::{
    decode_agent_frame_within, encode_hub_frame, AgentFrame, HubFrame, MAX_FRAME_BYTES,
};
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

/// How often the hub pings an idle agent. The agent answers with a `pong`; any
/// frame at all counts as a sign of life.
pub const HEARTBEAT: Duration = fleet_proto::HEARTBEAT;

/// How many heartbeats in a row may pass with nothing heard from the agent
/// before its connection is dropped. Two, as the design says: with
/// [`HEARTBEAT`] at 30 s a wedged agent is off the registry within a minute,
/// and the next `exec` for that host reports `E_AGENT_OFFLINE` immediately
/// instead of waiting out a wall clock.
pub const MISSED_HEARTBEATS: u32 = 2;

/// What a server with no agent registry answers — the desktop, which builds
/// `SshClient::new()` and routes nothing.
pub const NOT_ENABLED: &str = "agent connections are not enabled on this server";

/// What a caller that is not a host is told.
pub const NOT_A_HOST: &str = "an agent connection needs a per-host bearer token";

/// What a host that is not on the agent transport is told.
pub const NOT_AN_AGENT_HOST: &str = "this host is not an agent host: set its transport to agent \
before connecting a fleet-agent for it";

/// What an upgrade over the connection limits is told.
pub const TOO_MANY: &str = "too many agent connections for this host";

/// Connections one host may hold open at once, before and after `hello`.
/// Two: a restarted agent dials while its old connection is still being
/// noticed gone. Each can buffer one frame of up to
/// [`MAX_FRAME_BYTES`] (~267 MiB) — tungstenite reserves the whole declared
/// size on reading a frame's header — so an unbounded count multiplied that.
pub const MAX_CONNECTIONS_PER_HOST: usize = 2;

/// Connections the whole hub holds open at once, for every host together.
pub const MAX_CONNECTIONS: usize = 64;

/// How long one frame may take to write before the connection is given up
/// on. Long enough for a ~267 MiB frame on a slow link; what it bounds is a
/// peer that stops reading (or advertises a zero window and keeps
/// acknowledging the probes), which otherwise held the writer forever.
const SEND_TIMEOUT: Duration = Duration::from_secs(300);

/// How long closing the socket may take.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// What a host whose token is `readonly` is told.
pub const READONLY_HOST: &str = "this host's token is readonly, and an agent receives every \
command the hub runs on its host, secret-file uploads included; mint a full token for the \
host (set its token mode to full) and restart the agent";

/// The floor under [`Budgets::allowance`]: enough for a `hello`, a `pong`, an
/// `upload`'s empty `result`, and a failed one's `stderr`. It is the same
/// 64 KiB of envelope `fleet_proto::MAX_FRAME_BYTES` leaves around one payload.
const MIN_INBOUND_BYTES: usize = 64 * 1024;

/// Extra time a request's budget outlives its own wall clock. The hub has
/// stopped waiting by then; this only stops a slightly-late answer being
/// judged against a budget that has already been reclaimed.
const LATE_ANSWER_GRACE: Duration = Duration::from_secs(5);

/// What `GET /agent` needs: somewhere to register connections, and the two
/// things the tests vary.
#[derive(Clone)]
pub struct AgentWsState {
    /// `None` on a server that routes nothing; the route then answers
    /// [`NOT_ENABLED`] rather than upgrading a socket nothing would read.
    registry: Option<Arc<AgentRegistry>>,
    /// Where each live connection's token is re-checked, on every beat: the
    /// upgrade is the only other place it is looked at.
    store: Option<Arc<Mutex<Store>>>,
    slots: Arc<Slots>,
    limits: Limits,
}

/// Open connections per host and in total, each held by a [`Slot`] for the
/// life of its `serve`.
pub(crate) struct Slots {
    held: Mutex<HashMap<String, usize>>,
    per_host: usize,
    total: usize,
}

impl Default for Slots {
    fn default() -> Self {
        Self {
            held: Mutex::default(),
            per_host: MAX_CONNECTIONS_PER_HOST,
            total: MAX_CONNECTIONS,
        }
    }
}

impl Slots {
    /// A slot for `alias`, or `None` over either limit.
    fn take(self: &Arc<Self>, alias: &str) -> Option<Slot> {
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        let total: usize = held.values().sum();
        if total >= self.total {
            return None;
        }
        let mine = held.entry(alias.to_string()).or_default();
        if *mine >= self.per_host {
            return None;
        }
        *mine += 1;
        Some(Slot {
            slots: Arc::clone(self),
            alias: alias.to_string(),
        })
    }

    #[cfg(test)]
    pub(crate) fn held(&self, alias: &str) -> usize {
        let held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        held.get(alias).copied().unwrap_or(0)
    }
}

/// One held connection; dropping it gives the slot back.
struct Slot {
    slots: Arc<Slots>,
    alias: String,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut held = self.slots.held.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = held.get_mut(&self.alias) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                held.remove(&self.alias);
            }
        }
    }
}

/// The per-connection settings, cloned into each connection's tasks.
#[derive(Clone)]
struct Limits {
    beats: BeatSource,
    /// The hard protocol ceiling, set on the socket as BOTH
    /// `max_frame_size` and `max_message_size`. tungstenite checks a frame's
    /// declared length against the first as soon as it has the header, so a
    /// frame over the ceiling is refused before a byte of its payload is
    /// buffered. The frame limit has to be raised explicitly: its default is
    /// 16 MiB, a peer writes a whole message as one frame, and the ceiling
    /// exists to admit a ~267 MiB `result`.
    frame_cap: usize,
}

/// Where a connection's heartbeats come from.
///
/// Missed heartbeats are COUNTED, not timed: a connection is dropped when
/// [`MISSED_HEARTBEATS`] beats in a row pass with nothing heard. So the tests
/// can fire the beats themselves and never wait on a clock, while the
/// production path is a plain [`tokio::time::Interval`].
#[derive(Clone)]
enum BeatSource {
    Every(Duration),
    /// Each `send_modify` on the sender is one beat for every connection.
    #[cfg(test)]
    Manual(Arc<tokio::sync::watch::Sender<u64>>),
}

impl BeatSource {
    /// A ticker whose first beat is one interval from now.
    fn ticker(&self) -> Ticker {
        match self {
            BeatSource::Every(every) => Ticker::Every {
                next: Instant::now() + *every,
                every: *every,
            },
            #[cfg(test)]
            BeatSource::Manual(tx) => Ticker::Manual(tx.subscribe()),
        }
    }
}

/// A connection's heartbeat.
///
/// **Judged two ways, and the second is the one that matters under load.**
/// `tick` waits for the next beat, for a loop that is idle. `due` answers at
/// once, without waiting and without spending the task's cooperative budget,
/// whether a beat has come due, and the loops ask it at the top of EVERY
/// iteration. A `select!` alone decides a beat by which arm wins, and a peer
/// whose frames never run out made the socket win every time: the hello
/// deadline never fired, and neither did the token re-check that cuts off a
/// rotated connection (the re-review's NEW-2).
enum Ticker {
    Every {
        next: Instant,
        every: Duration,
    },
    #[cfg(test)]
    Manual(tokio::sync::watch::Receiver<u64>),
}

impl Ticker {
    async fn tick(&mut self) {
        match self {
            Ticker::Every { next, every } => {
                tokio::time::sleep_until(*next).await;
                // A beat missed while a large frame was being decoded is
                // caught up on the next one, not replayed in a tight loop.
                *next = Instant::now() + *every;
            }
            #[cfg(test)]
            Ticker::Manual(rx) => {
                if rx.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
        }
    }

    /// Has a beat come due? Consumes it if so. Never waits.
    fn due(&mut self) -> bool {
        match self {
            Ticker::Every { next, every } => {
                let now = Instant::now();
                if now < *next {
                    return false;
                }
                *next = now + *every;
                true
            }
            #[cfg(test)]
            Ticker::Manual(rx) => {
                if !rx.has_changed().unwrap_or(false) {
                    return false;
                }
                rx.borrow_and_update();
                true
            }
        }
    }
}

impl AgentWsState {
    /// A route that registers connections on `registry` and keeps checking
    /// their tokens against `store`, or refuses them all when there is none.
    pub fn new(enabled: Option<(Arc<AgentRegistry>, Arc<Mutex<Store>>)>) -> Self {
        let (registry, store) = match enabled {
            Some((r, s)) => (Some(r), Some(s)),
            None => (None, None),
        };
        Self {
            registry,
            store,
            slots: Arc::default(),
            limits: Limits {
                beats: BeatSource::Every(HEARTBEAT),
                frame_cap: MAX_FRAME_BYTES,
            },
        }
    }

    /// A route with no registry: every upgrade gets [`NOT_ENABLED`].
    pub fn disabled() -> Self {
        Self::new(None)
    }

    /// The connection counts, for a test to watch a connection end.
    #[cfg(test)]
    pub(crate) fn slots(&self) -> Arc<Slots> {
        Arc::clone(&self.slots)
    }

    /// Smaller connection limits, so a test can reach the hub-wide one
    /// without dialling 64 sockets.
    #[cfg(test)]
    pub(crate) fn with_connection_limits(mut self, per_host: usize, total: usize) -> Self {
        self.slots = Arc::new(Slots {
            held: Mutex::default(),
            per_host,
            total,
        });
        self
    }

    /// Heartbeats fired by the test through `beats`, instead of by a clock.
    #[cfg(test)]
    pub(crate) fn with_manual_beats(mut self, beats: Arc<tokio::sync::watch::Sender<u64>>) -> Self {
        self.limits.beats = BeatSource::Manual(beats);
        self
    }

    /// A smaller protocol ceiling, so a test can drive the "too large to
    /// buffer" path without a 267 MiB allocation.
    #[cfg(test)]
    pub(crate) fn with_frame_cap(mut self, cap: usize) -> Self {
        self.limits.frame_cap = cap;
        self
    }
}

/// `GET /agent`.
pub(crate) async fn handle_agent(
    State(state): State<AgentWsState>,
    Extension(caller): Extension<Caller>,
    headers: axum::http::HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let (Some(registry), Some(store)) = (state.registry.clone(), state.store.clone()) else {
        return (StatusCode::SERVICE_UNAVAILABLE, NOT_ENABLED).into_response();
    };
    // The alias comes from the TOKEN, never from the request: an agent cannot
    // choose which host it is. The master token and a paired client's token
    // both carry no alias, and `Caller` has exactly these three shapes.
    let Some(alias) = caller.host_alias.clone() else {
        tracing::warn!(
            caller = %caller.label(),
            "[agent] refused an upgrade: not a per-host token"
        );
        return (StatusCode::FORBIDDEN, NOT_A_HOST).into_response();
    };
    // A readonly token may observe the fleet; an agent is handed every
    // command the hub runs on its host, secret-file uploads included.
    if caller.mode == TokenMode::Readonly {
        tracing::warn!(host = %alias, "[agent] refused an upgrade: readonly host token");
        return (StatusCode::FORBIDDEN, READONLY_HOST).into_response();
    }
    // What this connection authenticated with, kept so a later rotation,
    // narrowing or removal of the host's token cuts it off. The `authorize`
    // layer already accepted this exact header, so it is present.
    let Some(token) =
        crate::mcp::auth::bearer_token(headers.get(axum::http::header::AUTHORIZATION))
    else {
        return (StatusCode::FORBIDDEN, NOT_A_HOST).into_response();
    };
    // Every provisioned host holds a token for its hooks, SSH hosts
    // included; only an agent host's may become an agent.
    if !is_agent_host(&store, &alias) {
        tracing::warn!(host = %alias, "[agent] refused an upgrade: not an agent host");
        return (StatusCode::FORBIDDEN, NOT_AN_AGENT_HOST).into_response();
    }
    let Some(slot) = state.slots.take(&alias) else {
        tracing::warn!(host = %alias, "[agent] refused an upgrade: too many connections");
        return (StatusCode::TOO_MANY_REQUESTS, TOO_MANY).into_response();
    };
    let session = Session {
        registry,
        store,
        alias,
        credential: crate::mcp::auth::sha256_hex(token),
        _slot: slot,
    };
    let limits = state.limits;
    // Started here, before the `101` goes out, so the hello deadline counts
    // from the upgrade and no beat after it can be missed.
    let hello_deadline = limits.beats.ticker();
    ws.max_frame_size(limits.frame_cap)
        .max_message_size(limits.frame_cap)
        .on_upgrade(move |socket| serve(socket, session, limits, hello_deadline))
}

/// Is `alias` a host on the agent transport?
fn is_agent_host(store: &Mutex<Store>, alias: &str) -> bool {
    store
        .lock()
        .ok()
        .and_then(|s| s.agent_host_alias(alias).ok().flatten())
        .as_deref()
        == Some(alias)
}

/// Who a connection is, and what it must keep proving.
struct Session {
    registry: Arc<AgentRegistry>,
    store: Arc<Mutex<Store>>,
    alias: String,
    /// SHA-256 of the host token the upgrade presented.
    credential: String,
    /// Held until the connection ends.
    _slot: Slot,
}

/// One connection, from the upgrade to the deregistration.
async fn serve(socket: WebSocket, session: Session, limits: Limits, hello_deadline: Ticker) {
    let Session {
        registry,
        store,
        alias,
        credential,
        _slot,
    } = session;
    let (mut sink, mut stream) = socket.split();
    let hello = match first_hello(&mut stream, hello_deadline).await {
        Ok(h) => h,
        Err(why) => {
            tracing::warn!(host = %alias, why, "[agent] closing before registration");
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, sink.close()).await;
            return;
        }
    };
    // Two channels into the writer. The registry holds the ONLY sender of
    // the first, so the writer sees it close exactly when the registry lets
    // go of this connection — deregistered, or replaced by a second
    // connection for the alias. Pings travel on their own channel so that the
    // read loop holding a sender cannot keep a replaced connection alive, and
    // so that they reach THIS socket: `registry.send` would reach whichever
    // connection is live.
    let (tx, rx) = mpsc::unbounded_channel();
    let (ping_tx, ping_rx) = mpsc::unbounded_channel();
    // Subscribed BEFORE the registration is visible, so no beat after it is
    // missed.
    let ticker = limits.beats.ticker();
    // The token is judged again at the hello, immediately before the
    // registration and once more right after it. The upgrade may be a
    // heartbeat old by now: a connection upgraded before a rotation and
    // saying hello after it must not register, because registering REPLACES
    // the live connection — the agent the operator just reinstalled on the
    // new token (the re-review's NEW-1). The second check catches a rotation
    // that commits between the first and the registration.
    let stale = || !super::router::credential_is_current(&store, &alias, &credential);
    if stale() {
        tracing::warn!(host = %alias, "[agent] refused a hello: its token is no longer current");
        let _ = tokio::time::timeout(CLOSE_TIMEOUT, sink.close()).await;
        return;
    }
    let conn_id = registry.connect_bound(&alias, hello, tx, credential.clone());
    if stale() {
        tracing::warn!(host = %alias, conn = conn_id, "[agent] its token changed as it registered; dropping");
        registry.disconnect(&alias, conn_id);
        let _ = tokio::time::timeout(CLOSE_TIMEOUT, sink.close()).await;
        return;
    }
    // Fires when this stops being the live connection, so the socket can be
    // torn down even while the writer is stuck in a send.
    let gone = registry.gone(&alias, conn_id);
    tracing::info!(host = %alias, conn = conn_id, "[agent] connected");

    let budgets = Arc::new(Mutex::new(Budgets::default()));
    let mut writer = crate::rt::spawn(write_loop(
        sink,
        rx,
        ping_rx,
        Arc::clone(&budgets),
        alias.clone(),
        conn_id,
    ));
    let who = Registered {
        registry: &registry,
        store: &store,
        alias: &alias,
        conn_id,
        credential: &credential,
    };
    // Whichever ends first ends the connection: the reader when the agent
    // goes (or goes quiet), the writer when the registry lets go of it or a
    // write times out, and `gone` when the registry lets go of it while the
    // writer is stuck mid-send and cannot notice.
    let end = tokio::select! {
        () = read_loop(&mut stream, who, &budgets, &ping_tx, ticker) => End::Reader,
        _ = &mut writer => End::Writer,
        () = gone.cancelled() => End::Gone,
    };

    // Deregister first: it drops the registry's half of the channel, which is
    // what tells the writer to close the socket. A connection that was already
    // replaced no longer owns the entry, and this does nothing to the
    // replacement (`AgentRegistry::disconnect` checks the generation).
    registry.disconnect(&alias, conn_id);
    drop(ping_tx);
    match end {
        End::Writer => {}
        // The writer may be blocked in a send nobody will ever finish:
        // dropping it, with the read half below, closes the socket.
        End::Gone => writer.abort(),
        // The writer closes the socket itself, within its own timeouts.
        End::Reader => {
            if tokio::time::timeout(CLOSE_TIMEOUT, &mut writer)
                .await
                .is_err()
            {
                writer.abort();
            }
        }
    }
    tracing::info!(host = %alias, conn = conn_id, "[agent] disconnected");
}

/// What ended a connection's `serve`.
enum End {
    Reader,
    Writer,
    Gone,
}

/// Read frames until the agent identifies itself.
///
/// Bounded by one heartbeat: an upgrade that never says hello would otherwise
/// hold a task and a socket for as long as the peer cared to keep the TCP
/// connection open, without ever appearing in the registry where an operator
/// could see it.
async fn first_hello(
    stream: &mut (impl futures_util::Stream<Item = Result<Message, axum::Error>> + Unpin),
    mut deadline: Ticker,
) -> Result<AgentHello, String> {
    const LATE: &str = "no hello within one heartbeat";
    loop {
        // Judged before every read, not only when the deadline wins the
        // select: see `Ticker`.
        if deadline.due() {
            return Err(LATE.into());
        }
        let msg = tokio::select! {
            biased;
            msg = stream.next() => msg,
            () = deadline.tick() => return Err(LATE.into()),
        };
        match msg {
            // The `hello` carries three short strings; nothing about it needs
            // more than the floor, whatever the connection's ceiling is.
            Some(Ok(Message::Text(text))) => {
                return match decode_agent_frame_within(&text, MIN_INBOUND_BYTES) {
                    Ok(AgentFrame::Hello {
                        agent_version,
                        host_name,
                        os,
                    }) => Ok(AgentHello {
                        agent_version,
                        host_name,
                        os,
                    }),
                    Ok(other) => Err(format!("the first frame must be hello, got {other:?}")),
                    Err(e) => Err(format!("the first frame did not decode: {e}")),
                }
            }
            // tungstenite answers these itself; they are not the protocol's.
            Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => continue,
            Some(Ok(Message::Binary(_))) => {
                return Err("the protocol is text frames; got binary".into())
            }
            Some(Ok(Message::Close(_))) | None => return Err("closed before hello".into()),
            Some(Err(e)) => return Err(format!("socket error before hello: {e}")),
        }
    }
}

/// Drain the registry's outbound channel, and the read loop's pings, into the
/// socket.
///
/// It ends when the registry's channel closes — which is what deregistering,
/// or being replaced by a second connection for the same alias, does to it.
async fn write_loop(
    mut sink: SplitSink<WebSocket, Message>,
    mut rx: mpsc::UnboundedReceiver<HubFrame>,
    mut pings: mpsc::UnboundedReceiver<HubFrame>,
    budgets: Arc<Mutex<Budgets>>,
    alias: String,
    conn_id: ConnId,
) {
    loop {
        let frame = tokio::select! {
            frame = rx.recv() => match frame {
                Some(frame) => frame,
                None => break,
            },
            // Disabled once the read loop has dropped its sender.
            Some(ping) = pings.recv() => ping,
        };
        // Armed BEFORE the frame goes out, so the answer can never beat its
        // own budget onto the socket.
        if let Some((id, budget, expires)) = answer_budget(&frame) {
            budgets
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .arm(id, budget, expires);
        }
        match encode_hub_frame(&frame) {
            Ok(text) => {
                // Bounded: a peer that stops reading must not hold this task,
                // and its buffers, for ever.
                match tokio::time::timeout(SEND_TIMEOUT, sink.send(Message::Text(text.into())))
                    .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => break,
                    Err(_) => {
                        tracing::warn!(
                            host = %alias, conn = conn_id,
                            "[agent] a write took over {}s; dropping", SEND_TIMEOUT.as_secs()
                        );
                        return;
                    }
                }
            }
            // Only reachable for a frame past MAX_FRAME_BYTES, which
            // `AgentTransport::upload_file` already refuses by the file's size.
            // Dropping it leaves its caller to time out, which is why this is
            // an error and not a debug line.
            Err(e) => tracing::error!(
                host = %alias, conn = conn_id, error = %e,
                "[agent] could not encode a frame; its caller will time out"
            ),
        }
    }
    let _ = tokio::time::timeout(CLOSE_TIMEOUT, sink.close()).await;
}

/// A registered connection, as the read loop needs to know it.
struct Registered<'a> {
    registry: &'a AgentRegistry,
    store: &'a Mutex<Store>,
    alias: &'a str,
    conn_id: ConnId,
    /// SHA-256 of the token it authenticated with.
    credential: &'a str,
}

/// Read frames and hand them to the registry, pinging an idle agent and
/// dropping one that has gone quiet.
async fn read_loop(
    stream: &mut (impl futures_util::Stream<Item = Result<Message, axum::Error>> + Unpin),
    who: Registered<'_>,
    budgets: &Mutex<Budgets>,
    pings: &mpsc::UnboundedSender<HubFrame>,
    mut ticker: Ticker,
) {
    let Registered {
        registry,
        store,
        alias,
        conn_id,
        credential,
    } = who;
    // Beats in a row with nothing heard. The `hello` does not count: silence
    // is measured from the registration.
    let mut missed = 0;
    let mut heard = false;
    loop {
        // A beat that has come due is taken first, before any read, however
        // many frames are waiting: see `Ticker`. Otherwise wait for either.
        // The futures are dropped at the end of this `let`, which is what
        // releases `stream` for the branches below. `biased`: a frame that is
        // already waiting is read before a beat is judged, so an answer that
        // raced its beat still counts.
        let step = if ticker.due() {
            Step::Beat
        } else {
            tokio::select! {
                biased;
                msg = stream.next() => Step::Incoming(msg),
                () = ticker.tick() => Step::Beat,
            }
        };
        match step {
            Step::Beat => {
                // The upgrade is the only other place the token is looked at.
                // A rotation, a `readonly` or a removal since then ends the
                // connection here, within one beat, even if nothing is ever
                // routed to it again.
                if !super::router::credential_is_current(store, alias, credential) {
                    tracing::warn!(
                        host = %alias, conn = conn_id,
                        "[agent] its token was rotated, narrowed or removed; dropping"
                    );
                    return;
                }
                missed = if std::mem::take(&mut heard) {
                    0
                } else {
                    missed + 1
                };
                if missed >= MISSED_HEARTBEATS {
                    tracing::warn!(
                        host = %alias, conn = conn_id,
                        "[agent] silent for {MISSED_HEARTBEATS} heartbeats; dropping"
                    );
                    return;
                }
                if pings
                    .send(HubFrame::Ping {
                        id: uuid::Uuid::new_v4().to_string(),
                    })
                    .is_err()
                {
                    // The writer is gone, so the socket is too.
                    return;
                }
            }
            Step::Incoming(None) => return,
            Step::Incoming(Some(Err(e))) => {
                // Includes tungstenite refusing a frame over the protocol
                // ceiling — refused from its header, so nothing that size was
                // ever buffered here.
                tracing::warn!(host = %alias, conn = conn_id, error = %e, "[agent] socket error");
                return;
            }
            Step::Incoming(Some(Ok(msg))) => {
                // Any frame at all is a sign of life, decodable or not.
                heard = true;
                match msg {
                    Message::Text(text) => {
                        let allowance = budgets
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .allowance();
                        match decode_agent_frame_within(&text, allowance) {
                            Ok(frame) => {
                                if let Some(id) = answered_id(&frame) {
                                    budgets.lock().unwrap_or_else(|e| e.into_inner()).done(id);
                                }
                                registry.deliver(alias, conn_id, frame);
                            }
                            Err(e) => {
                                // The design's rule: a frame the far side
                                // cannot parse closes the connection with a
                                // reason and the agent reconnects.
                                tracing::warn!(
                                    host = %alias, conn = conn_id, error = %e,
                                    "[agent] refusing a frame; closing"
                                );
                                return;
                            }
                        }
                    }
                    Message::Close(_) => return,
                    Message::Binary(_) => {
                        tracing::warn!(
                            host = %alias, conn = conn_id,
                            "[agent] binary frame on a text protocol; closing"
                        );
                        return;
                    }
                    Message::Ping(_) | Message::Pong(_) => {}
                }
            }
        }
    }
}

/// What the read loop woke up for.
enum Step {
    Beat,
    Incoming(Option<Result<Message, axum::Error>>),
}

/// The largest answer each in-flight request could legitimately carry, and
/// when the hub stops waiting for it.
///
/// This is the whole of the "decode with the cap you asked for" rule. The
/// allowance is the largest live entry rather than a per-frame lookup, because
/// a frame's size has to be judged *before* it is parsed, and its `id` is
/// inside it: the most an agent can spend is therefore the most any one
/// outstanding request could honestly need.
#[derive(Default)]
struct Budgets {
    armed: HashMap<String, (usize, Instant)>,
}

impl Budgets {
    fn arm(&mut self, id: String, budget: usize, expires: Instant) {
        self.armed.insert(id, (budget, expires));
    }

    fn done(&mut self, id: &str) {
        self.armed.remove(id);
    }

    /// The ceiling for the next inbound frame. Expired entries are dropped
    /// here rather than on a timer: without that, one uncapped `exec` would
    /// leave the connection at the full ceiling for as long as it stayed open.
    fn allowance(&mut self) -> usize {
        let now = Instant::now();
        self.armed.retain(|_, (_, expires)| *expires > now);
        self.armed
            .values()
            .map(|(budget, _)| *budget)
            .max()
            .unwrap_or(0)
            .max(MIN_INBOUND_BYTES)
    }
}

/// The budget a hub frame's answer earns, or `None` when the answer fits in
/// [`MIN_INBOUND_BYTES`] — an `upload`'s `result`, a `pong` — or when there is
/// no answer at all (`cancel`).
fn answer_budget(frame: &HubFrame) -> Option<(String, usize, Instant)> {
    match frame {
        HubFrame::Exec {
            id,
            timeout_ms,
            cap_bytes,
            ..
        } => Some((
            id.clone(),
            exec_budget(*cap_bytes),
            Instant::now() + Duration::from_millis(*timeout_ms) + LATE_ANSWER_GRACE,
        )),
        HubFrame::Upload { .. } | HubFrame::Cancel { .. } | HubFrame::Ping { .. } => None,
    }
}

/// What an `exec`'s `result` may cost. The number lives in `fleet-proto`
/// beside the stream limits the agent truncates to, so the two ends cannot
/// drift apart: [`fleet_proto::result_budget`].
fn exec_budget(cap_bytes: Option<u64>) -> usize {
    fleet_proto::result_budget(cap_bytes)
}

/// The request an agent frame answers — `None` for `hello`, which answers
/// nothing and therefore releases no budget.
fn answered_id(frame: &AgentFrame) -> Option<&str> {
    match frame {
        AgentFrame::Result { id, .. } | AgentFrame::Pong { id } => Some(id),
        AgentFrame::Hello { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::registry::AgentRegistry;
    use crate::store::Store;
    use fleet_proto::{encode_agent_frame, encode_b64, AgentFrame, HubFrame, MAX_FRAME_BYTES};
    use futures_util::{SinkExt, StreamExt};
    use std::net::{Ipv4Addr, SocketAddr};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::time::Instant;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;
    use tokio_tungstenite::WebSocketStream;

    // No socket test here waits on a clock. The heartbeats are fired by the
    // test (`Hub::beat`), every wait is on an observable condition, and the
    // real-clock bounds below are only how long to wait before calling a
    // test FAILED. Holding the real clock destabilised unrelated
    // `start_paused` tests in this binary before (the Task 3 flake in
    // progress.md). The paused clock is no answer over a real socket either:
    // tokio advances it whenever loopback readiness is not visible for an
    // instant, which fired 10 s wall clocks on answers still in flight.
    // `Ticker`'s production path is tested on its own, with no socket.

    /// How long a test waits for something before it fails.
    const PATIENCE: Duration = Duration::from_secs(5);

    const MASTER: &str = "master-token";
    const LAPTOP_TOKEN: &str = "laptop-host-token";
    const DESK_TOKEN: &str = "desk-host-token";
    const MEFISTOS_TOKEN: &str = "mefistos-ssh-host-token";
    const PHONE_TOKEN: &str = "phone-client-token";

    type Client = WebSocketStream<tokio::net::TcpStream>;

    /// A hub serving the REAL app: the real `authorize` layer, the real
    /// `/agent` route, over a real loopback socket.
    struct Hub {
        addr: SocketAddr,
        store: Arc<Mutex<Store>>,
        registry: Arc<AgentRegistry>,
        beats: Arc<tokio::sync::watch::Sender<u64>>,
        slots: Arc<Slots>,
    }

    impl Hub {
        /// One heartbeat, for every connection.
        fn beat(&self) {
            self.beats.send_modify(|n| *n += 1);
        }
    }

    async fn hub_with(tune: impl FnOnce(AgentWsState) -> AgentWsState) -> Hub {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        {
            let s = store.lock().unwrap();
            s.upsert_host("laptop").unwrap();
            s.set_host_transport("laptop", "agent").unwrap();
            s.upsert_host_token("laptop", LAPTOP_TOKEN).unwrap();
            s.upsert_host("desk").unwrap();
            s.set_host_transport("desk", "agent").unwrap();
            s.upsert_host_token("desk", DESK_TOKEN).unwrap();
            // An SSH host with a token, as every provisioned host has one.
            s.upsert_host("mefistos").unwrap();
            s.upsert_host_token("mefistos", MEFISTOS_TOKEN).unwrap();
            s.insert_client_token("phone", &crate::mcp::auth::sha256_hex(PHONE_TOKEN), "full")
                .unwrap();
        }
        let registry = AgentRegistry::new();
        let state = AgentWsState::new(Some((Arc::clone(&registry), Arc::clone(&store))));
        serve(store, tune(state), registry).await
    }

    async fn hub() -> Hub {
        hub_with(|s| s).await
    }

    async fn serve(
        store: Arc<Mutex<Store>>,
        state: AgentWsState,
        registry: Arc<AgentRegistry>,
    ) -> Hub {
        let beats = Arc::new(tokio::sync::watch::Sender::new(0));
        let state = state.with_manual_beats(Arc::clone(&beats));
        let slots = state.slots();
        let app = crate::mcp::test_app(Arc::clone(&store), MASTER, state);
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        // The listener is bound before the task is spawned, so a connect that
        // races the spawn queues in the backlog.
        crate::rt::spawn(async move {
            let _ = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await;
        });
        Hub {
            addr,
            store,
            registry,
            beats,
            slots,
        }
    }

    /// Dial `/agent`. `Err(status)` is what the HTTP layer answered instead of
    /// upgrading.
    async fn dial(addr: SocketAddr, token: Option<&str>) -> Result<Client, u16> {
        let mut req = format!("ws://{addr}/agent").into_client_request().unwrap();
        if let Some(t) = token {
            req.headers_mut()
                .insert("authorization", format!("Bearer {t}").parse().unwrap());
        }
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        // Small frames back to back (a pong, then a barrier) would otherwise
        // wait out Nagle against a delayed ACK on every round.
        tcp.set_nodelay(true).unwrap();
        // No client-side size limits: the tests below send frames the HUB must
        // refuse, so the client must not refuse them first.
        let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(None)
            .max_frame_size(None);
        match tokio_tungstenite::client_async_with_config(req, tcp, Some(config)).await {
            Ok((ws, _)) => Ok(ws),
            Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => Err(resp.status().as_u16()),
            Err(e) => panic!("unexpected dial failure: {e}"),
        }
    }

    /// Dial `/agent` expecting a refusal: its status and its body.
    async fn dial_refused(addr: SocketAddr, token: &str) -> (u16, String) {
        let mut req = format!("ws://{addr}/agent").into_client_request().unwrap();
        req.headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        match tokio_tungstenite::client_async(req, tcp).await {
            Ok(_) => panic!("the upgrade was accepted"),
            Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
                let body = resp.body().clone().unwrap_or_default();
                (
                    resp.status().as_u16(),
                    String::from_utf8(body).expect("a text body"),
                )
            }
            Err(e) => panic!("unexpected dial failure: {e}"),
        }
    }

    fn hello_as(version: &str) -> AgentFrame {
        AgentFrame::Hello {
            agent_version: version.into(),
            host_name: "laptop.local".into(),
            os: "linux".into(),
        }
    }

    async fn send(ws: &mut Client, frame: &AgentFrame) {
        let text = encode_agent_frame(frame).unwrap();
        ws.send(WsMessage::Text(text.into())).await.unwrap();
    }

    /// Dial, say hello, and wait until the hub has registered THIS connection
    /// — recognised by the version its hello carried, since `connected(alias)`
    /// is already true while an earlier connection for the alias is live.
    async fn connected_as(hub: &Hub, token: &str, alias: &str, version: &str) -> Client {
        let mut ws = dial(hub.addr, Some(token)).await.expect("the upgrade");
        send(&mut ws, &hello_as(version)).await;
        wait_until(&format!("{alias} is registered as {version}"), || {
            hub.registry
                .snapshot()
                .iter()
                .any(|s| s.alias == alias && s.agent_version == version)
        })
        .await;
        ws
    }

    async fn connected(hub: &Hub, token: &str, alias: &str) -> Client {
        connected_as(hub, token, alias, "1.2.3").await
    }

    /// The next hub frame on the socket, or `None` if the hub closed it.
    /// WebSocket-level control frames are skipped — they are tungstenite's,
    /// not the protocol's.
    async fn next_frame(ws: &mut Client) -> Option<HubFrame> {
        loop {
            match ws.next().await {
                Some(Ok(WsMessage::Text(t))) => {
                    return Some(fleet_proto::decode_hub_frame(&t).expect("a hub frame"))
                }
                Some(Ok(WsMessage::Ping(_))) | Some(Ok(WsMessage::Pong(_))) => continue,
                Some(Ok(WsMessage::Close(_))) | None => return None,
                Some(Ok(other)) => panic!("unexpected message {other:?}"),
                Some(Err(_)) => return None,
            }
        }
    }

    /// Wait until the hub's read loop has consumed everything sent before
    /// this call. A WebSocket-level ping is answered by the hub's socket only
    /// when its read loop asks for the NEXT message, which it does only after
    /// handling every frame ahead of the ping — so the echo is proof.
    async fn barrier(ws: &mut Client) {
        ws.send(WsMessage::Ping(b"barrier"[..].into()))
            .await
            .unwrap();
        loop {
            match tokio::time::timeout(PATIENCE, ws.next()).await {
                Ok(Some(Ok(WsMessage::Pong(p)))) if &p[..] == b"barrier" => return,
                Ok(Some(Ok(WsMessage::Pong(_)))) => continue,
                other => panic!("expected the barrier's echo, got {other:?}"),
            }
        }
    }

    /// Read until the hub closes the socket. `true` when it did before
    /// [`PATIENCE`] ran out; protocol frames on the way are ignored.
    async fn closed_by_hub(ws: &mut Client) -> bool {
        let deadline = Instant::now() + PATIENCE;
        loop {
            match tokio::time::timeout_at(deadline, ws.next()).await {
                Err(_) => return false,
                Ok(None) | Ok(Some(Ok(WsMessage::Close(_)))) | Ok(Some(Err(_))) => return true,
                Ok(Some(Ok(_))) => continue,
            }
        }
    }

    /// Spin on an observable condition, yielding rather than sleeping, so the
    /// hub's tasks run between checks without this test holding the clock.
    async fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if cond() {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            tokio::task::yield_now().await;
        }
    }

    fn exec(id: &str, cap_bytes: Option<u64>) -> HubFrame {
        HubFrame::Exec {
            id: id.into(),
            argv: vec!["bash".into(), "-c".into(), "true".into()],
            stdin: None,
            timeout_ms: 30_000,
            cap_bytes,
        }
    }

    fn result_of(id: &str, stdout: &[u8], stderr: &[u8]) -> AgentFrame {
        AgentFrame::Result {
            id: id.into(),
            exit_code: 0,
            stdout_b64: encode_b64(stdout),
            stderr_b64: encode_b64(stderr),
            truncated: false,
        }
    }

    /// Dispatch `frame` through the registry, the way `AgentTransport` does,
    /// and check it arrives on `ws` verbatim.
    async fn dispatch(
        hub: &Hub,
        ws: &mut Client,
        frame: HubFrame,
    ) -> tokio::task::JoinHandle<Result<AgentFrame, crate::ipc_error::IpcError>> {
        let reg = Arc::clone(&hub.registry);
        let sent = frame.clone();
        let call =
            crate::rt::spawn(
                async move { reg.request("laptop", sent, Duration::from_secs(10)).await },
            );
        assert_eq!(
            next_frame(ws).await,
            Some(frame),
            "the frame arrives verbatim"
        );
        call
    }

    // ── the upgrade is gated exactly like `/mcp` ────────────────────────────

    #[tokio::test]
    async fn an_upgrade_without_a_bearer_token_is_401() {
        let hub = hub().await;
        assert_eq!(dial(hub.addr, None).await.unwrap_err(), 401);
        assert_eq!(dial(hub.addr, Some("nonsense")).await.unwrap_err(), 401);
        assert!(hub.registry.snapshot().is_empty());
    }

    /// An agent IS a host: a paired phone's token authorizes `/mcp` and
    /// `/events`, and must not be able to register itself as some host's
    /// agent — it carries no alias, and everything the hub sends an agent is
    /// a command to run.
    #[tokio::test]
    async fn a_client_token_cannot_open_an_agent_connection() {
        let hub = hub().await;
        assert_eq!(dial(hub.addr, Some(PHONE_TOKEN)).await.unwrap_err(), 403);
        assert!(hub.registry.snapshot().is_empty());
    }

    /// The master token is not a host either, so it names no alias to
    /// register under.
    #[tokio::test]
    async fn the_master_token_cannot_open_an_agent_connection() {
        let hub = hub().await;
        assert_eq!(dial(hub.addr, Some(MASTER)).await.unwrap_err(), 403);
        assert!(hub.registry.snapshot().is_empty());
    }

    /// A READONLY per-host token does name a host, but registering as its
    /// agent means receiving every command the hub runs there, secret-file
    /// uploads included: strictly more than "readonly" promises. Refused, and
    /// the body says why and what to do instead.
    #[tokio::test]
    async fn a_readonly_host_token_cannot_open_an_agent_connection() {
        let hub = hub().await;
        // `laptop`'s token row already exists (see `hub_with`); flip it.
        // The layer reads the mode per request, so no restart is needed.
        let store = Arc::clone(&hub.store);
        store
            .lock()
            .unwrap()
            .set_host_token_mode("laptop", "readonly")
            .unwrap();

        let (status, body) = dial_refused(hub.addr, LAPTOP_TOKEN).await;
        assert_eq!(status, 403);
        assert_eq!(body, READONLY_HOST, "the body names the reason and the fix");
        assert!(hub.registry.snapshot().is_empty());

        // The same host with a full token registers, so it is the MODE that
        // was refused, not the host.
        store
            .lock()
            .unwrap()
            .set_host_token_mode("laptop", "full")
            .unwrap();
        let _ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;
    }

    // ── a token that changes under a live connection ─────────────────────
    //
    // The token is checked at the upgrade; these are the changes that come
    // after it. Each must cut the LIVE connection off, not only the next
    // dial: a rotation is how an operator answers a stolen token.

    /// Connect as laptop, apply `change` to the store, fire one beat, and
    /// expect the hub to close the socket and deregister it.
    async fn a_change_cuts_the_live_agent_off(change: impl FnOnce(&Store)) {
        let hub = hub().await;
        let mut ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;
        change(&hub.store.lock().unwrap());
        hub.beat();
        assert!(
            closed_by_hub(&mut ws).await,
            "the live connection outlived the change to its token"
        );
        wait_until("laptop leaves the registry", || {
            !hub.registry.connected("laptop")
        })
        .await;
    }

    #[tokio::test]
    async fn rotating_a_host_token_cuts_its_live_agent_off() {
        a_change_cuts_the_live_agent_off(|s| {
            s.upsert_host_token("laptop", "rotated-token").unwrap()
        })
        .await;
    }

    #[tokio::test]
    async fn a_host_token_set_readonly_cuts_its_live_agent_off() {
        a_change_cuts_the_live_agent_off(|s| s.set_host_token_mode("laptop", "readonly").unwrap())
            .await;
    }

    #[tokio::test]
    async fn removing_the_host_cuts_its_live_agent_off() {
        a_change_cuts_the_live_agent_off(|s| s.delete_host("laptop").unwrap()).await;
    }

    /// Between beats the check that holds is the router's: the first call
    /// routed to a host whose live agent's token was just rotated closes
    /// that socket WITHOUT sending it anything — the path by which a
    /// rotation used to hand its new token to the connection it revokes.
    #[tokio::test]
    async fn a_call_after_a_rotation_never_reaches_the_live_socket() {
        let hub = hub().await;
        hub.store
            .lock()
            .unwrap()
            .set_host_transport("laptop", "agent")
            .unwrap();
        let ssh =
            crate::ssh::SshClient::with_agents(Arc::clone(&hub.registry), Arc::clone(&hub.store));
        let mut ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;
        hub.store
            .lock()
            .unwrap()
            .upsert_host_token("laptop", "rotated-token")
            .unwrap();

        let err = crate::ssh::SshExec::run(&ssh, "laptop", &["echo", "secret"], PATIENCE)
            .await
            .unwrap_err();
        assert_eq!(err.code, crate::ipc_error::codes::E_AGENT_OFFLINE);
        assert_eq!(
            next_frame(&mut ws).await,
            None,
            "the socket closes with nothing sent down it"
        );
    }

    // ── bounded: which tokens, how many connections, how long a write ─────

    /// Every provisioned host has a token for its hooks, SSH hosts included;
    /// only an agent host's may become an agent.
    #[tokio::test]
    async fn a_host_on_the_ssh_transport_cannot_connect_an_agent() {
        let hub = hub().await;
        let (status, body) = dial_refused(hub.addr, MEFISTOS_TOKEN).await;
        assert_eq!(status, 403);
        assert_eq!(body, NOT_AN_AGENT_HOST);
        assert!(hub.registry.snapshot().is_empty());
    }

    #[tokio::test]
    async fn moving_a_host_back_to_ssh_cuts_its_live_agent_off() {
        a_change_cuts_the_live_agent_off(|s| s.set_host_transport("laptop", "ssh").unwrap()).await;
    }

    /// Each connection may buffer one ceiling-sized frame, and one reserved
    /// by a 14-byte header costs that much, so the count is what bounds the
    /// hub's memory. Refused before the upgrade, and given back when a
    /// connection ends.
    #[tokio::test]
    async fn a_host_may_hold_only_a_few_connections_at_once() {
        let hub = hub().await;
        let first = dial(hub.addr, Some(LAPTOP_TOKEN)).await.expect("one");
        let _second = dial(hub.addr, Some(LAPTOP_TOKEN)).await.expect("two");
        let (status, body) = dial_refused(hub.addr, LAPTOP_TOKEN).await;
        assert_eq!((status, body.as_str()), (429, TOO_MANY));
        // Another host is not affected.
        let _desk = dial(hub.addr, Some(DESK_TOKEN)).await.expect("desk");

        drop(first);
        wait_until("the first connection's slot is back", || {
            hub.slots.held("laptop") == 1
        })
        .await;
        dial(hub.addr, Some(LAPTOP_TOKEN))
            .await
            .expect("a slot is free again");
    }

    /// The hub-wide limit holds across hosts, whatever each host holds.
    #[tokio::test]
    async fn the_hub_holds_only_so_many_agent_connections_in_all() {
        let hub = hub_with(|s| s.with_connection_limits(2, 3)).await;
        let _a = dial(hub.addr, Some(LAPTOP_TOKEN)).await.expect("one");
        let _b = dial(hub.addr, Some(LAPTOP_TOKEN)).await.expect("two");
        let _c = dial(hub.addr, Some(DESK_TOKEN)).await.expect("three");
        let (status, _) = dial_refused(hub.addr, DESK_TOKEN).await;
        assert_eq!(status, 429, "desk holds one, but the hub holds three");
    }

    /// A replaced connection is torn down even while its writer is stuck
    /// mid-send to a peer that stopped reading — the case that used to hold
    /// its socket, and its buffer, for as long as the peer liked.
    #[tokio::test]
    async fn a_replaced_connection_is_torn_down_even_while_its_write_is_stuck() {
        let hub = hub().await;
        let _old = connected_as(&hub, LAPTOP_TOKEN, "laptop", "old").await;
        // A frame far bigger than the loopback buffers, to a client that
        // never reads: the writer blocks in `send`.
        let big = HubFrame::Upload {
            id: "stuck".into(),
            path: "/tmp/x".into(),
            mode: 0o600,
            bytes_b64: "A".repeat(48 * 1024 * 1024),
        };
        let reg = Arc::clone(&hub.registry);
        let _call =
            crate::rt::spawn(async move { reg.request("laptop", big, PATIENCE * 10).await });
        let _new = connected_as(&hub, LAPTOP_TOKEN, "laptop", "new").await;
        wait_until("the replaced connection's session to end", || {
            hub.slots.held("laptop") == 1
        })
        .await;
    }

    /// The other side of the same check: a beat with the token unchanged
    /// keeps the connection, or the check would be cutting everyone off.
    #[tokio::test]
    async fn an_unchanged_token_keeps_its_connection_across_beats() {
        let hub = hub().await;
        let mut ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;
        for _ in 0..3 {
            hub.beat();
            match next_frame(&mut ws).await {
                Some(HubFrame::Ping { id }) => send(&mut ws, &AgentFrame::Pong { id }).await,
                other => panic!("expected a ping, got {other:?}"),
            }
            barrier(&mut ws).await;
        }
        assert!(hub.registry.connected("laptop"));
    }

    /// A server that routes nothing (the desktop builds `SshClient::new()`)
    /// has no registry to put a connection in, and says so.
    #[tokio::test]
    async fn a_server_with_no_agent_registry_refuses_the_upgrade() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        {
            let s = store.lock().unwrap();
            s.upsert_host("laptop").unwrap();
            s.upsert_host_token("laptop", LAPTOP_TOKEN).unwrap();
        }
        let hub = serve(store, AgentWsState::disabled(), AgentRegistry::new()).await;
        assert_eq!(dial(hub.addr, Some(LAPTOP_TOKEN)).await.unwrap_err(), 503);
    }

    // ── registration ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_per_host_token_upgrades_and_registers_under_its_own_alias() {
        let hub = hub().await;
        let _ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;

        let snap = hub.registry.snapshot();
        assert_eq!(snap.len(), 1, "one row: {snap:?}");
        assert_eq!(snap[0].alias, "laptop", "the alias comes from the TOKEN");
        assert_eq!(snap[0].agent_version, "1.2.3");
        assert_eq!(snap[0].host_name, "laptop.local");
        assert_eq!(snap[0].os, "linux");
        assert!(snap[0].connected_at > 0);
        // The token names the alias; a second host's token registers a second
        // alias and nothing about the first.
        let _other = connected(&hub, DESK_TOKEN, "desk").await;
        let aliases: Vec<_> = hub
            .registry
            .snapshot()
            .into_iter()
            .map(|s| s.alias)
            .collect();
        assert_eq!(aliases, ["desk", "laptop"]);
    }

    /// The whole point of the endpoint: a service-layer call dispatched
    /// through the registry travels the socket and its answer comes back.
    #[tokio::test]
    async fn a_request_reaches_the_socket_and_its_answer_comes_back() {
        let hub = hub().await;
        let mut ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;

        let call = dispatch(&hub, &mut ws, exec("req-1", None)).await;
        send(&mut ws, &result_of("req-1", b"hello from the agent", b"")).await;

        let answer = call.await.unwrap().expect("the caller is answered");
        assert_eq!(answer, result_of("req-1", b"hello from the agent", b""));
    }

    #[tokio::test]
    async fn closing_the_socket_deregisters_the_alias() {
        let hub = hub().await;
        let mut ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;
        ws.close(None).await.unwrap();
        wait_until("laptop to be deregistered", || {
            !hub.registry.connected("laptop")
        })
        .await;
        assert!(hub.registry.snapshot().is_empty());
    }

    /// A dropped TCP connection with no close handshake — an agent that
    /// crashed — deregisters too. No beat is fired here, so the heartbeat
    /// cannot be what noticed.
    #[tokio::test]
    async fn dropping_the_connection_deregisters_the_alias_without_a_heartbeat() {
        let hub = hub().await;
        let ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;
        drop(ws);
        wait_until("laptop to be deregistered", || {
            !hub.registry.connected("laptop")
        })
        .await;
    }

    /// A restarted agent dials in again before the hub has noticed the old
    /// socket is dead. The new connection must take over immediately, and the
    /// old one must be torn down at once — not left open until it goes quiet
    /// for two heartbeats, which a still-running old agent never does. No beat
    /// is fired in this test.
    #[tokio::test]
    async fn a_second_connection_for_the_same_alias_replaces_the_first() {
        let hub = hub().await;
        let mut first = connected_as(&hub, LAPTOP_TOKEN, "laptop", "1.0.0").await;
        let mut second = connected_as(&hub, LAPTOP_TOKEN, "laptop", "2.0.0").await;

        assert_eq!(hub.registry.snapshot().len(), 1, "one row per alias");
        assert!(
            closed_by_hub(&mut first).await,
            "the replaced socket must be closed, not left open"
        );

        // The live connection is the SECOND one: the request arrives there…
        let call = dispatch(&hub, &mut second, exec("r", None)).await;
        send(&mut second, &result_of("r", b"second", b"")).await;
        assert_eq!(
            call.await.unwrap().unwrap(),
            result_of("r", b"second", b""),
            "the replacement answered"
        );
        // …and the first one's teardown did not deregister its replacement.
        let snap = hub.registry.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].agent_version, "2.0.0");
    }

    /// The replaced agent may be wedged, not merely restarted: it never
    /// answers the hub's Close. The hub must still drop the TCP connection,
    /// not keep a reader waiting on a peer that will never finish the
    /// handshake. Read here as raw TCP, below tungstenite — which would
    /// otherwise answer the Close itself and let the hub off the hook.
    #[tokio::test]
    async fn a_replaced_connection_is_dropped_even_if_it_never_answers_the_close() {
        use tokio::io::AsyncReadExt;
        let hub = hub().await;
        let mut first = connected_as(&hub, LAPTOP_TOKEN, "laptop", "1.0.0").await;
        let _second = connected_as(&hub, LAPTOP_TOKEN, "laptop", "2.0.0").await;

        let tcp = first.get_mut();
        let mut buf = [0u8; 4096];
        let eof = tokio::time::timeout(PATIENCE, async {
            loop {
                match tcp.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(_) => continue,
                }
            }
        })
        .await;
        assert!(eof.is_ok(), "the hub kept the replaced TCP connection open");
    }

    // ── heartbeats ─────────────────────────────────────────────────────────

    /// The hub pings on a beat, and drops a connection that has said nothing
    /// for two beats in a row — and not after the first.
    #[tokio::test]
    async fn a_silent_connection_is_dropped_after_two_missed_heartbeats() {
        let hub = hub().await;
        let mut ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;

        // The first missed beat: a ping goes out, and the connection stays.
        hub.beat();
        let ping = next_frame(&mut ws).await.expect("a ping, not a hang-up");
        assert!(matches!(ping, HubFrame::Ping { .. }), "got {ping:?}");
        assert!(
            hub.registry.connected("laptop"),
            "one missed beat is not two"
        );

        // The second, unanswered: the connection goes.
        hub.beat();
        assert!(closed_by_hub(&mut ws).await, "the socket is closed");
        wait_until("the silent connection to be deregistered", || {
            !hub.registry.connected("laptop")
        })
        .await;
    }

    /// The other half: an agent that answers its pings is NOT dropped. Without
    /// this, "drop on two missed heartbeats" could be "drop after two
    /// heartbeats" and every healthy agent would be cut off.
    #[tokio::test]
    async fn an_agent_that_answers_its_pings_stays_connected() {
        let hub = hub().await;
        let mut ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;

        for _ in 0..4 {
            hub.beat();
            match next_frame(&mut ws).await {
                Some(HubFrame::Ping { id }) => send(&mut ws, &AgentFrame::Pong { id }).await,
                other => panic!("expected a ping, got {other:?}"),
            }
            // The pong is read before the next beat is fired. In production
            // the two are 30 s apart; here they would race.
            barrier(&mut ws).await;
        }
        // Still connected, and still answering — not merely not yet noticed.
        let call = dispatch(&hub, &mut ws, exec("after", None)).await;
        send(&mut ws, &result_of("after", b"alive", b"")).await;
        assert_eq!(
            call.await.unwrap().unwrap(),
            result_of("after", b"alive", b"")
        );
    }

    /// Any frame is a sign of life, not only a `pong`: an agent busy sending
    /// results is not dropped for skipping its pings.
    #[tokio::test]
    async fn any_frame_counts_as_a_sign_of_life() {
        let hub = hub().await;
        let mut ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;

        for round in 0..3 {
            hub.beat();
            assert!(
                matches!(next_frame(&mut ws).await, Some(HubFrame::Ping { .. })),
                "round {round}: a ping, not a hang-up"
            );
            let id = format!("r{round}");
            let call = dispatch(&hub, &mut ws, exec(&id, None)).await;
            send(&mut ws, &result_of(&id, b"busy", b"")).await;
            call.await.unwrap().expect("answered");
        }
        assert!(hub.registry.connected("laptop"));
    }

    /// An upgrade that never sends its `hello` holds a task and a socket for
    /// nothing. It is hung up on at the first beat, and it never reaches the
    /// registry.
    #[tokio::test]
    async fn a_connection_that_never_says_hello_is_hung_up_on() {
        let hub = hub().await;
        let mut ws = dial(hub.addr, Some(LAPTOP_TOKEN)).await.expect("upgrade");
        hub.beat();
        assert!(closed_by_hub(&mut ws).await, "the hub must hang up");
        assert!(!hub.registry.connected("laptop"));
    }

    /// NEW-1 (re-review): a connection upgraded with a token that has since
    /// been rotated must not register when it finally says hello. It would
    /// replace — and so knock off — the agent reinstalled on the new token.
    #[tokio::test]
    async fn a_hello_after_a_rotation_does_not_register() {
        let hub = hub().await;
        // Upgraded with the current token, silent for now.
        let mut stale = dial(hub.addr, Some(LAPTOP_TOKEN)).await.expect("upgrade");
        hub.store
            .lock()
            .unwrap()
            .upsert_host_token("laptop", "rotated-token")
            .unwrap();
        let mut legit = connected_as(&hub, "rotated-token", "laptop", "legit").await;
        send(&mut stale, &hello_as("stale")).await;
        assert!(
            closed_by_hub(&mut stale).await,
            "the stale hello is hung up on"
        );
        let live: Vec<String> = hub
            .registry
            .snapshot()
            .into_iter()
            .map(|s| s.agent_version)
            .collect();
        assert_eq!(
            live,
            vec!["legit".to_string()],
            "the reinstalled agent stays live"
        );
        // And its socket is still open: a barrier round-trips.
        barrier(&mut legit).await;
    }

    /// Flood `ws` with `frame` (already framed, masked with a zero key) as
    /// fast as the socket takes it, and drain what the hub writes back so it
    /// never blocks on us. The flood ends when the hub hangs up, or when the
    /// returned handle is aborted.
    fn flood(ws: Client, frame: Vec<u8>, repeats: usize) -> tokio::task::JoinHandle<()> {
        let burst: Vec<u8> = frame
            .iter()
            .copied()
            .cycle()
            .take(frame.len() * repeats)
            .collect();
        let (mut rd, mut wr) = ws.into_inner().into_split();
        crate::rt::spawn(async move {
            let drain = crate::rt::spawn(async move {
                let mut b = vec![0u8; 65536];
                while tokio::io::AsyncReadExt::read(&mut rd, &mut b)
                    .await
                    .is_ok_and(|n| n > 0)
                {}
            });
            while wr.write_all(&burst).await.is_ok() {}
            drain.abort();
        })
    }

    /// One `pong` protocol frame, as raw client bytes.
    fn raw_pong_frame() -> Vec<u8> {
        let body = encode_agent_frame(&AgentFrame::Pong { id: "x".into() }).unwrap();
        let mut one = frame_header(body.len() as u64);
        one.extend_from_slice(body.as_bytes());
        one
    }

    /// Fire beats until `cond` holds, yielding between them; `false` if it
    /// never did within [`PATIENCE`].
    async fn beat_until(hub: &Hub, mut cond: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + PATIENCE;
        while Instant::now() < deadline {
            if cond() {
                return true;
            }
            hub.beat();
            tokio::task::yield_now().await;
        }
        cond()
    }

    /// NEW-2 (re-review): a connection that never stops sending must not
    /// starve its own heartbeat. The beat is where a rotation reaches a live
    /// connection that nothing is routed to, so a flood that always wins the
    /// read loop's `select!` kept a revoked agent registered indefinitely.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_flooding_agent_is_still_cut_off_by_a_rotation() {
        let hub = hub().await;
        let ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;
        let flooder = flood(ws, raw_pong_frame(), 4096);
        hub.store
            .lock()
            .unwrap()
            .upsert_host_token("laptop", "rotated-token")
            .unwrap();
        let cut = beat_until(&hub, || !hub.registry.connected("laptop")).await;
        flooder.abort();
        assert!(
            cut,
            "a flooding connection on a rotated token is still registered"
        );
    }

    /// NEW-2 (re-review): before its `hello`, a connection that floods
    /// WebSocket control frames must still meet its one-beat deadline, or two
    /// of them hold both of the host's slots and the reinstalled agent, on
    /// the NEW token, is turned away with 429.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pre_hello_flooders_cannot_lock_the_reinstalled_agent_out() {
        let hub = hub().await;
        // A WebSocket pong control frame, masked with a zero key, no payload.
        let pong = vec![0x8A, 0x80, 0, 0, 0, 0];
        let mut flooders = Vec::new();
        for _ in 0..MAX_CONNECTIONS_PER_HOST {
            let ws = dial(hub.addr, Some(LAPTOP_TOKEN)).await.expect("upgrade");
            flooders.push(flood(ws, pong.clone(), 65536));
        }
        hub.store
            .lock()
            .unwrap()
            .upsert_host_token("laptop", "rotated-token")
            .unwrap();
        let freed = beat_until(&hub, || hub.slots.held("laptop") == 0).await;
        let legit = dial(hub.addr, Some("rotated-token")).await.err();
        for f in &flooders {
            f.abort();
        }
        assert!(
            freed,
            "the flooders still hold {} slot(s)",
            hub.slots.held("laptop")
        );
        assert_eq!(legit, None, "the reinstalled agent was refused");
    }

    /// A stream that is never empty, like a socket under a flood: every poll
    /// has `msg` ready, except that it spends the task's cooperative budget
    /// the way a real socket read does, so it yields now and then and a
    /// select over it can be starved exactly as the socket starved it.
    fn endless(
        msg: Message,
    ) -> impl futures_util::Stream<Item = Result<Message, axum::Error>> + Unpin {
        Box::pin(futures_util::stream::unfold((), move |()| {
            let msg = msg.clone();
            async move {
                tokio::task::consume_budget().await;
                Some((Ok(msg), ()))
            }
        }))
    }

    fn manual_beats() -> (Arc<tokio::sync::watch::Sender<u64>>, Ticker) {
        let tx = Arc::new(tokio::sync::watch::Sender::new(0));
        let ticker = BeatSource::Manual(Arc::clone(&tx)).ticker();
        (tx, ticker)
    }

    /// NEW-2, deterministically: the hello deadline holds against a peer
    /// whose frames never run out. With the beat judged only when it wins the
    /// `select!`, a stream that is always ready meant it never did.
    #[tokio::test]
    async fn the_hello_deadline_holds_against_an_endless_stream() {
        let (beats, deadline) = manual_beats();
        let mut stream = endless(Message::Pong(Default::default()));
        let hello = crate::rt::spawn(async move { first_hello(&mut stream, deadline).await });
        beats.send_modify(|n| *n += 1);
        let ended = tokio::time::timeout(PATIENCE, hello).await;
        let why = ended
            .expect("first_hello never noticed its deadline under a flood")
            .unwrap()
            .expect_err("no hello was ever sent");
        assert!(why.contains("no hello"), "{why}");
    }

    /// NEW-2, deterministically: the token re-check on the beat runs even
    /// while frames never stop arriving — and, as the control, a current
    /// token under the same flood is pinged, not dropped.
    #[tokio::test]
    async fn the_beat_rechecks_the_token_against_an_endless_stream() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        {
            let s = store.lock().unwrap();
            s.upsert_host("laptop").unwrap();
            s.set_host_transport("laptop", "agent").unwrap();
            s.upsert_host_token("laptop", LAPTOP_TOKEN).unwrap();
        }
        let registry = AgentRegistry::new();
        let (beats, ticker) = manual_beats();
        let (ping_tx, mut ping_rx) = mpsc::unbounded_channel();
        let pong = encode_agent_frame(&AgentFrame::Pong { id: "x".into() }).unwrap();
        let loop_ = {
            let (store, registry) = (Arc::clone(&store), Arc::clone(&registry));
            crate::rt::spawn(async move {
                let mut stream = endless(Message::Text(pong.into()));
                let budgets = Mutex::new(Budgets::default());
                let credential = crate::mcp::auth::sha256_hex(LAPTOP_TOKEN);
                let who = Registered {
                    registry: &registry,
                    store: &store,
                    alias: "laptop",
                    conn_id: 1,
                    credential: &credential,
                };
                read_loop(&mut stream, who, &budgets, &ping_tx, ticker).await;
            })
        };
        // The control: the token is current, so a beat pings and the loop
        // goes on.
        beats.send_modify(|n| *n += 1);
        let ping = tokio::time::timeout(PATIENCE, ping_rx.recv()).await;
        assert!(
            matches!(ping, Ok(Some(HubFrame::Ping { .. }))),
            "a beat under a flood must still ping: {ping:?}"
        );
        assert!(!loop_.is_finished(), "a current token is not dropped");
        store
            .lock()
            .unwrap()
            .upsert_host_token("laptop", "rotated-token")
            .unwrap();
        beats.send_modify(|n| *n += 1);
        let ended = tokio::time::timeout(PATIENCE, loop_).await;
        assert!(
            ended.is_ok(),
            "the read loop never re-checked the rotated token under a flood"
        );
    }

    /// The production beat is a real interval at [`HEARTBEAT`], the design's
    /// 30 s — tested on the paused clock with no socket, which is the one
    /// place that clock is deterministic.
    #[tokio::test(start_paused = true)]
    async fn the_production_beat_is_every_heartbeat() {
        assert_eq!(HEARTBEAT, Duration::from_secs(30));
        assert_eq!(MISSED_HEARTBEATS, 2);
        let started = Instant::now();
        let mut ticker = BeatSource::Every(HEARTBEAT).ticker();
        ticker.tick().await;
        assert_eq!(
            started.elapsed(),
            HEARTBEAT,
            "the first beat is one interval in"
        );
        ticker.tick().await;
        assert_eq!(started.elapsed(), HEARTBEAT * 2);
    }

    #[tokio::test]
    async fn the_first_frame_must_be_a_hello() {
        let hub = hub().await;
        let mut ws = dial(hub.addr, Some(LAPTOP_TOKEN)).await.expect("upgrade");
        send(&mut ws, &AgentFrame::Pong { id: "1".into() }).await;
        assert!(closed_by_hub(&mut ws).await);
        assert!(!hub.registry.connected("laptop"));
    }

    #[tokio::test]
    async fn a_frame_that_does_not_decode_closes_the_connection() {
        let hub = hub().await;
        let mut ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;
        ws.send(WsMessage::Text("{\"kind\":\"nope\"}".into()))
            .await
            .unwrap();
        assert!(closed_by_hub(&mut ws).await);
        wait_until("the connection to be dropped", || {
            !hub.registry.connected("laptop")
        })
        .await;
    }

    // ── the inbound budget ─────────────────────────────────────────────────

    /// The security property this endpoint inherits: `MAX_FRAME_BYTES` has to
    /// admit a 200 MiB transcript, so a socket loop that decoded every frame
    /// against it would let any agent make the hub parse a quarter of a
    /// gigabyte whenever it liked. The hub asked for 1 KiB per stream; an
    /// answer far above that is refused, the connection goes, and the caller
    /// hears about it at once rather than at its wall clock.
    #[tokio::test]
    async fn an_answer_far_above_the_cap_the_hub_asked_for_is_refused() {
        let hub = hub().await;
        let mut ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;

        let call = dispatch(&hub, &mut ws, exec("r", Some(1024))).await;
        let started = Instant::now();
        send(&mut ws, &result_of("r", &vec![b'x'; 512 * 1024], b"")).await;

        let err = call.await.unwrap().unwrap_err();
        assert_eq!(err.code, crate::ipc_error::codes::E_AGENT_OFFLINE);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "refused at once, not at the 10 s wall clock: {:?}",
            started.elapsed()
        );
        assert!(!hub.registry.connected("laptop"));
    }

    /// The control for the test above: the SAME answer is accepted when the
    /// hub set no cap, so what refused it was the budget and not its size.
    #[tokio::test]
    async fn the_same_answer_is_accepted_when_the_hub_set_no_cap() {
        let hub = hub().await;
        let mut ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;

        let call = dispatch(&hub, &mut ws, exec("r", None)).await;
        let big = vec![b'x'; 512 * 1024];
        send(&mut ws, &result_of("r", &big, b"")).await;
        assert_eq!(call.await.unwrap().unwrap(), result_of("r", &big, b""));
        assert!(hub.registry.connected("laptop"));
    }

    /// An answered request gives its budget back. Without that, one uncapped
    /// `exec` would leave the connection at the full ceiling for the rest of
    /// its wall clock, and the next capped request would buy nothing.
    #[tokio::test]
    async fn an_answered_request_releases_its_budget() {
        let hub = hub().await;
        let mut ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;

        let call = dispatch(&hub, &mut ws, exec("uncapped", None)).await;
        send(&mut ws, &result_of("uncapped", b"small", b"")).await;
        call.await.unwrap().expect("answered");

        let call = dispatch(&hub, &mut ws, exec("capped", Some(1024))).await;
        send(&mut ws, &result_of("capped", &vec![b'x'; 512 * 1024], b"")).await;
        let err = call.await.unwrap().unwrap_err();
        assert_eq!(err.code, crate::ipc_error::codes::E_AGENT_OFFLINE);
    }

    /// The `hello` is judged against the floor, not whatever the connection's
    /// ceiling is: nothing about three short strings needs more.
    #[tokio::test]
    async fn an_oversize_hello_is_refused() {
        let hub = hub().await;
        let mut ws = dial(hub.addr, Some(LAPTOP_TOKEN)).await.expect("upgrade");
        let hello = AgentFrame::Hello {
            agent_version: "1.2.3".into(),
            host_name: "h".repeat(MIN_INBOUND_BYTES),
            os: "linux".into(),
        };
        send(&mut ws, &hello).await;
        assert!(closed_by_hub(&mut ws).await);
        assert!(!hub.registry.connected("laptop"));
    }

    /// The budget is not tighter than the request: an answer with BOTH
    /// streams at the cap — base64'd, in one envelope — is the most an honest
    /// agent can send, and it must get through.
    #[tokio::test]
    async fn an_answer_with_both_streams_at_the_cap_is_accepted() {
        let hub = hub().await;
        let mut ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;

        let cap = 256 * 1024;
        let call = dispatch(&hub, &mut ws, exec("r", Some(cap as u64))).await;
        let (out, err) = (vec![b'o'; cap], vec![b'e'; cap]);
        send(&mut ws, &result_of("r", &out, &err)).await;
        assert_eq!(call.await.unwrap().unwrap(), result_of("r", &out, &err));
        assert!(hub.registry.connected("laptop"));
    }

    /// tungstenite's own default refuses any single frame over 16 MiB, and a
    /// peer writes a whole message as one frame. Past that default is exactly
    /// where the transport lives: `MAX_FRAME_BYTES` is ~267 MiB so that a
    /// 200 MiB transcript crosses in one `result`. An uncapped answer just
    /// over 16 MiB must be accepted — otherwise `move_session` is broken for
    /// any real session, the defect f78f7da fixed in the codec.
    #[tokio::test]
    async fn an_answer_over_tungstenite_s_default_frame_size_is_accepted() {
        let hub = hub().await;
        let mut ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;

        let call = dispatch(&hub, &mut ws, exec("r", None)).await;
        // 13 MiB base64's to ~17.3 MiB: over 16 MiB, far under the ceiling.
        let big = vec![b't'; 13 * 1024 * 1024];
        send(&mut ws, &result_of("r", &big, b"")).await;
        let answer = call.await.unwrap().expect("a large answer is delivered");
        assert!(
            matches!(&answer, AgentFrame::Result { stdout_b64, .. } if stdout_b64.len() > 16 << 20),
            "the whole transcript-sized answer arrived"
        );
    }

    /// The header of one client text frame declaring `len` bytes of
    /// payload, with an all-zero mask so the payload goes on the wire as is.
    fn frame_header(len: u64) -> Vec<u8> {
        let mut raw = vec![0x81]; // FIN, text
        match len {
            0..=125 => raw.push(0x80 | len as u8),
            126..=0xffff => {
                raw.push(0x80 | 126);
                raw.extend_from_slice(&(len as u16).to_be_bytes());
            }
            _ => {
                raw.push(0x80 | 127);
                raw.extend_from_slice(&len.to_be_bytes());
            }
        }
        raw.extend_from_slice(&[0, 0, 0, 0]);
        raw
    }

    /// Write raw bytes straight onto the client's TCP socket, under its
    /// WebSocket framing.
    async fn write_raw(ws: &mut Client, bytes: &[u8]) {
        let tcp = ws.get_mut();
        tcp.write_all(bytes).await.unwrap();
        tcp.flush().await.unwrap();
    }

    /// Above the protocol ceiling a frame is refused from its HEADER: the hub
    /// never buffers the payload at all. The peer here declares a frame over
    /// the ceiling, sends 16 bytes of it, and keeps the connection open. A hub
    /// that tried to buffer the frame would sit waiting for the rest — and
    /// with no beat fired, nothing else would ever close it. Driven at a
    /// 256 KiB stand-in ceiling; the production number is the next test.
    #[tokio::test]
    async fn a_frame_over_the_ceiling_is_refused_from_its_header_without_being_buffered() {
        let hub = hub_with(|s| s.with_frame_cap(256 * 1024)).await;
        let mut ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;

        let mut raw = frame_header(1024 * 1024);
        raw.extend_from_slice(&[b'{'; 16]);
        write_raw(&mut ws, &raw).await;
        assert!(
            closed_by_hub(&mut ws).await,
            "the hub must refuse the frame without waiting for its payload"
        );
        wait_until("the connection to be dropped", || {
            !hub.registry.connected("laptop")
        })
        .await;
    }

    /// The same at the real `MAX_FRAME_BYTES` — no allocation needed, since
    /// only a header is sent.
    #[tokio::test]
    async fn a_frame_over_the_real_ceiling_is_refused_from_its_header() {
        let hub = hub().await;
        let mut ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;

        let mut raw = frame_header(MAX_FRAME_BYTES as u64 + 1);
        raw.extend_from_slice(&[b'{'; 16]);
        write_raw(&mut ws, &raw).await;
        assert!(closed_by_hub(&mut ws).await);
    }

    /// The control for the two above: the same trickle — a header, 16 bytes,
    /// then the rest — for a frame UNDER the ceiling is waited for and
    /// accepted, so what refused them was the ceiling and not the trickle.
    #[tokio::test]
    async fn a_frame_under_the_ceiling_is_waited_for_and_accepted() {
        let hub = hub_with(|s| s.with_frame_cap(256 * 1024)).await;
        let mut ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;

        // A `pong` padded to 48 KiB: under the stand-in ceiling, and under the
        // floor every frame is allowed whatever is in flight.
        let mut payload = encode_agent_frame(&AgentFrame::Pong { id: "pad".into() })
            .unwrap()
            .into_bytes();
        payload.resize(48 * 1024, b' ');
        let mut head = frame_header(payload.len() as u64);
        head.extend_from_slice(&payload[..16]);
        write_raw(&mut ws, &head).await;
        tokio::task::yield_now().await;
        write_raw(&mut ws, &payload[16..]).await;

        // Still connected, and the socket still works.
        let call = dispatch(&hub, &mut ws, exec("after", None)).await;
        send(&mut ws, &result_of("after", b"ok", b"")).await;
        assert_eq!(call.await.unwrap().unwrap(), result_of("after", b"ok", b""));
    }

    /// The ceiling holds for a whole MESSAGE, not only for each frame: a peer
    /// may fragment, and every fragment here is under the 256 KiB stand-in
    /// ceiling while the message is twice it. An uncapped request is in
    /// flight, so the budget would admit it — only the message limit stands
    /// between it and the decoder, and tungstenite's own default for that is
    /// 64 MiB.
    #[tokio::test]
    async fn a_fragmented_message_over_the_ceiling_is_refused() {
        let hub = hub_with(|s| s.with_frame_cap(256 * 1024)).await;
        let mut ws = connected(&hub, LAPTOP_TOKEN, "laptop").await;

        let call = dispatch(&hub, &mut ws, exec("r", None)).await;
        // A well-formed answer, so that a hub without the limit would accept
        // it rather than refuse it for some other reason.
        let text = encode_agent_frame(&result_of("r", &vec![b'x'; 384 * 1024], b"")).unwrap();
        let chunks: Vec<&[u8]> = text.as_bytes().chunks(128 * 1024).collect();
        assert!(chunks.len() >= 4, "{} fragments", chunks.len());
        for (i, chunk) in chunks.iter().enumerate() {
            let mut raw = frame_header(chunk.len() as u64);
            // Text for the first fragment, continuation after; FIN on the last.
            raw[0] =
                if i == 0 { 0x01 } else { 0x00 } | if i + 1 == chunks.len() { 0x80 } else { 0 };
            raw.extend_from_slice(chunk);
            write_raw(&mut ws, &raw).await;
        }

        let err = call.await.unwrap().unwrap_err();
        assert_eq!(err.code, crate::ipc_error::codes::E_AGENT_OFFLINE);
    }

    // ── the budget table, without a socket ─────────────────────────────────

    #[test]
    fn an_uncapped_exec_is_allowed_the_whole_ceiling() {
        assert_eq!(exec_budget(None), MAX_FRAME_BYTES);
    }

    #[test]
    fn a_capped_exec_is_allowed_only_what_its_cap_implies() {
        // Two streams at the cap, base64'd, plus one envelope.
        let budget = exec_budget(Some(1_000_000));
        assert!(budget > fleet_proto::base64_len(2_000_000));
        assert!(
            budget < MAX_FRAME_BYTES / 10,
            "a 1 MB cap must not buy the whole ceiling: {budget}"
        );
        assert!(
            exec_budget(Some(1024)) < exec_budget(Some(1024 * 1024)),
            "the budget must follow the cap"
        );
        // A hostile cap cannot overflow the arithmetic into a bigger budget.
        assert_eq!(exec_budget(Some(u64::MAX)), MAX_FRAME_BYTES);
    }

    #[tokio::test]
    async fn the_allowance_is_the_largest_request_in_flight_and_never_below_the_floor() {
        let mut b = Budgets::default();
        assert_eq!(b.allowance(), MIN_INBOUND_BYTES, "nothing in flight");
        b.arm(
            "small".into(),
            1_000,
            Instant::now() + Duration::from_secs(60),
        );
        assert_eq!(b.allowance(), MIN_INBOUND_BYTES, "still the floor");
        b.arm(
            "big".into(),
            MIN_INBOUND_BYTES * 4,
            Instant::now() + Duration::from_secs(60),
        );
        assert_eq!(b.allowance(), MIN_INBOUND_BYTES * 4);
        b.done("big");
        assert_eq!(
            b.allowance(),
            MIN_INBOUND_BYTES,
            "answered: budget released"
        );
    }

    /// A request whose caller has already given up must not keep its budget
    /// alive — otherwise one uncapped `exec` would leave the connection at the
    /// full ceiling forever.
    #[tokio::test(start_paused = true)]
    async fn a_request_past_its_deadline_stops_raising_the_allowance() {
        let mut b = Budgets::default();
        b.arm(
            "gone".into(),
            MIN_INBOUND_BYTES * 4,
            Instant::now() + Duration::from_secs(10),
        );
        assert_eq!(b.allowance(), MIN_INBOUND_BYTES * 4);
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert_eq!(b.allowance(), MIN_INBOUND_BYTES, "expired");
    }
}
