//! `/report` (any authenticated caller posts its error batch) and `/reports`
//! (the master reads them back). Both sit behind `authorize`, like `/hook`.
//! Spec: docs/superpowers/specs/2026-09-21-hub-error-channel-design.md

use super::auth::Caller;
use crate::service::reports::{ingest, RateWindows};
use crate::store::{ReportFilter, Store};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use fleet_proto::report::{ReportBatch, HTTP_BATCH_MAX};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct ReportState {
    store: Arc<Mutex<Store>>,
    windows: Arc<RateWindows>,
}

impl ReportState {
    pub fn new(store: Arc<Mutex<Store>>) -> Self {
        ReportState {
            store,
            windows: Arc::new(RateWindows::new()),
        }
    }
}

/// `POST /report`: any authenticated caller — `readonly` included, since
/// reporting an error changes nothing about the fleet.
pub async fn handle_report(
    State(state): State<ReportState>,
    Extension(caller): Extension<Caller>,
    body: axum::body::Bytes,
) -> StatusCode {
    use crate::ipc_error::codes;
    // A hub link's only door is `peer_exchange` on `/mcp`; this route is not
    // it. `auth::refuses_peer` is the same gate `/events` applies — kept in
    // one place so the two routes cannot drift on who counts as a peer.
    if crate::mcp::auth::refuses_peer(&caller).is_some() {
        return StatusCode::FORBIDDEN;
    }
    let batch: ReportBatch = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => {
            tracing::debug!(caller = %caller.label(), error = %e, "[report] rejected body");
            return StatusCode::BAD_REQUEST;
        }
    };
    let origin = caller.label();
    match ingest(
        &state.store,
        &state.windows,
        &origin,
        batch,
        HTTP_BATCH_MAX,
        fleet_proto::report::now_unix(),
    ) {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(e) if e.code == codes::E_VALIDATE => StatusCode::BAD_REQUEST,
        Err(e) if e.code == codes::E_RATE_LIMITED => StatusCode::TOO_MANY_REQUESTS,
        Err(e) => {
            tracing::error!(code = %e.code, error = %e.message, "[report] store failed");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// `GET /reports`: master only — the rows hold every client's messages.
pub async fn handle_reports(
    State(state): State<ReportState>,
    Extension(caller): Extension<Caller>,
    Query(mut filter): Query<ReportFilter>,
) -> Response {
    if !caller.is_master() {
        return (
            StatusCode::FORBIDDEN,
            "reports are the master token's to read",
        )
            .into_response();
    }
    filter.limit = filter.limit.clamp(1, 1000);
    let rows = match state.store.lock() {
        Ok(s) => s.list_reports(&filter),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    match rows {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => {
            tracing::error!(code = %e.code, error = %e.message, "[reports] list failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::guard::RateLimiter;
    use crate::mcp::{auth, build_app, events_route::EventsState, hooks, pairing, AuthState};
    use crate::ssh::SshClient;
    use fleet_proto::report::{Report, ReportBatch, BODY_MAX, HTTP_BATCH_MAX};
    use std::net::{Ipv4Addr, SocketAddr};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn app() -> (SocketAddr, Arc<Mutex<Store>>) {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        {
            let s = store.lock().unwrap();
            s.upsert_host("box").unwrap();
            s.upsert_host_token("box", "host-tok").unwrap();
            s.insert_client_token("phone", &auth::sha256_hex("ro-tok"), "readonly")
                .unwrap();
        }
        let app = build_app(
            crate::mcp::metrics::MetricsState {
                metrics: std::sync::Arc::new(crate::mcp::metrics::Metrics::new()),
                streams: crate::mcp::guard::LongPollLimiter::new(
                    crate::mcp::guard::MAX_LONG_POLLS_PER_CALLER,
                ),
            },
            axum::routing::any(|| async { "MCP_OK" }),
            None,
            hooks::HookState {
                store: Arc::clone(&store),
                ssh: Arc::new(SshClient::new()),
            },
            AuthState {
                master: Arc::new("s3cret".to_string()),
                store: Arc::clone(&store),
                allowed_hosts: Arc::new(vec![]),
                tokens: None,
            },
            pairing::PairState::new(
                Arc::clone(&store),
                Arc::new(pairing::PendingPairings::new()),
                Arc::new(RateLimiter::new()),
                "https://fleet.example.com".to_string(),
            ),
            EventsState::disabled(),
            crate::agent::ws::AgentWsState::disabled(),
            ReportState::new(Arc::clone(&store)),
        );
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
        (addr, store)
    }

    /// One request; returns (status, body).
    async fn http(
        addr: SocketAddr,
        method: &str,
        path: &str,
        token: &str,
        body: &str,
    ) -> (u16, String) {
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(req.as_bytes()).await.unwrap();
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).await.unwrap();
        let text = String::from_utf8_lossy(&raw).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, body.to_string())
    }

    fn batch(n: usize) -> String {
        serde_json::to_string(&ReportBatch {
            reports: vec![Report::error("frontend", "boom"); n],
            dropped: 0,
        })
        .unwrap()
    }

    #[tokio::test]
    async fn a_readonly_client_may_post_and_the_master_reads_it_back() {
        let (addr, _store) = app().await;
        let (st, _) = http(addr, "POST", "/report", "ro-tok", &batch(2)).await;
        assert_eq!(st, 204);
        let (st, _) = http(addr, "POST", "/report", "host-tok", &batch(1)).await;
        assert_eq!(st, 204);
        let (st, body) = http(addr, "GET", "/reports?limit=10", "s3cret", "").await;
        assert_eq!(st, 200);
        let rows: Vec<serde_json::Value> = serde_json::from_str(&body).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["origin"], "host:box");
        assert_eq!(rows[1]["origin"], "client:phone");
        let (st, body) = http(addr, "GET", "/reports?origin=host:box", "s3cret", "").await;
        assert_eq!(st, 200);
        assert_eq!(
            serde_json::from_str::<Vec<serde_json::Value>>(&body)
                .unwrap()
                .len(),
            1
        );
    }

    /// A peer token (a linked hub) is refused both routes: `/report` is not
    /// `peer_exchange`, and `/reports` is master-only already. A hub link's
    /// only door is `peer_exchange` on `/mcp`.
    #[tokio::test]
    async fn a_peer_token_is_refused_both_routes() {
        let (addr, store) = app().await;
        {
            let s = store.lock().unwrap();
            s.insert_client_token("hub-b", &auth::sha256_hex("peer-tok"), "peer")
                .unwrap();
        }
        assert_eq!(
            http(addr, "POST", "/report", "peer-tok", &batch(1)).await.0,
            403
        );
        assert_eq!(http(addr, "GET", "/reports", "peer-tok", "").await.0, 403);
    }

    #[tokio::test]
    async fn reading_is_master_only_and_posting_needs_a_token() {
        let (addr, _) = app().await;
        assert_eq!(http(addr, "GET", "/reports", "ro-tok", "").await.0, 403);
        assert_eq!(http(addr, "GET", "/reports", "host-tok", "").await.0, 403);
        assert_eq!(
            http(addr, "POST", "/report", "nonsense", &batch(1)).await.0,
            401
        );
    }

    #[tokio::test]
    async fn bad_shapes_are_400_and_413_and_the_rate_limit_is_429() {
        let (addr, store) = app().await;
        assert_eq!(
            http(addr, "POST", "/report", "ro-tok", "{not json").await.0,
            400
        );
        assert_eq!(
            http(
                addr,
                "POST",
                "/report",
                "ro-tok",
                &batch(HTTP_BATCH_MAX + 1)
            )
            .await
            .0,
            400
        );
        let huge = format!(
            r#"{{"reports":[],"dropped":0,"pad":"{}"}}"#,
            "x".repeat(BODY_MAX)
        );
        assert_eq!(http(addr, "POST", "/report", "ro-tok", &huge).await.0, 413);
        // 50 + 10 fills the minute; the 61st is refused and nothing of it stored.
        assert_eq!(
            http(addr, "POST", "/report", "ro-tok", &batch(50)).await.0,
            204
        );
        assert_eq!(
            http(addr, "POST", "/report", "ro-tok", &batch(10)).await.0,
            204
        );
        assert_eq!(
            http(addr, "POST", "/report", "ro-tok", &batch(1)).await.0,
            429
        );
        let n = store
            .lock()
            .unwrap()
            .list_reports(&crate::store::ReportFilter {
                limit: 1000,
                ..Default::default()
            })
            .unwrap()
            .len();
        assert_eq!(n, 60);
    }
}
