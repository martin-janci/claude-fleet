//! The control API's HTTP serve loop: what `axum::serve` does, plus the two
//! bounds it lacks.
//!
//! `axum::serve` builds hyper's connection builder without a timer, so hyper
//! never applies a header-read deadline, and it takes every connection the
//! listener hands it. A peer that opens connections and then sends one
//! header byte (or nothing) held each one, with its file descriptor, for as
//! long as TCP allowed, with no token needed (auth runs only once the
//! headers are in): about a thousand of them put the hub into `EMFILE`, and
//! with it SQLite, SSH and every agent socket. Here every connection must
//! finish its request head within [`HEADER_READ_TIMEOUT`], and at most
//! [`MAX_CONNECTIONS`] are served at once; one over that is closed at once.

use axum::extract::ConnectInfo;
use axum::Router;
use hyper_util::rt::{TokioIo, TokioTimer};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tower_service::Service as _;

/// How many connections the control API serves at once. Far above a real
/// fleet (an agent per host, a few desktops and phones, their long polls and
/// event streams), and far below a 1024 file-descriptor limit, so a flood
/// fills this before it starves the rest of the process.
pub(crate) const MAX_CONNECTIONS: usize = 512;

/// How long a connection has to deliver a request's head (the request line
/// and headers), counted from when hyper starts waiting for it: from the
/// accept for the first request, and from the end of the last response on a
/// kept-alive connection, so an idle keep-alive is closed after it too.
pub(crate) const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// The bounds [`serve`] applies; a parameter so a test can shrink them.
#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub max_connections: usize,
    pub header_read_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_connections: MAX_CONNECTIONS,
            header_read_timeout: HEADER_READ_TIMEOUT,
        }
    }
}

/// Serves `app` on `listener` until `shutdown`, then lets the connections
/// in flight finish (each told to close once its current request is done)
/// before returning: the same graceful shutdown `axum::serve` gives. Every
/// request carries the peer's `ConnectInfo<SocketAddr>`.
pub(crate) async fn serve<L>(listener: L, app: Router, shutdown: CancellationToken, limits: Limits)
where
    L: axum::serve::Listener<Addr = SocketAddr>,
    L::Io: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut listener = listener;
    let permits = Arc::new(Semaphore::new(limits.max_connections));
    let conns = TaskTracker::new();
    let mut last_full_warn: Option<tokio::time::Instant> = None;
    loop {
        let (io, peer) = tokio::select! {
            biased;
            () = shutdown.cancelled() => break,
            conn = listener.accept() => conn,
        };
        let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
            if last_full_warn.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
                last_full_warn = Some(tokio::time::Instant::now());
                tracing::warn!(
                    %peer,
                    max = limits.max_connections,
                    "[mcp] connection limit reached; closing new connections until some end"
                );
            }
            drop(io);
            continue;
        };
        let app = app.clone();
        let shutdown = shutdown.clone();
        conns.spawn(async move {
            let _permit = permit;
            let svc = hyper::service::service_fn(
                move |mut req: hyper::Request<hyper::body::Incoming>| {
                    req.extensions_mut().insert(ConnectInfo(peer));
                    let mut app = app.clone();
                    async move { app.call(req.map(axum::body::Body::new)).await }
                },
            );
            // hyper's own HTTP/1 builder, not hyper-util's auto one: the
            // auto builder sniffs the first bytes for an h2 preface with no
            // deadline at all, before any header timer starts, so a silent
            // connection escaped it. The hub's TLS offers no ALPN, so no
            // client speaks h2 to it anyway.
            let conn = hyper::server::conn::http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(limits.header_read_timeout)
                .serve_connection(TokioIo::new(io), svc)
                .with_upgrades();
            let mut conn = std::pin::pin!(conn);
            tokio::select! {
                r = conn.as_mut() => {
                    if let Err(e) = r {
                        tracing::trace!(%peer, error = %e, "[mcp] connection ended with an error");
                    }
                    return;
                }
                () = shutdown.cancelled() => conn.as_mut().graceful_shutdown(),
            }
            if let Err(e) = conn.await {
                tracing::trace!(%peer, error = %e, "[mcp] connection ended with an error");
            }
        });
    }
    conns.close();
    conns.wait().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn start(limits: Limits) -> (SocketAddr, CancellationToken, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route(
                "/peer",
                axum::routing::get(|ConnectInfo(peer): ConnectInfo<SocketAddr>| async move {
                    peer.ip().to_string()
                }),
            )
            .route(
                "/ws",
                axum::routing::get(|ws: axum::extract::ws::WebSocketUpgrade| async move {
                    ws.on_upgrade(|mut socket| async move {
                        while let Some(Ok(m)) = socket.recv().await {
                            if socket.send(m).await.is_err() {
                                break;
                            }
                        }
                    })
                }),
            );
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = CancellationToken::new();
        let task = tokio::spawn(serve(listener, app, stop.clone(), limits));
        (addr, stop, task)
    }

    const PATIENCE: Duration = Duration::from_secs(5);

    /// Reads until the peer closes; `None` if it is still open after `within`.
    async fn closes_within(s: &mut tokio::net::TcpStream, within: Duration) -> Option<Vec<u8>> {
        let mut buf = Vec::new();
        tokio::time::timeout(within, s.read_to_end(&mut buf))
            .await
            .ok()
            .map(|_| buf)
    }

    async fn get_peer(addr: SocketAddr) -> String {
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(b"GET /peer HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let raw = closes_within(&mut s, PATIENCE).await.expect("an answer");
        String::from_utf8_lossy(&raw).into_owned()
    }

    #[tokio::test]
    async fn a_request_is_served_with_the_peer_address() {
        let (addr, stop, task) = start(Limits::default()).await;
        let resp = get_peer(addr).await;
        assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
        assert!(resp.ends_with("127.0.0.1"), "{resp}");
        stop.cancel();
        tokio::time::timeout(PATIENCE, task).await.unwrap().unwrap();
    }

    /// A peer that sends part of a request head and then stalls is closed
    /// once the header-read deadline passes, as is one that sends nothing.
    #[tokio::test]
    async fn a_stalled_request_head_is_closed() {
        let limits = Limits {
            header_read_timeout: Duration::from_millis(300),
            ..Limits::default()
        };
        let (addr, _stop, _task) = start(limits).await;
        let mut dripping = tokio::net::TcpStream::connect(addr).await.unwrap();
        dripping
            .write_all(b"GET /peer HTTP/1.1\r\nHo")
            .await
            .unwrap();
        let mut silent = tokio::net::TcpStream::connect(addr).await.unwrap();
        assert!(
            closes_within(&mut dripping, PATIENCE).await.is_some(),
            "a half-sent head held its connection"
        );
        assert!(
            closes_within(&mut silent, PATIENCE).await.is_some(),
            "a silent connection was held"
        );
    }

    /// Past the cap a new connection is closed at once, and once one ends
    /// its slot is free again.
    #[tokio::test]
    async fn connections_past_the_cap_are_closed_until_one_ends() {
        let limits = Limits {
            max_connections: 2,
            header_read_timeout: Duration::from_secs(60),
        };
        let (addr, _stop, _task) = start(limits).await;
        let held1 = tokio::net::TcpStream::connect(addr).await.unwrap();
        let held2 = tokio::net::TcpStream::connect(addr).await.unwrap();
        // Let the server accept both before the third arrives.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut third = tokio::net::TcpStream::connect(addr).await.unwrap();
        let _ = third
            .write_all(b"GET /peer HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await;
        let raw = closes_within(&mut third, PATIENCE).await.expect("closed");
        assert!(raw.is_empty(), "a connection past the cap was served");
        drop(held1);
        let mut resp = String::new();
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            resp = get_peer(addr).await;
            if resp.starts_with("HTTP/1.1 200") {
                break;
            }
        }
        assert!(
            resp.starts_with("HTTP/1.1 200"),
            "the freed slot was not reused: {resp}"
        );
        drop(held2);
    }

    /// Websocket upgrades still work through the custom loop.
    #[tokio::test]
    async fn a_websocket_upgrades_and_echoes() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let (addr, stop, task) = start(Limits::default()).await;
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
            .await
            .unwrap();
        ws.send(Message::Text("hi".into())).await.unwrap();
        let got = tokio::time::timeout(PATIENCE, ws.next()).await.unwrap();
        assert_eq!(got.unwrap().unwrap(), Message::Text("hi".into()));
        drop(ws);
        stop.cancel();
        tokio::time::timeout(PATIENCE, task).await.unwrap().unwrap();
    }
}
