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
//! **The inbound budget is per request, not the global ceiling.** A frame may
//! legitimately be [`MAX_FRAME_BYTES`] — ~267 MiB, because the transport has to
//! carry a 200 MiB transcript — but only for a request that asked for output
//! that large. Every other frame is decoded against the size the hub actually
//! asked for ([`decode_agent_frame_within`]), refused if it is over, and the
//! connection closed. See `Budgets`.
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
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use fleet_proto::{
    decode_agent_frame_within, encode_hub_frame, AgentFrame, HubFrame, MAX_FRAME_BYTES,
};
use futures_util::stream::{SplitSink, SplitStream};
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
    limits: Limits,
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
            BeatSource::Every(every) => {
                let mut i = tokio::time::interval_at(Instant::now() + *every, *every);
                // A beat missed while a large frame was being decoded is
                // caught up on the next one, not replayed in a tight loop.
                i.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                Ticker::Every(i)
            }
            #[cfg(test)]
            BeatSource::Manual(tx) => Ticker::Manual(tx.subscribe()),
        }
    }
}

enum Ticker {
    Every(tokio::time::Interval),
    #[cfg(test)]
    Manual(tokio::sync::watch::Receiver<u64>),
}

impl Ticker {
    async fn tick(&mut self) {
        match self {
            Ticker::Every(i) => {
                i.tick().await;
            }
            #[cfg(test)]
            Ticker::Manual(rx) => {
                if rx.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
        }
    }
}

impl AgentWsState {
    /// A route that registers connections on `registry`, or refuses them all
    /// when there is none.
    pub fn new(registry: Option<Arc<AgentRegistry>>) -> Self {
        Self {
            registry,
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
    ws: WebSocketUpgrade,
) -> Response {
    let Some(registry) = state.registry.clone() else {
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
    let limits = state.limits;
    // Started here, before the `101` goes out, so the hello deadline counts
    // from the upgrade and no beat after it can be missed.
    let hello_deadline = limits.beats.ticker();
    ws.max_frame_size(limits.frame_cap)
        .max_message_size(limits.frame_cap)
        .on_upgrade(move |socket| serve(socket, registry, alias, limits, hello_deadline))
}

/// One connection, from the upgrade to the deregistration.
async fn serve(
    socket: WebSocket,
    registry: Arc<AgentRegistry>,
    alias: String,
    limits: Limits,
    hello_deadline: Ticker,
) {
    let (mut sink, mut stream) = socket.split();
    let hello = match first_hello(&mut stream, hello_deadline).await {
        Ok(h) => h,
        Err(why) => {
            tracing::warn!(host = %alias, why, "[agent] closing before registration");
            let _ = sink.close().await;
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
    let conn_id = registry.connect(&alias, hello, tx);
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
    // Whichever ends first ends the connection: the reader when the agent
    // goes (or goes quiet), the writer when the registry lets go of it.
    let writer_done = tokio::select! {
        () = read_loop(&mut stream, &registry, &alias, conn_id, &budgets, &ping_tx, ticker) => false,
        _ = &mut writer => true,
    };

    // Deregister first: it drops the registry's half of the channel, which is
    // what tells the writer to close the socket. A connection that was already
    // replaced no longer owns the entry, and this does nothing to the
    // replacement (`AgentRegistry::disconnect` checks the generation).
    registry.disconnect(&alias, conn_id);
    drop(ping_tx);
    if !writer_done {
        let _ = writer.await;
    }
    tracing::info!(host = %alias, conn = conn_id, "[agent] disconnected");
}

/// Read frames until the agent identifies itself.
///
/// Bounded by one heartbeat: an upgrade that never says hello would otherwise
/// hold a task and a socket for as long as the peer cared to keep the TCP
/// connection open, without ever appearing in the registry where an operator
/// could see it.
async fn first_hello(
    stream: &mut SplitStream<WebSocket>,
    mut deadline: Ticker,
) -> Result<AgentHello, String> {
    loop {
        let msg = tokio::select! {
            biased;
            msg = stream.next() => msg,
            () = deadline.tick() => return Err("no hello within one heartbeat".into()),
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
                if sink.send(Message::Text(text.into())).await.is_err() {
                    break;
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
    let _ = sink.close().await;
}

/// Read frames and hand them to the registry, pinging an idle agent and
/// dropping one that has gone quiet.
async fn read_loop(
    stream: &mut SplitStream<WebSocket>,
    registry: &AgentRegistry,
    alias: &str,
    conn_id: ConnId,
    budgets: &Mutex<Budgets>,
    pings: &mpsc::UnboundedSender<HubFrame>,
    mut ticker: Ticker,
) {
    // Beats in a row with nothing heard. The `hello` does not count: silence
    // is measured from the registration.
    let mut missed = 0;
    let mut heard = false;
    loop {
        // The futures are dropped at the end of this `let`, which is what
        // releases `stream` for the branches below. `biased`: a frame that is
        // already waiting is read before a beat is judged, so an answer that
        // raced its beat still counts.
        let step = tokio::select! {
            biased;
            msg = stream.next() => Step::Incoming(msg),
            () = ticker.tick() => Step::Beat,
        };
        match step {
            Step::Beat => {
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
    const PHONE_TOKEN: &str = "phone-client-token";

    type Client = WebSocketStream<tokio::net::TcpStream>;

    /// A hub serving the REAL app: the real `authorize` layer, the real
    /// `/agent` route, over a real loopback socket.
    struct Hub {
        addr: SocketAddr,
        store: Arc<Mutex<Store>>,
        registry: Arc<AgentRegistry>,
        beats: Arc<tokio::sync::watch::Sender<u64>>,
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
            s.upsert_host_token("laptop", LAPTOP_TOKEN).unwrap();
            s.upsert_host("desk").unwrap();
            s.upsert_host_token("desk", DESK_TOKEN).unwrap();
            s.insert_client_token("phone", &crate::mcp::auth::sha256_hex(PHONE_TOKEN), "full")
                .unwrap();
        }
        let registry = AgentRegistry::new();
        let state = AgentWsState::new(Some(Arc::clone(&registry)));
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
