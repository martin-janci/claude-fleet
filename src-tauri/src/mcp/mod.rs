//! Embedded MCP server — claude-fleet's control API.
//!
//! claude-fleet speaks the Model Context Protocol itself, over a localhost-only
//! streamable-HTTP transport, so an AI assistant can drive it directly. The
//! server is off by default and enabled from Settings; every request must
//! carry a bearer token. See `docs/specs/2026-05-21-control-api-mcp-design.md`.

mod auth;
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

/// Spawn the MCP server on a background task. Binds `127.0.0.1:<port>` only —
/// never a routable address. The server stops when `shutdown` is cancelled.
/// A bind failure (e.g. the port is taken) is logged and otherwise ignored:
/// the desktop app keeps running without a control API.
pub fn spawn_server(
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    reg: Arc<CancellationRegistry>,
    port: u16,
    token: String,
    shutdown: CancellationToken,
) {
    tauri::async_runtime::spawn(async move {
        let tools = FleetTools::new(store, ssh, reg);
        let service = StreamableHttpService::new(
            move || Ok(tools.clone()),
            LocalSessionManager::default().into(),
            StreamableHttpServerConfig::default().with_cancellation_token(shutdown.child_token()),
        );

        let token = Arc::new(token);
        let app =
            axum::Router::new()
                .nest_service("/mcp", service)
                .layer(axum::middleware::from_fn(
                    move |request: axum::extract::Request, next: axum::middleware::Next| {
                        let token = Arc::clone(&token);
                        async move {
                            let header = request.headers().get(axum::http::header::AUTHORIZATION);
                            if auth::bearer_matches(header, &token) {
                                Ok(next.run(request).await)
                            } else {
                                Err(axum::http::StatusCode::UNAUTHORIZED)
                            }
                        }
                    },
                ));

        // Localhost only. This is an invariant, not a configurable: a routable
        // bind would expose fleet control to the network.
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[mcp] failed to bind {addr}: {e} — control API not started");
                return;
            }
        };
        eprintln!("[mcp] control API listening on http://{addr}/mcp");

        let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
            shutdown.cancelled().await;
        });
        if let Err(e) = serve.await {
            eprintln!("[mcp] server error: {e}");
        }
    });
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
}
