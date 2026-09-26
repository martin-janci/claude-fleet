//! `POST /attachment`: a paired client stages one file in a session's
//! worktree. Sits behind `authorize`, like `/report`.
//! Spec: fleet-mobile docs/superpowers/specs/2026-09-26-phone-attachments-design.md
//!
//! Why a route and not an MCP tool: the tool surface is under budget
//! pressure, and `tools/call` would carry the bytes base64 through a
//! transport built for control messages. This takes them raw.

use super::auth::{Caller, TokenMode};
use crate::ipc_error::codes;
use crate::service::attachments::{
    self, basenames_of, dedupe_names, resolve_worktree_root, run_script, transfer_all,
    UPLOAD_TIMEOUT_SECS,
};
use crate::ssh::SshClient;
use crate::store::Store;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub struct AttachmentState {
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
}

impl AttachmentState {
    pub fn new(store: Arc<Mutex<Store>>, ssh: Arc<SshClient>) -> Self {
        AttachmentState { store, ssh }
    }
}

#[derive(Deserialize)]
pub struct AttachmentQuery {
    pub session_id: i64,
    /// The file's name as the phone knows it. Treated as a *suggestion*: it
    /// is reduced to a basename before it is used, so it cannot be a path.
    pub name: String,
}

pub async fn handle_attachment(
    State(state): State<AttachmentState>,
    Extension(caller): Extension<Caller>,
    Query(q): Query<AttachmentQuery>,
    body: axum::body::Bytes,
) -> Response {
    if crate::mcp::auth::refuses_peer(&caller).is_some() {
        return StatusCode::FORBIDDEN.into_response();
    }
    // Writing bytes onto a machine in the fleet is not something a readonly
    // credential does. `/report` accepts one because posting your own crash
    // changes nothing; this is not that.
    if caller.mode != TokenMode::Full {
        return (StatusCode::FORBIDDEN, "attaching needs a full token").into_response();
    }

    // The name is reduced to a basename BEFORE anything else looks at it: a
    // `name` of `../../.ssh/authorized_keys` must land as
    // `authorized_keys` inside ATTACH_DIR, never outside it. Refuse empty,
    // control-character, or whitespace-only names (e.g. from "." or "..").
    let name = match basenames_of(std::slice::from_ref(&q.name))
        .pop()
        .filter(|n| {
            !n.is_empty()
                && !n.chars().any(|c| c.is_control())
                && !n.chars().all(|c| c.is_whitespace())
        }) {
        Some(n) => n,
        None => return (StatusCode::BAD_REQUEST, "no usable filename").into_response(),
    };

    // This cannot currently trigger: the route's body limit IS `MAX_BYTES`, so an
    // oversized upload is refused by the transport. But defense in depth is good,
    // and the spec asks for it.
    if let Err(why) = attachments::check_budget(&[(name.clone(), body.len() as u64)]) {
        return (StatusCode::PAYLOAD_TOO_LARGE, why).into_response();
    }

    // The guard is released before the first `.await`.
    let session = {
        let s = match state.store.lock() {
            Ok(s) => s,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        match s.get_session_by_id(q.session_id) {
            Ok(Some(row)) => row,
            Ok(None) => return (StatusCode::NOT_FOUND, "no such session").into_response(),
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    };

    let timeout = Duration::from_secs(UPLOAD_TIMEOUT_SECS);
    let host = &session.host_alias;
    let root = match resolve_worktree_root(&state.ssh, host, &session.tmux_name, timeout).await {
        Ok(r) => r,
        Err(e) => return attach_error(e),
    };
    let dir = format!("{root}/{}", attachments::ATTACH_DIR);
    if let Err(e) = run_script(&state.ssh, host, &attachments::stage_script(&root), timeout).await {
        return attach_error(e);
    }

    // The bytes arrive in memory (the body limit is the per-file ceiling), and
    // `transfer_all` reads from disk, so they go to a temp file first. It is
    // removed whatever happens next.
    let tmp = match spill(&body, &name) {
        Ok(t) => t,
        Err(e) => return attach_error(e),
    };
    let local = vec![tmp.path().to_string_lossy().into_owned()];
    let names = dedupe_names(&[name]);
    let out = transfer_all(&state.ssh, host, &local, &names, &dir, timeout).await;
    drop(tmp);

    match out {
        Ok(paths) => match paths.into_iter().next() {
            Some(path) => Json(serde_json::json!({ "path": path })).into_response(),
            None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        },
        Err(e) => attach_error(e),
    }
}

fn attach_error(e: crate::ipc_error::IpcError) -> Response {
    tracing::warn!(code = %e.code, error = %e.message, "[attachment] failed");
    // Codes produced by the attachment path: `E_SSH` (SSH command failed), `E_SSH_TIMEOUT`
    // (SSH command timed out), `E_AGENT_OFFLINE` (no agent connected), `E_UPLOAD`
    // (temp file or transfer failure), `E_CANCELLED` (work cancelled by the embedder).
    // Retry-able network issues map to 502; everything else is 500.
    let status = if matches!(
        e.code.as_str(),
        codes::E_SSH | codes::E_SSH_TIMEOUT | codes::E_AGENT_OFFLINE | codes::E_HOST_OFFLINE
    ) {
        StatusCode::BAD_GATEWAY
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, e.message).into_response()
}

/// Write the body to a temp file `transfer_all` can read.
fn spill(bytes: &[u8], name: &str) -> Result<tempfile::NamedTempFile, crate::ipc_error::IpcError> {
    use std::io::Write;
    let mut f = tempfile::Builder::new()
        .prefix("fleet-attach-")
        .suffix(&format!("-{name}"))
        .tempfile()
        .map_err(|e| crate::ipc_error::IpcError::new(codes::E_UPLOAD, format!("temp file: {e}")))?;
    f.write_all(bytes).map_err(|e| {
        crate::ipc_error::IpcError::new(codes::E_UPLOAD, format!("temp write: {e}"))
    })?;
    Ok(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::guard::RateLimiter;
    use crate::mcp::{auth, build_app, events_route::EventsState, hooks, pairing, AuthState};
    use crate::ssh::SshClient;
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
            s.insert_client_token("phone-full", &auth::sha256_hex("full-tok"), "full")
                .unwrap();
        }
        let ssh = Arc::new(SshClient::new());
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
                ssh: Arc::clone(&ssh),
            },
            AuthState {
                master: Arc::new("s3cret".to_string()),
                store: Arc::clone(&store),
                allowed_hosts: Arc::new(vec![]),
            },
            pairing::PairState::new(
                Arc::clone(&store),
                Arc::new(pairing::PendingPairings::new()),
                Arc::new(RateLimiter::new()),
                "https://fleet.example.com".to_string(),
            ),
            EventsState::disabled(),
            crate::agent::ws::AgentWsState::disabled(),
            crate::mcp::report_route::ReportState::new(Arc::clone(&store)),
            AttachmentState::new(Arc::clone(&store), Arc::clone(&ssh)),
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
             Content-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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

    #[tokio::test]
    async fn a_readonly_client_may_not_upload() {
        let (addr, _store) = app().await;
        let (st, _) = http(
            addr,
            "POST",
            "/attachment?session_id=1&name=a.txt",
            "ro-tok",
            "hi",
        )
        .await;
        assert_eq!(
            st, 403,
            "a readonly credential must not write bytes to a host"
        );
    }

    #[tokio::test]
    async fn an_unauthenticated_upload_is_refused() {
        let (addr, _store) = app().await;
        let (st, _) = http(
            addr,
            "POST",
            "/attachment?session_id=1&name=a.txt",
            "wrong",
            "hi",
        )
        .await;
        assert_eq!(st, 401);
    }

    #[tokio::test]
    async fn a_full_client_gets_past_the_mode_gate() {
        let (addr, _store) = app().await;
        let (st, _) = http(
            addr,
            "POST",
            "/attachment?session_id=1&name=a.txt",
            "full-tok",
            "hi",
        )
        .await;
        assert_eq!(
            st, 404,
            "full token should pass the mode gate and reach the store lookup; \
             404 means no such session row (as expected)"
        );
    }

    #[tokio::test]
    async fn a_filename_with_no_usable_component_is_refused_with_400() {
        let (addr, _store) = app().await;
        // `name=..` (URL-encoded as %2E%2E) reduces to empty. This test pins the
        // hardening: without it, basenames_of would return "file", reaching the
        // session lookup and failing with 404. With hardening, it returns empty,
        // the filter rejects it with 400 "no usable filename" — the only
        // route-level assertion that discriminates the weak from hardened
        // basenames_of.
        let (st, body) = http(
            addr,
            "POST",
            "/attachment?session_id=1&name=%2E%2E",
            "full-tok",
            "content",
        )
        .await;
        assert_eq!(st, 400, ".. should be refused with 400, got {st}: {body}");
        assert!(
            body.contains("no usable filename"),
            "error should explain the problem: {body}"
        );
    }

    #[tokio::test]
    async fn a_body_over_the_per_file_ceiling_is_refused() {
        let (addr, _store) = app().await;
        let big = "x".repeat(fleet_core_max_bytes() + 1);
        let (st, _) = http(
            addr,
            "POST",
            "/attachment?session_id=1&name=big.bin",
            "full-tok",
            &big,
        )
        .await;
        assert_eq!(
            st, 413,
            "the body limit layer should refuse before the handler runs"
        );
    }

    fn fleet_core_max_bytes() -> usize {
        crate::service::attachments::MAX_BYTES as usize
    }
}
