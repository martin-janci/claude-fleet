//! Serving an already-bound listener, optionally behind a TLS acceptor the
//! *caller* supplies.
//!
//! `fleet-core` deliberately takes no TLS dependency: the desktop terminates
//! nothing (loopback only) and the `fleet-hub` daemon is the single place that
//! owns rustls. So the crate boundary is this trait — the hub implements it in
//! `fleet_hub::tls` — plus [`TlsListener`], an `axum::serve::Listener` that
//! hands axum the connections the acceptor has already wrapped.
//!
//! The handshake runs in its own task, never in the accept loop: a peer that
//! opens a connection and then says nothing must not stop the hub accepting
//! the next one.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

/// Wraps an accepted TCP connection — a TLS handshake, in the only
/// implementation there is.
pub trait TlsAcceptor: Send + Sync + 'static {
    /// The wrapped connection axum then speaks HTTP over.
    type Conn: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    /// Complete the handshake, or fail this one connection. An `Err` is a
    /// per-connection event (a bad ClientHello, a probe, a client that hung
    /// up) and never stops the server.
    fn accept(&self, stream: TcpStream)
        -> impl Future<Output = std::io::Result<Self::Conn>> + Send;
}

/// Stands in for the acceptor type on the plain-HTTP entry points, which
/// always pass `None`. Uninhabited: it is a type name, never a value.
pub struct NoTls(std::convert::Infallible);

impl TlsAcceptor for NoTls {
    type Conn = TcpStream;

    async fn accept(&self, _stream: TcpStream) -> std::io::Result<Self::Conn> {
        // `self.0` is `Infallible`, so there is no case to handle and no way
        // to reach here.
        match self.0 {}
    }
}

/// How many completed handshakes may queue ahead of axum's accept loop.
/// Bounded on purpose: back-pressure beats an unbounded queue of half-open
/// connections when a flood arrives.
const HANDSHAKE_BACKLOG: usize = 64;

/// How long a connection has to finish its handshake before it is dropped.
///
/// Without it, a peer that opens a TCP connection and then sends nothing holds
/// a task and a file descriptor until the TCP stack gives up — minutes, and
/// trivially repeatable. A real ClientHello arrives in one round trip, so this
/// is generous by orders of magnitude for anything legitimate.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// An `axum::serve::Listener` yielding connections `A` has already wrapped.
pub struct TlsListener<A: TlsAcceptor> {
    local_addr: SocketAddr,
    rx: mpsc::Receiver<(A::Conn, SocketAddr)>,
}

impl<A: TlsAcceptor> TlsListener<A> {
    /// Take over `listener`, wrapping every connection with `acceptor`.
    pub fn spawn(listener: TcpListener, acceptor: A) -> std::io::Result<Self> {
        Self::spawn_with_handshake_timeout(listener, acceptor, HANDSHAKE_TIMEOUT)
    }

    /// [`Self::spawn`] with the handshake deadline as a parameter, so a test
    /// can watch it expire on the real clock in milliseconds.
    fn spawn_with_handshake_timeout(
        listener: TcpListener,
        acceptor: A,
        handshake_timeout: std::time::Duration,
    ) -> std::io::Result<Self> {
        let local_addr = listener.local_addr()?;
        let (tx, rx) = mpsc::channel(HANDSHAKE_BACKLOG);
        let acceptor = Arc::new(acceptor);
        crate::rt::spawn(async move {
            loop {
                let (stream, peer) = tokio::select! {
                    // axum dropped the listener (the server stopped): stop
                    // accepting rather than leaking this task for the life of
                    // the process.
                    _ = tx.closed() => break,
                    accepted = listener.accept() => match accepted {
                        Ok(v) => v,
                        Err(e) if is_connection_error(&e) => continue,
                        Err(e) => {
                            // EMFILE and friends repeat immediately; pausing
                            // is what keeps this from becoming a busy loop.
                            tracing::warn!(error = %e, "[mcp] accept failed; retrying in 1s");
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                            continue;
                        }
                    },
                };
                let tx = tx.clone();
                let acceptor = Arc::clone(&acceptor);
                crate::rt::spawn(async move {
                    match tokio::time::timeout(handshake_timeout, acceptor.accept(stream)).await {
                        Ok(Ok(conn)) => {
                            // An `Err` here means the server is gone; the
                            // connection is dropped with it.
                            let _ = tx.send((conn, peer)).await;
                        }
                        // Routine: port scanners, plain-http requests to the
                        // https port, clients that reject our certificate.
                        Ok(Err(e)) => {
                            tracing::debug!(%peer, error = %e, "[mcp] TLS handshake failed")
                        }
                        Err(_) => tracing::debug!(
                            %peer,
                            timeout_secs = handshake_timeout.as_secs(),
                            "[mcp] TLS handshake timed out"
                        ),
                    }
                });
            }
        });
        Ok(Self { local_addr, rx })
    }
}

impl<A: TlsAcceptor> axum::serve::Listener for TlsListener<A> {
    type Io = A::Conn;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.rx.recv().await {
            Some(conn) => conn,
            // The accept task has stopped, which only happens once the server
            // is shutting down. `accept` has no way to report that, and must
            // not invent a connection, so park: graceful shutdown ends the
            // serve loop from the other side.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.local_addr)
    }
}

/// Errors that belong to the one connection being accepted, not the listener.
fn is_connection_error(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::*;
    matches!(
        e.kind(),
        ConnectionRefused | ConnectionAborted | ConnectionReset | Interrupted
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::serve::Listener as _;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Hands the connection straight back. Stands in for a handshake: what
    /// these tests exercise is the accept loop, not rustls.
    struct Passthrough;

    impl TlsAcceptor for Passthrough {
        type Conn = TcpStream;
        async fn accept(&self, stream: TcpStream) -> std::io::Result<TcpStream> {
            Ok(stream)
        }
    }

    struct AlwaysFails;

    impl TlsAcceptor for AlwaysFails {
        type Conn = TcpStream;
        async fn accept(&self, _stream: TcpStream) -> std::io::Result<TcpStream> {
            Err(std::io::Error::other("no"))
        }
    }

    async fn bound() -> (TcpListener, SocketAddr) {
        let l = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = l.local_addr().unwrap();
        (l, addr)
    }

    #[tokio::test]
    async fn accepted_connections_arrive_with_their_peer_address() {
        let (l, addr) = bound().await;
        let mut listener = TlsListener::spawn(l, Passthrough).unwrap();
        assert_eq!(listener.local_addr().unwrap(), addr);

        let client = tokio::spawn(async move {
            let mut s = TcpStream::connect(addr).await.unwrap();
            s.write_all(b"hi").await.unwrap();
            s.local_addr().unwrap()
        });
        let (mut conn, peer) = listener.accept().await;
        let mut buf = [0u8; 2];
        conn.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hi");
        assert_eq!(peer, client.await.unwrap(), "the peer's own address");
    }

    #[tokio::test]
    async fn a_slow_handshake_does_not_block_the_next_connection() {
        // The whole reason the handshake is spawned: a peer that stalls must
        // not hold the accept loop. The slow connection is opened FIRST, so a
        // handshake inlined in the accept loop would fail this test.
        let (l, addr) = bound().await;
        let mut listener = TlsListener::spawn(l, Slow).unwrap();
        let _slow = TcpStream::connect(addr).await.unwrap();
        let fast = TcpStream::connect(addr).await.unwrap();
        let fast_addr = fast.local_addr().unwrap();

        let (_conn, peer) =
            tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                .await
                .expect("the second connection must not wait for the first handshake");
        assert_eq!(peer, fast_addr);
    }

    /// Stalls the first connection it sees and lets every later one through.
    struct Slow;

    impl TlsAcceptor for Slow {
        type Conn = TcpStream;
        async fn accept(&self, stream: TcpStream) -> std::io::Result<TcpStream> {
            use std::sync::atomic::{AtomicBool, Ordering};
            static FIRST: AtomicBool = AtomicBool::new(true);
            if FIRST.swap(false, Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            Ok(stream)
        }
    }

    /// Never completes, standing in for a peer that opens a connection and
    /// then never sends a ClientHello.
    struct NeverCompletes;

    impl TlsAcceptor for NeverCompletes {
        type Conn = TcpStream;
        async fn accept(&self, stream: TcpStream) -> std::io::Result<TcpStream> {
            // Hold the connection open so the timeout, not a drop, is what
            // ends it.
            let _held = stream;
            std::future::pending().await
        }
    }

    /// Without [`HANDSHAKE_TIMEOUT`] such a peer holds a task and a file
    /// descriptor until the TCP stack gives up — minutes, and free to repeat.
    ///
    /// On the real clock with a short deadline, not a paused one: a paused
    /// clock jumps to the next timer whenever the runtime is idle, and a
    /// socket the OS has yet to make readable looks idle. The accept and the
    /// FIN both raced that jump, so the outer timeout fired first under load.
    #[tokio::test]
    async fn a_connection_that_never_handshakes_is_hung_up_on() {
        const DEADLINE: std::time::Duration = std::time::Duration::from_millis(100);
        let (l, addr) = bound().await;
        let mut listener =
            TlsListener::spawn_with_handshake_timeout(l, NeverCompletes, DEADLINE).unwrap();
        let connecting = std::time::Instant::now();
        let mut silent = TcpStream::connect(addr).await.unwrap();

        // The server closing its end is what the client sees as a 0-byte read.
        // The outer bound is liveness only: it is never waited out on a pass.
        let mut buf = [0u8; 1];
        let n = tokio::time::timeout(std::time::Duration::from_secs(60), silent.read(&mut buf))
            .await
            .expect("the hub must hang up on a peer that never handshakes")
            .unwrap();
        assert_eq!(n, 0, "the connection must be closed, not left open");
        // `NeverCompletes` holds the stream, so only the deadline can close it.
        assert!(
            connecting.elapsed() >= DEADLINE,
            "hung up by the deadline, not before it"
        );

        // Nothing was handed to axum, and the listener is still accepting.
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
        assert!(TcpStream::connect(addr).await.is_ok(), "still listening");
    }

    #[tokio::test]
    async fn a_failed_handshake_is_dropped_and_the_listener_keeps_serving() {
        let (l, addr) = bound().await;
        let mut listener = TlsListener::spawn(l, AlwaysFails).unwrap();
        let _ = TcpStream::connect(addr).await.unwrap();
        // Nothing is ever yielded, and the task is still alive to accept more.
        let timed_out =
            tokio::time::timeout(std::time::Duration::from_millis(200), listener.accept())
                .await
                .is_err();
        assert!(timed_out, "a failed handshake must not reach axum");
        assert!(TcpStream::connect(addr).await.is_ok(), "still listening");
    }
}
