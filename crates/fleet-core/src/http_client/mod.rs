//! The outbound HTTP/1 client: hand-written onto a `TcpStream`, TLS through
//! `tokio-rustls` with the platform trust store. Moved here from the desktop
//! (`src-tauri/src/backend/remote.rs`) so the headless hub can dial another
//! hub (federation). The desktop re-exports it; nothing about it changed in
//! the move.

pub mod http1;

#[cfg(test)]
#[path = "tests_transport.rs"]
mod tests;

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
/// way `fleet-hub`'s own CLI talks to `/mcp`. One request, one response,
/// `Connection: close`; there is no connection pool because a desktop makes a
/// handful of calls a second at worst, and no *usable outbound HTTP client*
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
/// (`rustls-native-certs`), not a bundled root set. That is deliberate: a
/// desktop is exactly the place where a corporate CA or a root the operator
/// installed themselves has to work, and a bundled set would silently reject
/// both. (`webpki-roots` would bundle them, and its CDLA-Permissive-2.0
/// licence is not on `deny.toml`'s allow list.)
pub struct TcpTransport;

/// Build the TLS client config: read the platform trust store and turn it
/// into a connector. Blocking (file I/O, and on macOS a keychain read), so
/// every caller goes through [`tls_connector`], which runs it on the blocking
/// pool.
fn build_tls_connector() -> Result<tokio_rustls::TlsConnector, String> {
    // Only the `ring` provider is compiled in, so rustls would pick it
    // anyway; installing it explicitly means a future second provider
    // cannot silently change which one is used. Same reasoning, and
    // the same line, as `fleet_hub::tls`.
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    let found = rustls_native_certs::load_native_certs();
    for cert in found.certs {
        // A single unparseable root is not fatal: the store is a bag
        // of certificates from the OS and one bad entry must not stop
        // the app trusting the rest.
        let _ = roots.add(cert);
    }
    if roots.is_empty() {
        // Failing closed. An empty root store would reject every hub
        // with an opaque certificate error; saying so once, here, is
        // the difference between a diagnosable problem and a mystery.
        return Err(format!(
            "no usable certificates in this machine's trust store \
             ({} error(s) while reading it); an https:// hub cannot be verified",
            found.errors.len()
        ));
    }
    let config = tokio_rustls::rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(tokio_rustls::TlsConnector::from(std::sync::Arc::new(
        config,
    )))
}

/// The TLS client config, built once and cached.
///
/// Two things this deliberately does NOT do:
///
/// - It does not load the trust store on a runtime worker.
///   `load_native_certs` reads (and on macOS unlocks and queries) the
///   platform store; on a locked-down or network-mounted box that is slow
///   blocking I/O, and a runtime worker parked in it is a worker not running
///   anyone else's future. It goes to [`tokio::task::spawn_blocking`].
/// - It does not cache a FAILURE. A trust store that was momentarily
///   unreadable — a keychain still locked at login, a profile not yet
///   mounted — used to poison every https call for the lifetime of the
///   process, so the app had to be restarted to recover from a condition that
///   had already cleared. Only a success is remembered; a failure is retried
///   on the next call.
///
/// Note that "the platform trust store" is really "the platform trust store,
/// unless the environment says otherwise": `load_native_certs` honours
/// `SSL_CERT_FILE` and `SSL_CERT_DIR` when they are set
/// (`rustls-native-certs`'s `CertPaths::from_env`).
async fn tls_connector() -> Result<&'static tokio_rustls::TlsConnector, String> {
    static CONNECTOR: std::sync::OnceLock<tokio_rustls::TlsConnector> = std::sync::OnceLock::new();
    if let Some(ready) = CONNECTOR.get() {
        return Ok(ready);
    }
    let built = tokio::task::spawn_blocking(build_tls_connector)
        .await
        .map_err(|e| format!("reading this machine's trust store failed: {e}"))??;
    // Two callers racing both build one; `get_or_init` keeps whichever
    // arrived first and drops the other. Both are equivalent.
    Ok(CONNECTOR.get_or_init(|| built))
}

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
/// Boxed because the two arms are different types and both the one-shot and
/// the streaming caller want one name for them.
pub type HubStream = Box<dyn Duplex>;

/// `AsyncRead + AsyncWrite`, object-safe.
pub trait Duplex: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin> Duplex for T {}

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
    let tcp = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio::net::TcpStream::connect((at.host(), at.port())),
    )
    .await
    .map_err(|_| {
        format!(
            "connect {}:{}: timed out after {CONNECT_TIMEOUT:?}",
            at.host(),
            at.port()
        )
    })?
    .map_err(|e| format!("connect {}:{}: {e}", at.host(), at.port()))?;
    if !at.is_tls() {
        return Ok(Box::new(tcp));
    }
    let connector = tls_connector().await?;
    // The name the certificate is checked against. An IP literal is accepted
    // by `ServerName` and matched as an IP SAN, which is what a hub reached
    // at `https://10.0.0.5` needs.
    let server_name = tokio_rustls::rustls::pki_types::ServerName::try_from(at.host().to_string())
        .map_err(|e| format!("{} is not a valid certificate name: {e}", at.host()))?;
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(server_name, tcp))
        .await
        .map_err(|_| {
            format!(
                "TLS handshake with {}:{} timed out after {CONNECT_TIMEOUT:?}",
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

/// Largest response read from the hub, so a stray listener cannot make the
/// app buffer without bound. A full `list_sessions` on a large fleet is a few
/// hundred kilobytes.
const MAX_RESPONSE: u64 = 8 * 1024 * 1024;

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
        // `Accept` carries both types because the transport answers
        // SSE-framed; rmcp refuses a request that does not accept
        // `text/event-stream`.
        let request = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {bearer}\r\n\
             Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            at.target(),
            at.authority(),
            body.len()
        );
        // Unbounded here on purpose: the caller that knows the tool
        // (`HubBackend::call_text`, in `src-tauri`) is the one that times
        // the exchange.
        let raw = exchange(&at, &request).await?;
        split_response(&raw)
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
async fn speak<S>(mut conn: S, host: &str, port: u16, request: &str) -> Result<Vec<u8>, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    conn.write_all(request.as_bytes())
        .await
        .map_err(|e| format!("send to {host}:{port}: {e}"))?;
    // TLS needs an explicit flush: the record is buffered until one.
    conn.flush()
        .await
        .map_err(|e| format!("send to {host}:{port}: {e}"))?;
    let mut raw = Vec::new();
    // One byte MORE than the cap, so "the answer was too big" is
    // distinguishable from "the answer was exactly the cap". Truncating
    // silently at the cap used to surface as `E_PARSE … returned unreadable
    // JSON`, which sends whoever reads it looking for a bug in the hub's
    // encoder rather than at a size limit.
    if let Err(e) = conn.take(MAX_RESPONSE + 1).read_to_end(&mut raw).await {
        // A peer that closes the TCP connection without sending `close_notify`
        // makes rustls 0.23 return `UnexpectedEof` (`rustls/src/conn.rs`,
        // `(false, true) => Err(UnexpectedEof)`), and `?` here used to throw
        // away a body that had already arrived in full. That is a spurious
        // failure against any hub or proxy that half-closes, and it is a
        // regression the plain-HTTP path does not have.
        //
        // This is the standard shape for HTTP-over-TLS with
        // `Connection: close`: the response is framed by the connection
        // ending, so bytes already read ARE the response. An empty buffer is
        // still a real failure — nothing arrived. And only `UnexpectedEof` is
        // forgiven: a reset mid-body leaves a TRUNCATED response, which must
        // not be parsed as if it were whole.
        if e.kind() != std::io::ErrorKind::UnexpectedEof || raw.is_empty() {
            return Err(format!("read from {host}:{port}: {e}"));
        }
        tracing::debug!(
            "{host}:{port} closed without close_notify after {} byte(s); \
             treating the response as complete",
            raw.len()
        );
    }
    if raw.len() as u64 > MAX_RESPONSE {
        return Err(format!(
            "{host}:{port} sent more than {} MiB; refusing to buffer it \
             (silently truncating at the cap surfaced as unreadable JSON, \
             which names the wrong problem)",
            MAX_RESPONSE / (1024 * 1024)
        ));
    }
    Ok(raw)
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
