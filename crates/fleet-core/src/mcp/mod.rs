//! Embedded MCP server — claude-fleet's control API.
//!
//! claude-fleet speaks the Model Context Protocol itself, over a localhost-only
//! streamable-HTTP transport, so an AI assistant can drive it directly. The
//! server is off by default and enabled from Settings; every request must
//! carry a bearer token. See `docs/specs/2026-05-21-control-api-mcp-design.md`.

// `pub(crate)` for `agent::ws`'s tests, which mint a client token the way the
// pairing flow does; the public surface stays the `pub use`s below.
pub(crate) mod auth;
#[cfg(test)]
mod doc_gen;
pub mod events_route;
pub mod guard;
pub mod hooks;
mod listener;
pub mod metrics;
pub mod pairing;
pub mod report_route;
pub mod settings;
mod tools;
pub mod wire;

use crate::cancel::CancellationRegistry;
use crate::ssh::SshClient;
use crate::store::Store;
use rmcp::transport::streamable_http_server::{
    session::never::NeverSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

pub use auth::normalize_allowed_hosts;
pub use auth::Caller;
#[cfg(test)]
pub use auth::TokenMode;
pub use events_route::{EventFeed, EventHistory, EventSubscriber, EventsState};
pub use guard::{ConfirmNotify, PendingConfirms, RateLimiter};
pub use listener::{NoTls, TlsAcceptor};
pub use pairing::{pair_url, PairingRequest, PendingPairings};
pub use tools::tool_deadline;
pub use tools::FleetTools;

/// `settings` table keys for the control API.
pub const SETTING_ENABLED: &str = "mcp.enabled";
pub const SETTING_PORT: &str = "mcp.port";
pub const SETTING_TOKEN: &str = "mcp.token";

/// Default localhost port for the control API.
pub const DEFAULT_PORT: u16 = 4180;

/// Live state of the embedded MCP server. Managed as `Mutex<McpRuntime>` so
/// the settings commands can start/stop it and the window-close handler can
/// shut it down.
#[derive(Default)]
pub struct McpRuntime {
    /// Cancellation token of the running server, or `None` when stopped.
    shutdown: Option<CancellationToken>,
    /// The most recent start failure (e.g. port in use), cleared on success.
    last_error: Option<String>,
}

impl McpRuntime {
    /// True while the server is listening.
    pub fn is_running(&self) -> bool {
        self.shutdown.is_some()
    }

    /// The most recent start failure, if the server is not running.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Record a freshly started server, stopping any previous one.
    pub fn set_running(&mut self, shutdown: CancellationToken) {
        self.stop();
        self.shutdown = Some(shutdown);
        self.last_error = None;
    }

    /// Record a start failure (the server is left stopped).
    pub fn set_error(&mut self, message: String) {
        self.stop();
        self.last_error = Some(message);
    }

    /// Stop the running server, if any. Idempotent.
    pub fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            shutdown.cancel();
        }
    }
}

/// Blast-radius state shared between the running server, the Tauri
/// commands that answer confirmation prompts, and tests. Cheap to clone —
/// every field is an `Arc`. Managed in Tauri state so it outlives server
/// restarts (a pending confirmation survives a port change).
#[derive(Clone)]
pub struct McpGuards {
    pub rate: Arc<RateLimiter>,
    /// Per-caller counters behind `GET /metrics`. See [`metrics`] for what is
    /// counted and, more to the point, what is deliberately not.
    pub metrics: Arc<metrics::Metrics>,
    pub confirms: Arc<PendingConfirms>,
    /// Surfaces a confirmation request to the desktop (`mcp:confirm-required`).
    pub notify: ConfirmNotify,
    /// Outstanding pairing codes. It lives here because the two halves of a
    /// pairing sit on opposite sides of the server: the tool that mints a
    /// code reaches it through `FleetTools`' guards, the unauthenticated
    /// `/pair` route that redeems it through its own state. One registry,
    /// one write path. Like the confirmations it outlives a server restart,
    /// so a code minted before a port change is still good.
    pub pairings: Arc<PendingPairings>,
    /// Someone can answer a confirmation here (the desktop's dialog). A hub
    /// has no approver: a call that MUST be confirmed there — the
    /// operator's starts and kills (work graph M9.7) — is refused outright
    /// rather than handed a nonce nobody can approve.
    pub approver: bool,
}

impl McpGuards {
    pub fn new(notify: ConfirmNotify) -> Self {
        Self {
            rate: Arc::new(RateLimiter::new()),
            metrics: Arc::new(metrics::Metrics::new()),
            confirms: Arc::new(PendingConfirms::new()),
            notify,
            pairings: Arc::new(PendingPairings::new()),
            approver: true,
        }
    }

    /// The same guards for a server with no one to approve a confirmation.
    pub fn without_approver(mut self) -> Self {
        self.approver = false;
        self
    }
}

/// Generate a fresh 256-bit bearer token, lowercase-hex-encoded (64 chars).
pub fn generate_token() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// What the auth middleware needs per request: the master token and a way
/// to read the current per-host tokens (they change on provision/rotate
/// without a server restart).
#[derive(Clone)]
struct AuthState {
    master: Arc<String>,
    store: Arc<Mutex<Store>>,
    /// Non-loopback `Host`/`Origin` values accepted besides loopback (see
    /// `auth::check_origin`). Empty on the desktop.
    allowed_hosts: Arc<Vec<String>>,
}

/// Pull `token=<v>` out of a raw query string. Tokens are hex, so no
/// percent-decoding is needed; anything else simply fails to match.
fn query_token(query: Option<&str>) -> Option<&str> {
    query?
        .split('&')
        .find_map(|kv| kv.strip_prefix("token="))
        .filter(|t| !t.is_empty())
}

/// Shared auth middleware for `/mcp` AND `/hook`: DNS-rebinding defense,
/// then bearer → [`Caller`] (master or per-host token), inserted into the
/// request extensions for the MCP handler / hook handler to read.
///
/// `/hook` additionally accepts the token as `?token=` — the form the
/// pre-Track-B `command` hooks used — so a host that has not been
/// re-provisioned yet keeps reporting until it is — but only on a server with
/// an empty Host allowlist (the desktop, a loopback hub). Header form is
/// preferred and the only form the current hook block writes.
async fn authorize(
    axum::extract::State(state): axum::extract::State<AuthState>,
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, axum::http::StatusCode> {
    use axum::http::StatusCode;
    // Sync lock, released before the next `.await` — never held across one.
    // `active_client_tokens` drops revoked pairings, so a revoked client token
    // simply stops resolving on the next request.
    let (host_tokens, client_tokens) = {
        let s = state
            .store
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        (
            s.list_host_tokens().unwrap_or_default(),
            s.active_client_tokens().unwrap_or_default(),
        )
    };
    let caller = match auth::check_request(
        request.headers(),
        &state.master,
        &host_tokens,
        &client_tokens,
        &state.allowed_hosts,
    ) {
        Ok(c) => c,
        // Header missing/unknown on /hook: try the legacy query form. Only
        // the MASTER token is accepted here — that is the only token the
        // pre-0.3 `curl …?token=` command hooks ever carried, and a per-host
        // token must never travel in a URL. TRANSITIONAL: removed in 0.4;
        // re-provision every host before then. Only with an empty Host
        // allowlist (desktop / loopback hub): a public hub's master token
        // must never be accepted from a URL that proxies and logs can see.
        Err(StatusCode::UNAUTHORIZED)
            if request.uri().path() == "/hook" && state.allowed_hosts.is_empty() =>
        {
            // No host rows and no client rows are passed: only the master
            // token can satisfy this legacy path.
            query_token(request.uri().query())
                .and_then(|t| auth::resolve_token(t, &state.master, &[], &[]))
                .ok_or_else(|| {
                    tracing::warn!("[mcp] rejected /hook request: no valid token");
                    StatusCode::UNAUTHORIZED
                })?
        }
        Err(status) => {
            // The path only: the URI / query can carry the legacy `?token=`.
            tracing::warn!(%status, path = %request.uri().path(), "[mcp] rejected request");
            return Err(status);
        }
    };
    // Liveness for the pairing UI ("last seen"). Best-effort and rate-limited
    // in the store (at most one write a minute per client); a second brief
    // sync lock, again released before the `.await` below.
    if let Some(client) = caller.client.as_ref() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if let Ok(s) = state.store.lock() {
            if let Err(e) = s.touch_client_token(client.id, now) {
                tracing::debug!(error = %e.message, "[mcp] could not touch client token");
            }
        }
    }
    request.extensions_mut().insert(caller);
    Ok(next.run(request).await)
}

/// The liveness body, exactly as `/healthz` answers it.
const HEALTHZ_BODY: &str = "fleet-hub ok\n";

/// `GET /healthz` — a liveness probe, deliberately **unauthenticated**.
///
/// It is the one route outside the [`authorize`] layer: no bearer token, no
/// `Host`/`Origin` allowlist. That is safe because it reveals nothing — it
/// never touches the store and never names a version, a host, a session or
/// any setting; it answers a fixed string that only means "this process is
/// accepting HTTP". Container health checks (`fleet-hub healthcheck`, Docker's
/// `HEALTHCHECK`) can therefore probe it without a credential and without
/// filling the log with rejected requests.
async fn healthz() -> impl axum::response::IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        HEALTHZ_BODY,
    )
}

/// Build the axum app: `/mcp` and `/mcp/json` (rmcp services), `/hook`,
/// `/report` and `/reports` behind [`authorize`], plus the unauthenticated
/// `/healthz` liveness and `/pair` exchange routes. Shared by `start` and the
/// routing test.
///
/// `mcp_json_service` is the same tool surface answering unframed
/// `application/json` instead of SSE — see [`streamable_service`] for why
/// that is a second mount rather than a flag on the first. `None` leaves the
/// path unrouted, which is what every test that does not exercise it passes.
#[allow(clippy::too_many_arguments)]
fn build_app(
    metrics_state: metrics::MetricsState,
    mcp_service: axum::routing::MethodRouter<hooks::HookState>,
    mcp_json_service: Option<axum::routing::MethodRouter<hooks::HookState>>,
    hook_state: hooks::HookState,
    auth_state: AuthState,
    pair_state: pairing::PairState,
    events_state: EventsState,
    agent_state: crate::agent::ws::AgentWsState,
    report_state: report_route::ReportState,
) -> axum::Router {
    // The MCP streamable-HTTP service is mounted with `route_service` at the
    // exact `/mcp` path — NOT `nest_service("/", …)` under `nest("/mcp", …)`.
    // axum 0.8 panics on nesting a service at the root; that panic fired
    // inside the spawned serve task *after* the listener had bound, so
    // `start` returned Ok and the UI showed the server "running" while
    // nothing was actually accepting connections.
    let mut mcp_routes = axum::Router::new().route("/mcp", mcp_service);
    // A sibling path, not a header switch: rmcp's `json_response` is a
    // per-service flag (`StreamableHttpServerConfig`), so the two answer
    // shapes cannot share one mount. `/mcp` keeps SSE framing for the long
    // polls whose keep-alive rides it; `/mcp/json` is for a client that wants
    // a body a proxy will compress.
    if let Some(json_service) = mcp_json_service {
        mcp_routes = mcp_routes.route("/mcp/json", json_service);
    }
    let authorized = mcp_routes
        .route("/hook", axum::routing::post(hooks::handle_hook))
        .with_state(hook_state)
        // `/events` carries its own state, so it is a second router merged in
        // BEFORE the `authorize` layer — which is what puts the change stream
        // behind the same bearer token as `/mcp`, unlike `/healthz` and
        // `/pair` below. See `events_route`.
        .merge(
            axum::Router::new()
                .route("/events", axum::routing::get(events_route::handle_events))
                .with_state(events_state),
        )
        // `/metrics` is authorized like the rest and then master-gated inside
        // the handler: a per-host token and a paired phone are both callers it
        // reports ON, so one reading the others' figures would make a
        // read-only device a traffic monitor for the operator's own work.
        .merge(
            axum::Router::new()
                .route("/metrics", axum::routing::get(metrics::handle_metrics))
                .with_state(metrics_state),
        )
        // `/agent` likewise carries its own state, and likewise belongs BEHIND
        // `authorize`: the upgrade needs a valid bearer token, and the handler
        // then reads the `Caller` this layer inserted to learn which host is
        // dialling in. An agent is a host, so nothing but a per-host token gets
        // past it — see `crate::agent::ws`.
        .merge(
            axum::Router::new()
                .route("/agent", axum::routing::get(crate::agent::ws::handle_agent))
                .with_state(agent_state),
        )
        // `/report` and `/reports` carry their own state and sit behind
        // `authorize` like `/events`: a phone posts its errors with its own
        // token; only the master reads them. The body cap is the spec's.
        .merge(
            axum::Router::new()
                .route("/report", axum::routing::post(report_route::handle_report))
                .route("/reports", axum::routing::get(report_route::handle_reports))
                .layer(axum::extract::DefaultBodyLimit::max(
                    fleet_proto::report::BODY_MAX,
                ))
                .with_state(report_state),
        )
        .layer(axum::middleware::from_fn_with_state(auth_state, authorize));
    // `/healthz` and `/pair` are registered on a SEPARATE router merged after
    // the layered one: in axum 0.8 `.layer` wraps only the routes added
    // before it, so merging afterwards is what keeps these two outside
    // `authorize` while every other route stays behind it.
    //
    // `/pair` is unauthenticated on purpose and it is NOT a second liveness
    // probe: it is how a client gets its first credential, so it cannot be
    // made to present one. What stands in for the bearer token is the pairing
    // code — single-use, minutes-long, minted by a master-token holder at a
    // terminal, compared in constant time and rate-limited per address (see
    // [`pairing::handle_pair`]). It is also outside the `Host`/`Origin`
    // allowlist, like `/healthz`: a phone scanning a QR may well reach the
    // hub by a name nobody listed, and the code — not the Host header — is
    // what authorizes the exchange.
    axum::Router::new()
        .route("/healthz", axum::routing::get(healthz))
        .route(
            "/pair",
            // GET is the page a camera scan lands on (static, no JavaScript,
            // no secret — the code lives in the fragment, which the browser
            // never sends); POST is the exchange itself.
            axum::routing::get(pairing::handle_pair_page)
                .post(pairing::handle_pair)
                .with_state(pair_state),
        )
        .merge(authorized)
}

/// Test-only: the real app — the real [`authorize`] layer over the real
/// routes — on an in-memory store.
///
/// It exists for `crate::agent::ws`'s tests, which dial `/agent` over a real
/// socket and must go through the SAME auth as production rather than a
/// hand-built router that could differ from it. `/mcp` answers a stub, since
/// nothing here drives a tool call.
#[cfg(test)]
pub(crate) fn test_app(
    store: Arc<Mutex<Store>>,
    master: &str,
    agent_state: crate::agent::ws::AgentWsState,
) -> axum::Router {
    build_app(
        metrics::MetricsState {
            metrics: Arc::new(metrics::Metrics::new()),
            streams: guard::LongPollLimiter::new(guard::MAX_LONG_POLLS_PER_CALLER),
        },
        axum::routing::any(|| async { "MCP_OK" }),
        None,
        hooks::HookState {
            store: Arc::clone(&store),
            ssh: Arc::new(SshClient::new()),
        },
        AuthState {
            master: Arc::new(master.to_string()),
            store: Arc::clone(&store),
            allowed_hosts: Arc::new(vec![]),
        },
        pairing::PairState::new(
            Arc::clone(&store),
            Arc::new(pairing::PendingPairings::new()),
            Arc::new(RateLimiter::new()),
            "http://127.0.0.1".to_string(),
        ),
        EventsState::disabled(),
        agent_state,
        report_route::ReportState::new(Arc::clone(&store)),
    )
}

/// Which answer shape a mount produces. The two differ in one rmcp config
/// flag and in what follows from it — same tools, same auth, same Host
/// allowlist; only the SSE mount can carry `notifications/tools/list_changed`
/// ahead of a result, so only it advertises `listChanged`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Framing {
    /// `text/event-stream`, the JSON-RPC body on a `data:` line.
    Sse,
    /// `application/json`, the body as it is.
    Json,
}

/// The stateless rmcp service behind `/mcp` and `/mcp/json`.
///
/// Stateless means every POST is a self-contained JSON-RPC exchange served by
/// a fresh `FleetTools` clone: no `Mcp-Session-Id` is issued or required, and
/// `GET`/`DELETE` are refused (405). The one server-initiated message it has,
/// `notifications/tools/list_changed`, rides the next `tools/call`'s own SSE
/// response (see `tools::list_changed`), so a session would buy nothing and
/// cost a reconnect after every app restart, port or token change, or tunnel
/// bounce.
///
/// rmcp keeps its own DNS-rebinding Host check, separate from fleet's
/// `authorize` layer, and by default it admits only loopback Hosts. A Host
/// that fleet's allowlist admits must also be on rmcp's list, or rmcp answers
/// 403 after fleet already let the request through. So rmcp gets the loopback
/// names followed by the same normalized `allowed_hosts` the `AuthState`
/// holds. The list is never empty (an empty list would make rmcp allow every
/// Host). Origin stays unchecked by rmcp: `authorize` enforces it first.
///
/// # Why the framing is a second mount rather than content negotiation
///
/// A phone pays for SSE framing twice. Once in bytes: the body is JSON
/// escaped inside a JSON string, measured at 9–12 % of each list answer.
/// And once in compression, which is the larger half — neither Caddy nor
/// Cloudflare will compress `text/event-stream`, by design and correctly, so
/// today *nothing* on this API is ever compressed. Measured on the live hub,
/// `gzip -9` over the captured bodies: `list_sessions {summary:false}`
/// 51 968 → 7 767 B, `list_projects` 7 660 → 1 308 B, `list_hosts`
/// 1 707 → 475 B. A phone's cold start is three of those calls: 61 335 B
/// today, ~9 550 B once a proxy may compress them.
///
/// The SSE mount cannot simply be switched over, because its framing is load
/// bearing: `wait_for_session` and `run_prompt` are long polls whose 15 s
/// keep-alive is what holds a reverse tunnel open. And rmcp's flag is
/// per-service rather than per-request (`StreamableHttpServerConfig::
/// with_json_response`), so one mount cannot answer both. Hence two paths,
/// from one `FleetTools`, behind one `authorize` layer.
pub(crate) fn streamable_service(
    tools: FleetTools,
    cancel: CancellationToken,
    allowed_hosts: &[String],
    framing: Framing,
) -> StreamableHttpService<FleetTools, NeverSessionManager> {
    let rmcp_hosts: Vec<String> = ["localhost", "127.0.0.1", "::1"]
        .into_iter()
        .map(str::to_string)
        .chain(allowed_hosts.iter().cloned())
        .collect();
    let tools = tools.for_framing(framing);
    StreamableHttpService::new(
        move || Ok(tools.clone()),
        NeverSessionManager::default().into(),
        StreamableHttpServerConfig::default()
            .with_stateful_mode(false)
            .with_cancellation_token(cancel)
            .with_allowed_hosts(rmcp_hosts)
            .with_json_response(framing == Framing::Json),
    )
}

/// Bind the listener and spawn the serve loop. Returns the server's
/// cancellation token on success; an `Err` carries a human-readable bind
/// failure (e.g. port in use).
// The desktop always passes loopback + an empty allowlist; only the `fleet-hub`
// daemon binds a routable address and a non-empty allowlist, and only behind
// TLS or an explicit `--allow-plaintext` (see `crates/fleet-hub`, `docs/hub.md`).
#[allow(clippy::too_many_arguments)]
pub async fn start(
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    reg: Arc<CancellationRegistry>,
    tunnels: Arc<crate::service::tunnel::TunnelSupervisor>,
    guards: McpGuards,
    bind: std::net::IpAddr,
    port: u16,
    token: String,
    allowed_hosts: Vec<String>,
) -> Result<CancellationToken, String> {
    start_with_handle(
        store,
        ssh,
        reg,
        tunnels,
        guards,
        bind,
        port,
        token,
        allowed_hosts,
        // The desktop streams row changes to its own frontend over Tauri
        // events, not over HTTP: `/events` there answers 503.
        None,
    )
    .await
    .map(|(shutdown, _serve)| shutdown)
}

/// [`start`], also handing back the serve task, which finishes once in-flight
/// requests have drained after the token is cancelled. `fleet-hub` awaits it
/// on shutdown; the desktop drops it (dropping a `JoinHandle` detaches the
/// task, it does not abort it).
#[allow(clippy::too_many_arguments)]
pub async fn start_with_handle(
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    reg: Arc<CancellationRegistry>,
    tunnels: Arc<crate::service::tunnel::TunnelSupervisor>,
    guards: McpGuards,
    bind: std::net::IpAddr,
    port: u16,
    token: String,
    allowed_hosts: Vec<String>,
    // Hands `GET /events` a fresh subscription per connection. `None` on a
    // server whose store does not publish to a broadcast bus (the desktop),
    // where `/events` answers 503.
    events: Option<EventFeed>,
) -> Result<(CancellationToken, tokio::task::JoinHandle<()>), String> {
    let addr = SocketAddr::from((bind, port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("could not bind {addr}: {e}"))?;
    start_with_listener(
        store,
        ssh,
        reg,
        tunnels,
        guards,
        listener,
        token,
        allowed_hosts,
        events,
        None::<NoTls>,
    )
    .await
}

/// [`start_with_handle`] on a listener the caller has already bound, and —
/// when `tls` is `Some` — behind the caller's TLS acceptor.
///
/// This is the variant `fleet-hub` uses to serve HTTPS from the daemon itself:
/// the TLS stack stays in the hub (see [`listener`]), and everything below the
/// accept loop is the same server the desktop runs. The plain-HTTP path is
/// unchanged — `axum::serve` is still handed the `TcpListener` directly.
#[allow(clippy::too_many_arguments)]
pub async fn start_with_listener<A: TlsAcceptor>(
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    reg: Arc<CancellationRegistry>,
    tunnels: Arc<crate::service::tunnel::TunnelSupervisor>,
    guards: McpGuards,
    listener: tokio::net::TcpListener,
    token: String,
    allowed_hosts: Vec<String>,
    events: Option<EventFeed>,
    tls: Option<A>,
) -> Result<(CancellationToken, tokio::task::JoinHandle<()>), String> {
    // The bound address, not a requested one: with the listener already open
    // there is nothing left to guess, and an ephemeral (`:0`) bind reports the
    // port it actually got. For every caller that named a port this is the
    // address it asked for.
    let addr = listener
        .local_addr()
        .map_err(|e| format!("could not read the listening address: {e}"))?;
    let port = addr.port();

    let shutdown = CancellationToken::new();
    let serve_shutdown = shutdown.clone();
    // Where a freshly paired client is told to come back. The public URL when
    // one is configured, else this server's own loopback base on the port it
    // actually bound (not the stored `mcp.port`, which a `--port` flag can
    // override). A malformed stored URL falls back to loopback rather than
    // failing the start: an unusable `hub` field is better than no hub.
    let base_url = {
        let public = store
            .lock()
            .ok()
            .and_then(|s| s.get_setting(crate::service::hub::SETTING_PUBLIC_URL).ok())
            .flatten()
            .filter(|u| !u.trim().is_empty());
        let loopback = || crate::service::hub::HubBase::loopback(port).url;
        match public {
            Some(u) => crate::service::hub::HubBase::public(&u, port)
                .map(|b| b.url)
                .unwrap_or_else(|_| loopback()),
            None => loopback(),
        }
    };
    // Normalized once and shared: fleet's `authorize` layer and rmcp's own
    // Host check must admit exactly the same hosts.
    let allowed_hosts = auth::normalize_allowed_hosts(&allowed_hosts);
    let serve_task = crate::rt::spawn(async move {
        // Taken before `store` is moved into `FleetTools` below: `/events`
        // re-reads it to notice a client revoked mid-stream.
        let events_store = Arc::clone(&store);
        // Likewise taken before `ssh` is moved into `FleetTools`: `/agent`
        // registers on the very registry this client routes agent hosts
        // through. `None` when the embedder built an SSH-only client.
        let agent_registry = ssh.agent_registry().cloned();
        // Each live agent connection's token is re-checked against the store.
        let agents_store = Arc::clone(&store);
        // Likewise taken before `store` is moved into `FleetTools`: `/report`
        // and `/reports` keep their own handle to the store.
        let reports_store = Arc::clone(&store);
        let hook_state = hooks::HookState {
            store: Arc::clone(&store),
            ssh: Arc::clone(&ssh),
        };
        let auth_state = AuthState {
            master: Arc::new(token),
            store: Arc::clone(&store),
            allowed_hosts: Arc::new(allowed_hosts.clone()),
        };
        let pair_state = pairing::PairState::new(
            Arc::clone(&store),
            Arc::clone(&guards.pairings),
            Arc::clone(&guards.rate),
            base_url,
        );
        // Cloned before `guards` moves into `FleetTools`: the route and the
        // tool router must write and read the same counters.
        let metrics_for_route = Arc::clone(&guards.metrics);
        let events_state =
            EventsState::new(events, events_store).with_shutdown(serve_shutdown.child_token());
        let tools = FleetTools::new(store, ssh, reg, tunnels, guards);
        let service = streamable_service(
            tools.clone(),
            serve_shutdown.child_token(),
            &allowed_hosts,
            Framing::Sse,
        );
        let json_service = streamable_service(
            tools,
            serve_shutdown.child_token(),
            &allowed_hosts,
            Framing::Json,
        );
        let app = build_app(
            metrics::MetricsState {
                metrics: Arc::clone(&metrics_for_route),
                streams: events_state.stream_limiter(),
            },
            axum::routing::any_service(service),
            Some(axum::routing::any_service(json_service)),
            hook_state,
            auth_state,
            pair_state,
            // The stream ends itself when the server stops, so an attached
            // client never holds the graceful drain open.
            events_state,
            // The SAME registry `SshClient` routes agent hosts through, so a
            // connection registered here is the one `AgentTransport` dispatches
            // to. `None` on an SSH-only client (the desktop): `/agent` then
            // answers 503 rather than upgrading a socket nothing would read.
            crate::agent::ws::AgentWsState::new(agent_registry.map(|r| (r, agents_store))),
            report_route::ReportState::new(reports_store),
        );

        let scheme = if tls.is_some() { "https" } else { "http" };
        tracing::info!("[mcp] control API listening on {scheme}://{addr}/mcp");
        // `into_make_service_with_connect_info` is what puts the peer address
        // in the request extensions, which is what `/pair` keys its
        // per-address attempt budget on.
        let shutdown_signal = async move {
            serve_shutdown.cancelled().await;
        };
        let result = match tls {
            // Unchanged: the `TcpListener` goes straight to axum.
            None => {
                axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .with_graceful_shutdown(shutdown_signal)
                .await
            }
            Some(acceptor) => match listener::TlsListener::spawn(listener, acceptor) {
                Err(e) => {
                    tracing::error!(error = %e, "[mcp] could not take over the listener");
                    return;
                }
                // `tap_io` is a no-op here; it is how axum lets a custom
                // listener keep `SocketAddr` as its connect info (the
                // `Connected` impl is written against `TapIo`).
                Ok(tls_listener) => {
                    use axum::serve::ListenerExt as _;
                    axum::serve(
                        tls_listener.tap_io(|_| {}),
                        app.into_make_service_with_connect_info::<SocketAddr>(),
                    )
                    .with_graceful_shutdown(shutdown_signal)
                    .await
                }
            },
        };
        if let Err(e) = result {
            tracing::error!(error = %e, "[mcp] server error");
        }
        tracing::info!("[mcp] control API stopped");
    });

    Ok((shutdown, serve_task))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_token_is_64_hex_chars_and_unique() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 64, "256-bit token = 64 hex chars");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "tokens must be drawn fresh each call");
    }

    #[test]
    fn query_token_extracts_only_the_token_param() {
        assert_eq!(query_token(Some("token=abc")), Some("abc"));
        assert_eq!(query_token(Some("x=1&token=abc&y=2")), Some("abc"));
        assert_eq!(query_token(Some("token=")), None);
        assert_eq!(query_token(Some("tok=abc")), None);
        assert_eq!(query_token(None), None);
    }

    /// Regression + Track B: the real routing shape must (a) accept requests on
    /// `/mcp` (the previous `nest_service("/")` panicked at construction in
    /// axum 0.8 — listener bound, nothing served), (b) gate `/mcp` on the
    /// bearer token, and (c) gate `/hook` behind the SAME origin/bearer layer,
    /// accepting a per-host token in the header (and, transitionally, the
    /// master token as `?token=`), rejecting everything else.
    #[tokio::test]
    async fn mcp_and_hook_routes_serve_behind_shared_auth() {
        use axum::routing::any;
        use std::net::Ipv4Addr;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        {
            let s = store.lock().unwrap();
            s.upsert_host("mefistos").unwrap();
            s.upsert_host_token("mefistos", "host-tok").unwrap();
            s.insert_client_token("phone", &auth::sha256_hex("client-tok"), "full")
                .unwrap();
            s.insert_client_token("old", &auth::sha256_hex("revoked-tok"), "full")
                .unwrap();
            s.revoke_client_token("old").unwrap();
        }
        let hook_state = hooks::HookState {
            store: Arc::clone(&store),
            ssh: Arc::new(SshClient::new()),
        };
        let auth_state = AuthState {
            master: Arc::new("s3cret".to_string()),
            store: Arc::clone(&store),
            allowed_hosts: Arc::new(vec![]),
        };
        // The pairing registry the test mints into and `/pair` redeems from.
        let pairings = Arc::new(pairing::PendingPairings::new());
        let mut pair_state = pairing::PairState::new(
            Arc::clone(&store),
            Arc::clone(&pairings),
            Arc::new(RateLimiter::new()),
            "https://fleet.example.com".to_string(),
        );
        // Every request below comes from 127.0.0.1, so the production
        // one-attempt-per-6s budget would refuse the second one. The budget
        // gets its own app at the end of this test.
        pair_state.attempt_interval = std::time::Duration::ZERO;
        let app = build_app(
            metrics::MetricsState {
                metrics: Arc::new(metrics::Metrics::new()),
                streams: guard::LongPollLimiter::new(guard::MAX_LONG_POLLS_PER_CALLER),
            },
            any(|| async { "MCP_OK" }),
            None,
            hook_state,
            auth_state,
            pair_state,
            EventsState::disabled(),
            crate::agent::ws::AgentWsState::disabled(),
            report_route::ReportState::new(Arc::clone(&store)),
        );

        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        async fn round_trip(addr: std::net::SocketAddr, req: &str) -> String {
            let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
            s.write_all(req.as_bytes()).await.unwrap();
            let mut buf = Vec::new();
            let _ = tokio::time::timeout(std::time::Duration::from_millis(500), async {
                let mut tmp = [0u8; 4096];
                loop {
                    match s.read(&mut tmp).await {
                        Ok(0) => break,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                        Err(_) => break,
                    }
                }
            })
            .await;
            String::from_utf8_lossy(&buf).into_owned()
        }

        let post = |path: &str, auth: Option<&str>, origin: Option<&str>, body: &str| {
            let mut h = format!(
                "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: application/json, \
                 text/event-stream\r\nContent-Type: application/json\r\nContent-Length: \
                 {}\r\nConnection: close\r\n",
                body.len()
            );
            if let Some(a) = auth {
                h.push_str(&format!("Authorization: Bearer {a}\r\n"));
            }
            if let Some(o) = origin {
                h.push_str(&format!("Origin: {o}\r\n"));
            }
            h.push_str("\r\n");
            h.push_str(body);
            h
        };

        // `/healthz` is the ONE route outside the auth layer: no bearer token,
        // and a Host nobody allowlisted still gets the liveness body.
        let health = round_trip(
            addr,
            "GET /healthz HTTP/1.1\r\nHost: evil.example.com\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(
            health.contains("200 OK"),
            "expected 200 on /healthz without a token:\n{health}"
        );
        assert!(
            health.ends_with("fleet-hub ok\n"),
            "expected the exact liveness body:\n{health}"
        );
        assert!(
            health
                .to_ascii_lowercase()
                .contains("text/plain; charset=utf-8"),
            "expected a text/plain content type:\n{health}"
        );
        // Nothing about this host, version or store leaks through it.
        assert!(
            !health.contains(env!("CARGO_PKG_VERSION")),
            "the liveness body must reveal no version:\n{health}"
        );
        // Only GET: anything else is a 405 from axum's method router.
        let health_post = round_trip(addr, &post("/healthz", None, None, "{}")).await;
        assert!(
            health_post.contains("405"),
            "expected 405 for POST /healthz:\n{health_post}"
        );

        // ---- `/pair`: the other unauthenticated route ----
        // Pull `"<key>":"<value>"` out of a raw HTTP response.
        fn json_field(resp: &str, key: &str) -> String {
            let needle = format!("\"{key}\":\"");
            let at = resp
                .find(&needle)
                .unwrap_or_else(|| panic!("no {key} in:\n{resp}"))
                + needle.len();
            let rest = &resp[at..];
            rest[..rest.find('"').expect("closing quote")].to_string()
        }
        // Everything after the header block.
        fn body_of(resp: &str) -> &str {
            resp.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("")
        }
        let pair_req = |code: &str, host: &str| {
            let body = format!("{{\"code\":\"{code}\"}}");
            format!(
                "POST /pair HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
        };

        // A minted code is redeemed with NO Authorization header and from a
        // Host nobody allowlisted — a phone that scanned a QR knows neither.
        let minted = pairings.mint(
            "kiosk",
            "readonly",
            false,
            std::time::Duration::from_secs(600),
        );
        let paired = round_trip(addr, &pair_req(&minted.code, "phone.invalid")).await;
        assert!(
            paired.contains("200 OK"),
            "expected 200 on /pair with a minted code:\n{paired}"
        );
        let paired_token = json_field(&paired, "token");
        assert_eq!(paired_token.len(), 64, "a 256-bit token:\n{paired}");
        assert!(
            paired_token
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "the token must be LOWERCASE hex — the stored digest is, and an \
             uppercase one would never match:\n{paired}"
        );
        // The one response that ever carries the plaintext token must not be
        // written to a proxy or browser cache.
        assert!(
            paired
                .to_ascii_lowercase()
                .contains("cache-control: no-store"),
            "the token response must be no-store:\n{paired}"
        );
        assert_eq!(json_field(&paired, "name"), "kiosk");
        assert_eq!(json_field(&paired, "mode"), "readonly");
        assert_eq!(json_field(&paired, "hub"), "https://fleet.example.com");
        assert!(
            body_of(&paired).contains("\"trusted\":false"),
            "a plain pairing lands untrusted:\n{paired}"
        );
        // The code never comes back in the answer.
        assert!(
            !paired.contains(&minted.code),
            "the pairing code must not be echoed:\n{paired}"
        );
        // A code minted `--trusted` lands a trusted row: the grant rides on
        // the code, not on anything the phone sends.
        let vouched = pairings.mint("desk", "full", true, std::time::Duration::from_secs(600));
        let paired_desk = round_trip(addr, &pair_req(&vouched.code, "phone.invalid")).await;
        assert!(paired_desk.contains("200 OK"), "{paired_desk}");
        assert!(
            body_of(&paired_desk).contains("\"trusted\":true"),
            "{paired_desk}"
        );
        {
            let s = store.lock().unwrap();
            let rows = s.active_client_tokens().unwrap();
            let desk = rows.iter().find(|r| r.name == "desk").expect("desk row");
            assert!(desk.trusted_at.is_some(), "the row is trusted");
            let kiosk = rows.iter().find(|r| r.name == "kiosk").expect("kiosk row");
            assert!(kiosk.trusted_at.is_none(), "the plain pairing is not");
        }

        // A camera scan opens `GET /pair`, which must explain what to do
        // rather than answer 405. No secret is in it: the code lives in the
        // URL fragment, which a browser never sends.
        let page = round_trip(
            addr,
            "GET /pair HTTP/1.1\r\nHost: phone.invalid\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(
            page.contains("200 OK"),
            "expected 200 on GET /pair:\n{page}"
        );
        assert!(
            page.to_ascii_lowercase().contains("text/html"),
            "GET /pair must answer HTML:\n{page}"
        );
        assert!(
            !page.to_ascii_lowercase().contains("<script"),
            "the pairing page must carry no JavaScript:\n{page}"
        );

        // Used, never-minted and expired are one and the same answer.
        let reused = round_trip(addr, &pair_req(&minted.code, "127.0.0.1")).await;
        let unknown = round_trip(addr, &pair_req("ZZZZZZZZ", "127.0.0.1")).await;
        let stale = pairings.mint("late", "full", false, std::time::Duration::from_millis(1));
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let expired = round_trip(addr, &pair_req(&stale.code, "127.0.0.1")).await;
        for (what, resp) in [
            ("a used code", &reused),
            ("an unknown code", &unknown),
            ("an expired code", &expired),
        ] {
            assert!(resp.contains("404"), "expected 404 for {what}:\n{resp}");
        }
        assert_eq!(
            body_of(&reused),
            r#"{"error":"invalid code"}"#,
            "the refusal body must carry no detail:\n{reused}"
        );
        assert_eq!(
            body_of(&unknown),
            body_of(&reused),
            "an unknown code must be indistinguishable from a used one"
        );
        assert_eq!(
            body_of(&expired),
            body_of(&reused),
            "an expired code must be indistinguishable from a used one"
        );

        // The token `/pair` handed out actually authenticates `/mcp`.
        let as_client = round_trip(addr, &post("/mcp", Some(&paired_token), None, "{}")).await;
        assert!(
            as_client.contains("200 OK"),
            "the paired token must authorize /mcp:\n{as_client}"
        );
        // A garbage body is a 400, not a 500 and not a hint about codes.
        let junk = "POST /pair HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
                    Content-Length: 5\r\nConnection: close\r\n\r\nnope!";
        let junk_resp = round_trip(addr, junk).await;
        assert!(
            junk_resp.contains("400"),
            "expected 400 for a malformed /pair body:\n{junk_resp}"
        );

        // Valid master token reaches the mounted service (proves /mcp routes, no panic).
        let ok = round_trip(addr, &post("/mcp", Some("s3cret"), None, "{}")).await;
        assert!(ok.contains("200 OK"), "expected 200 on /mcp, got:\n{ok}");
        assert!(
            ok.contains("MCP_OK"),
            "request did not reach service:\n{ok}"
        );

        // A per-host token is accepted on /mcp too.
        let host_ok = round_trip(addr, &post("/mcp", Some("host-tok"), None, "{}")).await;
        assert!(
            host_ok.contains("200 OK"),
            "host token must pass:\n{host_ok}"
        );

        // Missing/wrong token is rejected with 401 — not a dropped connection.
        let unauth = round_trip(addr, &post("/mcp", None, None, "{}")).await;
        assert!(
            unauth.contains("401"),
            "expected 401 without token, got:\n{unauth}"
        );
        let wrong = round_trip(addr, &post("/mcp", Some("nope"), None, "{}")).await;
        assert!(
            wrong.contains("401"),
            "expected 401 with wrong token:\n{wrong}"
        );

        // /hook is now behind the same layer: no token → 401.
        let stop = r#"{"hook_event_name":"Stop","session_id":"no-such"}"#;
        let hook_unauth = round_trip(addr, &post("/hook", None, None, stop)).await;
        assert!(
            hook_unauth.contains("401"),
            "expected 401 on /hook without token:\n{hook_unauth}"
        );
        // Header bearer with a per-host token → handled (204, unknown session is a no-op).
        let hook_ok = round_trip(addr, &post("/hook", Some("host-tok"), None, stop)).await;
        assert!(
            hook_ok.contains("204"),
            "expected 204 on /hook with host token:\n{hook_ok}"
        );
        // Transitional `?token=` form with the master token still works…
        let hook_q = round_trip(addr, &post("/hook?token=s3cret", None, None, stop)).await;
        assert!(
            hook_q.contains("204"),
            "expected 204 on /hook?token=:\n{hook_q}"
        );
        // …but a per-host token is header-only, even on /hook…
        let hook_hq = round_trip(addr, &post("/hook?token=host-tok", None, None, stop)).await;
        assert!(
            hook_hq.contains("401"),
            "per-host token must not authorize via query:\n{hook_hq}"
        );
        // A paired client's token authorizes /mcp…
        let client_ok = round_trip(addr, &post("/mcp", Some("client-tok"), None, "{}")).await;
        assert!(
            client_ok.contains("200 OK"),
            "client token must pass on /mcp:\n{client_ok}"
        );
        // …and the `authorize` middleware's liveness touch actually reached
        // the store: the "phone" row's `last_seen_at` moved off its initial
        // `None`.
        {
            let s = store.lock().unwrap();
            let phone = s
                .list_client_tokens(true)
                .unwrap()
                .into_iter()
                .find(|c| c.name == "phone")
                .expect("phone client token row");
            assert!(
                phone.last_seen_at.is_some(),
                "authorize() must touch last_seen_at for a client request"
            );
        }
        // …a revoked one never does (the store filters it out)…
        let revoked = round_trip(addr, &post("/mcp", Some("revoked-tok"), None, "{}")).await;
        assert!(
            revoked.contains("401"),
            "a revoked client token must be refused:\n{revoked}"
        );
        // …and a client is refused on /hook, in the header form…
        let hook_client = round_trip(addr, &post("/hook", Some("client-tok"), None, stop)).await;
        assert!(
            hook_client.contains("403"),
            "a client must not report hook events:\n{hook_client}"
        );
        // …and in the legacy query form (master-token-only path).
        let hook_cq = round_trip(addr, &post("/hook?token=client-tok", None, None, stop)).await;
        assert!(
            hook_cq.contains("401"),
            "a client token must not authorize via query:\n{hook_cq}"
        );
        // …and /mcp never accepts a query token.
        let mcp_q = round_trip(addr, &post("/mcp?token=s3cret", None, None, "{}")).await;
        assert!(
            mcp_q.contains("401"),
            "query token must not authorize /mcp:\n{mcp_q}"
        );
        // Origin/Host check applies to /hook (DNS-rebinding defense).
        let hook_rebind = round_trip(
            addr,
            &post("/hook", Some("host-tok"), Some("http://evil.com"), stop),
        )
        .await;
        assert!(
            hook_rebind.contains("403"),
            "expected 403 on cross-origin /hook:\n{hook_rebind}"
        );
        // A malformed worktree hook body is a 400, not a 500 / silent row.
        let bad = r#"{"hook_event_name":"PostToolUse","tool_name":"WorktreeCreate","tool_input":{"worktree_path":"../../etc"}}"#;
        let hook_bad = round_trip(addr, &post("/hook", Some("host-tok"), None, bad)).await;
        assert!(
            hook_bad.contains("400"),
            "expected 400 on invalid worktree_path:\n{hook_bad}"
        );

        // A second app with a non-empty allowlist: the allowlisted Host
        // passes, an unlisted one still gets 403.
        let hook_state2 = hooks::HookState {
            store: Arc::clone(&store),
            ssh: Arc::new(SshClient::new()),
        };
        let auth_state2 = AuthState {
            master: Arc::new("s3cret".to_string()),
            store: Arc::clone(&store),
            allowed_hosts: Arc::new(vec!["fleet.example.com".to_string()]),
        };
        let app2 = build_app(
            metrics::MetricsState {
                metrics: Arc::new(metrics::Metrics::new()),
                streams: guard::LongPollLimiter::new(guard::MAX_LONG_POLLS_PER_CALLER),
            },
            any(|| async { "MCP_OK" }),
            None,
            hook_state2,
            auth_state2,
            pairing::PairState::new(
                Arc::clone(&store),
                Arc::new(pairing::PendingPairings::new()),
                Arc::new(RateLimiter::new()),
                "https://fleet.example.com".to_string(),
            ),
            EventsState::disabled(),
            crate::agent::ws::AgentWsState::disabled(),
            report_route::ReportState::new(Arc::clone(&store)),
        );
        let listener2 = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr2 = listener2.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener2, app2).await.unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let allowed_post =
            "POST /mcp HTTP/1.1\r\nHost: fleet.example.com\r\nAccept: application/json, \
             text/event-stream\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\
             Authorization: Bearer s3cret\r\nConnection: close\r\n\r\n{}";
        let allowed_resp = round_trip(addr2, allowed_post).await;
        assert!(
            allowed_resp.contains("200 OK"),
            "expected 200 for allowlisted Host:\n{allowed_resp}"
        );

        let other_post =
            "POST /mcp HTTP/1.1\r\nHost: other.example.com\r\nAccept: application/json, \
             text/event-stream\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\
             Authorization: Bearer s3cret\r\nConnection: close\r\n\r\n{}";
        let other_resp = round_trip(addr2, other_post).await;
        assert!(
            other_resp.contains("403"),
            "expected 403 for non-allowlisted Host:\n{other_resp}"
        );

        // A third app with the PRODUCTION attempt budget: guessing codes is
        // throttled per address, whether or not the code is any good. Served
        // with connect-info, which is what makes the peer address available
        // for the bucket key.
        let hook_state3 = hooks::HookState {
            store: Arc::clone(&store),
            ssh: Arc::new(SshClient::new()),
        };
        let auth_state3 = AuthState {
            master: Arc::new("s3cret".to_string()),
            store: Arc::clone(&store),
            allowed_hosts: Arc::new(vec![]),
        };
        let limited_pairings = Arc::new(pairing::PendingPairings::new());
        let report_state3 = report_route::ReportState::new(Arc::clone(&store));
        let app3 = build_app(
            metrics::MetricsState {
                metrics: Arc::new(metrics::Metrics::new()),
                streams: guard::LongPollLimiter::new(guard::MAX_LONG_POLLS_PER_CALLER),
            },
            any(|| async { "MCP_OK" }),
            None,
            hook_state3,
            auth_state3,
            pairing::PairState::new(
                store,
                Arc::clone(&limited_pairings),
                Arc::new(RateLimiter::new()),
                "https://fleet.example.com".to_string(),
            ),
            EventsState::disabled(),
            crate::agent::ws::AgentWsState::disabled(),
            report_state3,
        );
        let listener3 = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr3 = listener3.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener3,
                app3.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let first = round_trip(addr3, &pair_req("ZZZZZZZZ", "127.0.0.1")).await;
        assert!(
            first.contains("404"),
            "the first guess is answered, not throttled:\n{first}"
        );
        // Even a GOOD code is refused inside the window: the budget is spent
        // before the code is looked at.
        let good =
            limited_pairings.mint("phone2", "full", false, std::time::Duration::from_secs(600));
        let throttled = round_trip(addr3, &pair_req(&good.code, "127.0.0.1")).await;
        assert!(
            throttled.contains("429"),
            "expected 429 on the second attempt from one address:\n{throttled}"
        );
        assert!(
            throttled.to_ascii_lowercase().contains("retry-after:"),
            "a 429 must say when to come back:\n{throttled}"
        );
        // Throttled means untouched: the code is still good afterwards.
        assert!(
            limited_pairings.consume(&good.code).is_some(),
            "a throttled attempt must not spend the code"
        );
    }

    #[test]
    fn runtime_tracks_running_and_error_state() {
        let mut rt = McpRuntime::default();
        assert!(!rt.is_running());
        assert!(rt.last_error().is_none());

        rt.set_running(CancellationToken::new());
        assert!(rt.is_running());
        assert!(rt.last_error().is_none());

        rt.set_error("port in use".to_string());
        assert!(!rt.is_running(), "an error leaves the server stopped");
        assert_eq!(rt.last_error(), Some("port in use"));

        rt.set_running(CancellationToken::new());
        assert!(rt.last_error().is_none(), "a restart clears the error");

        rt.stop();
        assert!(!rt.is_running());
    }

    // ---- real service, in-process (stateless transport contract) ----

    /// Real `FleetTools` behind the real stateless service, bound on an
    /// ephemeral loopback port. Requests authenticate with the master token
    /// `s3cret`.
    async fn serve_real_tools() -> std::net::SocketAddr {
        serve_real_tools_with(vec![]).await
    }

    /// [`serve_real_tools`] with a fleet Host/Origin allowlist.
    async fn serve_real_tools_with(allowed_hosts: Vec<String>) -> std::net::SocketAddr {
        use std::net::Ipv4Addr;
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let hook_state = hooks::HookState {
            store: Arc::clone(&store),
            ssh: Arc::new(SshClient::new()),
        };
        let auth_state = AuthState {
            master: Arc::new("s3cret".to_string()),
            store: Arc::clone(&store),
            allowed_hosts: Arc::new(allowed_hosts.clone()),
        };
        let guards = McpGuards::new(Arc::new(|_: &guard::ConfirmRequest| {}));
        let pair_state = pairing::PairState::new(
            Arc::clone(&store),
            Arc::clone(&guards.pairings),
            Arc::clone(&guards.rate),
            "https://fleet.example.com".to_string(),
        );
        let report_state = report_route::ReportState::new(Arc::clone(&store));
        let tools = FleetTools::new(
            store,
            Arc::new(SshClient::new()),
            crate::cancel::CancellationRegistry::new(),
            Arc::new(crate::service::tunnel::TunnelSupervisor::new()),
            guards,
        );
        // Both mounts, because this harness is the only place a real tool
        // call crosses a real socket, and `/mcp/json` differs from `/mcp`
        // exactly in what comes back over one.
        let service = streamable_service(
            tools.clone(),
            CancellationToken::new(),
            &allowed_hosts,
            Framing::Sse,
        );
        let json_service = streamable_service(
            tools,
            CancellationToken::new(),
            &allowed_hosts,
            Framing::Json,
        );
        let app = build_app(
            metrics::MetricsState {
                metrics: Arc::new(metrics::Metrics::new()),
                streams: guard::LongPollLimiter::new(guard::MAX_LONG_POLLS_PER_CALLER),
            },
            axum::routing::any_service(service),
            Some(axum::routing::any_service(json_service)),
            hook_state,
            auth_state,
            pair_state,
            EventsState::disabled(),
            crate::agent::ws::AgentWsState::disabled(),
            report_state,
        );
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        addr
    }

    /// One raw HTTP/1.1 exchange; reads until the server closes or 800 ms.
    async fn raw_round_trip(addr: std::net::SocketAddr, req: &str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        let _ = tokio::time::timeout(std::time::Duration::from_millis(800), async {
            let mut tmp = [0u8; 8192];
            loop {
                match s.read(&mut tmp).await {
                    Ok(0) => break,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    Err(_) => break,
                }
            }
        })
        .await;
        String::from_utf8_lossy(&buf).into_owned()
    }

    /// `POST /mcp` with the master token and the Accept pair rmcp requires.
    fn post_mcp(body: &str) -> String {
        post_mcp_to("127.0.0.1", body)
    }

    /// [`post_mcp_to`] against a chosen path, for the second mount.
    fn post_mcp_path(path: &str, host: &str, body: &str) -> String {
        post_mcp_to(host, body).replacen("POST /mcp ", &format!("POST {path} "), 1)
    }

    /// [`post_mcp`] with an explicit `Host` header.
    fn post_mcp_to(host: &str, body: &str) -> String {
        format!(
            "POST /mcp HTTP/1.1\r\nHost: {host}\r\nAccept: application/json, \
             text/event-stream\r\nContent-Type: application/json\r\n\
             Authorization: Bearer s3cret\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            body.len()
        )
    }

    /// The control API is stateless streamable HTTP: no `Mcp-Session-Id` is
    /// issued or required, every POST stands alone, `GET /mcp` is not served,
    /// and the server advertises MCP 2025-11-25.
    #[tokio::test]
    async fn mcp_is_stateless_and_advertises_latest_protocol() {
        let addr = serve_real_tools().await;

        // initialize without any session header → 200, latest protocol, and
        // no Mcp-Session-Id handed back.
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#;
        let r = raw_round_trip(addr, &post_mcp(init)).await;
        assert!(r.contains("200 OK"), "initialize:\n{r}");
        assert!(
            r.contains(r#""protocolVersion":"2025-11-25""#),
            "must advertise 2025-11-25:\n{r}"
        );
        assert!(
            !r.to_ascii_lowercase().contains("mcp-session-id"),
            "stateless: no session id must be issued:\n{r}"
        );

        // A second, unrelated POST (no session header) is served on its own.
        let list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
        let r = raw_round_trip(addr, &post_mcp(list)).await;
        assert!(r.contains("200 OK"), "tools/list:\n{r}");
        assert!(
            r.contains(r#""name":"list_sessions""#),
            "tools listed:\n{r}"
        );

        // GET /mcp (the stateful SSE channel) is not part of the contract.
        let get = "GET /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: text/event-stream\r\n\
                   Authorization: Bearer s3cret\r\nConnection: close\r\n\r\n";
        let r = raw_round_trip(addr, get).await;
        assert!(r.contains("405"), "GET must be 405 in stateless mode:\n{r}");
    }

    /// `/mcp/json` is the same tool surface with the framing taken off, so a
    /// reverse proxy may compress it. Both mounts sit behind the same
    /// `authorize` layer; neither changes what the other answers.
    ///
    /// The point of the unframed body is what a proxy can then do with it:
    /// `text/event-stream` is excluded from compression by Caddy's `encode`
    /// matcher and by Cloudflare, so on the live hub `--compressed` returns
    /// byte-identical responses today. `application/json` is not.
    #[tokio::test]
    async fn mcp_json_answers_unframed_and_is_still_behind_the_token() {
        let addr = serve_real_tools().await;
        let list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;

        let framed = raw_round_trip(addr, &post_mcp(list)).await;
        assert!(
            framed.to_ascii_lowercase().contains("text/event-stream"),
            "/mcp keeps its framing — the long polls' keep-alive rides it:\n{framed}"
        );
        assert!(
            framed.contains("data: {"),
            "/mcp puts the body on a data: line:\n{framed}"
        );

        let plain = raw_round_trip(addr, &post_mcp_path("/mcp/json", "127.0.0.1", list)).await;
        assert!(plain.contains("200 OK"), "/mcp/json:\n{plain}");
        assert!(
            plain
                .to_ascii_lowercase()
                .contains("content-type: application/json"),
            "/mcp/json answers application/json:\n{plain}"
        );
        assert!(
            !plain.contains("data: "),
            "/mcp/json is unframed — no data: line:\n{plain}"
        );
        assert!(
            plain.contains(r#""name":"list_sessions""#),
            "the same tools are behind it:\n{plain}"
        );

        // The second mount is inside the authorize layer, not beside it.
        let no_token = format!(
            "POST /mcp/json HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: application/json, \
             text/event-stream\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{list}",
            list.len()
        );
        let r = raw_round_trip(addr, &no_token).await;
        assert!(
            r.contains("401"),
            "an unauthenticated tool endpoint is the way to get this wrong:\n{r}"
        );
    }

    /// `listChanged` is advertised where it can be delivered: the SSE mount,
    /// whose response stream can carry a notification ahead of the result.
    /// `/mcp/json` answers with the first message the handler sends, so it
    /// neither advertises nor sends one.
    #[tokio::test]
    async fn list_changed_is_advertised_on_the_sse_mount_only() {
        let addr = serve_real_tools().await;
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#;
        let sse = raw_round_trip(addr, &post_mcp(init)).await;
        assert!(sse.contains(r#""listChanged":true"#), "/mcp:\n{sse}");
        let json = raw_round_trip(addr, &post_mcp_path("/mcp/json", "127.0.0.1", init)).await;
        assert!(json.contains("200 OK"), "/mcp/json:\n{json}");
        assert!(!json.contains("listChanged"), "/mcp/json:\n{json}");
    }

    const LIST_CHANGED: &str = "notifications/tools/list_changed";
    const A_CALL: &str = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"set_friendly_name","arguments":{"session_id":999999,"friendly_name":"x"}}}"#;

    /// A client left connected across a restart holds a list the new process
    /// never served it. Its first call carries the notification ahead of the
    /// result — once — and the result is still the last frame, which is what
    /// every in-repo reader (`wire::last_event_payload`) takes.
    #[tokio::test]
    async fn a_caller_that_never_listed_is_told_once_ahead_of_the_result() {
        let addr = serve_real_tools().await;
        let first = raw_round_trip(addr, &post_mcp(A_CALL)).await;
        let notice = first
            .find(LIST_CHANGED)
            .unwrap_or_else(|| panic!("notified:\n{first}"));
        let result = first
            .find(r#""id":3"#)
            .unwrap_or_else(|| panic!("answered:\n{first}"));
        assert!(
            notice < result,
            "the notification precedes the result:\n{first}"
        );
        let body = &first[first.find("\r\n\r\n").unwrap()..];
        assert!(
            wire::last_event_payload(body).contains("E_NOTFOUND"),
            "the result is the last frame:\n{first}"
        );

        let second = raw_round_trip(addr, &post_mcp(A_CALL)).await;
        assert!(second.contains(r#""id":3"#), "answered:\n{second}");
        assert!(!second.contains(LIST_CHANGED), "told once:\n{second}");
    }

    /// A client that listed on this process holds the current list: nothing
    /// is sent. Neither is anything on `/mcp/json`, where it would replace
    /// the result.
    #[tokio::test]
    async fn a_caller_that_listed_or_uses_the_json_mount_is_not_told() {
        let addr = serve_real_tools().await;
        let json = raw_round_trip(addr, &post_mcp_path("/mcp/json", "127.0.0.1", A_CALL)).await;
        assert!(json.contains(r#""id":3"#), "/mcp/json answered:\n{json}");
        assert!(!json.contains(LIST_CHANGED), "/mcp/json:\n{json}");

        let list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
        raw_round_trip(addr, &post_mcp(list)).await;
        let r = raw_round_trip(addr, &post_mcp(A_CALL)).await;
        assert!(r.contains(r#""id":3"#), "answered:\n{r}");
        assert!(!r.contains(LIST_CHANGED), "listed, so current:\n{r}");
    }

    /// `/metrics` answers the master token and refuses everything else with a
    /// sentence. A per-host token and a paired phone are both callers it
    /// reports ON; letting either read the figures would make a read-only
    /// device a traffic monitor for the operator's own work.
    #[tokio::test]
    async fn metrics_answers_the_master_token_and_refuses_the_rest() {
        let addr = serve_real_tools().await;

        // A call to count.
        let list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
        raw_round_trip(addr, &post_mcp(list)).await;

        let get = |auth: &str| {
            format!(
                "GET /metrics HTTP/1.1\r\nHost: 127.0.0.1\r\n\
                 Authorization: Bearer {auth}\r\nConnection: close\r\n\r\n"
            )
        };
        let r = raw_round_trip(addr, &get("s3cret")).await;
        assert!(r.contains("200 OK"), "master:\n{r}");
        assert!(
            r.contains("fleet_tool_calls_total"),
            "the exposition must carry the call counter:\n{r}"
        );
        assert!(
            r.to_ascii_lowercase().contains("text/plain"),
            "Prometheus scrapes text/plain:\n{r}"
        );
        // No token at all is the authorize layer's answer, not the handler's.
        let r = raw_round_trip(
            addr,
            "GET /metrics HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(r.contains("401"), "unauthenticated:\n{r}");
    }

    /// rmcp keeps its own DNS-rebinding Host check (loopback only by
    /// default); the fleet allowlist must reach it, or a public Host that
    /// passed fleet's `authorize` layer is still refused with 403 by rmcp.
    #[tokio::test]
    async fn real_service_accepts_an_allowlisted_public_host() {
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#;

        let addr = serve_real_tools_with(vec!["fleet.example.com".to_string()]).await;
        let r = raw_round_trip(addr, &post_mcp_to("fleet.example.com", init)).await;
        assert!(r.contains("200 OK"), "allowlisted public Host:\n{r}");
        assert!(
            r.contains(r#""protocolVersion""#),
            "initialize result:\n{r}"
        );
        let r = raw_round_trip(addr, &post_mcp_to("other.example.com", init)).await;
        assert!(r.contains("403"), "unlisted Host:\n{r}");
        let r = raw_round_trip(addr, &post_mcp(init)).await;
        assert!(
            r.contains("200 OK"),
            "loopback alongside an allowlist:\n{r}"
        );

        // An empty fleet allowlist still serves loopback (and only loopback).
        let addr = serve_real_tools().await;
        let r = raw_round_trip(addr, &post_mcp(init)).await;
        assert!(r.contains("200 OK"), "loopback, empty allowlist:\n{r}");
        let r = raw_round_trip(addr, &post_mcp_to("localhost:1234", init)).await;
        assert!(r.contains("200 OK"), "localhost, empty allowlist:\n{r}");
        let r = raw_round_trip(addr, &post_mcp_to("fleet.example.com", init)).await;
        assert!(r.contains("403"), "public Host, empty allowlist:\n{r}");
    }

    /// The desktop embeds the same server but opens its store with the
    /// frontend's own bus, so there is nothing to stream: `/events` must say
    /// so in as many words rather than 404 (which reads as "wrong URL") or
    /// hang on an empty stream.
    #[tokio::test]
    async fn events_is_unavailable_without_a_bus() {
        let addr = serve_real_tools().await;
        let r = raw_round_trip(addr, &get_events(Some("s3cret"), "")).await;
        assert!(r.contains("503"), "expected 503 without a bus:\n{r}");
        assert!(
            r.contains(events_route::NOT_ENABLED),
            "expected the documented reason:\n{r}"
        );
    }

    /// `GET /events` with the master token and an optional raw query string.
    fn get_events(auth: Option<&str>, query: &str) -> String {
        get_events_resuming(auth, query, None)
    }

    /// [`get_events`] with a `Last-Event-ID`, the header a reconnecting
    /// client sends to say where it got to.
    fn get_events_resuming(auth: Option<&str>, query: &str, last_id: Option<&str>) -> String {
        let mut h = format!(
            "GET /events{query} HTTP/1.1\r\nHost: 127.0.0.1\r\n\
             Accept: text/event-stream\r\n"
        );
        if let Some(a) = auth {
            h.push_str(&format!("Authorization: Bearer {a}\r\n"));
        }
        if let Some(id) = last_id {
            h.push_str(&format!("Last-Event-ID: {id}\r\n"));
        }
        h.push_str("\r\n");
        h
    }

    /// The same app shape as [`serve_real_tools`], with an event source
    /// wired to `bus` — what `fleet-hub serve` builds.
    async fn serve_with_events(
        bus: &Arc<crate::events::BroadcastEventBus>,
    ) -> std::net::SocketAddr {
        serve_with_events_every(bus, events_route::KEEPALIVE_INTERVAL).await
    }

    /// [`serve_with_events`] with a shortened heartbeat.
    async fn serve_with_events_every(
        bus: &Arc<crate::events::BroadcastEventBus>,
        keepalive: std::time::Duration,
    ) -> std::net::SocketAddr {
        serve_events_app(
            bus,
            keepalive,
            CancellationToken::new(),
            Arc::new(Mutex::new(Store::open_in_memory().unwrap())),
        )
        .await
        .0
    }

    /// [`serve_with_events`], also taking the server's shutdown token and the
    /// store the route re-reads, and handing back the route state so a test
    /// can watch a stream slot free up.
    async fn serve_events_app(
        bus: &Arc<crate::events::BroadcastEventBus>,
        keepalive: std::time::Duration,
        stop: CancellationToken,
        store: Arc<Mutex<Store>>,
    ) -> (std::net::SocketAddr, EventsState) {
        use axum::routing::any;
        use std::net::Ipv4Addr;
        let hook_state = hooks::HookState {
            store: Arc::clone(&store),
            ssh: Arc::new(SshClient::new()),
        };
        let auth_state = AuthState {
            master: Arc::new("s3cret".to_string()),
            store: Arc::clone(&store),
            allowed_hosts: Arc::new(vec![]),
        };
        let pair_state = pairing::PairState::new(
            Arc::clone(&store),
            Arc::new(pairing::PendingPairings::new()),
            Arc::new(RateLimiter::new()),
            "https://fleet.example.com".to_string(),
        );
        // The feed, not a bare subscriber: `/events` resumes from the bus's
        // own replay history, and a harness without it could not exercise it.
        let events_state = EventsState::new(Some(Arc::clone(bus).into()), Arc::clone(&store))
            .with_keepalive(keepalive)
            .with_shutdown(stop);
        let app = build_app(
            metrics::MetricsState {
                metrics: Arc::new(metrics::Metrics::new()),
                streams: guard::LongPollLimiter::new(guard::MAX_LONG_POLLS_PER_CALLER),
            },
            any(|| async { "MCP_OK" }),
            None,
            hook_state,
            auth_state,
            pair_state,
            events_state.clone(),
            crate::agent::ws::AgentWsState::disabled(),
            report_route::ReportState::new(Arc::clone(&store)),
        );
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        (addr, events_state)
    }

    /// One open SSE connection, read incrementally: unlike `raw_round_trip`
    /// the server never closes a stream, so the test reads until it has seen
    /// what it is waiting for and then drops the socket.
    struct SseConn {
        sock: tokio::net::TcpStream,
        seen: String,
    }

    impl SseConn {
        async fn open(addr: std::net::SocketAddr, auth: Option<&str>, query: &str) -> Self {
            Self::open_resuming(addr, auth, query, None).await
        }

        /// [`SseConn::open`] with a `Last-Event-ID`.
        async fn open_resuming(
            addr: std::net::SocketAddr,
            auth: Option<&str>,
            query: &str,
            last_id: Option<&str>,
        ) -> Self {
            use tokio::io::AsyncWriteExt;
            let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
            sock.write_all(get_events_resuming(auth, query, last_id).as_bytes())
                .await
                .unwrap();
            Self {
                sock,
                seen: String::new(),
            }
        }

        /// Read until `needle` shows up, or fail after 2 s — a hang must fail
        /// the test, not block the suite.
        async fn wait_for(&mut self, needle: &str) -> &str {
            use tokio::io::AsyncReadExt;
            let deadline = std::time::Duration::from_secs(2);
            let read = tokio::time::timeout(deadline, async {
                let mut tmp = [0u8; 4096];
                while !self.seen.contains(needle) {
                    match self.sock.read(&mut tmp).await {
                        Ok(0) => break,
                        Ok(n) => self.seen.push_str(&String::from_utf8_lossy(&tmp[..n])),
                        Err(_) => break,
                    }
                }
            })
            .await;
            assert!(
                read.is_ok() && self.seen.contains(needle),
                "waited for {needle:?}; stream so far:\n{}",
                self.seen
            );
            &self.seen
        }
    }

    /// A reconnect costs the events missed, not a full re-list.
    ///
    /// Without this a phone paid 61 335 B and three round trips every time it
    /// went through a lift, a tunnel or an app switch, because no frame
    /// carried an `id:` and there was nothing to resume from.
    #[tokio::test]
    async fn a_reconnect_resumes_from_its_last_event_id() {
        use crate::events::{BroadcastEventBus, EventBus};
        let bus = Arc::new(BroadcastEventBus::default());
        let addr = serve_with_events(&bus).await;

        let mut first = SseConn::open(addr, Some("s3cret"), "").await;
        first.wait_for("event: ready").await;
        bus.session_killed(1);
        let seen = first.wait_for("event: session:killed").await.to_string();
        // Every row frame carries the id a resume names.
        let id = seen
            .lines()
            .find_map(|l| l.strip_prefix("id: "))
            .expect("a row frame must carry an id")
            .trim()
            .to_string();
        drop(first);

        // Two changes while nothing is connected: the ring's grace window is
        // what keeps them replayable.
        bus.session_killed(2);
        bus.session_killed(3);

        let mut again = SseConn::open_resuming(addr, Some("s3cret"), "", Some(&id)).await;
        let head = again.wait_for("event: ready").await.to_string();
        assert!(
            head.contains("\"resumed\":true"),
            "the ready frame must say the resume was honoured:\n{head}"
        );
        let replayed = again.wait_for(r#"data: {"id":3}"#).await.to_string();
        assert!(
            replayed.contains(r#"data: {"id":2}"#),
            "both missed events replay, in order:\n{replayed}"
        );
        assert!(
            !replayed.contains(r#"data: {"id":1}"#),
            "and nothing the client already had:\n{replayed}"
        );
    }

    /// An id this hub never minted — a restarted process, a mangled header —
    /// must not be replayed against the current sequence. Saying so lets the
    /// client re-list; pretending would leave it convinced it was current.
    #[tokio::test]
    async fn an_unknown_last_event_id_is_refused_and_says_so() {
        use crate::events::BroadcastEventBus;
        let bus = Arc::new(BroadcastEventBus::default());
        let addr = serve_with_events(&bus).await;

        let mut sse = SseConn::open_resuming(addr, Some("s3cret"), "", Some("999999-5")).await;
        let head = sse.wait_for("event: ready").await.to_string();
        assert!(
            head.contains("\"resumed\":false"),
            "a foreign generation must not be replayed:\n{head}"
        );
    }

    /// `?fields=` keeps a phone from decoding columns it never draws. The
    /// measured session row is ~1 227 B of which about a third is fields no
    /// screen reads.
    #[tokio::test]
    async fn fields_projects_the_payload_and_the_ready_frame_echoes_it() {
        use crate::events::{BroadcastEventBus, EventBus};
        let bus = Arc::new(BroadcastEventBus::default());
        let addr = serve_with_events(&bus).await;

        let mut sse = SseConn::open(addr, Some("s3cret"), "?fields=alias").await;
        let head = sse.wait_for("event: ready").await.to_string();
        assert!(
            head.contains(r#""fields":["alias"]"#),
            "the ready frame echoes what was honoured:\n{head}"
        );

        bus.host_removed("box");
        let frame = sse.wait_for("event: host:removed").await.to_string();
        assert!(
            frame.contains(r#"data: {"alias":"box"}"#),
            "the asked-for key survives:\n{frame}"
        );

        // And a key that was not asked for does not.
        let mut narrow = SseConn::open(addr, Some("s3cret"), "?fields=nothing_like_this").await;
        narrow.wait_for("event: ready").await;
        bus.host_removed("box2");
        let frame = narrow.wait_for("event: host:removed").await.to_string();
        assert!(
            frame.contains("data: {}"),
            "an unasked-for key is projected away:\n{frame}"
        );
    }

    /// The stream itself: authenticated, `text/event-stream`, a `ready` frame
    /// first, then one frame per change emitted AFTER the subscription — the
    /// whole reason a phone can stop polling.
    #[tokio::test]
    async fn events_streams_changes_emitted_after_subscribing() {
        use crate::events::{BroadcastEventBus, EventBus};
        let bus = Arc::new(BroadcastEventBus::default());
        let addr = serve_with_events(&bus).await;

        let mut sse = SseConn::open(addr, Some("s3cret"), "").await;
        let head = sse.wait_for("event: ready").await.to_string();
        assert!(head.contains("200 OK"), "expected 200:\n{head}");
        assert!(
            head.to_ascii_lowercase().contains("text/event-stream"),
            "expected an SSE content type:\n{head}"
        );
        // The ready frame carries the server version and its clock, so a
        // reconnecting client can tell a restart from a hiccup.
        assert!(
            head.contains(crate::app_version::get()) && head.contains("\"now\""),
            "the ready frame must carry version and now:\n{head}"
        );
        // The wire-contract revision: additive next to `version`/`now`, and
        // this is a client's only way to tell "an older hub whose rows I
        // still understand" from "a hub whose rows changed shape under me".
        assert!(
            head.contains(&format!(
                "\"contract\":{}",
                crate::wire_contract::CONTRACT_REVISION
            )),
            "the ready frame must carry the wire-contract revision:\n{head}"
        );

        // Emitted only now, with the subscription already live.
        bus.session_killed(42);
        let frame = sse.wait_for("event: session:killed").await.to_string();
        assert!(
            frame.contains(r#"data: {"id":42}"#),
            "the payload must be the frontend's:\n{frame}"
        );

        // `?kinds=host` drops a session change and passes a host one.
        let mut filtered = SseConn::open(addr, Some("s3cret"), "?kinds=host").await;
        filtered.wait_for("event: ready").await;
        bus.session_killed(43);
        bus.host_removed("box");
        let seen = filtered.wait_for("event: host:removed").await.to_string();
        assert!(
            !seen.contains("session:killed"),
            "?kinds=host must drop session events:\n{seen}"
        );
        assert!(
            seen.contains(r#"data: {"alias":"box"}"#),
            "the host payload:\n{seen}"
        );

        // A stream is not a public resource: no token, no stream.
        let unauth = raw_round_trip(addr, &get_events(None, "")).await;
        assert!(
            unauth.contains("401"),
            "expected 401 without a token:\n{unauth}"
        );
        // Nor does a token in the URL open one (it would land in proxy logs).
        let via_query = raw_round_trip(addr, &get_events(None, "?token=s3cret")).await;
        assert!(
            via_query.contains("401"),
            "a query token must not open a stream:\n{via_query}"
        );
    }

    /// A subscriber that falls behind the ring is told how much it missed and
    /// the stream ends — the bus never waits for it. The emits below run
    /// without an await, so the server task cannot drain between them.
    #[tokio::test]
    async fn a_lagging_stream_is_told_and_closed() {
        use crate::events::{BroadcastEventBus, EventBus};
        let bus = Arc::new(BroadcastEventBus::new(2));
        let addr = serve_with_events(&bus).await;
        let mut sse = SseConn::open(addr, Some("s3cret"), "").await;
        sse.wait_for("event: ready").await;

        for i in 0..50 {
            bus.emit(&crate::events::RowChange::SessionKilled(i));
        }
        let seen = sse.wait_for("event: lagged").await.to_string();
        assert!(
            seen.contains(r#""skipped":"#),
            "the lagged frame must say how many were missed:\n{seen}"
        );
        // …and the stream ENDED rather than streaming on with a hole in it:
        // the chunked body is terminated (the socket itself stays up — HTTP
        // keep-alive — so EOF is not the signal to look for).
        let ended = sse.wait_for("\r\n0\r\n\r\n").await.to_string();
        assert!(
            ended.rfind("event: lagged") < ended.rfind("\r\n0\r\n\r\n"),
            "the lagged frame must be the last one:\n{ended}"
        );
    }

    /// An idle stream sends a comment line on the keep-alive interval, which
    /// is what holds the connection open through a phone's NAT, the reverse
    /// tunnel and any proxy in between. Production waits
    /// [`events_route::KEEPALIVE_INTERVAL`]; this asks for 50 ms and watches
    /// it arrive.
    #[tokio::test]
    async fn an_idle_stream_sends_a_heartbeat() {
        use crate::events::BroadcastEventBus;
        let bus = Arc::new(BroadcastEventBus::default());
        let addr = serve_with_events_every(&bus, std::time::Duration::from_millis(50)).await;
        let mut sse = SseConn::open(addr, Some("s3cret"), "").await;
        sse.wait_for("event: ready").await;
        // Nothing is emitted: what arrives next can only be the heartbeat.
        let seen = sse.wait_for(":\n\n").await.to_string();
        assert!(
            !seen.contains("event: session"),
            "no change was emitted; only a heartbeat may follow:\n{seen}"
        );
    }

    /// A stream is an in-flight request that never ends on its own, and
    /// axum's graceful shutdown waits for in-flight requests: an attached
    /// client must not hold a stopping hub open for its whole drain timeout.
    #[tokio::test]
    async fn a_stream_ends_when_the_server_stops() {
        use crate::events::BroadcastEventBus;
        let bus = Arc::new(BroadcastEventBus::default());
        let stop = CancellationToken::new();
        let (addr, _state) = serve_events_app(
            &bus,
            events_route::KEEPALIVE_INTERVAL,
            stop.child_token(),
            Arc::new(Mutex::new(Store::open_in_memory().unwrap())),
        )
        .await;
        let mut sse = SseConn::open(addr, Some("s3cret"), "").await;
        sse.wait_for("event: ready").await;
        stop.cancel();
        // The chunked body is terminated, promptly — no drain timeout.
        sse.wait_for("\r\n0\r\n\r\n").await;
    }

    /// `authorize` runs once, at connect; a stream then outlives it. So a
    /// paired client's stream re-checks the store and ends when its row stops
    /// being live — otherwise a revoked phone would keep receiving the whole
    /// fleet change feed until it chose to disconnect.
    #[tokio::test]
    async fn a_revoked_clients_stream_ends_at_the_next_heartbeat() {
        use crate::events::BroadcastEventBus;
        let bus = Arc::new(BroadcastEventBus::default());
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        {
            let s = store.lock().unwrap();
            s.insert_client_token("phone", &auth::sha256_hex("client-tok"), "full")
                .unwrap();
        }
        let (addr, state) = serve_events_app(
            &bus,
            std::time::Duration::from_millis(50),
            CancellationToken::new(),
            Arc::clone(&store),
        )
        .await;

        let mut sse = SseConn::open(addr, Some("client-tok"), "").await;
        sse.wait_for("event: ready").await;
        assert_eq!(state.active("client:phone"), 1, "the stream holds a slot");

        store.lock().unwrap().revoke_client_token("phone").unwrap();

        // The body ends — within a heartbeat, not when the device feels like
        // disconnecting.
        sse.wait_for("\r\n0\r\n\r\n").await;
        // …and the slot it held comes back.
        let freed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while state.active("client:phone") != 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(
            freed.is_ok(),
            "the permit must be released when the stream ends"
        );
    }

    /// Streams are capped per caller like the long-polling tools: the ninth
    /// concurrent one from the same token is refused, and told when to retry.
    #[tokio::test]
    async fn the_ninth_concurrent_stream_from_one_caller_is_refused() {
        use crate::events::BroadcastEventBus;
        let bus = Arc::new(BroadcastEventBus::default());
        // A short heartbeat, so the server writes to a dropped connection
        // soon enough for the test to observe the slot come back.
        let (addr, state) = serve_events_app(
            &bus,
            std::time::Duration::from_millis(50),
            CancellationToken::new(),
            Arc::new(Mutex::new(Store::open_in_memory().unwrap())),
        )
        .await;
        let mut held = Vec::new();
        for _ in 0..guard::MAX_LONG_POLLS_PER_CALLER {
            let mut c = SseConn::open(addr, Some("s3cret"), "").await;
            c.wait_for("event: ready").await;
            held.push(c);
        }
        let refused = raw_round_trip(addr, &get_events(Some("s3cret"), "")).await;
        assert!(
            refused.contains("429"),
            "expected 429 for the ninth stream:\n{refused}"
        );
        assert!(
            refused.to_ascii_lowercase().contains("retry-after: 1"),
            "a 429 must say when to come back:\n{refused}"
        );
        assert!(
            refused.contains(events_route::TOO_MANY_STREAMS),
            "expected the documented reason:\n{refused}"
        );
        assert_eq!(state.active("master"), guard::MAX_LONG_POLLS_PER_CALLER);

        // The third leg the cap rests on: axum drops the body when the client
        // goes away, which drops the permit — so hanging up frees a slot.
        held.pop();
        let freed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if state.active("master") < guard::MAX_LONG_POLLS_PER_CALLER {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(freed.is_ok(), "a hung-up stream must release its slot");
        let again = SseConn::open(addr, Some("s3cret"), "")
            .await
            .wait_for("event: ready")
            .await
            .to_string();
        assert!(
            again.contains("200 OK"),
            "the freed slot must be usable:\n{again}"
        );
    }

    /// The transitional `/hook?token=<master>` form is a desktop/loopback
    /// affordance only: a server with a Host allowlist (a public hub) refuses
    /// it like any request without a bearer header.
    #[tokio::test]
    async fn legacy_query_token_is_refused_when_an_allowlist_is_set() {
        let stop = r#"{"hook_event_name":"Stop","session_id":"no-such"}"#;
        let hook_q = |host: &str| {
            format!(
                "POST /hook?token=s3cret HTTP/1.1\r\nHost: {host}\r\n\
                 Content-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{stop}",
                stop.len()
            )
        };

        let loopback = serve_real_tools().await;
        let r = raw_round_trip(loopback, &hook_q("127.0.0.1")).await;
        assert!(r.contains("204"), "empty allowlist accepts ?token=:\n{r}");

        let public = serve_real_tools_with(vec!["fleet.example.com".to_string()]).await;
        let r = raw_round_trip(public, &hook_q("fleet.example.com")).await;
        assert!(r.contains("401"), "public hub refuses ?token=:\n{r}");
        let r = raw_round_trip(public, &hook_q("127.0.0.1")).await;
        assert!(r.contains("401"), "even over loopback:\n{r}");
    }

    /// A tool that fails in the service layer answers with a tool RESULT
    /// carrying `isError: true` and the `E_*` code, not a JSON-RPC error.
    #[tokio::test]
    async fn tools_call_failure_is_an_is_error_result() {
        let addr = serve_real_tools().await;
        let call = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"set_friendly_name","arguments":{"session_id":999999,"friendly_name":"x"}}}"#;
        let r = raw_round_trip(addr, &post_mcp(call)).await;
        assert!(r.contains("200 OK"), "tools/call:\n{r}");
        assert!(
            r.contains(r#""isError":true"#),
            "must be a tool result:\n{r}"
        );
        assert!(r.contains("E_NOTFOUND"), "code preserved:\n{r}");
        assert!(
            !r.contains(r#""error":{"#),
            "must not be a JSON-RPC error:\n{r}"
        );
    }

    /// On Linux every 127.0.0.0/8 address is loopback, so binding 127.0.0.2
    /// discriminates: the server must answer there and NOT on 127.0.0.1,
    /// which a listener that ignored the address (0.0.0.0) would.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn start_binds_the_requested_address() {
        let ip = std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 2));
        // A port just free on 127.0.0.1, so a refusal there is the server's
        // doing, not a leftover listener's.
        let port = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let guards = McpGuards::new(Arc::new(|_| {}));
        let (shutdown, task) = start_with_handle(
            store,
            Arc::new(SshClient::new()),
            crate::cancel::CancellationRegistry::new(),
            Arc::new(crate::service::tunnel::TunnelSupervisor::new()),
            guards,
            ip,
            port,
            "tok".into(),
            vec![],
            None,
        )
        .await
        .expect("bind 127.0.0.2");
        tokio::net::TcpStream::connect((ip, port))
            .await
            .expect("the requested address accepts");
        let other = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await;
        assert_eq!(
            other.map(|_| ()).map_err(|e| e.kind()),
            Err(std::io::ErrorKind::ConnectionRefused),
            "127.0.0.1:{port} must not be served"
        );
        shutdown.cancel();
        let _ = task.await;
    }

    #[cfg(not(target_os = "linux"))]
    #[tokio::test]
    async fn start_binds_the_requested_address() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let guards = McpGuards::new(Arc::new(|_| {}));
        let shutdown = start(
            store,
            Arc::new(SshClient::new()),
            crate::cancel::CancellationRegistry::new(),
            Arc::new(crate::service::tunnel::TunnelSupervisor::new()),
            guards,
            std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            0, // any free port is fine: we only check the bind succeeds
            "tok".into(),
            vec![],
        )
        .await;
        assert!(shutdown.is_ok(), "{shutdown:?}");
        shutdown.unwrap().cancel();
    }
}
