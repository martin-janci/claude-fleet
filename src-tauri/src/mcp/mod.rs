//! Embedded MCP server — claude-fleet's control API.
//!
//! claude-fleet speaks the Model Context Protocol itself, over a localhost-only
//! streamable-HTTP transport, so an AI assistant can drive it directly. The
//! server is off by default and enabled from Settings; every request must
//! carry a bearer token. See `docs/specs/2026-05-21-control-api-mcp-design.md`.

mod auth;
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
        let tools = FleetTools::new(store, ssh, reg);
        let service = StreamableHttpService::new(
            move || Ok(tools.clone()),
            LocalSessionManager::default().into(),
            StreamableHttpServerConfig::default()
                .with_cancellation_token(serve_shutdown.child_token()),
        );

        let token = Arc::new(token);

        // Bearer-auth applies ONLY to /mcp routes.
        let mcp_token = Arc::clone(&token);
        let mcp_router =
            axum::Router::new()
                .nest_service("/", service)
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
            .route("/", axum::routing::post(hooks::handle_hook))
            .with_state(hook_state);

        let app = axum::Router::new()
            .nest("/mcp", mcp_router)
            .nest("/hook", hook_router);

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
