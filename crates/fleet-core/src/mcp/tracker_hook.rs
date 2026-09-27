//! `POST /hooks/tracker/{id}`: a tracker's webhook nudge (work graph
//! M13.4f, decisions D13 / D28). The logic is
//! [`crate::service::trackers::webhook`]; this is the HTTP edge.
//!
//! **Unauthenticated by bearer token, by design**, like `/pair`: a tracker
//! cannot hold a fleet token. What stands in for it is the tracker's own
//! HMAC signature over the body. The route is mounted only on a hub
//! ([`crate::mcp::McpGuards::with_tracker_hooks`]), answers 404 unless the
//! hub has a public URL and the tracker a webhook secret, spends a rate
//! budget per tracker and per address before reading anything, and reads at
//! most [`webhook::BODY_MAX`] bytes. It is outside the `Host` allowlist too:
//! a tracker calls the public name, and the signature, not the Host header,
//! is what authorizes it.
//!
//! Nothing from the body is logged, and a refusal says no more than its
//! status code.

use super::guard::RateLimiter;
use crate::service::trackers::webhook::{self, Coalescer, Outcome};
use crate::store::Store;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Deliveries to one tracker, at most one per this interval.
pub const PER_TRACKER: Duration = Duration::from_millis(100);
/// Deliveries from one address, at most one per this interval.
pub const PER_ADDRESS: Duration = Duration::from_millis(50);

/// The route's state.
#[derive(Clone)]
pub struct TrackerHookState {
    store: Arc<Mutex<Store>>,
    rate: Arc<RateLimiter>,
    coalescer: Arc<Coalescer>,
    /// How long a burst for one item waits for its one fetch.
    pub(crate) window: Duration,
}

impl TrackerHookState {
    pub fn new(store: Arc<Mutex<Store>>, rate: Arc<RateLimiter>) -> Self {
        Self {
            store,
            rate,
            coalescer: Arc::new(Coalescer::new()),
            window: webhook::COALESCE,
        }
    }
}

/// The route, with its state.
pub fn router(state: TrackerHookState) -> axum::Router {
    axum::Router::new()
        .route("/hooks/tracker/{id}", axum::routing::post(handle))
        .with_state(state)
}

fn too_many(left: Duration) -> Response {
    let retry = left.as_secs().max(1).to_string();
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(axum::http::header::RETRY_AFTER, retry)],
    )
        .into_response()
}

async fn handle(
    axum::extract::State(state): axum::extract::State<TrackerHookState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: axum::extract::Request,
) -> Response {
    let Ok(id) = id.parse::<i64>() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let peer = super::pairing::limiter_key(
        request
            .extensions()
            .get::<axum::extract::ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip()),
        request.headers(),
    );
    // Budgets first: a flood costs a hash-map probe, not a body read.
    if let Err(left) = state.rate.check(&format!("hook-addr:{peer}"), PER_ADDRESS) {
        return too_many(left);
    }
    if let Err(left) = state.rate.check(&format!("hook-tracker:{id}"), PER_TRACKER) {
        return too_many(left);
    }
    let headers = request.headers().clone();
    let body = match axum::body::to_bytes(request.into_body(), webhook::BODY_MAX).await {
        Ok(b) => b,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default();
    match webhook::deliver(
        &state.coalescer,
        &state.store,
        id,
        &header,
        &body,
        now_ms,
        state.window,
    ) {
        Outcome::NotHere => StatusCode::NOT_FOUND.into_response(),
        Outcome::Rejected(_) => {
            tracing::warn!(tracker = id, peer = %peer, "[webhook] refused a delivery");
            StatusCode::UNAUTHORIZED.into_response()
        }
        Outcome::Malformed => StatusCode::BAD_REQUEST.into_response(),
        Outcome::Accepted(_) => StatusCode::ACCEPTED.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const SECRET: &str = "0123456789abcdef0123456789abcdef";

    /// A served route on an ephemeral port; returns its address and the
    /// Jira tracker's id.
    async fn serve(public: bool) -> (SocketAddr, i64) {
        let s = Store::open_in_memory().unwrap();
        if public {
            s.set_setting(crate::service::hub::SETTING_PUBLIC_URL, "hub.example.com")
                .unwrap();
        }
        let t = s
            .add_tracker("jira", "Acme", "https://acme.atlassian.net")
            .unwrap();
        s.set_tracker_webhook_secret(t.id, &crate::store::Secret::new(SECRET))
            .unwrap();
        let mut state =
            TrackerHookState::new(Arc::new(Mutex::new(s)), Arc::new(RateLimiter::new()));
        state.window = Duration::from_secs(3_600);
        let app = router(state);
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        (addr, t.id)
    }

    /// One POST; returns the status and the response head.
    async fn post(addr: SocketAddr, id: &str, body: &[u8], sig: Option<String>) -> (u16, String) {
        let sig = sig
            .map(|s| format!("X-Hub-Signature: {s}\r\n"))
            .unwrap_or_default();
        let head = format!(
            "POST /hooks/tracker/{id} HTTP/1.1\r\nHost: tracker.example\r\n{sig}\
             Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(head.as_bytes()).await.unwrap();
        // The server may answer (413) before the whole body is sent.
        let _ = s.write_all(body).await;
        let mut raw = Vec::new();
        let _ = s.read_to_end(&mut raw).await;
        let text = String::from_utf8_lossy(&raw).into_owned();
        let head = text
            .split("\r\n\r\n")
            .next()
            .unwrap_or_default()
            .to_string();
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        (status, head)
    }

    fn signed(body: &[u8]) -> Option<String> {
        Some(format!(
            "sha256={}",
            webhook::hmac_sha256_hex(SECRET.as_bytes(), body)
        ))
    }

    const BODY: &[u8] = br#"{"issue":{"key":"ABC-1"}}"#;

    #[tokio::test]
    async fn a_signed_delivery_is_accepted_and_a_forged_one_refused() {
        let (addr, id) = serve(true).await;
        let id = id.to_string();
        assert_eq!(post(addr, &id, BODY, signed(BODY)).await.0, 202);
        tokio::time::sleep(PER_TRACKER * 2).await;
        assert_eq!(post(addr, &id, BODY, Some("sha256=00".into())).await.0, 401);
        tokio::time::sleep(PER_TRACKER * 2).await;
        assert_eq!(post(addr, &id, BODY, None).await.0, 401);
    }

    #[tokio::test]
    async fn without_a_public_url_or_for_an_unknown_tracker_it_is_not_there() {
        let (addr, id) = serve(false).await;
        assert_eq!(post(addr, &id.to_string(), BODY, signed(BODY)).await.0, 404);
        let (addr, _) = serve(true).await;
        for id in ["999", "x", "-1"] {
            assert_eq!(post(addr, id, BODY, signed(BODY)).await.0, 404, "{id}");
            tokio::time::sleep(PER_ADDRESS * 2).await;
        }
    }

    #[tokio::test]
    async fn a_body_over_the_cap_is_refused_before_it_is_verified() {
        let (addr, id) = serve(true).await;
        let body = vec![b' '; webhook::BODY_MAX + 1];
        assert_eq!(
            post(addr, &id.to_string(), &body, signed(&body)).await.0,
            413
        );
    }

    #[tokio::test]
    async fn a_burst_is_rate_limited_with_a_retry_after() {
        let (addr, id) = serve(true).await;
        let id = id.to_string();
        assert_eq!(post(addr, &id, BODY, signed(BODY)).await.0, 202);
        let (status, head) = post(addr, &id, BODY, signed(BODY)).await;
        assert_eq!(status, 429);
        assert!(head.to_ascii_lowercase().contains("retry-after:"), "{head}");
    }
}
