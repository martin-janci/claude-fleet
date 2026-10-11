//! The outbound HTTP/1 client: hand-written onto a `TcpStream`, TLS through
//! `tokio-rustls` with the platform trust store. Moved here from the desktop
//! (`src-tauri/src/backend/remote.rs`) so the headless hub can dial another
//! hub (federation). The desktop re-exports it; nothing about it changed in
//! the move.

mod client_header;
pub mod http1;
pub mod keep_alive;

#[cfg(test)]
#[path = "tests_transport.rs"]
mod tests;

#[cfg(test)]
#[path = "tests_keep_alive.rs"]
mod tests_keep_alive;

use client_header::client_header_line;
pub use client_header::set_client_header;

/// What the hub answered: the HTTP status and the body exactly as received,
/// SSE framing still intact. Interpreting it is the job of the desktop's
/// `HubBackend` (`src-tauri`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubResponse {
    pub status: u16,
    pub body: String,
}

/// One HTTP exchange with the hub.
///
/// A trait, not a concrete client, so the whole tool mapping is testable
/// against recorded responses with no network — which is what the design
/// calls for, and which is why [`TcpTransport`] could grow TLS without a line
/// of the mapping changing.
///
/// `bearer` is the client token. An implementation must put it in the
/// `Authorization` header and must not log it.
#[async_trait::async_trait]
pub trait HubTransport: Send + Sync {
    async fn post_json(&self, url: &str, bearer: &str, body: String)
        -> Result<HubResponse, String>;

    /// `GET url` streamed into `dest` ([`download_to`]): a file download's
    /// bytes, which no tool result carries. A transport that cannot (the
    /// test fakes, [`NoTransport`]) refuses.
    async fn get_to_file(
        &self,
        _url: &str,
        _bearer: &str,
        _dest: &std::path::Path,
        _max: u64,
    ) -> Result<u64, String> {
        Err("this transport does not download files".into())
    }
}

/// The transport of a hub this launch cannot use: it sends nothing. See
/// `HubBackend::unavailable`.
pub struct NoTransport;

#[async_trait::async_trait]
impl HubTransport for NoTransport {
    async fn post_json(
        &self,
        _url: &str,
        _bearer: &str,
        _body: String,
    ) -> Result<HubResponse, String> {
        Err("no request was sent: the configured hub cannot be used by this launch".into())
    }
}

// --- the real transport ------------------------------------------------------

/// The hub over HTTP or HTTPS, written by hand onto a `TcpStream` — the same
/// way `fleet-hub`'s own CLI talks to `/mcp`. `POST /mcp` reuses kept-alive
/// connections ([`keep_alive`]: a TLS handshake per call was one to three
/// round trips in front of every answer); everything else is one request,
/// one response, `Connection: close`. Written by hand because no *usable
/// outbound HTTP client*
/// crate is in this workspace's graph to borrow one from: `hyper` is present
/// only as `axum`'s server side (via `fleet-core`'s embedded MCP server), and
/// `reqwest` appears in `Cargo.lock` only through a target-specific `tauri`
/// dependency that is not compiled here (`cargo tree -i reqwest` prints
/// nothing on this platform).
///
/// `https://` is the case that matters: `docs/hub.md` refuses to serve a
/// public hub in plaintext, so a real hub is always TLS. `http://` stays for a
/// loopback or tunnelled hub.
///
/// Certificates are verified against the **platform trust store**
/// (`rustls-native-certs`, via [`crate::net::tls`]), not a bundled root set. That is deliberate: a
/// desktop is exactly the place where a corporate CA or a root the operator
/// installed themselves has to work, and a bundled set would silently reject
/// both. (`webpki-roots` would bundle them, and its CDLA-Permissive-2.0
/// licence is not on `deny.toml`'s allow list.)
pub struct TcpTransport;

/// Where a hub URL points, split into the pieces a hand-written request
/// needs. One implementation for every request this app makes — `POST /mcp`
/// and the `GET /events` stream alike — so a fix to the parsing cannot land
/// in one and not the other.
///
/// The parsing itself is [`fleet_proto::net::Endpoint`]'s, shared with
/// `fleet-agent` and `fleet-hub`. What stays here is this transport's own
/// policy — a WebSocket URL is not a hub address for a request this app
/// writes by hand — and the request-line target that policy needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    at: fleet_proto::net::Endpoint,
}

impl Endpoint {
    /// Parse a hub URL: scheme, authority and a path prefix, with `/mcp`,
    /// `/events` or `/pair` already appended by the caller.
    pub fn parse(url: &str) -> Result<Self, String> {
        let at = fleet_proto::net::Endpoint::parse(url).map_err(|e| format!("{url}: {e}"))?;
        if at.scheme().is_websocket() {
            // The agent dials a WebSocket; everything this app sends is HTTP
            // it writes itself.
            return Err(format!("{}:// is not a hub address", at.scheme()));
        }
        Ok(Self { at })
    }

    /// The host to connect to and to check the certificate against.
    /// **Unbracketed**, so an IPv6 literal works: the bracketed `[::1]`
    /// neither resolves nor parses as a `ServerName`, and an IPv6 hub was
    /// simply unreachable before that was fixed.
    pub fn host(&self) -> &str {
        self.at.host()
    }

    pub fn port(&self) -> u16 {
        self.at.port()
    }

    pub fn is_tls(&self) -> bool {
        self.at.is_tls()
    }

    /// What the `Host` header must carry. Here the brackets are REQUIRED
    /// (`[::1]:8787`), and the port is part of it unless it is the scheme's
    /// default — the hub's allowlist is matched against exactly this string.
    pub fn authority(&self) -> &str {
        self.at.authority()
    }

    /// The path, ready to go on the request line. A query would have been
    /// refused by the parser: this value is built by concatenation
    /// (`{base_url}/mcp`), which a query silently breaks.
    pub fn target(&self) -> String {
        self.at.request_target()
    }

    /// A loopback host (`127.0.0.0/8`, `::1`, `localhost`): the only place a
    /// plain `http://` peer is allowed, as with `fleet-agent --insecure`.
    pub fn is_loopback(&self) -> bool {
        fleet_proto::net::is_loopback(self.at.host())
    }
}

/// A connected stream to the hub, TLS-wrapped when the URL said `https`.
/// The type (and the connect/read code behind it) is [`crate::net::conn`]'s,
/// shared with the tracker client.
pub type HubStream = crate::net::conn::Stream;
pub use crate::net::conn::Duplex;

/// Whether a failed `GET /events` attempt failed at the CONNECT phase — no
/// socket to the hub at all — rather than after the hub answered.
///
/// The two prefixes are [`connect`]'s own, and
/// `a_real_failed_connect_is_recognised_as_a_connect_failure` pins them
/// against the real function rather than against this list. Everything else
/// `open_stream` can return (`the hub answered 503 to GET /events`, `read
/// from …`, `the hub closed the connection before answering`) describes a hub
/// that IS reachable, and must not arm `HubBackend::offline_error` (in
/// `src-tauri`).
///
/// It errs open: `connect`'s two configuration-shaped failures (an
/// unparseable certificate name, a TLS root store that would not load) are
/// not matched, so a call still goes out and fails on its own terms. Letting
/// a doomed call run costs one bound; refusing a call that would have worked
/// costs the window every read it has.
pub fn is_connect_failure(reason: &str) -> bool {
    reason.starts_with("connect ") || reason.starts_with("TLS handshake with ")
}

/// Connect to `at`, wrapping in TLS when it says so. The one place a socket
/// to a hub is opened.
pub async fn connect(at: &Endpoint) -> Result<HubStream, String> {
    crate::net::conn::connect(&at.at, CONNECT_TIMEOUT).await
}

/// Largest response read from the hub, so a stray listener cannot make the
/// app buffer without bound. A full `list_sessions` on a large fleet is a few
/// hundred kilobytes; the largest answer is `work { attachment }`, a task
/// attachment as base64, so the bound is the larger of 8 MiB and an
/// attachment at its ceiling on the wire
/// ([`crate::store::ATTACHMENT_WIRE_BYTES`]).
const MAX_RESPONSE: u64 = if crate::store::ATTACHMENT_WIRE_BYTES as u64 > 8 * 1024 * 1024 {
    crate::store::ATTACHMENT_WIRE_BYTES as u64
} else {
    8 * 1024 * 1024
};

/// TCP connect, and separately the TLS handshake, each get this long. A
/// black-holed hub then costs seconds, not the whole call bound.
pub const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[async_trait::async_trait]
impl HubTransport for TcpTransport {
    async fn post_json(
        &self,
        url: &str,
        bearer: &str,
        body: String,
    ) -> Result<HubResponse, String> {
        let at = Endpoint::parse(url)?;
        check_bearer(bearer)?;
        // `Accept` carries both types because the transport answers
        // SSE-framed; rmcp refuses a request that does not accept
        // `text/event-stream`.
        let request = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {bearer}\r\n{}\
             Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
             Content-Length: {}\r\nConnection: keep-alive\r\n\r\n{body}",
            at.target(),
            at.authority(),
            client_header_line(),
            body.len()
        );
        // Unbounded here on purpose: the caller that knows the tool
        // (`HubBackend::call_text`, in `src-tauri`) is the one that times
        // the exchange.
        post_kept_alive(&at, request.as_bytes()).await
    }

    async fn get_to_file(
        &self,
        url: &str,
        bearer: &str,
        dest: &std::path::Path,
        max: u64,
    ) -> Result<u64, String> {
        download_to(url, bearer, dest, max).await
    }
}

/// Send `request` on a kept-alive connection to `at` ([`keep_alive`]),
/// opening one when none is kept, and keep it again if the response allows.
/// A kept connection that answers nothing at all (the server closed it while
/// it sat idle) costs one retry on a fresh connection; nothing else is ever
/// sent twice.
async fn post_kept_alive(at: &Endpoint, request: &[u8]) -> Result<HubResponse, String> {
    let key = format!(
        "{}://{}",
        if at.is_tls() { "https" } else { "http" },
        at.authority()
    );
    let mut kept = keep_alive::take(&key);
    loop {
        let reused = kept.is_some();
        let mut conn = match kept.take() {
            Some(conn) => conn,
            None => connect(at).await?,
        };
        match keep_alive::exchange(&mut conn, at.host(), at.port(), request, MAX_RESPONSE).await {
            Ok((response, reusable)) => {
                if reusable {
                    keep_alive::put(&key, conn);
                }
                return Ok(response);
            }
            Err(keep_alive::Failure::NothingBack(why)) if reused => {
                tracing::debug!(
                    why,
                    "[hub] a kept connection had gone; sending on a fresh one"
                );
            }
            Err(keep_alive::Failure::NothingBack(why) | keep_alive::Failure::Other(why)) => {
                return Err(why)
            }
        }
    }
}

/// Connect, write the request, read the whole answer back — as bytes, since
/// the body may be chunked and only [`split_response`] may decode it.
pub async fn exchange(at: &Endpoint, request: &str) -> Result<Vec<u8>, String> {
    let conn = connect(at).await?;
    speak(conn, at.host(), at.port(), request).await
}

/// Write `request` and read until the peer closes. Generic over the stream so
/// the plain and TLS paths share one implementation and cannot drift.
///
/// Returns raw bytes, NOT text: decoding here, before [`split_response`]
/// de-chunks, corrupted any character a chunk boundary split — the same order
/// the event stream already gets right (de-chunk bytes, then decode).
async fn speak<S>(conn: S, host: &str, port: u16, request: &str) -> Result<Vec<u8>, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    crate::net::conn::speak(conn, host, port, request.as_bytes(), MAX_RESPONSE).await
}

/// Split a raw HTTP response into its status code and body, undoing
/// `Transfer-Encoding: chunked` when the head declares it.
///
/// The de-chunking is not decoration. Without it this worked only by
/// accident: a chunk-size line is not a `data:` line, so `last_event_payload`
/// skipped it, and the boundaries happened to align because rmcp writes one
/// SSE frame per body frame. Nothing guarantees either, and the day a frame is
/// split across two chunks the size line lands in the middle of a `data:`
/// line and the JSON is quietly corrupt. (`fleet-hub/src/pair.rs` still takes
/// the shortcut; it is the same latent bug, not a different one.)
///
/// Bytes in, text out, decoded ONCE at the end: a chunk size is a byte count,
/// and a chunk boundary may fall inside a multi-byte character. The head
/// parsing and de-chunking themselves are [`http1`]'s, shared with
/// `events.rs`'s streaming reader; this is the one-shot assembly on top.
pub fn split_response(raw: impl AsRef<[u8]>) -> Result<HubResponse, String> {
    let raw = raw.as_ref();
    let split = http1::find(raw, b"\r\n\r\n").ok_or("the hub sent a malformed HTTP response")?;
    // The head is ASCII by the grammar; a stray byte in it is not worth
    // failing over.
    let head = String::from_utf8_lossy(&raw[..split]);
    let head = head.as_ref();
    let body = &raw[split + 4..];
    let status = http1::parse_status(head)?;
    let body = if http1::head_is_chunked(head) {
        String::from_utf8_lossy(&http1::dechunk(body)?).into_owned()
    } else {
        String::from_utf8_lossy(body).into_owned()
    };
    Ok(HubResponse { status, body })
}

/// Largest response head [`download_to`] reads before the body.
const MAX_HEAD: usize = 64 * 1024;

/// How long a download may go without a single byte before it is abandoned.
/// `download_to` has no caller that bounds it as a whole (a file may
/// legitimately take minutes), so without this a hub or proxy that stalled
/// mid-body held the save, and its open `.part` file, forever.
pub const DOWNLOAD_IDLE: std::time::Duration = std::time::Duration::from_secs(60);

/// One read, or an error once [`DOWNLOAD_IDLE`] passes with nothing.
async fn read_or_stall<R: tokio::io::AsyncRead + Unpin>(
    conn: &mut R,
    buf: &mut [u8],
    host: &str,
    port: u16,
) -> Result<std::io::Result<usize>, String> {
    use tokio::io::AsyncReadExt;
    tokio::time::timeout(DOWNLOAD_IDLE, conn.read(buf))
        .await
        .map_err(|_| {
            format!(
                "{host}:{port} sent nothing for {}s; the download stalled",
                DOWNLOAD_IDLE.as_secs()
            )
        })
}

/// A bearer token is written into a request head by hand, so one carrying a
/// CR/LF (or any control, space or non-ASCII byte) would add headers — or a
/// second request — of its sender's choosing. A peer hub's `/pair` answer is
/// where a token comes from that this process did not mint.
pub fn check_bearer(bearer: &str) -> Result<(), String> {
    if bearer.is_empty() || bearer.len() > 4096 || !bearer.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("the token is not one a request header can carry".to_string());
    }
    Ok(())
}

/// `GET url` and stream the body into `dest` — the bytes of a file download
/// (`GET /downloads/<id>`), which may be far past [`MAX_RESPONSE`], so they
/// are never buffered whole. Written to `<dest>.<pid>-<random>.part` and renamed into place
/// only once complete, so `dest` is either the whole file or untouched. The
/// byte count on success; on a non-200, `Err("HTTP <status>: <body>")`.
pub async fn download_to(
    url: &str,
    bearer: &str,
    dest: &std::path::Path,
    max: u64,
) -> Result<u64, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let at = Endpoint::parse(url)?;
    check_bearer(bearer)?;
    let (host, port) = (at.host().to_string(), at.port());
    let mut conn = connect(&at).await?;
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {bearer}\r\n{}\
         Accept: */*\r\nConnection: close\r\n\r\n",
        at.target(),
        at.authority(),
        client_header_line(),
    );
    let io = |e: std::io::Error| format!("read from {host}:{port}: {e}");
    conn.write_all(request.as_bytes()).await.map_err(io)?;
    conn.flush().await.map_err(io)?;

    let mut raw = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    let split = loop {
        if let Some(i) = http1::find(&raw, b"\r\n\r\n") {
            break i;
        }
        if raw.len() > MAX_HEAD {
            return Err(format!("{host}:{port} sent an oversized response head"));
        }
        let n = read_or_stall(&mut conn, &mut buf, &host, port)
            .await?
            .map_err(io)?;
        if n == 0 {
            return Err(format!("{host}:{port} closed before answering"));
        }
        raw.extend_from_slice(&buf[..n]);
    };
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut rest = raw.split_off(split + 4);
    let status = http1::parse_status(&head)?;
    if status != 200 {
        let _ = conn.take(4096).read_to_end(&mut rest).await;
        let body = String::from_utf8_lossy(&rest).trim().to_string();
        return Err(format!("HTTP {status}: {body}"));
    }
    let expected: Option<u64> = head.lines().skip(1).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case("content-length")
            .then(|| v.trim().parse().ok())?
    });
    if expected.is_some_and(|n| n > max) {
        return Err(too_large_download(max));
    }
    let chunked = http1::head_is_chunked(&head);
    let mut chunks = http1::Dechunker::new(chunked);
    // A name of this download's own, created new: a fixed `<dest>.part`
    // truncated (and on failure deleted) a file the user already had under
    // that name, and two saves to one destination wrote into one file.
    let part = {
        let mut p = dest.as_os_str().to_owned();
        p.push(format!(
            ".{}-{}.part",
            std::process::id(),
            &crate::mcp::generate_token()[..12]
        ));
        std::path::PathBuf::from(p)
    };
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part)
        .await
        .map_err(|e| format!("create {}: {e}", part.display()))?;
    let result: Result<u64, String> = async {
        let mut got: u64 = 0;
        loop {
            let data = chunks.take(&mut rest)?;
            if !data.is_empty() {
                got += data.len() as u64;
                if got > max {
                    return Err(too_large_download(max));
                }
                file.write_all(&data)
                    .await
                    .map_err(|e| format!("write {}: {e}", part.display()))?;
            }
            if chunks.finished() || expected.is_some_and(|n| got >= n) {
                break;
            }
            match read_or_stall(&mut conn, &mut buf, &host, port).await? {
                Ok(0) => break,
                Ok(n) => rest.extend_from_slice(&buf[..n]),
                // The rustls close without `close_notify` [`speak`] forgives:
                // the length check below says whether the body is whole.
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(io(e)),
            }
        }
        if let Some(n) = expected {
            if got != n {
                return Err(format!(
                    "{host}:{port} sent {got} of {n} bytes; the download is incomplete"
                ));
            }
        }
        // A chunked body (a proxy in front of the hub re-chunks) carries no
        // length: only its terminating chunk says it is whole. A connection
        // cut before it is a partial file, not a download.
        if chunked && !chunks.finished() {
            return Err(format!(
                "{host}:{port} closed mid-download before the last chunk; the download is incomplete"
            ));
        }
        file.sync_all()
            .await
            .map_err(|e| format!("write {}: {e}", part.display()))?;
        Ok(got)
    }
    .await;
    drop(file);
    match result {
        Ok(n) => {
            tokio::fs::rename(&part, dest)
                .await
                .map_err(|e| format!("move into {}: {e}", dest.display()))?;
            Ok(n)
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&part).await;
            Err(e)
        }
    }
}

fn too_large_download(max: u64) -> String {
    format!(
        "the download is larger than {} MiB; refusing it",
        max / (1024 * 1024)
    )
}
