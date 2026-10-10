//! Kept-alive connections to a hub, for [`super::TcpTransport::post_json`].
//!
//! Every call used to open a socket, do a TLS handshake and close it again
//! (`Connection: close`): one to three extra round trips in front of every
//! answer a person sends from the desktop to a hub, which on a phone tether
//! or a distant hub is most of the wait. Now an exchange that ends cleanly
//! leaves its connection here, and the next call to the same hub takes it.
//!
//! What makes a reuse safe:
//! - A response is read by its own framing (`Content-Length`, or chunked to
//!   the last chunk), never "until the peer closes" — so the connection is
//!   still usable afterwards. A response with neither, or one saying
//!   `Connection: close`, or HTTP/1.0, is read to the end and not kept.
//! - A connection is kept at most [`IDLE_FOR`], under the idle timeouts
//!   servers and proxies apply, and at most [`MAX_IDLE`] per hub.
//! - A KEPT connection that turns out dead — the write fails, or the peer
//!   closes before sending a single byte back — is the classic race with the
//!   server's own idle close: the request is sent once more on a fresh
//!   connection. Only then, and only once: a request the server may have
//!   started on (any byte answered) is never repeated.

use super::{http1, HubResponse, HubStream};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// How long an idle connection is kept for reuse.
pub const IDLE_FOR: Duration = Duration::from_secs(20);

/// Idle connections kept per hub.
pub const MAX_IDLE: usize = 4;

/// The largest response head read before the body.
const MAX_HEAD: usize = 64 * 1024;

/// One hub's idle connections, each with when it went idle, newest last.
type Idle = Vec<(HubStream, Instant)>;

/// Idle connections by hub (`scheme://authority`).
static POOL: std::sync::LazyLock<Mutex<HashMap<String, Idle>>> =
    std::sync::LazyLock::new(Default::default);

/// A kept connection to `key` still inside [`IDLE_FOR`], if there is one.
pub fn take(key: &str) -> Option<HubStream> {
    let mut pool = POOL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let idle = pool.get_mut(key)?;
    idle.retain(|(_, since)| since.elapsed() < IDLE_FOR);
    idle.pop().map(|(conn, _)| conn)
}

/// Keep `conn` for the next call to `key`.
pub fn put(key: &str, conn: HubStream) {
    let mut pool = POOL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let idle = pool.entry(key.to_string()).or_default();
    idle.retain(|(_, since)| since.elapsed() < IDLE_FOR);
    if idle.len() >= MAX_IDLE {
        idle.remove(0);
    }
    idle.push((conn, Instant::now()));
}

/// How many connections to `key` are kept now (tests).
#[cfg(test)]
pub fn idle(key: &str) -> usize {
    POOL.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(key)
        .map_or(0, Vec::len)
}

/// Why an exchange failed.
#[derive(Debug)]
pub enum Failure {
    /// Nothing came back: the write failed, or the peer closed before the
    /// first byte. On a KEPT connection this is the idle-close race, and the
    /// request may be sent once more on a fresh one.
    NothingBack(String),
    /// Anything else: not repeated.
    Other(String),
}

/// Write `request` and read one response framed by its own head. `Ok` carries
/// the response and whether the connection may be kept.
pub async fn exchange<S>(
    conn: &mut S,
    host: &str,
    port: u16,
    request: &[u8],
    max: u64,
) -> Result<(HubResponse, bool), Failure>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let at = format!("{host}:{port}");
    if let Err(e) = conn.write_all(request).await {
        return Err(Failure::NothingBack(format!("send to {at}: {e}")));
    }
    // TLS needs an explicit flush: the record is buffered until one.
    if let Err(e) = conn.flush().await {
        return Err(Failure::NothingBack(format!("send to {at}: {e}")));
    }
    read_response(conn, &at, max).await
}

/// One response off `conn`. See [`exchange`].
pub async fn read_response<S>(
    conn: &mut S,
    at: &str,
    max: u64,
) -> Result<(HubResponse, bool), Failure>
where
    S: AsyncRead + Unpin,
{
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 16 * 1024];
    // The head.
    let split = loop {
        if let Some(i) = http1::find(&buf, b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEAD {
            return Err(Failure::Other(format!(
                "{at} sent a response head over {MAX_HEAD} bytes"
            )));
        }
        match conn.read(&mut chunk).await {
            Ok(0) if buf.is_empty() => {
                return Err(Failure::NothingBack(format!(
                    "the hub at {at} closed the connection before answering"
                )))
            }
            Ok(0) => {
                return Err(Failure::Other(format!(
                    "{at} closed mid-way through a response head"
                )))
            }
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if buf.is_empty() => {
                return Err(Failure::NothingBack(format!("read from {at}: {e}")))
            }
            Err(e) => return Err(Failure::Other(format!("read from {at}: {e}"))),
        }
    };
    let head = String::from_utf8_lossy(&buf[..split]).into_owned();
    let mut rest = buf.split_off(split + 4);
    let status = http1::parse_status(&head).map_err(Failure::Other)?;
    let headers = crate::net::http1::parse_headers(&head);
    let header = |name: &str| {
        headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };
    let http10 = head.starts_with("HTTP/1.0");
    let closes = header("connection")
        .is_some_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case("close")));
    let too_large = || {
        Failure::Other(format!(
            "{at} sent more than {} MiB; refusing to buffer it",
            max / (1024 * 1024)
        ))
    };
    let mut body = Vec::new();
    let reusable;
    if http1::head_is_chunked(&head) {
        let mut chunks = http1::Dechunker::new(true);
        loop {
            body.extend(chunks.take(&mut rest).map_err(Failure::Other)?);
            if body.len() as u64 > max {
                return Err(too_large());
            }
            if chunks.finished() {
                break;
            }
            match conn.read(&mut chunk).await {
                Ok(0) => {
                    return Err(Failure::Other(format!(
                        "{at} closed before the last chunk of its response"
                    )))
                }
                Ok(n) => rest.extend_from_slice(&chunk[..n]),
                Err(e) => return Err(Failure::Other(format!("read from {at}: {e}"))),
            }
        }
        // `Dechunker` stops at the zero-size chunk's line; what follows it
        // is the trailer section and its blank line, which nothing here
        // sends. Anything left unread would be read as the NEXT response's
        // head, so a connection that may hold some is not kept.
        reusable = !closes && !http10;
    } else if let Some(len) = header("content-length") {
        let len: u64 = len.trim().parse().map_err(|_| {
            Failure::Other(format!("{at} sent an unreadable Content-Length {len:?}"))
        })?;
        if len > max {
            return Err(too_large());
        }
        body = rest;
        while (body.len() as u64) < len {
            match conn.read(&mut chunk).await {
                Ok(0) => {
                    return Err(Failure::Other(format!(
                        "{at} closed {} byte(s) short of its response",
                        len - body.len() as u64
                    )))
                }
                Ok(n) => body.extend_from_slice(&chunk[..n]),
                Err(e) => return Err(Failure::Other(format!("read from {at}: {e}"))),
            }
        }
        // More than it announced: the next response's bytes, or garbage —
        // either way this connection's framing can no longer be trusted.
        reusable = body.len() as u64 == len && !closes && !http10;
        body.truncate(len as usize);
    } else {
        // Framed by the connection ending, as every response used to be.
        body = rest;
        loop {
            match conn.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => body.extend_from_slice(&chunk[..n]),
                // A peer that closes without TLS `close_notify`: the bytes
                // already read ARE the response (as in `net::conn::speak`).
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(Failure::Other(format!("read from {at}: {e}"))),
            }
            if body.len() as u64 > max {
                return Err(too_large());
            }
        }
        reusable = false;
    }
    let body = String::from_utf8_lossy(&body).into_owned();
    Ok((HubResponse { status, body }, reusable))
}
