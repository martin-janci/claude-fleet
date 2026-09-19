//! `run`: dial the hub, say hello, serve its frames, and dial again when the
//! connection goes.
//!
//! **Heartbeats follow the hub, not the design doc.** The design says the
//! agent "sends a heartbeat every 30 seconds"; the hub as built
//! (`fleet-core/src/agent/ws.rs`) sends the `ping` itself every
//! [`HEARTBEAT`], counts beats with nothing heard, and drops an agent after
//! two. So the agent never originates a heartbeat: it answers each `ping`
//! with a `pong`, and it counts the hub's silence the same way — beats with no
//! frame at all — to notice a hub that has gone without closing the socket.
//!
//! **Frame sizes follow the hub too.** The hub sends each message as ONE
//! WebSocket frame and admits frames up to `fleet_proto::MAX_FRAME_BYTES`
//! (~267 MiB, a 200 MiB transcript after base64), so the agent's socket
//! accepts frames and messages that large. An inbound frame is decoded against
//! that ceiling: unlike the hub, the agent has no request of its own to size
//! an inbound budget from — every hub frame is a request. What the agent does
//! size is what it SENDS: each `result` is truncated to
//! `fleet_proto::result_stream_limits(cap_bytes)` and encoded within
//! `fleet_proto::result_budget(cap_bytes)`, the same number the hub decodes it
//! against.

use crate::exec::{self, ExecRequest, SeenIds};
use fleet_proto::{
    decode_b64, decode_hub_frame_lenient, encode_agent_frame, encode_agent_frame_within,
    encode_b64, judge_proto, result_budget, result_stream_limits, AgentFrame, Decoded, HubFrame,
    HEARTBEAT, MAX_FRAME_BYTES, VERSION_REFUSED_CLOSE_CODE,
};
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot, Semaphore};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Message, WebSocketConfig};
use tokio_tungstenite::WebSocketStream;

/// Beats in a row with nothing from the hub before the agent gives up on the
/// connection and dials again. One more than the hub's own two: the hub pings
/// every beat, so silence this long means the hub is gone, and the margin
/// keeps the agent from dropping a connection the hub still considers live.
pub const SILENT_BEATS: u32 = 3;

/// The first reconnect delay's ceiling, doubled per failed attempt…
pub const BACKOFF_BASE: Duration = Duration::from_secs(1);
/// …up to this.
pub const BACKOFF_CAP: Duration = Duration::from_secs(60);

/// How long a dial (TCP, TLS and the upgrade) may take before it is abandoned
/// and retried.
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(20);

/// How the agent's `STATUS=` line starts while it is connected — what
/// `status` looks for.
pub const CONNECTED: &str = "connected to";

/// Any stream a WebSocket can run over: plain TCP, or TLS on top of it.
pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// The dialled socket.
pub type Socket = WebSocketStream<Box<dyn Io>>;

/// Where the hub is, as a WebSocket URL ending in `/agent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    url: String,
    host: String,
    port: u16,
    tls: bool,
}

impl Endpoint {
    /// Accepts the hub's public URL (`https://hub.example`) or its WebSocket
    /// URL (`wss://hub.example/agent`). A plain `http://`/`ws://` hub is
    /// refused unless `insecure`: the token would cross the network in clear.
    pub fn parse(hub: &str, insecure: bool) -> Result<Self, String> {
        let not_a_hub =
            || format!("{hub:?} is not a hub URL (expected https://host[:port][/path])");
        let (scheme, rest) = hub.split_once("://").ok_or_else(not_a_hub)?;
        let tls = match scheme.to_ascii_lowercase().as_str() {
            "https" | "wss" => true,
            "http" | "ws" if insecure => false,
            "http" | "ws" => {
                return Err(format!(
                    "refusing the plain {scheme}:// hub {hub}: this host's token would cross \
                     the network in clear. Use https://, or pass --insecure for a loopback test"
                ))
            }
            _ => return Err(not_a_hub()),
        };
        if rest.contains(['?', '#']) {
            return Err(format!("{hub:?}: a hub URL has no query or fragment"));
        }
        let (authority, path) = match rest.find('/') {
            Some(i) => rest.split_at(i),
            None => (rest, ""),
        };
        if authority.is_empty() || authority.contains('@') {
            return Err(not_a_hub());
        }
        let default_port = if tls { 443 } else { 80 };
        let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
            let (host, after) = v6.split_once(']').ok_or_else(not_a_hub)?;
            let port = match after.strip_prefix(':') {
                Some(p) => p.parse().map_err(|_| not_a_hub())?,
                None if after.is_empty() => default_port,
                None => return Err(not_a_hub()),
            };
            (host.to_string(), port)
        } else {
            match authority.rsplit_once(':') {
                Some((host, p)) => (host.to_string(), p.parse().map_err(|_| not_a_hub())?),
                None => (authority.to_string(), default_port),
            }
        };
        if host.is_empty() {
            return Err(not_a_hub());
        }
        if !tls && !is_loopback(&host) {
            return Err(format!(
                "refusing the plain hub {hub}: --insecure is for a loopback test only, and \
                 {host} is not loopback. Use https://"
            ));
        }
        let mut path = path.trim_end_matches('/').to_string();
        if !path.ends_with("/agent") {
            path.push_str("/agent");
        }
        Ok(Self {
            url: format!("{}://{authority}{path}", if tls { "wss" } else { "ws" }),
            host,
            port,
            tls,
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn is_tls(&self) -> bool {
        self.tls
    }
}

/// `localhost`, or an address in 127.0.0.0/8 or `::1`.
fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Why a dial did not produce a socket.
#[derive(Debug)]
pub enum DialError {
    /// The hub answered the upgrade with an HTTP error — a bad or revoked
    /// token (401), or a token that is not allowed to be an agent (403).
    Refused { status: u16, body: String },
    /// Anything else: DNS, TCP, TLS, a malformed handshake.
    Failed(String),
}

impl std::fmt::Display for DialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused { status, body } if body.is_empty() => write!(f, "hub refused: {status}"),
            Self::Refused { status, body } => write!(f, "hub refused: {status}: {body}"),
            Self::Failed(why) => f.write_str(why),
        }
    }
}

/// How to reach the hub: where, as whom, and whom to trust.
pub struct Dialer {
    endpoint: Endpoint,
    token: String,
    tls: Option<tokio_rustls::TlsConnector>,
}

impl Dialer {
    /// `ca_file` replaces the host's system roots, for a hub with a private CA.
    pub fn new(endpoint: Endpoint, token: String, ca_file: Option<&Path>) -> Result<Self, String> {
        let tls = if endpoint.tls {
            Some(tls_connector(ca_file)?)
        } else {
            None
        };
        Ok(Self {
            endpoint,
            token,
            tls,
        })
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub async fn dial(&self) -> Result<Socket, DialError> {
        match tokio::time::timeout(DIAL_TIMEOUT, self.dial_once()).await {
            Ok(result) => result,
            Err(_) => Err(DialError::Failed(format!(
                "no answer from {} within {}s",
                self.endpoint.url,
                DIAL_TIMEOUT.as_secs()
            ))),
        }
    }

    async fn dial_once(&self) -> Result<Socket, DialError> {
        let ep = &self.endpoint;
        let tcp = tokio::net::TcpStream::connect((ep.host.as_str(), ep.port))
            .await
            .map_err(|e| DialError::Failed(format!("{}:{}: {e}", ep.host, ep.port)))?;
        let _ = tcp.set_nodelay(true);
        let io: Box<dyn Io> = match &self.tls {
            None => Box::new(tcp),
            Some(tls) => {
                let name = rustls_pki_types::ServerName::try_from(ep.host.clone())
                    .map_err(|e| DialError::Failed(format!("{}: {e}", ep.host)))?;
                let stream = tls
                    .connect(name, tcp)
                    .await
                    .map_err(|e| DialError::Failed(format!("TLS with {}: {e}", ep.host)))?;
                Box::new(stream)
            }
        };
        let mut request = ep
            .url
            .as_str()
            .into_client_request()
            .map_err(|e| DialError::Failed(format!("{}: {e}", ep.url)))?;
        let bearer = format!("Bearer {}", self.token)
            .parse()
            .map_err(|_| DialError::Failed("the token cannot be sent as a header".into()))?;
        request.headers_mut().insert("authorization", bearer);
        match tokio_tungstenite::client_async_with_config(request, io, Some(socket_config())).await
        {
            Ok((ws, _)) => Ok(ws),
            Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => Err(DialError::Refused {
                status: resp.status().as_u16(),
                body: resp
                    .body()
                    .as_deref()
                    .map(|b| String::from_utf8_lossy(b).trim().to_string())
                    .unwrap_or_default(),
            }),
            Err(e) => Err(DialError::Failed(format!("{}: {e}", ep.url))),
        }
    }
}

/// The hub writes a whole message as one frame, up to the protocol ceiling,
/// so both of tungstenite's limits have to admit it (its frame default is
/// 16 MiB, which would refuse any transcript bigger than that).
fn socket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES))
}

/// Where the host keeps its CA bundle. `SSL_CERT_FILE` first, as OpenSSL
/// honours it; then the usual places, Debian/Ubuntu, Fedora/RHEL, Alpine and
/// macOS, openSUSE.
const CA_BUNDLES: [&str; 4] = [
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/cert.pem",
    "/etc/ssl/ca-bundle.pem",
];

/// A TLS client trusting `ca_file`, or else the host's own CA bundle. The
/// host's bundle rather than a root list compiled in: it is what the operator
/// already maintains, and it adds no crate to the build.
fn tls_connector(ca_file: Option<&Path>) -> Result<tokio_rustls::TlsConnector, String> {
    use rustls_pki_types::pem::PemObject;
    use rustls_pki_types::CertificateDer;
    use tokio_rustls::rustls;

    let bundle = match ca_file {
        Some(path) => path.to_path_buf(),
        None => std::env::var_os("SSL_CERT_FILE")
            .map(PathBuf::from)
            .or_else(|| CA_BUNDLES.iter().map(PathBuf::from).find(|p| p.is_file()))
            .ok_or("no system CA bundle found; pass --ca-file with the hub's CA")?,
    };
    let mut roots = rustls::RootCertStore::empty();
    let certs =
        CertificateDer::pem_file_iter(&bundle).map_err(|e| format!("{}: {e}", bundle.display()))?;
    for cert in certs {
        let cert = cert.map_err(|e| format!("{}: {e}", bundle.display()))?;
        // One unusable certificate in a system bundle is not a reason to
        // trust none of the others.
        let _ = roots.add(cert);
    }
    if roots.is_empty() {
        return Err(format!("{}: no usable certificate in it", bundle.display()));
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(tokio_rustls::TlsConnector::from(Arc::new(config)))
}

/// What a connection shares with the next one: the ids already seen, so a
/// frame replayed onto a new connection is refused too, and the concurrency
/// ceiling.
pub struct Agent {
    seen: Mutex<SeenIds>,
    slots: Arc<Semaphore>,
    home: Option<PathBuf>,
    /// Set once by [`Agent::stop`]; every child listens for it.
    stopping: tokio::sync::watch::Sender<bool>,
    /// How many `exec`s are running or queued, so `stop` knows when the
    /// last child is gone.
    running: tokio::sync::watch::Sender<usize>,
}

/// One `exec` counted in [`Agent::running`] for as long as it lives.
struct Running(Arc<Agent>);

impl Drop for Running {
    fn drop(&mut self) {
        self.0.running.send_modify(|n| *n = n.saturating_sub(1));
    }
}

impl Agent {
    /// `home` is where children start and relative uploads land — `$HOME`,
    /// the directory an ssh remote command starts in.
    pub fn new(home: Option<PathBuf>, concurrency: usize) -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(SeenIds::new(exec::SEEN_IDS)),
            slots: Arc::new(Semaphore::new(concurrency.max(1))),
            home,
            stopping: tokio::sync::watch::Sender::new(false),
            running: tokio::sync::watch::Sender::new(0),
        })
    }

    /// Resolves once [`Agent::stop`] has been called.
    fn stopped(&self) -> impl std::future::Future<Output = ()> + Send + 'static {
        let mut rx = self.stopping.subscribe();
        async move {
            let _ = rx.wait_for(|s| *s).await;
        }
    }

    /// Stop: kill every child this agent is running, refuse new ones, and
    /// return once they are gone.
    ///
    /// `fleet-agent run` calls this on SIGTERM. The unit's `KillMode=process`
    /// has systemd signal only the agent — so the tmux servers it started,
    /// which `setsid` out of every child's process group, survive a restart
    /// — which means that without this, a running child would be orphaned
    /// with nothing left to enforce its timeout.
    pub async fn stop(&self) {
        self.stopping.send_replace(true);
        let mut running = self.running.subscribe();
        let _ = running.wait_for(|n| *n == 0).await;
    }

    /// `false` for an id this agent has already been sent.
    fn first_sight(&self, id: &str) -> bool {
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id)
    }
}

/// Where a connection's beats come from. Silence is COUNTED in beats, the way
/// the hub counts it, so a test fires them by hand instead of waiting.
pub enum Beats {
    Every(Duration),
    /// One beat per message; the agent answers each on its `oneshot` with
    /// whether the connection survived it.
    Manual(mpsc::UnboundedReceiver<oneshot::Sender<bool>>),
}

impl Beats {
    /// The production source: one beat per [`HEARTBEAT`].
    pub fn heartbeat() -> Self {
        Beats::Every(HEARTBEAT)
    }

    fn ticker(self) -> Ticker {
        match self {
            Beats::Every(every) => {
                let mut i = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
                i.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                Ticker::Every(i)
            }
            Beats::Manual(rx) => Ticker::Manual(rx),
        }
    }
}

enum Ticker {
    Every(tokio::time::Interval),
    Manual(mpsc::UnboundedReceiver<oneshot::Sender<bool>>),
}

impl Ticker {
    /// The next beat, and where to report the verdict on it, if anywhere.
    async fn tick(&mut self) -> Option<oneshot::Sender<bool>> {
        match self {
            Ticker::Every(i) => {
                i.tick().await;
                None
            }
            Ticker::Manual(rx) => match rx.recv().await {
                Some(ack) => Some(ack),
                None => std::future::pending().await,
            },
        }
    }
}

/// Why a connection ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEnd {
    /// The hub closed it, or the socket went.
    Closed(String),
    /// [`SILENT_BEATS`] beats with nothing from the hub.
    HubSilent,
    /// The hub sent something that is not a hub frame; the agent closed it.
    Protocol(String),
    /// Either side's protocol version was out of the other's range — the
    /// hub's close carried [`fleet_proto::VERSION_REFUSED_CLOSE_CODE`], or
    /// this agent found the hub's own `welcome.proto` out of ITS range and
    /// closed first. `run_with` reads this to back off at the maximum
    /// interval instead of the normal growing sequence: nothing on either
    /// end fixes itself by retrying sooner.
    VersionRefused(String),
}

impl std::fmt::Display for SessionEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed(why) => write!(f, "connection closed: {why}"),
            Self::HubSilent => write!(f, "nothing from the hub for {SILENT_BEATS} heartbeats"),
            Self::Protocol(why) => write!(f, "the hub broke the protocol: {why}"),
            Self::VersionRefused(why) => write!(f, "protocol version refused: {why}"),
        }
    }
}

/// How a connection went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Served {
    pub end: SessionEnd,
    /// The hub sent at least one frame — it accepted this agent — so the next
    /// dial starts the backoff over.
    pub heard_any: bool,
}

/// What the writer task sends.
enum Out {
    Frame(String),
    /// Close the socket with this code and reason, and stop.
    Close(CloseCode, String),
}

/// Each in-flight `exec`'s cancel handle. Removing an entry and firing it,
/// or dropping it, kills that child.
type InFlight = Arc<Mutex<HashMap<String, oneshot::Sender<()>>>>;

/// Say hello and serve one connection until it ends. Children still running
/// when it ends are killed: their answers have nowhere to go.
///
/// Reading and writing are separate tasks, like the hub's: a single loop that
/// did both in turn could deadlock against a hub built the same way the moment
/// both had a large frame to send.
pub async fn serve<S>(ws: WebSocketStream<S>, agent: &Arc<Agent>, beats: Beats) -> Served
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sink, mut stream) = ws.split();
    let hello = AgentFrame::Hello {
        agent_version: env!("CARGO_PKG_VERSION").to_string(),
        host_name: host_name(),
        os: std::env::consts::OS.to_string(),
        proto: fleet_proto::PROTO_VERSION,
    };
    let hello = encode_agent_frame(&hello).expect("a hello is three short strings and a number");
    if let Err(e) = sink.send(Message::Text(hello.into())).await {
        return Served {
            end: SessionEnd::Closed(format!("before hello: {e}")),
            heard_any: false,
        };
    }

    let (out, out_rx) = mpsc::unbounded_channel();
    let mut writer = tokio::spawn(write_loop(sink, out_rx));
    let inflight: InFlight = Arc::default();
    let mut ticker = beats.ticker();
    let mut missed = 0;
    let mut heard = false;
    let mut heard_any = false;
    // Bounded, sanitised tracking of unknown frame kinds on this connection
    // — see `fleet_proto::UnknownKinds`'s doc.
    let mut unknown_kinds = fleet_proto::UnknownKinds::new();
    // Nothing else the hub sends is acted on until a compatible `welcome`
    // arrives. A hub built before this protocol version exists sends no
    // `welcome` at all — nothing about the connection stops it from sending
    // a command straight after accepting `hello`, the way every hub did
    // before this change — so seeing `hello` answered at the WebSocket
    // layer is not enough to trust anything the hub sends next. Anything
    // else arriving first, or nothing at all within one heartbeat (below),
    // ends the session exactly as an out-of-range `welcome.proto` would:
    // `SessionEnd::VersionRefused`, which `run_with` backs off on at the
    // maximum interval instead of dialling this hub again right away.
    let mut welcomed = false;

    let end = loop {
        tokio::select! {
            // A frame already waiting is read before a beat is judged, so an
            // answer that raced its beat still counts.
            biased;
            msg = stream.next() => {
                let msg = match msg {
                    None => break SessionEnd::Closed("the hub went away".into()),
                    Some(Err(e)) => break SessionEnd::Closed(e.to_string()),
                    Some(Ok(msg)) => msg,
                };
                heard = true;
                heard_any = true;
                match msg {
                    // Lenient: an unknown `kind` is skipped, not fatal — see
                    // the crate doc. `welcome` is judged here, not in
                    // `handle`, because a refusal has to END this loop, which
                    // a plain function cannot do.
                    Message::Text(text) => match decode_hub_frame_lenient(&text) {
                        Ok(Decoded::Frame(HubFrame::Welcome { hub_version, proto })) => {
                            if let Some(reason) =
                                judge_proto(proto).refusal_reason("fleet-agent", "the hub")
                            {
                                break SessionEnd::VersionRefused(reason);
                            }
                            welcomed = true;
                            tracing::info!(
                                hub_version, proto, "[agent] hub protocol compatible"
                            );
                        }
                        // Anything else at all, before a compatible welcome:
                        // this hub either predates the protocol entirely (no
                        // welcome coming, ever) or is not behaving like one
                        // that does — either way, nothing it sends is acted
                        // on until that is resolved.
                        Ok(_) if !welcomed => {
                            break SessionEnd::VersionRefused(no_welcome_reason());
                        }
                        Ok(Decoded::Frame(frame)) => handle(frame, agent, &out, &inflight),
                        Ok(Decoded::Unknown { kind }) => {
                            match unknown_kinds.record(&kind) {
                                fleet_proto::UnknownKindAction::LogOnce(kind) => {
                                    tracing::warn!(kind, "[agent] unknown frame kind; skipping");
                                }
                                fleet_proto::UnknownKindAction::Silent => {}
                                // Unlike the hub (which is exposed to any
                                // agent holding a valid host token, and
                                // closes past this same cap), the agent
                                // trusts the one hub it was configured to
                                // dial: dropping that connection over noisy
                                // unknown kinds would be more disruptive
                                // than the risk it guards against, so here
                                // it only stops logging, never disconnects.
                                fleet_proto::UnknownKindAction::LogCapReached => {
                                    tracing::warn!(
                                        "[agent] too many distinct unknown frame kinds; no longer logging them"
                                    );
                                }
                            }
                        }
                        Err(e) => break SessionEnd::Protocol(e.to_string()),
                    },
                    Message::Binary(_) => {
                        break SessionEnd::Protocol("a binary frame on a text protocol".into())
                    }
                    Message::Close(frame) => break close_end(frame),
                    Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
                }
            }
            ack = ticker.tick() => {
                if !welcomed {
                    // One heartbeat with no welcome at all: this hub is not
                    // going to send one. Treated the same as an
                    // out-of-range one, not `HubSilent` — the fix is a hub
                    // upgrade, not a network problem, and `run_with` must
                    // back off accordingly.
                    if let Some(ack) = ack {
                        let _ = ack.send(false);
                    }
                    break SessionEnd::VersionRefused(no_welcome_timeout_reason());
                }
                missed = if std::mem::take(&mut heard) { 0 } else { missed + 1 };
                let alive = missed < SILENT_BEATS;
                if let Some(ack) = ack {
                    let _ = ack.send(alive);
                }
                if !alive {
                    break SessionEnd::HubSilent;
                }
            }
        }
    };

    // Every child of this connection is killed: dropping its cancel handle
    // is what does it.
    inflight.lock().unwrap_or_else(|e| e.into_inner()).clear();
    match &end {
        SessionEnd::Closed(_) => writer.abort(),
        SessionEnd::HubSilent | SessionEnd::Protocol(_) | SessionEnd::VersionRefused(_) => {
            let code = match &end {
                SessionEnd::VersionRefused(_) => CloseCode::from(VERSION_REFUSED_CLOSE_CODE),
                _ => CloseCode::Policy,
            };
            let _ = out.send(Out::Close(code, end.to_string()));
            // Bounded: a hub that stopped reading must not hold this here.
            if tokio::time::timeout(Duration::from_secs(5), &mut writer)
                .await
                .is_err()
            {
                writer.abort();
            }
        }
    }
    Served { end, heard_any }
}

/// What a WebSocket close means for this connection: a version refusal (the
/// hub's [`VERSION_REFUSED_CLOSE_CODE`]) is told apart from every other
/// close by its CODE, never by parsing the reason text — the reason is
/// carried through either way, for the log line.
///
/// The reason is hub-controlled text that `run_with` puts straight into a
/// `tracing::error!`/`warn!`, so it is neutralised first, exactly as an
/// unknown frame `kind` already is: a newline or an ANSI escape in it must
/// not be able to forge a journald line.
fn close_end(frame: Option<CloseFrame>) -> SessionEnd {
    let reason = |f: &CloseFrame| fleet_proto::sanitize_for_log(&f.reason, CLOSE_REASON_MAX);
    match frame {
        Some(f) if u16::from(f.code) == VERSION_REFUSED_CLOSE_CODE => {
            SessionEnd::VersionRefused(reason(&f))
        }
        Some(f) if !f.reason.is_empty() => SessionEnd::Closed(reason(&f)),
        _ => SessionEnd::Closed("the hub closed it".into()),
    }
}

/// The most a WebSocket close frame's reason may carry, per RFC 6455.
const CLOSE_REASON_MAX: usize = 123;

/// Why the connection ends when the hub sends something other than
/// `welcome` before ever sending one.
fn no_welcome_reason() -> String {
    format!(
        "this hub sent no welcome — it predates protocol v{}; update the hub",
        fleet_proto::MIN_SUPPORTED_PROTO
    )
}

/// Why the connection ends when a whole heartbeat passes with no `welcome`
/// at all.
fn no_welcome_timeout_reason() -> String {
    format!(
        "no welcome within one heartbeat — this hub predates protocol v{}; update the hub",
        fleet_proto::MIN_SUPPORTED_PROTO
    )
}

async fn write_loop<S>(
    mut sink: SplitSink<WebSocketStream<S>, Message>,
    mut rx: mpsc::UnboundedReceiver<Out>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    while let Some(out) = rx.recv().await {
        match out {
            Out::Frame(text) => {
                if sink.send(Message::Text(text.into())).await.is_err() {
                    return;
                }
            }
            Out::Close(code, reason) => {
                // A close reason is at most `CLOSE_REASON_MAX` bytes on the
                // wire.
                let mut reason = reason;
                while reason.len() > CLOSE_REASON_MAX {
                    reason.pop();
                }
                let frame = CloseFrame {
                    code,
                    reason: reason.into(),
                };
                let _ = sink.send(Message::Close(Some(frame))).await;
                let _ = sink.close().await;
                return;
            }
        }
    }
    let _ = sink.close().await;
}

/// Act on one hub frame. Nothing here waits: work is spawned, and answers go
/// out through the writer.
fn handle(
    frame: HubFrame,
    agent: &Arc<Agent>,
    out: &mpsc::UnboundedSender<Out>,
    inflight: &InFlight,
) {
    match frame {
        HubFrame::Ping { id } => {
            let pong = encode_agent_frame(&AgentFrame::Pong { id }).expect("a pong is small");
            let _ = out.send(Out::Frame(pong));
        }
        HubFrame::Cancel { id } => {
            // Unknown or finished: nothing to do, as the hub expects.
            if let Some(cancel) = inflight
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id)
            {
                let _ = cancel.send(());
            }
        }
        // Judged, and acted on, at the loop level in `serve` — the ONLY
        // frame that can end the connection, which this function has no way
        // to do. Reaching here at all means the version was already found
        // compatible (or this is an unexpected second `welcome`, which is
        // not a command either way): nothing to do.
        HubFrame::Welcome { .. } => {}
        HubFrame::Exec {
            id,
            argv,
            stdin,
            timeout_ms,
            cap_bytes,
        } => {
            if !agent.first_sight(&id) {
                refuse_duplicate(&id);
                return;
            }
            if *agent.stopping.borrow() {
                let refused = exec::ExecOutcome {
                    exit_code: -9,
                    stdout: Vec::new(),
                    stderr: b"the agent is stopping".to_vec(),
                    truncated: false,
                };
                let _ = out.send(Out::Frame(result_frame(id, refused, cap_bytes)));
                return;
            }
            // Counted before the task is spawned, so a `stop` that races it
            // still waits for this child.
            agent.running.send_modify(|n| *n += 1);
            let running = Running(Arc::clone(agent));
            let (cancel, cancelled) = oneshot::channel();
            inflight
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id.clone(), cancel);
            let request = ExecRequest {
                argv,
                stdin,
                timeout: Duration::from_millis(timeout_ms),
                limits: result_stream_limits(cap_bytes),
                cwd: agent.home.clone(),
            };
            let (agent, out, inflight) = (Arc::clone(agent), out.clone(), Arc::clone(inflight));
            tokio::spawn(async move {
                let _running = running;
                // Resolves on a cancel, when the handle is dropped because
                // the connection went, or when the agent is stopping.
                let stopped = agent.stopped();
                let cancelled = async move {
                    tokio::select! {
                        _ = cancelled => {}
                        () = stopped => {}
                    }
                };
                tokio::pin!(cancelled);
                let outcome = tokio::select! {
                    permit = Arc::clone(&agent.slots).acquire_owned() => {
                        let _permit = permit;
                        exec::execute(request, cancelled).await
                    }
                    () = &mut cancelled => exec::ExecOutcome {
                        exit_code: -9,
                        stdout: Vec::new(),
                        stderr: b"cancelled before it started".to_vec(),
                        truncated: false,
                    },
                };
                inflight
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                let _ = out.send(Out::Frame(result_frame(id, outcome, cap_bytes)));
            });
        }
        HubFrame::Upload {
            id,
            path,
            mode,
            bytes_b64,
        } => {
            if !agent.first_sight(&id) {
                refuse_duplicate(&id);
                return;
            }
            let (agent, out) = (Arc::clone(agent), out.clone());
            tokio::spawn(async move {
                let _permit = Arc::clone(&agent.slots).acquire_owned().await;
                let home = agent.home.clone().unwrap_or_default();
                let written = tokio::task::spawn_blocking(move || {
                    let target = exec::resolve_path(&home, &path);
                    let bytes = decode_b64(&bytes_b64).map_err(|e| format!("{path}: {e}"))?;
                    exec::write_upload(&target, mode, &bytes)
                        .map_err(|e| format!("{}: {e}", target.display()))
                })
                .await
                .unwrap_or_else(|e| Err(format!("the upload task failed: {e}")));
                let outcome = match written {
                    Ok(()) => exec::ExecOutcome {
                        exit_code: 0,
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                        truncated: false,
                    },
                    Err(why) => exec::ExecOutcome {
                        exit_code: 1,
                        stdout: Vec::new(),
                        stderr: why.into_bytes(),
                        truncated: false,
                    },
                };
                // An upload's answer is empty or one error line; the hub
                // judges it against its floor, which a smallest cap fits.
                let _ = out.send(Out::Frame(result_frame(id, outcome, Some(0))));
            });
        }
    }
}

/// A replayed id runs nothing and answers nothing. An answer would carry the
/// same id, and the hub would hand it to whichever caller holds that id now.
fn refuse_duplicate(id: &str) {
    tracing::warn!(id, "[agent] refusing a request id already seen");
}

/// Encode a `result` that fits the budget the hub will decode it against.
/// `execute` already truncated to the matching limits, so the fallback only
/// fires for an absurd id.
fn result_frame(id: String, outcome: exec::ExecOutcome, cap_bytes: Option<u64>) -> String {
    let frame = AgentFrame::Result {
        id: id.clone(),
        exit_code: outcome.exit_code,
        stdout_b64: encode_b64(&outcome.stdout),
        stderr_b64: encode_b64(&outcome.stderr),
        truncated: outcome.truncated,
    };
    encode_agent_frame_within(&frame, result_budget(cap_bytes)).unwrap_or_else(|e| {
        let fallback = AgentFrame::Result {
            id,
            exit_code: -1,
            stdout_b64: String::new(),
            stderr_b64: encode_b64(format!("the answer did not fit: {e}").as_bytes()),
            truncated: true,
        };
        encode_agent_frame(&fallback).expect("an empty result fits")
    })
}

/// This machine's name, for `hello`.
fn host_name() -> String {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        // SAFETY: the buffer and its length are this function's own.
        let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
        if rc == 0 {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            let name = String::from_utf8_lossy(&buf[..end]).into_owned();
            if !name.is_empty() {
                return name;
            }
        }
    }
    "unknown".to_string()
}

/// Capped exponential backoff with jitter.
#[derive(Debug, Default)]
pub struct Backoff {
    attempt: u32,
}

impl Backoff {
    /// The next delay. `jitter` in `[0, 1)` picks a point in the upper half of
    /// the current step, so dials never bunch at zero and a fleet of agents
    /// restarted together does not dial the hub in lockstep.
    pub fn next(&mut self, jitter: f64) -> Duration {
        let step = BACKOFF_BASE
            .checked_mul(1u32 << self.attempt.min(16))
            .unwrap_or(BACKOFF_CAP)
            .min(BACKOFF_CAP);
        self.attempt = self.attempt.saturating_add(1);
        let half = step / 2;
        half + half.mul_f64(jitter.clamp(0.0, 1.0))
    }

    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    /// Jump straight to [`BACKOFF_CAP`] — for a version refusal, where
    /// retrying sooner cannot help: only a hub upgrade (or an agent one)
    /// fixes it, and the agent should be reachable for that without
    /// hammering a hub that has already said no. Every following delay
    /// (`next`'s `attempt` only ever grows) stays at the cap too, until an
    /// ordinary reconnect calls [`Backoff::reset`].
    pub fn force_max(&mut self) {
        // Whatever step makes `next`'s `1u32 << attempt.min(16)` overflow
        // `BACKOFF_CAP` on its own; `next` still clamps with `.min`, so this
        // only has to be big enough, not exact.
        self.attempt = self.attempt.max(6);
    }
}

/// A number in `[0, 1)` that differs between processes and between calls:
/// std's per-process random hash keys, fed a counter. No crate needed for a
/// reconnect delay.
pub fn jitter() -> f64 {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static CALLS: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(CALLS.fetch_add(1, Ordering::Relaxed));
    (h.finish() >> 11) as f64 / (1u64 << 53) as f64
}

/// Reports to systemd what the agent is doing (`sd_notify`'s `STATUS=`), which
/// `status` reads back through `systemctl show`. The agent keeps no state
/// file: this is how "is it connected" is answered.
pub struct Notifier {
    socket: Option<PathBuf>,
}

impl Notifier {
    /// The socket systemd named in `$NOTIFY_SOCKET`, if any.
    pub fn from_env() -> Self {
        Self {
            socket: std::env::var_os("NOTIFY_SOCKET").map(PathBuf::from),
        }
    }

    pub fn at(socket: Option<PathBuf>) -> Self {
        Self { socket }
    }

    /// Best effort: a failed report must never stop the agent.
    pub fn status(&self, text: &str) {
        #[cfg(unix)]
        if let Some(socket) = &self.socket {
            let _ = send_notify(
                socket,
                format!("STATUS={}\n", text.replace('\n', " ")).as_bytes(),
            );
        }
        #[cfg(not(unix))]
        let _ = text;
    }
}

#[cfg(unix)]
fn send_notify(socket: &Path, message: &[u8]) -> std::io::Result<()> {
    let sock = std::os::unix::net::UnixDatagram::unbound()?;
    // systemd usually names an abstract socket, spelled with a leading `@`.
    #[cfg(target_os = "linux")]
    if let Some(name) = socket
        .as_os_str()
        .to_str()
        .and_then(|s| s.strip_prefix('@'))
    {
        use std::os::linux::net::SocketAddrExt;
        let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())?;
        sock.send_to_addr(message, &addr)?;
        return Ok(());
    }
    sock.send_to(message, socket)?;
    Ok(())
}

/// A wall-clock time for a status line, in UTC.
fn utc(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let secs = unix_secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

fn now_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    utc(secs)
}

/// Dial, serve, and dial again, forever. `beats` makes each connection's beat
/// source and `sleep` waits out a backoff delay — both injected so a test can
/// drive reconnection without a clock.
pub async fn run_with<B, F, Fut>(
    dialer: &Dialer,
    agent: &Arc<Agent>,
    notifier: &Notifier,
    mut beats: B,
    mut sleep: F,
) -> std::convert::Infallible
where
    B: FnMut() -> Beats,
    F: FnMut(Duration) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let url = dialer.endpoint().url().to_string();
    let mut backoff = Backoff::default();
    loop {
        notifier.status(&format!("connecting to {url}"));
        let (why, version_refused) = match dialer.dial().await {
            Ok(ws) => {
                tracing::info!(hub = %url, "[agent] connected");
                notifier.status(&format!("{CONNECTED} {url} since {}", now_utc()));
                let served = serve(ws, agent, beats()).await;
                let version_refused = matches!(served.end, SessionEnd::VersionRefused(_));
                if served.heard_any && !version_refused {
                    backoff.reset();
                }
                (served.end.to_string(), version_refused)
            }
            Err(e) => (e.to_string(), false),
        };
        if version_refused {
            // Not the normal growing sequence from the start: a version
            // mismatch does not heal by retrying sooner, only by a hub (or
            // agent) upgrade, so jump straight to the slowest interval and
            // stay there — quietly enough not to spam, but still reachable
            // once the fix lands.
            backoff.force_max();
            tracing::error!(hub = %url, "[agent] {why}");
        }
        let delay = backoff.next(jitter());
        if !version_refused {
            tracing::warn!(hub = %url, "[agent] {why}; dialling again in {:.1}s", delay.as_secs_f64());
        }
        notifier.status(&format!(
            "reconnecting in {}s: {why}",
            delay.as_secs_f64().ceil()
        ));
        sleep(delay).await;
    }
}

/// `fleet-agent run`: serve `config`'s hub until SIGTERM or SIGINT, then kill
/// whatever is still running and return.
pub async fn run(config: crate::config::Config) -> Result<(), String> {
    let endpoint = Endpoint::parse(&config.hub, config.insecure)?;
    let dialer = Dialer::new(endpoint, config.token, config.ca_file.as_deref())?;
    let agent = Agent::new(
        std::env::var_os("HOME").map(PathBuf::from),
        exec::MAX_CONCURRENT,
    );
    let notifier = Notifier::from_env();
    let stop = shutdown_signal()?;
    tokio::select! {
        never = run_with(&dialer, &agent, &notifier, Beats::heartbeat, tokio::time::sleep) => match never {},
        () = stop => {}
    }
    tracing::info!("[agent] stopping: killing what is still running");
    notifier.status("stopping");
    // Bounded well inside systemd's own 90 s stop timeout.
    if tokio::time::timeout(Duration::from_secs(10), agent.stop())
        .await
        .is_err()
    {
        tracing::warn!("[agent] children still running after 10 s; exiting anyway");
    }
    Ok(())
}

/// Resolves on SIGTERM (systemd's stop) or SIGINT (a terminal's Ctrl-C).
/// Installed before serving starts, so a stop that arrives early is not lost.
fn shutdown_signal() -> Result<impl std::future::Future<Output = ()>, String> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term =
            signal(SignalKind::terminate()).map_err(|e| format!("SIGTERM handler: {e}"))?;
        let mut int =
            signal(SignalKind::interrupt()).map_err(|e| format!("SIGINT handler: {e}"))?;
        Ok(async move {
            tokio::select! {
                _ = term.recv() => {}
                _ = int.recv() => {}
            }
        })
    }
    #[cfg(not(unix))]
    {
        Ok(async {
            let _ = tokio::signal::ctrl_c().await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{alive, wait_for_pid};
    use fleet_proto::{
        decode_agent_frame, decode_b64, encode_b64, encode_hub_frame, result_budget,
        result_stream_limits, AgentFrame, HubFrame,
    };
    use futures_util::{SinkExt, StreamExt};
    use std::net::{Ipv4Addr, SocketAddr};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_tungstenite::tungstenite::handshake::server::{
        ErrorResponse, Request as HsRequest, Response as HsResponse,
    };
    use tokio_tungstenite::tungstenite::protocol::{Message, WebSocketConfig};

    // No test here waits on a clock. Beats are fired by hand, backoff delays
    // are recorded instead of slept, and every real-clock bound below is only
    // how long to wait before calling a test FAILED.

    const PATIENCE: Duration = Duration::from_secs(10);
    const TOKEN: &str = "laptop-host-token";

    fn big_config() -> WebSocketConfig {
        WebSocketConfig::default()
            .max_message_size(Some(fleet_proto::MAX_FRAME_BYTES))
            .max_frame_size(Some(fleet_proto::MAX_FRAME_BYTES))
    }

    /// What the hub side of a test saw of the upgrade request.
    #[derive(Debug, Clone, Default)]
    struct Seen {
        path: String,
        authorization: Option<String>,
    }

    /// A stand-in hub: a real listener speaking real WebSocket, driven by the
    /// test frame by frame.
    struct FakeHub {
        listener: TcpListener,
        addr: SocketAddr,
    }

    type HubSide = WebSocketStream<TcpStream>;

    // The handshake callbacks' `Result` type is tungstenite's, not ours.
    #[allow(clippy::result_large_err)]
    impl FakeHub {
        async fn new() -> Self {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
            let addr = listener.local_addr().unwrap();
            Self { listener, addr }
        }

        fn dialer(&self) -> Dialer {
            let ep = Endpoint::parse(&format!("ws://{}", self.addr), true).unwrap();
            Dialer::new(ep, TOKEN.into(), None).unwrap()
        }

        /// Accept one upgrade, recording what it asked for.
        async fn accept(&self) -> (HubSide, Seen) {
            let (tcp, _) = tokio::time::timeout(PATIENCE, self.listener.accept())
                .await
                .expect("the agent dialled")
                .unwrap();
            let seen = Arc::new(Mutex::new(Seen::default()));
            let record = Arc::clone(&seen);
            let cb = move |req: &HsRequest, resp: HsResponse| {
                let mut s = record.lock().unwrap();
                s.path = req.uri().path().to_string();
                s.authorization = req
                    .headers()
                    .get("authorization")
                    .map(|v| v.to_str().unwrap().to_string());
                Ok(resp)
            };
            let ws = tokio_tungstenite::accept_hdr_async_with_config(tcp, cb, Some(big_config()))
                .await
                .unwrap();
            let seen = seen.lock().unwrap().clone();
            (ws, seen)
        }

        /// Answer one upgrade with an HTTP error instead.
        async fn refuse(&self, status: u16, body: &str) {
            let (tcp, _) = tokio::time::timeout(PATIENCE, self.listener.accept())
                .await
                .expect("the agent dialled")
                .unwrap();
            let body = body.to_string();
            let cb = move |_: &HsRequest, _: HsResponse| -> Result<HsResponse, ErrorResponse> {
                let mut e = ErrorResponse::new(Some(body));
                *e.status_mut() =
                    tokio_tungstenite::tungstenite::http::StatusCode::from_u16(status).unwrap();
                Err(e)
            };
            let _ = tokio_tungstenite::accept_hdr_async(tcp, cb).await;
        }
    }

    /// The next agent frame, or `None` once the agent closed the socket.
    async fn next_frame(ws: &mut HubSide) -> Option<(AgentFrame, usize)> {
        loop {
            match tokio::time::timeout(PATIENCE, ws.next())
                .await
                .expect("a frame in time")
            {
                Some(Ok(Message::Text(t))) => {
                    return Some((decode_agent_frame(&t).expect("an agent frame"), t.len()))
                }
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => continue,
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return None,
                Some(Ok(other)) => panic!("unexpected message {other:?}"),
            }
        }
    }

    async fn send(ws: &mut HubSide, frame: &HubFrame) {
        ws.send(Message::Text(encode_hub_frame(frame).unwrap().into()))
            .await
            .unwrap();
    }

    fn exec(id: &str, argv: &[&str], cap: Option<u64>) -> HubFrame {
        HubFrame::Exec {
            id: id.into(),
            argv: argv.iter().map(|s| s.to_string()).collect(),
            stdin: None,
            timeout_ms: 60_000,
            cap_bytes: cap,
        }
    }

    /// The result for `id`, skipping anything else.
    async fn result_for(ws: &mut HubSide, id: &str) -> (i32, Vec<u8>, Vec<u8>, bool, usize) {
        loop {
            match next_frame(ws).await.expect("the socket is open") {
                (
                    AgentFrame::Result {
                        id: got,
                        exit_code,
                        stdout_b64,
                        stderr_b64,
                        truncated,
                    },
                    len,
                ) if got == id => {
                    return (
                        exit_code,
                        decode_b64(&stdout_b64).unwrap(),
                        decode_b64(&stderr_b64).unwrap(),
                        truncated,
                        len,
                    )
                }
                _ => continue,
            }
        }
    }

    /// A connected pair: the hub's side, the agent's `serve` running, and the
    /// hand that fires its beats.
    struct Pair {
        hub: HubSide,
        seen: Seen,
        served: tokio::task::JoinHandle<Served>,
        beats: mpsc::UnboundedSender<oneshot::Sender<bool>>,
    }

    impl Pair {
        /// One beat; whether the connection survived it.
        async fn beat(&self) -> bool {
            let (tx, rx) = oneshot::channel();
            self.beats.send(tx).unwrap();
            tokio::time::timeout(PATIENCE, rx)
                .await
                .unwrap()
                .unwrap_or(false)
        }
    }

    /// A `welcome` naming a compatible, current `PROTO_VERSION` — what a
    /// real hub built from this same crate sends. Every test using [`pair`]/
    /// [`pair_with`] gets one automatically, so it exercises the SAME
    /// must-see-welcome-first gate every real connection does, without every
    /// other test in this file needing to know that gate exists.
    fn compatible_welcome() -> HubFrame {
        HubFrame::Welcome {
            hub_version: "0.9.0".into(),
            proto: fleet_proto::PROTO_VERSION,
        }
    }

    async fn pair_with(agent: Arc<Agent>) -> Pair {
        let fake = FakeHub::new().await;
        let dialer = fake.dialer();
        let (beats, rx) = mpsc::unbounded_channel();
        let served = tokio::spawn(async move {
            let ws = dialer.dial().await.expect("the dial");
            serve(ws, &agent, Beats::Manual(rx)).await
        });
        let (mut hub, seen) = fake.accept().await;
        match next_frame(&mut hub).await {
            Some((AgentFrame::Hello { .. }, _)) => {}
            other => panic!("the first frame must be hello, got {other:?}"),
        }
        send(&mut hub, &compatible_welcome()).await;
        // A barrier: the agent's `serve` loop processes one message at a
        // time in order, so a `pong` for THIS ping can only come after the
        // welcome just sent was already acted on (`welcomed = true`) —
        // without this, a test that fires its first beat immediately after
        // `pair_with` returns could race the agent still processing the
        // welcome, and see a beat with no welcome yet as a version refusal.
        send(
            &mut hub,
            &HubFrame::Ping {
                id: "pair-barrier".into(),
            },
        )
        .await;
        match next_frame(&mut hub).await {
            Some((AgentFrame::Pong { id }, _)) if id == "pair-barrier" => {}
            other => panic!("expected the barrier's pong, got {other:?}"),
        }
        Pair {
            hub,
            seen,
            served,
            beats,
        }
    }

    async fn pair() -> Pair {
        pair_with(Agent::new(None, crate::exec::MAX_CONCURRENT)).await
    }

    /// [`pair_with`], but the fake hub never sends `welcome` — what a hub
    /// built before this protocol version exists does. Only for the tests
    /// about that gate itself; everything else should use [`pair`].
    async fn pair_before_welcome(agent: Arc<Agent>) -> Pair {
        let fake = FakeHub::new().await;
        let dialer = fake.dialer();
        let (beats, rx) = mpsc::unbounded_channel();
        let served = tokio::spawn(async move {
            let ws = dialer.dial().await.expect("the dial");
            serve(ws, &agent, Beats::Manual(rx)).await
        });
        let (mut hub, seen) = fake.accept().await;
        match next_frame(&mut hub).await {
            Some((AgentFrame::Hello { .. }, _)) => {}
            other => panic!("the first frame must be hello, got {other:?}"),
        }
        Pair {
            hub,
            seen,
            served,
            beats,
        }
    }

    // ── the endpoint ───────────────────────────────────────────────────────

    #[test]
    fn a_hub_url_becomes_its_agent_websocket_url() {
        let cases = [
            ("https://hub.example", "wss://hub.example/agent", 443),
            ("https://hub.example/", "wss://hub.example/agent", 443),
            (
                "https://hub.example:8443",
                "wss://hub.example:8443/agent",
                8443,
            ),
            ("wss://hub.example/agent", "wss://hub.example/agent", 443),
            (
                "https://example.com/fleet",
                "wss://example.com/fleet/agent",
                443,
            ),
        ];
        for (hub, want, port) in cases {
            let ep = Endpoint::parse(hub, false).unwrap_or_else(|e| panic!("{hub}: {e}"));
            assert_eq!(ep.url(), want, "{hub}");
            assert_eq!(ep.port, port, "{hub}");
            assert!(ep.is_tls());
        }
    }

    /// The design's rule: a plain hub is refused unless the operator says
    /// `--insecure`, which the docs reserve for a loopback test.
    #[test]
    fn a_plain_hub_is_refused_without_insecure() {
        for hub in ["ws://hub.example", "http://hub.example"] {
            let err = Endpoint::parse(hub, false).unwrap_err();
            assert!(err.contains("--insecure"), "{hub}: {err}");
        }
        let ep = Endpoint::parse("http://127.0.0.1:7777", true).unwrap();
        assert_eq!(ep.url(), "ws://127.0.0.1:7777/agent");
        assert_eq!(ep.port, 7777);
        assert!(!ep.is_tls());
    }

    /// `--insecure` is for a loopback test, as the design reserves it: a
    /// plain hub anywhere else would carry the token across a network.
    #[test]
    fn insecure_is_confined_to_loopback() {
        for ok in [
            "http://127.0.0.1:7777",
            "ws://localhost",
            "http://[::1]:9",
            "ws://127.8.9.10",
        ] {
            assert!(Endpoint::parse(ok, true).is_ok(), "{ok}");
        }
        for far in [
            "http://10.0.0.5",
            "ws://hub.example",
            "http://[fe80::1]:9",
            "http://127.0.0.1.example",
        ] {
            let err = Endpoint::parse(far, true).unwrap_err();
            assert!(err.contains("loopback"), "{far}: {err}");
        }
    }

    #[test]
    fn a_url_that_is_not_a_hub_is_refused() {
        for hub in [
            "",
            "hub.example",
            "ftp://hub.example",
            "https://",
            "https://hub.example?x=1",
        ] {
            assert!(Endpoint::parse(hub, true).is_err(), "{hub:?}");
        }
    }

    // ── the dial ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn the_dial_carries_the_token_to_slash_agent() {
        let p = pair().await;
        assert_eq!(p.seen.path, "/agent");
        assert_eq!(
            p.seen.authorization.as_deref(),
            Some("Bearer laptop-host-token")
        );
    }

    #[tokio::test]
    async fn a_refused_dial_reports_the_status_and_the_hub_s_reason() {
        let fake = FakeHub::new().await;
        let dialer = fake.dialer();
        let dial = tokio::spawn(async move { dialer.dial().await });
        fake.refuse(403, "this host's token is readonly").await;
        match dial.await.unwrap() {
            Err(DialError::Refused { status, body }) => {
                assert_eq!(status, 403);
                assert_eq!(body, "this host's token is readonly");
            }
            other => panic!("expected a refusal, got {:?}", other.map(|_| ())),
        }
    }

    #[tokio::test]
    async fn the_hello_names_this_agent() {
        let fake = FakeHub::new().await;
        let dialer = fake.dialer();
        let agent = Agent::new(None, 1);
        let (_beats, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let ws = dialer.dial().await.unwrap();
            serve(ws, &agent, Beats::Manual(rx)).await
        });
        let (mut hub, _) = fake.accept().await;
        match next_frame(&mut hub).await {
            Some((
                AgentFrame::Hello {
                    agent_version,
                    host_name,
                    os,
                    proto,
                },
                _,
            )) => {
                assert_eq!(agent_version, env!("CARGO_PKG_VERSION"));
                assert!(!host_name.is_empty());
                assert_eq!(os, std::env::consts::OS);
                assert_eq!(proto, fleet_proto::PROTO_VERSION);
            }
            other => panic!("expected hello, got {other:?}"),
        }
    }

    // ── serving ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn an_exec_runs_and_its_result_comes_back() {
        let mut p = pair().await;
        send(&mut p.hub, &exec("e1", &["bash", "-c", "echo hello"], None)).await;
        let (code, out, err, truncated, _) = result_for(&mut p.hub, "e1").await;
        assert_eq!(
            (code, out.as_slice(), err.as_slice(), truncated),
            (0, &b"hello\n"[..], &b""[..], false)
        );
    }

    /// The answer honours the cap the request carried, and fits the budget
    /// the hub decodes it against — both streams full is the worst case.
    #[tokio::test]
    async fn a_capped_answer_is_truncated_and_fits_the_hub_s_budget() {
        let mut p = pair().await;
        let cap = 100;
        send(
            &mut p.hub,
            &exec(
                "e2",
                &[
                    "bash",
                    "-c",
                    "head -c 5000 /dev/zero; head -c 5000 /dev/zero >&2",
                ],
                Some(cap),
            ),
        )
        .await;
        let (code, out, err, truncated, len) = result_for(&mut p.hub, "e2").await;
        assert_eq!(code, 0);
        assert_eq!(out.len(), cap as usize);
        assert_eq!(err.len(), cap as usize);
        assert!(truncated);
        assert!(
            len <= result_budget(Some(cap)),
            "{len} > {}",
            result_budget(Some(cap))
        );
        assert_eq!(result_stream_limits(Some(cap)).per_stream, cap as usize);
    }

    #[tokio::test]
    async fn a_ping_is_answered_with_a_pong_carrying_its_id() {
        let mut p = pair().await;
        send(&mut p.hub, &HubFrame::Ping { id: "p1".into() }).await;
        assert_eq!(
            next_frame(&mut p.hub).await.map(|f| f.0),
            Some(AgentFrame::Pong { id: "p1".into() })
        );
    }

    /// A replayed id runs nothing and answers nothing: the first run's answer
    /// is the only one, and answering the replay under the same id could hand
    /// it to the wrong caller.
    #[tokio::test]
    async fn a_duplicate_request_id_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("runs");
        let script = format!("echo ran >> {}", log.display());
        // One slot, on this single-threaded runtime: requests run in the
        // order they arrived, so "later" finishing proves the replay was
        // either run before it or refused.
        let mut p = pair_with(Agent::new(None, 1)).await;
        send(&mut p.hub, &exec("dup", &["bash", "-c", &script], None)).await;
        result_for(&mut p.hub, "dup").await;
        send(&mut p.hub, &exec("dup", &["bash", "-c", &script], None)).await;
        send(&mut p.hub, &exec("later", &["true"], None)).await;
        // Nothing but "later"'s answer may arrive.
        match next_frame(&mut p.hub).await {
            Some((AgentFrame::Result { id, .. }, _)) => assert_eq!(id, "later"),
            other => panic!("expected later's result, got {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "ran\n",
            "the replay ran"
        );
    }

    #[tokio::test]
    async fn cancel_kills_an_in_flight_exec_and_it_still_answers() {
        let mut p = pair().await;
        send(&mut p.hub, &exec("slow", &["sleep", "1000"], None)).await;
        send(&mut p.hub, &HubFrame::Cancel { id: "slow".into() }).await;
        let (code, ..) = result_for(&mut p.hub, "slow").await;
        assert_eq!(code, -9);
    }

    #[tokio::test]
    async fn an_upload_writes_the_file_with_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/token");
        let mut p = pair().await;
        send(
            &mut p.hub,
            &HubFrame::Upload {
                id: "u1".into(),
                path: path.to_str().unwrap().into(),
                mode: 0o600,
                bytes_b64: encode_b64(b"secret"),
            },
        )
        .await;
        let (code, ..) = result_for(&mut p.hub, "u1").await;
        assert_eq!(code, 0);
        assert_eq!(std::fs::read(&path).unwrap(), b"secret");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    /// The hub writes a whole message as ONE frame, and a transcript is far
    /// past tungstenite's 16 MiB default frame limit; the agent's socket must
    /// admit it.
    #[tokio::test]
    async fn an_upload_over_tungstenite_s_default_frame_size_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript.jsonl");
        let body = vec![b'x'; 17 * 1024 * 1024];
        let mut p = pair().await;
        send(
            &mut p.hub,
            &HubFrame::Upload {
                id: "big".into(),
                path: path.to_str().unwrap().into(),
                mode: 0o600,
                bytes_b64: encode_b64(&body),
            },
        )
        .await;
        let (code, _, err, ..) = result_for(&mut p.hub, "big").await;
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&err));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), body.len() as u64);
    }

    /// A replayed upload writes nothing either.
    #[tokio::test]
    async fn a_duplicate_upload_id_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        let upload = |body: &[u8]| HubFrame::Upload {
            id: "up".into(),
            path: path.to_str().unwrap().into(),
            mode: 0o600,
            bytes_b64: encode_b64(body),
        };
        // One slot on this single-threaded runtime: see the exec twin above.
        let mut p = pair_with(Agent::new(None, 1)).await;
        send(&mut p.hub, &upload(b"first")).await;
        result_for(&mut p.hub, "up").await;
        send(&mut p.hub, &upload(b"replayed")).await;
        send(&mut p.hub, &exec("later", &["true"], None)).await;
        match next_frame(&mut p.hub).await {
            Some((AgentFrame::Result { id, .. }, _)) => assert_eq!(id, "later"),
            other => panic!("expected later's result, got {other:?}"),
        }
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
    }

    #[tokio::test]
    async fn a_failed_upload_answers_nonzero_with_the_reason() {
        let mut p = pair().await;
        send(
            &mut p.hub,
            &HubFrame::Upload {
                id: "u2".into(),
                path: "/proc/fleet-agent-cannot-write-here/x".into(),
                mode: 0o644,
                bytes_b64: encode_b64(b"x"),
            },
        )
        .await;
        let (code, _, err, ..) = result_for(&mut p.hub, "u2").await;
        assert_ne!(code, 0);
        assert!(!err.is_empty());
    }

    /// Once past the handshake, a `kind` this build does not know is
    /// skipped, not fatal — the connection stays open, and the NEXT, real
    /// frame is still answered.
    #[tokio::test]
    async fn an_unknown_kind_is_skipped_not_fatal() {
        let mut p = pair().await;
        p.hub
            .send(Message::Text(r#"{"kind":"launch_missiles"}"#.into()))
            .await
            .unwrap();
        send(
            &mut p.hub,
            &HubFrame::Ping {
                id: "still-alive".into(),
            },
        )
        .await;
        match next_frame(&mut p.hub).await {
            Some((AgentFrame::Pong { id }, _)) => assert_eq!(id, "still-alive"),
            other => panic!("expected a pong after the unknown frame, got {other:?}"),
        }
    }

    /// A `kind` the agent DOES know, but whose body will not parse, is
    /// corruption, not evolution: it still closes the connection.
    #[tokio::test]
    async fn a_known_kind_that_will_not_parse_still_closes_the_connection() {
        let mut p = pair().await;
        p.hub
            .send(Message::Text(r#"{"kind":"exec","id":"1"}"#.into()))
            .await
            .unwrap();
        assert!(
            next_frame(&mut p.hub).await.is_none(),
            "the agent closed it"
        );
        let served = tokio::time::timeout(PATIENCE, p.served)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(served.end, SessionEnd::Protocol(_)), "{served:?}");
    }

    // ── liveness ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_silent_hub_is_given_up_on_after_three_beats() {
        let mut p = pair().await;
        // `pair`'s own setup already exchanged a ping/pong (its welcome
        // barrier), which counts as life the same as any other frame does —
        // one beat's worth of "heard" is still outstanding from it. Spend
        // that beat explicitly, so the THREE that follow are unambiguously
        // silent, the same count the connection has always needed.
        assert!(p.beat().await, "still riding the barrier's own life signal");
        assert!(p.beat().await, "one silent beat");
        assert!(p.beat().await, "two silent beats");
        assert!(!p.beat().await, "three: the connection goes");
        assert!(
            next_frame(&mut p.hub).await.is_none(),
            "and the socket closes"
        );
        let served = tokio::time::timeout(PATIENCE, p.served)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(served.end, SessionEnd::HubSilent);
        // Unlike before this task, `heard_any` can no longer be false for a
        // connection that reached `pair()` at all: a compliant hub always
        // welcomes it first, and that alone is something heard.
        assert!(served.heard_any);
    }

    #[tokio::test]
    async fn any_frame_from_the_hub_resets_the_count() {
        let mut p = pair().await;
        assert!(p.beat().await);
        assert!(p.beat().await);
        send(&mut p.hub, &HubFrame::Ping { id: "alive".into() }).await;
        next_frame(&mut p.hub).await.expect("the pong");
        assert!(
            p.beat().await,
            "heard since the last beat: the count restarts"
        );
        assert!(p.beat().await, "one silent");
        assert!(p.beat().await, "two silent");
        assert!(!p.beat().await, "three silent");
        let served = tokio::time::timeout(PATIENCE, p.served)
            .await
            .unwrap()
            .unwrap();
        assert!(served.heard_any);
    }

    /// Children of a connection that went are killed: their answers could
    /// only reach a connection that never asked.
    #[tokio::test]
    async fn a_lost_connection_kills_its_in_flight_children() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let script = format!("echo $$ > {}; exec sleep 1000", pidfile.display());
        let mut p = pair().await;
        send(&mut p.hub, &exec("orphan", &["bash", "-c", &script], None)).await;
        let pid = wait_for_pid(&pidfile).await;
        p.hub.close(None).await.unwrap();
        drop(p.hub);
        tokio::time::timeout(PATIENCE, p.served)
            .await
            .unwrap()
            .unwrap();
        let deadline = std::time::Instant::now() + PATIENCE;
        while alive(pid) {
            assert!(
                std::time::Instant::now() < deadline,
                "{pid} outlived its connection"
            );
            tokio::task::yield_now().await;
        }
    }

    /// Stopping the agent — systemd's SIGTERM on `stop` or `restart` — kills
    /// what it is running. The unit's `KillMode=process` leaves children
    /// alone (so tmux servers, which `setsid` themselves, survive), so
    /// nothing else would: a hung child would run, and bill, for ever.
    #[tokio::test]
    async fn stopping_the_agent_kills_its_running_children() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let script = format!("echo $$ > {}; exec sleep 1000", pidfile.display());
        let agent = Agent::new(None, 4);
        let mut p = pair_with(Arc::clone(&agent)).await;
        send(&mut p.hub, &exec("hung", &["bash", "-c", &script], None)).await;
        let pid = wait_for_pid(&pidfile).await;

        tokio::time::timeout(PATIENCE, agent.stop())
            .await
            .expect("stop returns once its children are gone");
        assert!(!alive(pid), "{pid} outlived the agent's stop");

        // And nothing new starts once it is stopping: refused before a child
        // is spawned, saying why — not spawned and killed at once, which
        // would let a command start to act.
        send(&mut p.hub, &exec("late", &["true"], None)).await;
        let (code, _, err, ..) = result_for(&mut p.hub, "late").await;
        assert_eq!(code, -9);
        assert_eq!(String::from_utf8_lossy(&err), "the agent is stopping");
    }

    // ── reconnecting ───────────────────────────────────────────────────────

    #[test]
    fn the_backoff_doubles_to_its_cap_and_stays_in_the_upper_half_of_each_step() {
        let mut b = Backoff::default();
        let mut step = BACKOFF_BASE;
        for _ in 0..12 {
            let low = b.next(0.0);
            // Fresh attempt at the same step for the high end.
            b.attempt -= 1;
            let high = b.next(0.999_999);
            assert_eq!(low, step / 2, "the floor of the step");
            assert!(high <= step && high > step * 9 / 10, "{high:?} vs {step:?}");
            step = (step * 2).min(BACKOFF_CAP);
        }
        assert_eq!(step, BACKOFF_CAP);
        b.reset();
        assert_eq!(b.next(0.0), BACKOFF_BASE / 2);
    }

    #[test]
    fn jitter_is_in_range_and_varies() {
        let draws: Vec<f64> = (0..64).map(|_| jitter()).collect();
        assert!(draws.iter().all(|j| (0.0..1.0).contains(j)), "{draws:?}");
        assert!(draws.windows(2).any(|w| w[0] != w[1]), "{draws:?}");
    }

    /// A lost connection is dialled again, after a recorded (not slept)
    /// backoff; refusals push the delay up, and a connection the hub
    /// accepted starts it over.
    #[tokio::test]
    async fn a_lost_connection_is_dialled_again_with_a_growing_backoff() {
        let fake = FakeHub::new().await;
        let dialer = fake.dialer();
        let agent = Agent::new(None, 1);
        let delays = Arc::new(Mutex::new(Vec::<Duration>::new()));
        let record = Arc::clone(&delays);
        let (beat_keep, _) = mpsc::unbounded_channel::<oneshot::Sender<bool>>();
        let run = tokio::spawn(async move {
            let notifier = Notifier::at(None);
            run_with(
                &dialer,
                &agent,
                &notifier,
                || {
                    // Never fed: the connection only ends when the hub ends it.
                    let (_tx, rx) = mpsc::unbounded_channel();
                    std::mem::forget(_tx);
                    Beats::Manual(rx)
                },
                move |d| {
                    record.lock().unwrap().push(d);
                    std::future::ready(())
                },
            )
            .await
        });

        // Refused once: the backoff starts to grow.
        fake.refuse(401, "unknown token").await;
        // Accepted, heard from (a ping), then dropped by the hub: that
        // connection was a success, so the backoff starts over.
        let (mut ws, _) = fake.accept().await;
        assert!(matches!(
            next_frame(&mut ws).await,
            Some((AgentFrame::Hello { .. }, _))
        ));
        send(&mut ws, &compatible_welcome()).await;
        send(&mut ws, &HubFrame::Ping { id: "x".into() }).await;
        next_frame(&mut ws).await.expect("pong");
        drop(ws);
        // Refused twice: it grows again, from the bottom.
        fake.refuse(401, "unknown token").await;
        fake.refuse(401, "unknown token").await;
        // And accepted again.
        let (mut ws, _) = fake.accept().await;
        assert!(
            matches!(
                next_frame(&mut ws).await,
                Some((AgentFrame::Hello { .. }, _))
            ),
            "a hello on the new connection"
        );
        run.abort();
        drop(beat_keep);

        let d = delays.lock().unwrap().clone();
        assert_eq!(d.len(), 4, "{d:?}");
        assert!(d[0] <= BACKOFF_BASE, "the first step: {d:?}");
        assert!(
            d[1] <= BACKOFF_BASE,
            "after a connection the hub accepted, the first step again: {d:?}"
        );
        assert!(d[2] >= BACKOFF_BASE && d[2] <= BACKOFF_BASE * 2, "{d:?}");
        assert!(
            d[3] >= BACKOFF_BASE * 2 && d[3] <= BACKOFF_BASE * 4,
            "{d:?}"
        );
    }

    #[test]
    fn force_max_jumps_straight_to_the_cap_and_stays_there() {
        let mut b = Backoff::default();
        b.force_max();
        // Same shape as `the_backoff_doubles_to_its_cap...` above: `next`
        // always returns a point in the upper half of the CURRENT step, so
        // jitter 0.0 is the step's floor and ~1.0 is its ceiling.
        let low = b.next(0.0);
        b.attempt -= 1; // a fresh attempt at the same (already capped) step
        let high = b.next(0.999_999);
        assert_eq!(
            low,
            BACKOFF_CAP / 2,
            "the step is already the cap's: {low:?}"
        );
        assert!(
            high <= BACKOFF_CAP && high > BACKOFF_CAP * 9 / 10,
            "{high:?}"
        );
        // And the step stays at the cap on the NEXT call too, with no reset.
        assert_eq!(b.next(0.0), BACKOFF_CAP / 2);
    }

    /// A hub whose `welcome.proto` this agent's window refuses: the agent
    /// closes the connection ITSELF, with the version-refused code and a
    /// reason naming which side to update — never the normal
    /// `SessionEnd::Closed`.
    #[tokio::test]
    async fn an_incompatible_hub_is_refused_by_the_agent_with_the_version_code() {
        let mut p = pair_before_welcome(Agent::new(None, crate::exec::MAX_CONCURRENT)).await;
        p.hub
            .send(Message::Text(
                encode_hub_frame(&HubFrame::Welcome {
                    hub_version: "0.1.0".into(),
                    proto: 0,
                })
                .unwrap()
                .into(),
            ))
            .await
            .unwrap();
        match tokio::time::timeout(PATIENCE, p.hub.next())
            .await
            .expect("a close in time")
        {
            Some(Ok(Message::Close(Some(frame)))) => {
                assert_eq!(u16::from(frame.code), VERSION_REFUSED_CLOSE_CODE);
                let reason = frame.reason.to_string();
                assert!(reason.contains("update the hub"), "{reason}");
            }
            other => panic!("expected a version-refused close, got {other:?}"),
        }
        let served = tokio::time::timeout(PATIENCE, p.served)
            .await
            .unwrap()
            .unwrap();
        match served.end {
            SessionEnd::VersionRefused(reason) => {
                assert!(reason.contains("update the hub"), "{reason}");
            }
            other => panic!("expected VersionRefused, got {other:?}"),
        }
    }

    /// A close reason is hub-controlled text that goes straight into
    /// `tracing::error!`/`warn!` (through `SessionEnd`'s `Display`, in
    /// `run_with`), so it is neutralised on the way in — the same treatment
    /// an unknown frame kind already gets. A reason carrying a newline or
    /// an ANSI escape must not be able to forge a journald line.
    #[test]
    fn a_hostile_close_reason_cannot_forge_a_log_line() {
        let hostile = "closed\nfleet-agent: fake line\x1b[31mred\x1b[0m\r";
        for code in [
            CloseCode::Policy,
            CloseCode::from(VERSION_REFUSED_CLOSE_CODE),
        ] {
            let end = close_end(Some(CloseFrame {
                code,
                reason: hostile.into(),
            }));
            let rendered = end.to_string();
            assert!(!rendered.contains('\n'), "{rendered:?}");
            assert!(!rendered.contains('\r'), "{rendered:?}");
            assert!(!rendered.contains('\x1b'), "{rendered:?}");
            // Still readable: the neutralisation replaces, it does not drop
            // the reason.
            assert!(rendered.contains("fake line"), "{rendered:?}");
        }
    }

    /// The same hazard as the close reason above, by the other route: a
    /// frame the agent REFUSES ends the session as `SessionEnd::Protocol`,
    /// carrying the decoder's own rejection — which quotes the peer's text.
    /// `run_with` puts that into a `tracing::warn!` AND into
    /// `notifier.status`, which becomes systemd's `STATUS=` (it strips a
    /// newline, but not a carriage return or an ANSI escape). It must be
    /// clean before it leaves `fleet-proto`.
    #[tokio::test]
    async fn a_refused_frame_s_session_end_cannot_forge_a_log_line() {
        // Two `kind` keys, the first one hostile: serde's enum reads the
        // FIRST, so its complaint quotes the forged text verbatim.
        let forged =
            serde_json::to_string("\nfleet-agent: forged \x1b[31mred\x1b[0m\r").expect("encodes");
        let hostile = format!(r#"{{"kind":{forged},"kind":"ping"}}"#);
        let mut p = pair().await;
        p.hub.send(Message::Text(hostile.into())).await.unwrap();
        let served = tokio::time::timeout(PATIENCE, p.served)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(served.end, SessionEnd::Protocol(_)), "{served:?}");
        let rendered = served.end.to_string();
        assert!(
            !rendered.chars().any(char::is_control),
            "control character in {rendered:?}"
        );
        // Still says something: the peer's forgery is neutralised, not the
        // reason for the refusal.
        assert!(rendered.contains("malformed frame"), "{rendered:?}");
    }

    /// The hub side of a version refusal: a close carrying
    /// `VERSION_REFUSED_CLOSE_CODE` is read back as `SessionEnd::VersionRefused`
    /// with its reason, not the generic `Closed`.
    #[tokio::test]
    async fn a_close_carrying_the_version_code_is_read_as_a_version_refusal() {
        let mut p = pair().await;
        let frame = CloseFrame {
            code: CloseCode::from(VERSION_REFUSED_CLOSE_CODE),
            reason: "fleet-agent speaks protocol v1; the hub needs at least v2 — update \
                     fleet-agent"
                .into(),
        };
        p.hub.send(Message::Close(Some(frame))).await.unwrap();
        let served = tokio::time::timeout(PATIENCE, p.served)
            .await
            .unwrap()
            .unwrap();
        match served.end {
            SessionEnd::VersionRefused(reason) => {
                assert!(reason.contains("update fleet-agent"), "{reason}");
            }
            other => panic!("expected VersionRefused, got {other:?}"),
        }
    }

    /// A hub predating this protocol version sends no `welcome` at all —
    /// nothing about the connection stops it sending a command straight
    /// after `hello`, which is exactly what every hub did before this
    /// change. The agent must not run it: anything other than `welcome`,
    /// before a compatible one has arrived, ends the session as
    /// `VersionRefused` and the command is never acted on.
    #[tokio::test]
    async fn a_command_before_welcome_ends_the_session_without_running_it() {
        let mut p = pair_before_welcome(Agent::new(None, crate::exec::MAX_CONCURRENT)).await;
        send(&mut p.hub, &exec("premature", &["true"], None)).await;

        // The hub closes it with the version-refused code, and — critically
        // — never sees a `result` for "premature" first: the very next
        // thing on the wire is the close, not an answer.
        match tokio::time::timeout(PATIENCE, p.hub.next())
            .await
            .expect("a close in time")
        {
            Some(Ok(Message::Close(Some(frame)))) => {
                assert_eq!(u16::from(frame.code), VERSION_REFUSED_CLOSE_CODE);
                let reason = frame.reason.to_string();
                assert!(reason.contains("no welcome"), "{reason}");
                assert!(reason.contains("update the hub"), "{reason}");
            }
            other => panic!("expected a version-refused close, got {other:?}"),
        }
        let served = tokio::time::timeout(PATIENCE, p.served)
            .await
            .unwrap()
            .unwrap();
        match served.end {
            SessionEnd::VersionRefused(reason) => assert!(reason.contains("welcome"), "{reason}"),
            other => panic!("expected VersionRefused, got {other:?}"),
        }
    }

    /// The other half of the same gate: a whole heartbeat of silence with no
    /// `welcome` ends the session the same way — no real sleep, the fake
    /// clock fires the one beat by hand.
    #[tokio::test]
    async fn silence_past_one_heartbeat_with_no_welcome_ends_the_session() {
        let mut p = pair_before_welcome(Agent::new(None, crate::exec::MAX_CONCURRENT)).await;
        assert!(
            !p.beat().await,
            "one heartbeat with no welcome must not survive"
        );
        match tokio::time::timeout(PATIENCE, p.hub.next())
            .await
            .expect("a close in time")
        {
            Some(Ok(Message::Close(Some(frame)))) => {
                assert_eq!(u16::from(frame.code), VERSION_REFUSED_CLOSE_CODE);
                let reason = frame.reason.to_string();
                assert!(reason.contains("heartbeat"), "{reason}");
                assert!(reason.contains("update the hub"), "{reason}");
            }
            other => panic!("expected a version-refused close, got {other:?}"),
        }
        let served = tokio::time::timeout(PATIENCE, p.served)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(served.end, SessionEnd::VersionRefused(_)),
            "{served:?}"
        );
        assert!(!served.heard_any, "nothing but silence was ever heard");
    }

    /// The normal path is unaffected: `pair`'s automatic `welcome` means
    /// every other test in this file already proves ordinary traffic keeps
    /// working after it — this one just says so explicitly, end to end.
    #[tokio::test]
    async fn the_normal_path_still_works_once_welcomed() {
        let mut p = pair().await;
        send(&mut p.hub, &exec("ok", &["true"], None)).await;
        let (code, ..) = result_for(&mut p.hub, "ok").await;
        assert_eq!(code, 0);
    }

    /// End to end through `run_with`: a version refusal forces the maximum
    /// backoff on the very next delay — not the normal growing sequence —
    /// and stays there on a following ordinary refusal too.
    #[tokio::test]
    async fn a_version_refusal_forces_the_maximum_backoff() {
        let fake = FakeHub::new().await;
        let dialer = fake.dialer();
        let agent = Agent::new(None, 1);
        let delays = Arc::new(Mutex::new(Vec::<Duration>::new()));
        let record = Arc::clone(&delays);
        let (beat_keep, _) = mpsc::unbounded_channel::<oneshot::Sender<bool>>();
        let run = tokio::spawn(async move {
            let notifier = Notifier::at(None);
            run_with(
                &dialer,
                &agent,
                &notifier,
                || {
                    // Never fed: the connection only ends when the hub ends it.
                    let (_tx, rx) = mpsc::unbounded_channel();
                    std::mem::forget(_tx);
                    Beats::Manual(rx)
                },
                move |d| {
                    record.lock().unwrap().push(d);
                    std::future::ready(())
                },
            )
            .await
        });

        let (mut ws, _) = fake.accept().await;
        assert!(matches!(
            next_frame(&mut ws).await,
            Some((AgentFrame::Hello { .. }, _))
        ));
        send(
            &mut ws,
            &HubFrame::Welcome {
                hub_version: "0.1.0".into(),
                proto: 0,
            },
        )
        .await;
        // Read the agent's own close, so we know this session has fully
        // ended — and the delay for it recorded — before checking anything.
        match tokio::time::timeout(PATIENCE, ws.next()).await {
            Ok(Some(Ok(Message::Close(_)))) => {}
            other => panic!("expected the agent to close on the bad version, got {other:?}"),
        }
        drop(ws);

        // An ordinary refusal right after: with no version refusal, this
        // would still be the FIRST step of the growing backoff.
        fake.refuse(401, "irrelevant").await;
        // One more dial-and-hello round, the same synchronisation the
        // growing-backoff test above uses: it guarantees the refusal's own
        // delay was already recorded before `run.abort()`.
        let (mut ws2, _) = fake.accept().await;
        assert!(matches!(
            next_frame(&mut ws2).await,
            Some((AgentFrame::Hello { .. }, _))
        ));

        run.abort();
        drop(beat_keep);

        let d = delays.lock().unwrap().clone();
        assert_eq!(d.len(), 2, "{d:?}");
        assert!(
            d[0] >= BACKOFF_CAP / 2 && d[0] <= BACKOFF_CAP,
            "the version refusal forces the max: {d:?}"
        );
        assert!(
            d[1] >= BACKOFF_CAP / 2 && d[1] <= BACKOFF_CAP,
            "and it stays there: {d:?}"
        );
    }

    // ── TLS ────────────────────────────────────────────────────────────────

    /// A wss:// hub with a private CA: trusted through `ca_file`, and refused
    /// without it — a certificate nobody vouched for is not a hub.
    #[tokio::test]
    async fn a_tls_hub_is_dialled_with_its_ca_and_refused_without_it() {
        let dir = tempfile::tempdir().unwrap();
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let ca = dir.path().join("ca.pem");
        std::fs::write(&ca, cert.cert.pem()).unwrap();

        let key = rustls_pki_types::PrivateKeyDer::Pkcs8(
            rustls_pki_types::PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()),
        );
        let server = tokio_rustls::rustls::ServerConfig::builder_with_provider(Arc::new(
            tokio_rustls::rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.cert.der().clone()], key)
        .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server));
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hub = format!("https://localhost:{port}");

        let serve_one = |acceptor: tokio_rustls::TlsAcceptor, listener: TcpListener| async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let tls = acceptor.accept(tcp).await;
            match tls {
                Ok(tls) => {
                    let mut ws = tokio_tungstenite::accept_async(tls).await.unwrap();
                    let first = ws.next().await;
                    (listener, matches!(first, Some(Ok(Message::Text(_)))))
                }
                Err(_) => (listener, false),
            }
        };

        // Trusted through the CA file: the handshake completes and hello arrives.
        let dialer = Dialer::new(
            Endpoint::parse(&hub, false).unwrap(),
            TOKEN.into(),
            Some(&ca),
        )
        .unwrap();
        let server = tokio::spawn(serve_one(acceptor.clone(), listener));
        let ws = dialer.dial().await.expect("a trusted TLS dial");
        let agent = Agent::new(None, 1);
        let (_b, rx) = mpsc::unbounded_channel();
        let client = tokio::spawn(async move { serve(ws, &agent, Beats::Manual(rx)).await });
        let (listener, got_hello) = tokio::time::timeout(PATIENCE, server)
            .await
            .unwrap()
            .unwrap();
        assert!(got_hello);
        client.abort();

        // The system roots have never heard of this CA.
        let dialer =
            Dialer::new(Endpoint::parse(&hub, false).unwrap(), TOKEN.into(), None).unwrap();
        let server = tokio::spawn(serve_one(acceptor, listener));
        match dialer.dial().await {
            Err(DialError::Failed(why)) => {
                assert!(why.to_lowercase().contains("certificate"), "{why}")
            }
            other => panic!(
                "expected a certificate failure, got {:?}",
                other.map(|_| ())
            ),
        }
        let _ = tokio::time::timeout(PATIENCE, server).await;
    }

    // ── status reporting ───────────────────────────────────────────────────

    #[test]
    fn the_notifier_sends_a_status_line_to_systemd_s_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notify");
        let sock = std::os::unix::net::UnixDatagram::bind(&path).unwrap();
        Notifier::at(Some(path)).status("connected to wss://hub.example/agent");
        let mut buf = [0u8; 256];
        sock.set_read_timeout(Some(PATIENCE)).unwrap();
        let n = sock.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"STATUS=connected to wss://hub.example/agent\n");
    }

    #[test]
    fn a_status_time_is_utc_calendar_time() {
        assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc(1_789_725_600), "2026-09-18 10:00:00 UTC");
        assert_eq!(utc(951_868_799), "2000-02-29 23:59:59 UTC");
    }

    #[test]
    fn a_notifier_with_nowhere_to_report_is_quiet() {
        Notifier::at(None).status("anything");
        Notifier::at(Some(PathBuf::from("/nonexistent/notify"))).status("anything");
    }
}
