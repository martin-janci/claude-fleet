//! Embedded MCP server — claude-fleet's control API.
//!
//! claude-fleet speaks the Model Context Protocol itself, over a localhost-only
//! streamable-HTTP transport, so an AI assistant can drive it directly. The
//! server is off by default and enabled from Settings; every request must
//! carry a bearer token. See `docs/specs/2026-05-21-control-api-mcp-design.md`.

mod auth;
#[cfg(test)]
mod doc_gen;
pub mod guard;
pub mod hooks;
pub mod settings;
mod tools;

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
pub use guard::{ConfirmNotify, PendingConfirms, RateLimiter};
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
    pub confirms: Arc<PendingConfirms>,
    /// Surfaces a confirmation request to the desktop (`mcp:confirm-required`).
    pub notify: ConfirmNotify,
}

impl McpGuards {
    pub fn new(notify: ConfirmNotify) -> Self {
        Self {
            rate: Arc::new(RateLimiter::new()),
            confirms: Arc::new(PendingConfirms::new()),
            notify,
        }
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

/// Build the axum app: `/mcp` (rmcp service) and `/hook` behind [`authorize`],
/// plus the unauthenticated `/healthz` liveness route. Shared by `start` and
/// the routing test.
fn build_app(
    mcp_service: axum::routing::MethodRouter<hooks::HookState>,
    hook_state: hooks::HookState,
    auth_state: AuthState,
) -> axum::Router {
    // The MCP streamable-HTTP service is mounted with `route_service` at the
    // exact `/mcp` path — NOT `nest_service("/", …)` under `nest("/mcp", …)`.
    // axum 0.8 panics on nesting a service at the root; that panic fired
    // inside the spawned serve task *after* the listener had bound, so
    // `start` returned Ok and the UI showed the server "running" while
    // nothing was actually accepting connections.
    let authorized = axum::Router::new()
        .route("/mcp", mcp_service)
        .route("/hook", axum::routing::post(hooks::handle_hook))
        .with_state(hook_state)
        .layer(axum::middleware::from_fn_with_state(auth_state, authorize));
    // `/healthz` is registered on a SEPARATE router merged after the layered
    // one: in axum 0.8 `.layer` wraps only the routes added before it, so
    // merging afterwards is what keeps the liveness probe outside `authorize`
    // while every other route stays behind it.
    axum::Router::new()
        .route("/healthz", axum::routing::get(healthz))
        .merge(authorized)
}

/// The stateless rmcp service behind `/mcp`.
///
/// Stateless means every POST is a self-contained JSON-RPC exchange served by
/// a fresh `FleetTools` clone: no `Mcp-Session-Id` is issued or required, and
/// `GET`/`DELETE` are refused (405). The server never sends server-initiated
/// messages (tools only), so a session bought nothing and cost a reconnect
/// after every app restart, port or token change, or tunnel bounce. Responses
/// keep SSE framing (`json_response` default `false`) so the 15 s keep-alive
/// still flows on long polls (`wait_for_session`, `run_prompt`) through the
/// reverse tunnel.
///
/// rmcp keeps its own DNS-rebinding Host check, separate from fleet's
/// `authorize` layer, and by default it admits only loopback Hosts. A Host
/// that fleet's allowlist admits must also be on rmcp's list, or rmcp answers
/// 403 after fleet already let the request through. So rmcp gets the loopback
/// names followed by the same normalized `allowed_hosts` the `AuthState`
/// holds. The list is never empty (an empty list would make rmcp allow every
/// Host). Origin stays unchecked by rmcp: `authorize` enforces it first.
pub(crate) fn streamable_service(
    tools: FleetTools,
    cancel: CancellationToken,
    allowed_hosts: &[String],
) -> StreamableHttpService<FleetTools, NeverSessionManager> {
    let rmcp_hosts: Vec<String> = ["localhost", "127.0.0.1", "::1"]
        .into_iter()
        .map(str::to_string)
        .chain(allowed_hosts.iter().cloned())
        .collect();
    StreamableHttpService::new(
        move || Ok(tools.clone()),
        NeverSessionManager::default().into(),
        StreamableHttpServerConfig::default()
            .with_stateful_mode(false)
            .with_cancellation_token(cancel)
            .with_allowed_hosts(rmcp_hosts),
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
) -> Result<(CancellationToken, tokio::task::JoinHandle<()>), String> {
    let addr = SocketAddr::from((bind, port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("could not bind {addr}: {e}"))?;

    let shutdown = CancellationToken::new();
    let serve_shutdown = shutdown.clone();
    // Normalized once and shared: fleet's `authorize` layer and rmcp's own
    // Host check must admit exactly the same hosts.
    let allowed_hosts = auth::normalize_allowed_hosts(&allowed_hosts);
    let serve_task = crate::rt::spawn(async move {
        let hook_state = hooks::HookState {
            store: Arc::clone(&store),
            ssh: Arc::clone(&ssh),
        };
        let auth_state = AuthState {
            master: Arc::new(token),
            store: Arc::clone(&store),
            allowed_hosts: Arc::new(allowed_hosts.clone()),
        };
        let tools = FleetTools::new(store, ssh, reg, tunnels, guards);
        let service = streamable_service(tools, serve_shutdown.child_token(), &allowed_hosts);
        let app = build_app(axum::routing::any_service(service), hook_state, auth_state);

        tracing::info!("[mcp] control API listening on http://{addr}/mcp");
        let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
            serve_shutdown.cancelled().await;
        });
        if let Err(e) = serve.await {
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
        let app = build_app(any(|| async { "MCP_OK" }), hook_state, auth_state);

        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
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
            store,
            allowed_hosts: Arc::new(vec!["fleet.example.com".to_string()]),
        };
        let app2 = build_app(any(|| async { "MCP_OK" }), hook_state2, auth_state2);
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
        let tools = FleetTools::new(
            store,
            Arc::new(SshClient::new()),
            crate::cancel::CancellationRegistry::new(),
            Arc::new(crate::service::tunnel::TunnelSupervisor::new()),
            McpGuards::new(Arc::new(|_: &guard::ConfirmRequest| {})),
        );
        let service = streamable_service(tools, CancellationToken::new(), &allowed_hosts);
        let app = build_app(axum::routing::any_service(service), hook_state, auth_state);
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
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
