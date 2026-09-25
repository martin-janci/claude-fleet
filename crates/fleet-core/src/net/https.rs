//! The one seam tracker providers talk HTTP through (work graph M3.0).
//!
//! [`HttpTransport`] is `async fn send(Request) -> Result<Response>`:
//!
//! * [`DirectTransport`] speaks HTTPS from this process, over [`super::conn`].
//!   It never follows a redirect (a 3xx comes back as a [`Response`] for the
//!   caller to refuse), caps the body, bounds the whole exchange by the
//!   request's timeout, refuses plaintext, and refuses any host its policy
//!   does not allow — the SSRF fence (`service::trackers` allows only
//!   `*.atlassian.net`).
//! * [`FakeTransport`] answers from a script and records every request, so no
//!   test ever reaches a real tracker.
//! * A `ViaHost` transport (curl / `gh` / `acli` on a host) arrives with M6.
//!
//! [`Request`]'s `Debug` masks credential headers, so a request logged by
//! accident does not leak one.

use super::{conn, http1};
use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Whole-exchange bound when a request names none.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);
/// TCP connect, and separately the TLS handshake.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Largest response body [`DirectTransport`] buffers by default. A Jira
/// search page of 100 issues with an explicit field list is well under this.
pub const DEFAULT_MAX_BODY: u64 = 4 * 1024 * 1024;

/// Header names whose value is a credential; masked by `Debug`.
const SECRET_HEADERS: &[&str] = &["authorization", "cookie", "proxy-authorization"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
        }
    }
}

/// One outbound request.
#[derive(Clone)]
pub struct Request {
    pub method: Method,
    /// Absolute `https://` URL; a query is allowed, a fragment is not.
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
}

impl Request {
    pub fn get(url: impl Into<String>) -> Self {
        Request {
            method: Method::Get,
            url: url.into(),
            headers: vec![("Accept".into(), "application/json".into())],
            body: None,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn post_json(url: impl Into<String>, body: &serde_json::Value) -> Self {
        let mut r = Request::get(url);
        r.method = Method::Post;
        r.headers
            .push(("Content-Type".into(), "application/json".into()));
        r.body = Some(body.to_string().into_bytes());
        r
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The first header called `name` (ASCII case-insensitive).
    pub fn header_value(&self, name: &str) -> Option<&str> {
        header_in(&self.headers, name)
    }

    /// The body as JSON, for tests and fakes.
    pub fn json_body(&self) -> Option<serde_json::Value> {
        self.body
            .as_deref()
            .and_then(|b| serde_json::from_slice(b).ok())
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(k, v)| {
                let masked = SECRET_HEADERS.iter().any(|s| k.eq_ignore_ascii_case(s));
                (
                    k.as_str(),
                    if masked {
                        crate::logging::REDACTED
                    } else {
                        v.as_str()
                    },
                )
            })
            .collect();
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &headers)
            .field("body_len", &self.body.as_ref().map(Vec::len))
            .finish()
    }
}

/// One response. Redirects are NOT followed: a 3xx arrives here as is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Response {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn json(status: u16, body: &serde_json::Value) -> Self {
        Response::new(status, body.to_string()).with_header("Content-Type", "application/json")
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        header_in(&self.headers, name)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn parse_json<T: serde::de::DeserializeOwned>(&self) -> Result<T, String> {
        serde_json::from_slice(&self.body).map_err(|e| format!("unreadable JSON: {e}"))
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

fn header_in<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Why no [`Response`] came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// Refused before anything was sent: plaintext, a host the policy does
    /// not allow, a malformed URL or header.
    Refused(String),
    /// No connection: DNS, TCP, TLS.
    Connect(String),
    /// The whole exchange outlived the request's timeout.
    Timeout,
    /// The peer sent more than the cap.
    TooLarge(String),
    /// A connection, but an unreadable answer.
    Protocol(String),
}

impl TransportError {
    /// Network-shaped: the tracker could not be reached (as opposed to a
    /// request this process refused to send).
    pub fn is_unreachable(&self) -> bool {
        matches!(self, TransportError::Connect(_) | TransportError::Timeout)
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportError::Refused(m) => write!(f, "refused: {m}"),
            TransportError::Connect(m) => write!(f, "unreachable: {m}"),
            TransportError::Timeout => write!(f, "timed out"),
            TransportError::TooLarge(m) => write!(f, "response too large: {m}"),
            TransportError::Protocol(m) => write!(f, "bad response: {m}"),
        }
    }
}

impl std::error::Error for TransportError {}

#[async_trait::async_trait]
pub trait HttpTransport: Send + Sync {
    async fn send(&self, req: Request) -> Result<Response, TransportError>;
}

/// Which hosts a [`DirectTransport`] may connect to.
pub type HostPolicy = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Why an address a host name resolved to is refused (work graph M6.5: an
/// admin-configured site must not reach this machine or a cloud metadata
/// service). IPv4-mapped IPv6 is judged as the IPv4 it carries.
pub fn refused_address(ip: std::net::IpAddr) -> Option<&'static str> {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            if v4.is_loopback() {
                Some("loopback")
            } else if v4.is_link_local() {
                Some("link-local (cloud metadata lives at 169.254.169.254)")
            } else if o[0] == 0 {
                Some("unspecified")
            } else if v4.is_broadcast() || v4.is_multicast() {
                Some("broadcast or multicast")
            } else {
                None
            }
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return refused_address(IpAddr::V4(v4));
            }
            if v6.is_loopback() {
                Some("loopback")
            } else if v6.is_unspecified() {
                Some("unspecified")
            } else if (v6.segments()[0] & 0xffc0) == 0xfe80 {
                Some("link-local")
            } else if v6.is_multicast() {
                Some("multicast")
            } else {
                None
            }
        }
    }
}

/// HTTPS from this process. See the module docs for what it refuses.
pub struct DirectTransport {
    allow_host: HostPolicy,
    max_body: u64,
    /// Resolve first and refuse loopback / link-local / unspecified
    /// addresses (`Some(false)`), or allow them on an admin's opt-in
    /// (`Some(true)`); either way connect to the checked address only.
    /// `None`: the plain path (a fixed public API host).
    addr_guard: Option<bool>,
    /// Extra trusted CA (PEM) for this transport's connections.
    extra_ca: Option<String>,
}

impl DirectTransport {
    pub fn new(allow_host: HostPolicy) -> Self {
        DirectTransport {
            allow_host,
            max_body: DEFAULT_MAX_BODY,
            addr_guard: None,
            extra_ca: None,
        }
    }

    /// Check every address the host resolves to (see [`refused_address`])
    /// before connecting, and connect only to a checked one. `allow_private`
    /// is the admin's explicit opt-in to loopback / link-local targets.
    pub fn with_address_guard(mut self, allow_private: bool) -> Self {
        self.addr_guard = Some(allow_private);
        self
    }

    /// Trust `pem` (one or more CA certificates) besides the platform store.
    /// Implies the address guard's pinned connect path.
    pub fn with_extra_ca(mut self, pem: Option<String>) -> Self {
        self.extra_ca = pem.filter(|p| !p.trim().is_empty());
        self
    }

    /// The guarded path: resolve, check, connect to a checked address.
    async fn connect_guarded(
        &self,
        at: &Target,
        allow_private: bool,
    ) -> Result<super::conn::Stream, TransportError> {
        let host = at.endpoint.host();
        let port = at.endpoint.port();
        let addrs: Vec<std::net::SocketAddr> =
            tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((host, port)))
                .await
                .map_err(|_| TransportError::Connect(format!("resolving {host} timed out")))?
                .map_err(|e| TransportError::Connect(format!("resolve {host}: {e}")))?
                .collect();
        if addrs.is_empty() {
            return Err(TransportError::Connect(format!("{host} has no address")));
        }
        for a in &addrs {
            let why = refused_address(a.ip());
            let multicast = a.ip().is_multicast();
            if let Some(why) = why.filter(|_| !allow_private || multicast) {
                return Err(TransportError::Refused(format!(
                    "{host} resolves to {} ({why}); a tracker site may not point there \
                     unless an admin sets allow_private_network",
                    a.ip()
                )));
            }
        }
        let connector = match &self.extra_ca {
            Some(pem) => super::tls::tls_connector_with_extra_roots(pem)
                .await
                .map_err(TransportError::Refused)?,
            None => super::tls::tls_connector()
                .await
                .map_err(TransportError::Connect)?
                .clone(),
        };
        super::conn::connect_pinned(&addrs, host, &connector, CONNECT_TIMEOUT)
            .await
            .map_err(TransportError::Connect)
    }

    pub fn with_max_body(mut self, max: u64) -> Self {
        self.max_body = max;
        self
    }
}

/// A URL split into the endpoint to connect to and the request-line target
/// (path plus query).
#[derive(Debug)]
pub struct Target {
    pub endpoint: fleet_proto::net::Endpoint,
    pub target: String,
}

/// Parse an absolute URL for a hand-written request. `fleet_proto`'s parser
/// refuses a query (its callers concatenate paths onto a base); a tracker API
/// needs one, so it is split off here and checked on its own: printable
/// ASCII, no space, no fragment.
pub fn parse_target(url: &str) -> Result<Target, TransportError> {
    let (base, query) = match url.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (url, None),
    };
    if url.contains('#') {
        return Err(TransportError::Refused("a URL with a fragment".into()));
    }
    let endpoint = fleet_proto::net::Endpoint::parse(base).map_err(TransportError::Refused)?;
    let mut target = endpoint.request_target();
    if let Some(q) = query {
        if !q.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(TransportError::Refused(
                "a query with a space or a control character".into(),
            ));
        }
        target.push('?');
        target.push_str(q);
    }
    Ok(Target { endpoint, target })
}

/// A header name or value that would let a caller inject a line.
fn header_ok(s: &str) -> bool {
    !s.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
}

/// The request bytes [`DirectTransport`] writes.
pub fn render_request(req: &Request, at: &Target) -> Result<Vec<u8>, TransportError> {
    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: claude-fleet/{}\r\n\
         Accept-Encoding: identity\r\nConnection: close\r\n",
        req.method.as_str(),
        at.target,
        at.endpoint.authority(),
        crate::app_version::get(),
    );
    for (k, v) in &req.headers {
        if !header_ok(k) || !header_ok(v) || k.is_empty() || k.contains(':') {
            return Err(TransportError::Refused(format!("malformed header {k:?}")));
        }
        head.push_str(k);
        head.push_str(": ");
        head.push_str(v);
        head.push_str("\r\n");
    }
    let body = req.body.as_deref().unwrap_or_default();
    if req.body.is_some() || req.method == Method::Post {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    Ok(out)
}

/// Split raw response bytes into a [`Response`], de-chunking when the head
/// says so, and checking the framing: `conn::speak` forgives a peer that
/// closes without `close_notify` (bytes in hand ARE the answer under
/// `Connection: close`), which is only safe because this is where a body
/// the network cut mid-way is caught. A `Content-Length` must be met
/// exactly and a chunked body must reach its last chunk; a shortfall is
/// [`TransportError::Connect`] (`truncated response`), so the tracker
/// reads as unreachable rather than as answering unreadable JSON.
pub fn parse_response(raw: &[u8]) -> Result<Response, TransportError> {
    let split = http1::find(raw, b"\r\n\r\n")
        .ok_or_else(|| TransportError::Protocol("no end of the response head".into()))?;
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let status = http1::parse_status(&head).map_err(TransportError::Protocol)?;
    let headers = http1::parse_headers(&head);
    let body = &raw[split + 4..];
    // No body by definition (RFC 9112 §6.3), whatever the head declares.
    let bodiless = status / 100 == 1 || status == 204 || status == 304;
    let body = if bodiless {
        Vec::new()
    } else if http1::head_is_chunked(&head) {
        let mut rest = body.to_vec();
        let mut dechunker = http1::Dechunker::new(true);
        let out = dechunker
            .take(&mut rest)
            .map_err(TransportError::Protocol)?;
        if !dechunker.finished() {
            return Err(TransportError::Connect(format!(
                "truncated response: the chunked body stopped after {} byte(s), before its last chunk",
                out.len()
            )));
        }
        out
    } else {
        let declared = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .map(|(_, v)| {
                v.trim().parse::<usize>().map_err(|_| {
                    TransportError::Protocol(format!("unreadable Content-Length {v:?}"))
                })
            })
            .transpose()?;
        match declared {
            Some(n) if body.len() < n => {
                return Err(TransportError::Connect(format!(
                    "truncated response: {} of {n} body bytes arrived",
                    body.len()
                )));
            }
            Some(n) if body.len() > n => {
                return Err(TransportError::Protocol(format!(
                    "{} byte(s) past the declared Content-Length of {n}",
                    body.len() - n
                )));
            }
            // Framed by the connection closing: what arrived is the body.
            _ => body.to_vec(),
        }
    };
    Ok(Response {
        status,
        headers,
        body,
    })
}

#[async_trait::async_trait]
impl HttpTransport for DirectTransport {
    async fn send(&self, req: Request) -> Result<Response, TransportError> {
        let at = parse_target(&req.url)?;
        if !at.endpoint.is_tls() {
            return Err(TransportError::Refused(
                "plaintext http:// is never used for a tracker".into(),
            ));
        }
        if !(self.allow_host)(at.endpoint.host()) {
            return Err(TransportError::Refused(format!(
                "{} is not an allowed tracker host",
                at.endpoint.host()
            )));
        }
        let bytes = render_request(&req, &at)?;
        let max = self.max_body;
        let guard = self.addr_guard.or(self.extra_ca.as_ref().map(|_| false));
        let exchange = async {
            let stream = match guard {
                Some(allow_private) => self.connect_guarded(&at, allow_private).await?,
                None => conn::connect(&at.endpoint, CONNECT_TIMEOUT)
                    .await
                    .map_err(TransportError::Connect)?,
            };
            let host = at.endpoint.host();
            let port = at.endpoint.port();
            conn::speak(stream, host, port, &bytes, max)
                .await
                .map_err(|e| {
                    if e == conn::too_large(host, port, max) {
                        TransportError::TooLarge(e)
                    } else {
                        TransportError::Connect(e)
                    }
                })
        };
        let raw = tokio::time::timeout(req.timeout, exchange)
            .await
            .map_err(|_| TransportError::Timeout)??;
        parse_response(&raw)
    }
}

// --- the fake ---------------------------------------------------------------

type Answer = Result<Response, TransportError>;

struct Route {
    method: Method,
    /// Matched against the URL's path and query (`/rest/api/3/myself`).
    needle: String,
    answers: VecDeque<Answer>,
    /// The last answer repeats forever instead of being used up.
    sticky: bool,
}

/// A scripted [`HttpTransport`] for tests: routes by method and a substring
/// of the URL, answers in order, and records every request it was sent. A
/// request no route matches fails as [`TransportError::Connect`], the way an
/// unreachable tracker would, so a test that forgot a route fails loudly.
#[derive(Default, Clone)]
pub struct FakeTransport {
    routes: Arc<Mutex<Vec<Route>>>,
    sent: Arc<Mutex<Vec<Request>>>,
}

impl FakeTransport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Answer the next request matching `method` + `needle` once.
    pub fn once(&self, method: Method, needle: &str, answer: Answer) -> &Self {
        self.push(method, needle, answer, false)
    }

    /// Answer every request matching `method` + `needle` (after any queued
    /// one-shot answers of the same route).
    pub fn always(&self, method: Method, needle: &str, answer: Answer) -> &Self {
        self.push(method, needle, answer, true)
    }

    fn push(&self, method: Method, needle: &str, answer: Answer, sticky: bool) -> &Self {
        let mut routes = self.routes.lock().unwrap();
        match routes
            .iter_mut()
            .find(|r| r.method == method && r.needle == needle && !r.sticky)
        {
            Some(r) => {
                r.answers.push_back(answer);
                r.sticky = sticky;
            }
            None => routes.push(Route {
                method,
                needle: needle.to_string(),
                answers: VecDeque::from([answer]),
                sticky,
            }),
        }
        self
    }

    /// Every request sent so far, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.sent.lock().unwrap().clone()
    }

    /// How many requests hit a URL containing `needle`.
    pub fn count(&self, needle: &str) -> usize {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.url.contains(needle))
            .count()
    }

    /// Forget every route (the recorded requests stay).
    pub fn clear_routes(&self) {
        self.routes.lock().unwrap().clear();
    }
}

#[async_trait::async_trait]
impl HttpTransport for FakeTransport {
    async fn send(&self, req: Request) -> Result<Response, TransportError> {
        self.sent.lock().unwrap().push(req.clone());
        let mut routes = self.routes.lock().unwrap();
        // Longest needle first, so `/search/jql` beats `/search`.
        let mut idx: Vec<usize> = (0..routes.len()).collect();
        idx.sort_by_key(|&i| std::cmp::Reverse(routes[i].needle.len()));
        for i in idx {
            let r = &mut routes[i];
            if r.method != req.method || !req.url.contains(&r.needle) || r.answers.is_empty() {
                continue;
            }
            return if r.sticky && r.answers.len() == 1 {
                r.answers[0].clone()
            } else {
                r.answers.pop_front().unwrap()
            };
        }
        Err(TransportError::Connect(format!(
            "no fake route for {} {}",
            req.method.as_str(),
            req.url
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn debug_never_prints_a_credential_header() {
        let r = Request::get("https://acme.atlassian.net/rest/api/3/myself")
            .header("Authorization", "Basic dXNlcjpBVEFUVHNlY3JldA==")
            .header("cookie", "tenant.session.token=abc");
        let dbg = format!("{r:?}");
        assert!(!dbg.contains("dXNlcj"), "{dbg}");
        assert!(!dbg.contains("abc"), "{dbg}");
        assert!(dbg.contains("[REDACTED]"));
    }

    #[test]
    fn a_query_is_carried_but_a_fragment_or_space_is_refused() {
        let t = parse_target("https://acme.atlassian.net/rest/api/3/project/search?startAt=50")
            .unwrap();
        assert_eq!(t.target, "/rest/api/3/project/search?startAt=50");
        assert_eq!(t.endpoint.host(), "acme.atlassian.net");
        for bad in [
            "https://acme.atlassian.net/x#frag",
            "https://acme.atlassian.net/x?a=b c",
            "https://user:pw@acme.atlassian.net/x",
        ] {
            assert!(
                matches!(parse_target(bad), Err(TransportError::Refused(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_request_renders_with_close_and_a_length_and_refuses_header_injection() {
        let req = Request::post_json(
            "https://acme.atlassian.net/rest/api/3/search/jql",
            &json!({"a":1}),
        )
        .header("Authorization", "Basic x");
        let at = parse_target(&req.url).unwrap();
        let text = String::from_utf8(render_request(&req, &at).unwrap()).unwrap();
        assert!(text
            .starts_with("POST /rest/api/3/search/jql HTTP/1.1\r\nHost: acme.atlassian.net\r\n"));
        assert!(text.contains("Connection: close\r\n"));
        assert!(text.contains("Content-Length: 7\r\n"));
        assert!(text.ends_with("\r\n\r\n{\"a\":1}"));
        let evil = Request::get("https://acme.atlassian.net/x").header("X", "a\r\nHost: evil");
        assert!(matches!(
            render_request(&evil, &parse_target(&evil.url).unwrap()),
            Err(TransportError::Refused(_))
        ));
    }

    #[test]
    fn a_chunked_response_is_decoded_and_headers_are_kept() {
        let raw = b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 7\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n{\"a\r\n4\r\n\":1}\r\n0\r\n\r\n";
        let r = parse_response(raw).unwrap();
        assert_eq!(r.status, 429);
        assert_eq!(r.header("retry-after"), Some("7"));
        assert_eq!(r.text(), "{\"a\":1}");
    }

    /// `speak` forgives a close without close_notify; the framing check
    /// here is what makes that safe. A body the network cut is refused as
    /// unreachable, never handed back with its real status.
    #[test]
    fn a_truncated_body_is_refused_not_returned_with_its_status() {
        let e =
            parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\n{\"a\":1}").unwrap_err();
        assert!(
            matches!(&e, TransportError::Connect(m) if m.contains("truncated") && m.contains("7 of 12")),
            "{e}"
        );
        assert!(e.is_unreachable());
        for cut in [
            // The last chunk never came.
            &b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n{\"a\r\n4\r\n\":1}\r\n"[..],
            // Mid-chunk.
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n{\"a\r\n4\r\n\":",
            // Nothing of the body at all.
            b"HTTP/1.1 200 OK\r\nContent-Length: 300000\r\n\r\n",
        ] {
            let e = parse_response(cut).unwrap_err();
            assert!(
                matches!(&e, TransportError::Connect(m) if m.contains("truncated")),
                "{e}"
            );
        }
        // More than declared is the server's fault, not the network's.
        let e =
            parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\n{\"a\":1}").unwrap_err();
        assert!(matches!(e, TransportError::Protocol(_)), "{e}");
        let e = parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: many\r\n\r\nx").unwrap_err();
        assert!(matches!(e, TransportError::Protocol(_)), "{e}");

        // Exactly the declared length, a bodiless status whatever its
        // head declares, and a close-framed body all pass.
        let r = parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\n{\"a\":1}").unwrap();
        assert_eq!(r.text(), "{\"a\":1}");
        let r =
            parse_response(b"HTTP/1.1 304 Not Modified\r\nContent-Length: 500\r\n\r\n").unwrap();
        assert_eq!((r.status, r.body.len()), (304, 0));
        assert_eq!(
            parse_response(b"HTTP/1.1 204 No Content\r\n\r\n")
                .unwrap()
                .status,
            204
        );
        let r = parse_response(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nabc").unwrap();
        assert_eq!(r.text(), "abc");
    }

    /// End to end over a socket: the peer drops the connection mid-body.
    #[tokio::test]
    async fn a_connection_dropped_mid_body_is_a_truncated_response() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = s.read(&mut buf).await.unwrap();
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 300000\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            s.write_all(&[b'{'; 120]).await.unwrap();
            // Dropped here: the backend went away.
        });
        let at = parse_target(&format!("http://127.0.0.1:{port}/x")).unwrap();
        let req = Request::get(format!("http://127.0.0.1:{port}/x"));
        let stream = conn::connect(&at.endpoint, CONNECT_TIMEOUT).await.unwrap();
        let raw = conn::speak(
            stream,
            "127.0.0.1",
            port,
            &render_request(&req, &at).unwrap(),
            1024 * 1024,
        )
        .await
        .unwrap();
        let e = parse_response(&raw).unwrap_err();
        assert!(
            matches!(&e, TransportError::Connect(m) if m.contains("120 of 300000")),
            "{e}"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn direct_refuses_plaintext_and_hosts_outside_its_policy_before_connecting() {
        let t = DirectTransport::new(Arc::new(|h: &str| h.ends_with(".atlassian.net")));
        for url in [
            "http://acme.atlassian.net/rest/api/3/myself",
            "https://evil.example.com/rest/api/3/myself",
            "https://127.0.0.1/rest/api/3/myself",
        ] {
            let err = t.send(Request::get(url)).await.unwrap_err();
            assert!(matches!(err, TransportError::Refused(_)), "{url}: {err}");
        }
    }

    #[test]
    fn loopback_link_local_and_unspecified_addresses_are_refused() {
        use std::net::IpAddr;
        for ip in [
            "127.0.0.1",
            "127.1.2.3",
            "169.254.169.254",
            "169.254.0.1",
            "0.0.0.0",
            "0.1.2.3",
            "::1",
            "::",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "255.255.255.255",
            "224.0.0.1",
        ] {
            assert!(
                refused_address(ip.parse::<IpAddr>().unwrap()).is_some(),
                "{ip}"
            );
        }
        for ip in [
            "10.0.0.5",
            "192.168.1.10",
            "172.16.0.1",
            "93.184.216.34",
            "2606:4700::1",
        ] {
            assert_eq!(refused_address(ip.parse::<IpAddr>().unwrap()), None, "{ip}");
        }
    }

    /// After resolution, not before: a name that resolves to loopback (or
    /// an IP literal) is refused, unless the admin opted in.
    #[tokio::test]
    async fn the_guard_checks_what_the_name_resolved_to() {
        let t = DirectTransport::new(Arc::new(|_: &str| true)).with_address_guard(false);
        for url in [
            "https://localhost/rest/api/2/myself",
            "https://127.0.0.1/rest/api/2/myself",
            "https://169.254.169.254/latest/meta-data",
            "https://[::1]/x",
        ] {
            let e = t.send(Request::get(url)).await.unwrap_err();
            assert!(
                matches!(&e, TransportError::Refused(m) if m.contains("allow_private_network")),
                "{url}: {e}"
            );
        }
        // Opted in: the connect is attempted (and fails: nothing listens).
        let t = DirectTransport::new(Arc::new(|_: &str| true)).with_address_guard(true);
        let e = t
            .send(Request::get("https://127.0.0.1:9/x").with_timeout(Duration::from_secs(5)))
            .await
            .unwrap_err();
        assert!(!matches!(e, TransportError::Refused(_)), "{e}");
    }

    /// A 3xx is handed back, never followed: the Location is not dialled.
    #[tokio::test]
    async fn direct_does_not_follow_redirects() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = s.read(&mut buf).await.unwrap();
            s.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });
        // Plain http to loopback only to exercise the read path: `send`
        // refuses plaintext, so this drives the lower half directly.
        let at = parse_target(&format!("http://127.0.0.1:{port}/x")).unwrap();
        let req = Request::get(format!("http://127.0.0.1:{port}/x"));
        let stream = conn::connect(&at.endpoint, CONNECT_TIMEOUT).await.unwrap();
        let raw = conn::speak(
            stream,
            "127.0.0.1",
            port,
            &render_request(&req, &at).unwrap(),
            1024,
        )
        .await
        .unwrap();
        let r = parse_response(&raw).unwrap();
        assert_eq!(r.status, 302);
        assert_eq!(r.header("location"), Some("http://169.254.169.254/"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn fake_answers_in_order_then_sticks_and_records() {
        let f = FakeTransport::new();
        f.once(Method::Get, "/myself", Ok(Response::new(500, "x")))
            .always(
                Method::Get,
                "/myself",
                Ok(Response::json(200, &json!({"ok":true}))),
            );
        let url = "https://a.atlassian.net/rest/api/3/myself";
        assert_eq!(f.send(Request::get(url)).await.unwrap().status, 500);
        assert_eq!(f.send(Request::get(url)).await.unwrap().status, 200);
        assert_eq!(f.send(Request::get(url)).await.unwrap().status, 200);
        assert!(f
            .send(Request::get("https://a.atlassian.net/other"))
            .await
            .unwrap_err()
            .is_unreachable());
        assert_eq!(f.count("/myself"), 3);
        assert_eq!(f.requests().len(), 4);
    }
}
