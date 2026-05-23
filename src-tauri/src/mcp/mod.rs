//! Embedded MCP server — claude-fleet's control API.
//!
//! claude-fleet speaks the Model Context Protocol itself, over a localhost-only
//! streamable-HTTP transport, so an AI assistant can drive it directly. The
//! server is off by default and enabled from Settings; every request must
//! carry a bearer token. See `docs/specs/2026-05-21-control-api-mcp-design.md`.

mod auth;
#[cfg(test)]
mod doc_gen;
pub mod hooks;
mod tools;

use crate::cancel::CancellationRegistry;
use crate::ssh::SshClient;
use crate::store::Store;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

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

/// Bind the listener and spawn the serve loop. Binds `127.0.0.1:<port>` only —
/// never a routable address. Returns the server's cancellation token on
/// success; an `Err` carries a human-readable bind failure (e.g. port in use).
pub async fn start(
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    reg: Arc<CancellationRegistry>,
    tunnels: Arc<crate::service::tunnel::TunnelSupervisor>,
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
        let hook_store = Arc::clone(&store);
        let tools = FleetTools::new(store, ssh, reg, tunnels);
        let service = StreamableHttpService::new(
            move || Ok(tools.clone()),
            LocalSessionManager::default().into(),
            StreamableHttpServerConfig::default()
                .with_cancellation_token(serve_shutdown.child_token()),
        );

        let token = Arc::new(token);

        // Bearer-auth applies ONLY to the /mcp route. The MCP streamable-HTTP
        // service is mounted with `route_service` at the exact `/mcp` path —
        // NOT `nest_service("/", …)` under `nest("/mcp", …)`. axum 0.8 panics on
        // nesting a service at the root ("Nesting at the root is no longer
        // supported"); that panic fired inside the spawned serve task *after*
        // the listener had bound, so `start` returned Ok and the UI showed the
        // server "running" while nothing was actually accepting connections.
        // Remote hosts then saw their reverse-tunnelled requests closed mid-
        // handshake ("socket connection was closed unexpectedly").
        let mcp_token = Arc::clone(&token);
        let mcp_router =
            axum::Router::new()
                .route_service("/mcp", service)
                .layer(axum::middleware::from_fn(
                    move |request: axum::extract::Request, next: axum::middleware::Next| {
                        let token = Arc::clone(&mcp_token);
                        async move {
                            // DNS-rebinding defense + bearer token, in that order.
                            match auth::check_request(request.headers(), &token) {
                                Ok(()) => Ok(next.run(request).await),
                                Err(status) => {
                                    eprintln!("[mcp] rejected request: {status}");
                                    Err(status)
                                }
                            }
                        }
                    },
                ));

        // /hook validates token via ?token= query param inside the handler.
        let hook_state = hooks::HookState {
            store: hook_store,
            token: Arc::clone(&token),
        };
        let hook_router = axum::Router::new()
            .route("/hook", axum::routing::post(hooks::handle_hook))
            .with_state(hook_state);

        let app = mcp_router.merge(hook_router);

        eprintln!("[mcp] control API listening on http://{addr}/mcp");
        let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
            serve_shutdown.cancelled().await;
        });
        if let Err(e) = serve.await {
            eprintln!("[mcp] server error: {e}");
        }
        eprintln!("[mcp] control API stopped");
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

    /// Regression: the serve task must build a router that actually accepts
    /// requests on `/mcp`. The previous `Router::new().nest_service("/", svc)`
    /// under `nest("/mcp", …)` panicked at construction in axum 0.8 ("Nesting at
    /// the root is no longer supported"), so the listener bound but nothing
    /// served — remote tunnelled clients saw the socket close mid-handshake.
    /// This boots the real routing shape (a dummy stands in for the rmcp
    /// service) and checks that `/mcp` routes through, gated by the bearer
    /// token, and that `/hook` stays un-gated.
    #[tokio::test]
    async fn mcp_route_serves_and_is_token_gated() {
        use axum::routing::{any, post};
        use std::net::Ipv4Addr;
        use std::sync::Arc;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let token = Arc::new("s3cret".to_string());
        let mcp_token = Arc::clone(&token);
        let app = axum::Router::new()
            .route_service("/mcp", any(|| async { "MCP_OK" }))
            .layer(axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| {
                    let token = Arc::clone(&mcp_token);
                    async move {
                        match auth::check_request(request.headers(), &token) {
                            Ok(()) => Ok(next.run(request).await),
                            Err(status) => Err(status),
                        }
                    }
                },
            ))
            .merge(axum::Router::new().route("/hook", post(|| async { "HOOK_OK" })));

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

        let post = |path: &str, auth: Option<&str>| {
            let mut h = format!(
                "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: application/json, \
                 text/event-stream\r\nContent-Type: application/json\r\nContent-Length: \
                 2\r\nConnection: close\r\n"
            );
            if let Some(a) = auth {
                h.push_str(&format!("Authorization: Bearer {a}\r\n"));
            }
            h.push_str("\r\n{}");
            h
        };

        // Valid token reaches the mounted service (proves /mcp routes, no panic).
        let ok = round_trip(addr, &post("/mcp", Some("s3cret"))).await;
        assert!(ok.contains("200 OK"), "expected 200 on /mcp, got:\n{ok}");
        assert!(
            ok.contains("MCP_OK"),
            "request did not reach service:\n{ok}"
        );

        // Missing/wrong token is rejected with 401 — not a dropped connection.
        let unauth = round_trip(addr, &post("/mcp", None)).await;
        assert!(
            unauth.contains("401"),
            "expected 401 without token, got:\n{unauth}"
        );

        // /hook is reachable and NOT behind the bearer layer.
        let hook = round_trip(addr, &post("/hook", None)).await;
        assert!(
            hook.contains("200 OK") && hook.contains("HOOK_OK"),
            "expected /hook to serve un-gated, got:\n{hook}"
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
}
