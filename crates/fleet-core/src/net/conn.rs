//! Opening a connection and speaking one `Connection: close` exchange over it,
//! plain or TLS. Lifted from the desktop's `backend/remote.rs` with work graph
//! M3.0; the desktop's hub client and [`super::https`] both go through here,
//! so a fix to the read loop cannot land in one and not the other.

use fleet_proto::net::Endpoint;
use std::time::Duration;

/// A connected stream, TLS-wrapped when the endpoint said so. Boxed because
/// the two arms are different types and every caller wants one name for them.
pub type Stream = Box<dyn Duplex>;

/// `AsyncRead + AsyncWrite`, object-safe.
pub trait Duplex: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin> Duplex for T {}

/// Connect to `at`, wrapping in TLS when it says so. TCP connect and the TLS
/// handshake each get `timeout`, so a black-holed peer costs seconds, not the
/// caller's whole bound.
///
/// The error strings start with `connect ` or `TLS handshake with ` when the
/// failure is at that phase; the desktop's `remote::is_connect_failure`
/// matches on exactly those prefixes, and a test there pins them against this
/// function.
pub async fn connect(at: &Endpoint, timeout: Duration) -> Result<Stream, String> {
    let tcp = tokio::time::timeout(
        timeout,
        tokio::net::TcpStream::connect((at.host(), at.port())),
    )
    .await
    .map_err(|_| {
        format!(
            "connect {}:{}: timed out after {timeout:?}",
            at.host(),
            at.port()
        )
    })?
    .map_err(|e| format!("connect {}:{}: {e}", at.host(), at.port()))?;
    if !at.is_tls() {
        return Ok(Box::new(tcp));
    }
    let connector = super::tls::tls_connector().await?;
    // The name the certificate is checked against. An IP literal is accepted
    // by `ServerName` and matched as an IP SAN.
    let server_name = tokio_rustls::rustls::pki_types::ServerName::try_from(at.host().to_string())
        .map_err(|e| format!("{} is not a valid certificate name: {e}", at.host()))?;
    let stream = tokio::time::timeout(timeout, connector.connect(server_name, tcp))
        .await
        .map_err(|_| {
            format!(
                "TLS handshake with {}:{} timed out after {timeout:?}",
                at.host(),
                at.port()
            )
        })?
        // The usual causes are an expired or self-signed certificate and a
        // name that does not match; rustls says which, and the operator needs
        // to hear it verbatim.
        .map_err(|e| format!("TLS handshake with {}:{} failed: {e}", at.host(), at.port()))?;
    Ok(Box::new(stream))
}

/// Connect to one of `addrs` (already resolved and checked by the caller,
/// so a second DNS answer cannot swap the address — no rebinding), in
/// order, each TCP attempt bounded by `timeout`; then TLS with `connector`,
/// the certificate checked against `host`.
pub async fn connect_pinned(
    addrs: &[std::net::SocketAddr],
    host: &str,
    connector: &tokio_rustls::TlsConnector,
    timeout: Duration,
) -> Result<Stream, String> {
    let mut last = format!("connect {host}: no address");
    let mut tcp = None;
    for a in addrs {
        match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(a)).await {
            Ok(Ok(s)) => {
                tcp = Some(s);
                break;
            }
            Ok(Err(e)) => last = format!("connect {host} ({a}): {e}"),
            Err(_) => last = format!("connect {host} ({a}): timed out after {timeout:?}"),
        }
    }
    let tcp = tcp.ok_or(last)?;
    let server_name = tokio_rustls::rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| format!("{host} is not a valid certificate name: {e}"))?;
    let stream = tokio::time::timeout(timeout, connector.connect(server_name, tcp))
        .await
        .map_err(|_| format!("TLS handshake with {host} timed out after {timeout:?}"))?
        .map_err(|e| format!("TLS handshake with {host} failed: {e}"))?;
    Ok(Box::new(stream))
}

/// Write `request` and read until the peer closes, at most `max` bytes.
/// Generic over the stream so the plain and TLS paths share one
/// implementation and cannot drift.
///
/// Returns raw bytes, NOT text: a chunked body must be de-chunked before it
/// is decoded, or a character a chunk boundary split is corrupted.
pub async fn speak<S>(
    mut conn: S,
    host: &str,
    port: u16,
    request: &[u8],
    max: u64,
) -> Result<Vec<u8>, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    conn.write_all(request)
        .await
        .map_err(|e| format!("send to {host}:{port}: {e}"))?;
    // TLS needs an explicit flush: the record is buffered until one.
    conn.flush()
        .await
        .map_err(|e| format!("send to {host}:{port}: {e}"))?;
    let mut raw = Vec::new();
    // One byte MORE than the cap, so "the answer was too big" is
    // distinguishable from "the answer was exactly the cap". Truncating
    // silently at the cap surfaced as unreadable JSON, which names the wrong
    // problem.
    if let Err(e) = conn.take(max + 1).read_to_end(&mut raw).await {
        // A peer that closes the TCP connection without sending `close_notify`
        // makes rustls 0.23 return `UnexpectedEof`, and failing here would
        // throw away a body that had already arrived in full. With
        // `Connection: close` the response is framed by the connection
        // ending, so bytes already read ARE the response. An empty buffer is
        // still a real failure, and only `UnexpectedEof` is forgiven: a reset
        // mid-body leaves a TRUNCATED response.
        if e.kind() != std::io::ErrorKind::UnexpectedEof || raw.is_empty() {
            return Err(format!("read from {host}:{port}: {e}"));
        }
        tracing::debug!(
            "{host}:{port} closed without close_notify after {} byte(s); \
             treating the response as complete",
            raw.len()
        );
    }
    if raw.len() as u64 > max {
        return Err(too_large(host, port, max));
    }
    Ok(raw)
}

/// The message [`speak`] refuses an over-cap answer with.
pub fn too_large(host: &str, port: u16, max: u64) -> String {
    format!(
        "{host}:{port} sent more than {} MiB; refusing to buffer it \
         (silently truncating at the cap surfaced as unreadable JSON, \
         which names the wrong problem)",
        max / (1024 * 1024)
    )
}
