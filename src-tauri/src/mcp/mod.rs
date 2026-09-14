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
mod tools;

use crate::cancel::CancellationRegistry;
use crate::ssh::SshClient;
use crate::store::Store;
use rmcp::transport::streamable_http_server::{
    session::never::NeverSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

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
/// re-provisioned yet keeps reporting until it is. Header form is preferred
/// and the only form the current hook block writes.
async fn authorize(
    axum::extract::State(state): axum::extract::State<AuthState>,
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, axum::http::StatusCode> {
    use axum::http::StatusCode;
    // Sync lock, released before the next `.await` — never held across one.
    let host_tokens = {
        let s = state
            .store
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        s.list_host_tokens().unwrap_or_default()
    };
    let caller = match auth::check_request(request.headers(), &state.master, &host_tokens) {
        Ok(c) => c,
        // Header missing/unknown on /hook: try the legacy query form. Only
        // the MASTER token is accepted here — that is the only token the
        // pre-0.3 `curl …?token=` command hooks ever carried, and a per-host
        // token must never travel in a URL. TRANSITIONAL: removed in 0.4;
        // re-provision every host before then.
        Err(StatusCode::UNAUTHORIZED) if request.uri().path() == "/hook" => {
            query_token(request.uri().query())
                .and_then(|t| auth::resolve_token(t, &state.master, &[]))
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
    request.extensions_mut().insert(caller);
    Ok(next.run(request).await)
}

/// Build the axum app: `/mcp` (rmcp service) and `/hook`, both behind
/// [`authorize`]. Shared by `start` and the routing test.
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
    axum::Router::new()
        .route("/mcp", mcp_service)
        .route("/hook", axum::routing::post(hooks::handle_hook))
        .with_state(hook_state)
        .layer(axum::middleware::from_fn_with_state(auth_state, authorize))
}

/// The rmcp streamable-HTTP service in **stateless** mode: every POST is a
/// self-contained JSON-RPC exchange served by a fresh `FleetTools` clone, no
/// `Mcp-Session-Id` is issued or required, and `GET`/`DELETE` are refused
/// (405). The server never sends server-initiated messages (tools only), so a
/// session bought nothing and cost a reconnect after every app restart, port
/// or token change, or tunnel bounce. Responses keep SSE framing
/// (`json_response` default `false`) so the 15 s keep-alive still flows on
/// long polls (`wait_for_session`, `run_prompt`) through the reverse tunnel.
pub(crate) fn streamable_service(
    tools: FleetTools,
    cancel: CancellationToken,
) -> StreamableHttpService<FleetTools, NeverSessionManager> {
    StreamableHttpService::new(
        move || Ok(tools.clone()),
        NeverSessionManager::default().into(),
        StreamableHttpServerConfig::default()
            .with_stateful_mode(false)
            .with_cancellation_token(cancel),
    )
}

/// Bind the listener and spawn the serve loop. Binds `127.0.0.1:<port>` only —
/// never a routable address. Returns the server's cancellation token on
/// success; an `Err` carries a human-readable bind failure (e.g. port in use).
pub async fn start(
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    reg: Arc<CancellationRegistry>,
    tunnels: Arc<crate::service::tunnel::TunnelSupervisor>,
    guards: McpGuards,
    port: u16,
    token: String,
) -> Result<CancellationToken, String> {
    // Localhost only. This is an invariant, not a configurable: a routable
    // bind would expose fleet control to the network.
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("could not bind {addr}: {e}"))?;

    let shutdown = CancellationToken::new();
    let serve_shutdown = shutdown.clone();
    tauri::async_runtime::spawn(async move {
        let hook_state = hooks::HookState {
            store: Arc::clone(&store),
            ssh: Arc::clone(&ssh),
        };
        let auth_state = AuthState {
            master: Arc::new(token),
            store: Arc::clone(&store),
        };
        let tools = FleetTools::new(store, ssh, reg, tunnels, guards);
        let service = streamable_service(tools, serve_shutdown.child_token());
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

    Ok(shutdown)
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
        }
        let hook_state = hooks::HookState {
            store: Arc::clone(&store),
            ssh: Arc::new(SshClient::new()),
        };
        let auth_state = AuthState {
            master: Arc::new("s3cret".to_string()),
            store,
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
        use std::net::Ipv4Addr;
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        let hook_state = hooks::HookState {
            store: Arc::clone(&store),
            ssh: Arc::new(SshClient::new()),
        };
        let auth_state = AuthState {
            master: Arc::new("s3cret".to_string()),
            store: Arc::clone(&store),
        };
        let tools = FleetTools::new(
            store,
            Arc::new(SshClient::new()),
            crate::cancel::CancellationRegistry::new(),
            Arc::new(crate::service::tunnel::TunnelSupervisor::new()),
            McpGuards::new(Arc::new(|_: &guard::ConfirmRequest| {})),
        );
        let service = streamable_service(tools, CancellationToken::new());
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
        format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: application/json, \
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
}
