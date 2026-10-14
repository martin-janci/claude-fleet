//! The hook relay: Claude Code's hooks on this host post to
//! `http://127.0.0.1:<port>/hook`, and the agent forwards each one to the
//! hub's own `/hook` over HTTPS.
//!
//! Why it exists. Claude Code refuses a `type: "http"` hook whose host
//! resolves to a private or link-local address ("HTTP hook blocked: … resolves
//! to 192.168.x.x (private/link-local address). Loopback … is allowed"). A hub
//! whose public name resolves to an in-cluster ingress is exactly that, so
//! every http hook was dropped and the hub learned a session's state only
//! from reconcile. Loopback is allowed, and the agent already reaches the hub
//! by that same name (it dials `wss://<hub>/agent`), so the agent carries the
//! hook the last hop. The hub learns the port from the agent's `hello`
//! (`fleet_proto::AgentFrame::Hello::hook_port`) and writes this URL into the
//! host's hooks instead of its public one.
//!
//! What it forwards, and what it does not:
//!
//! - Only `POST /hook`. Anything else is answered here and never reaches the
//!   hub: the relay is not a general proxy to the hub's API.
//! - Only the headers the hook contract reads — `Authorization`,
//!   `Content-Type`, `X-Fleet-Pane`, `X-Fleet-Sync`. The bearer is the hook's
//!   OWN (the host token provisioning wrote into `settings.json`), checked by
//!   the hub exactly as when Claude Code posted directly; the agent adds no
//!   credential of its own.
//! - A request with an `Origin`, or a `Host` that is not loopback, is
//!   refused: Claude Code sends neither, a browser page aimed at this port
//!   (DNS rebinding included) sends one of them.
//! - The hub's answer — status, `Content-Type` and body — goes back as it
//!   came, because Claude reads it (`additionalContext`, a `PreToolUse`
//!   denial, a blocking `Stop`).
//!
//! One TCP (+TLS) connection per hook, closed after it: hooks are a handful
//! per turn, and nothing is held between them. It does not need the agent's
//! WebSocket to be up, only the hub.

use crate::conn::Dialer;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

/// The relay's port when the config names none. Not the hub's own default
/// (4180), which a desktop on the same machine may already hold.
pub const DEFAULT_PORT: u16 = 4181;

/// The largest hook body the relay accepts. The hub's `/hook` takes at most
/// axum's 2 MiB default, so anything over that would be refused there anyway;
/// the margin keeps the hub the one that says so for a body near the line.
pub const MAX_BODY: usize = 4 * 1024 * 1024;

/// The largest answer relayed back. The hub's are a few KB of
/// `additionalContext` at most.
const MAX_REPLY_BODY: usize = 1024 * 1024;

/// The most bytes a request or response head may take.
const MAX_HEAD: usize = 16 * 1024;

/// The most headers either head may carry.
const MAX_HEADERS: usize = 64;

/// One hook, end to end. Claude Code gives a hook 5 s; this only bounds a
/// relay whose client has gone, so nothing waits on a dead hub for ever.
pub const RELAY_TIMEOUT: Duration = Duration::from_secs(15);

/// Hooks relayed at once; one past this is answered `503` at once.
const MAX_IN_FLIGHT: usize = 32;

/// The request headers that cross to the hub, lower-cased. `Content-Length`
/// is recomputed; `Host` is the hub's.
const FORWARDED: [&str; 4] = [
    "authorization",
    "content-type",
    "x-fleet-pane",
    "x-fleet-sync",
];

/// Listen on `127.0.0.1:<port>` — loopback only, never a routable address.
pub async fn bind(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await
}

/// Relay every connection `listener` accepts to `upstream`'s hub, for as
/// long as the process runs.
pub async fn serve(listener: TcpListener, upstream: Arc<Dialer>) {
    let slots = Arc::new(Semaphore::new(MAX_IN_FLIGHT));
    loop {
        let mut stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(e) => {
                // EMFILE and friends: back off a moment rather than spin.
                tracing::warn!(error = %e, "[hook-relay] accept failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let permit = Arc::clone(&slots).try_acquire_owned();
        let upstream = Arc::clone(&upstream);
        tokio::spawn(async move {
            let reply = match permit {
                Err(_) => Reply::status(503),
                Ok(_permit) => relay(&mut stream, &upstream).await,
            };
            let _ = tokio::time::timeout(RELAY_TIMEOUT, write_reply(&mut stream, &reply)).await;
            let _ = stream.shutdown().await;
        });
    }
}

/// Read one request from `client`, forward it, and return what to answer.
/// Writes nothing to `client` but an interim `100 Continue` when asked for one;
/// the answer is the caller's to write.
pub async fn relay<S: AsyncRead + AsyncWrite + Unpin>(client: &mut S, upstream: &Dialer) -> Reply {
    match tokio::time::timeout(RELAY_TIMEOUT, relay_inner(client, upstream)).await {
        Ok(reply) => reply,
        Err(_) => {
            tracing::warn!("[hook-relay] a hook took over {}s", RELAY_TIMEOUT.as_secs());
            Reply::status(504)
        }
    }
}

async fn relay_inner<S: AsyncRead + AsyncWrite + Unpin>(
    client: &mut S,
    upstream: &Dialer,
) -> Reply {
    let request = match read_request(client).await {
        Ok(r) => r,
        Err(reply) => return reply,
    };
    match forward(&request, upstream).await {
        Ok(reply) => reply,
        Err(why) => {
            // Never the body or a header value: those carry the prompt and
            // the bearer.
            tracing::warn!(why = %why, "[hook-relay] could not reach the hub");
            Reply::status(502)
        }
    }
}

/// What the relay answers Claude Code with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

impl Reply {
    fn status(status: u16) -> Self {
        Self {
            status,
            content_type: None,
            body: Vec::new(),
        }
    }
}

/// A request that passed every check, ready to forward.
#[derive(Debug)]
struct Inbound {
    /// `FORWARDED` headers only, names lower-cased.
    headers: Vec<(String, Vec<u8>)>,
    body: Vec<u8>,
}

/// How a body is delimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framing {
    Length(usize),
    Chunked,
    ToEof,
    Empty,
}

/// Why a body could not be read.
#[derive(Debug)]
enum BodyError {
    TooLarge,
    Malformed(&'static str),
    Io(std::io::Error),
}

/// Read and check one request. `Err` is the refusal to answer with.
async fn read_request<S: AsyncRead + AsyncWrite + Unpin>(client: &mut S) -> Result<Inbound, Reply> {
    let mut buf = Vec::with_capacity(4096);
    let (head_len, method, path, headers) = loop {
        if read_more(client, &mut buf)
            .await
            .map_err(|_| Reply::status(400))?
            == 0
        {
            return Err(Reply::status(400));
        }
        let mut slots = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut req = httparse::Request::new(&mut slots);
        match req.parse(&buf) {
            Ok(httparse::Status::Complete(n)) => {
                let headers: Vec<(String, Vec<u8>)> = req
                    .headers
                    .iter()
                    .map(|h| (h.name.to_ascii_lowercase(), h.value.to_vec()))
                    .collect();
                break (
                    n,
                    req.method.unwrap_or("").to_string(),
                    req.path.unwrap_or("").to_string(),
                    headers,
                );
            }
            Ok(httparse::Status::Partial) if buf.len() < MAX_HEAD => continue,
            Ok(httparse::Status::Partial) => return Err(Reply::status(431)),
            Err(httparse::Error::TooManyHeaders) => return Err(Reply::status(431)),
            Err(_) => return Err(Reply::status(400)),
        }
    };
    let header = |name: &str| {
        headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_slice())
    };
    // Path first: anything but the hook endpoint is not ours to answer.
    if path != "/hook" {
        return Err(Reply::status(404));
    }
    if method != "POST" {
        return Err(Reply::status(405));
    }
    if header("origin").is_some() || !header("host").is_some_and(host_is_loopback) {
        tracing::warn!("[hook-relay] refused a request with an Origin or a non-loopback Host");
        return Err(Reply::status(403));
    }
    let framing = request_framing(header("transfer-encoding"), header("content-length"))?;
    // curl asks before sending a larger body, and waits a second for leave
    // unless told to go on.
    if header("expect").is_some_and(|v| trimmed_eq(v, b"100-continue")) {
        client
            .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
            .await
            .map_err(|_| Reply::status(400))?;
    }
    let body = read_body(client, buf.split_off(head_len), framing, MAX_BODY)
        .await
        .map_err(|e| match e {
            BodyError::TooLarge => Reply::status(413),
            BodyError::Malformed(_) | BodyError::Io(_) => Reply::status(400),
        })?;
    Ok(Inbound {
        headers: headers
            .into_iter()
            .filter(|(n, _)| FORWARDED.contains(&n.as_str()))
            .collect(),
        body,
    })
}

/// The request body's framing. A request must say how long it is.
fn request_framing(te: Option<&[u8]>, cl: Option<&[u8]>) -> Result<Framing, Reply> {
    if let Some(te) = te {
        // Only the one coding a client sends; anything else is not ours.
        return if trimmed_eq(te, b"chunked") {
            Ok(Framing::Chunked)
        } else {
            Err(Reply::status(501))
        };
    }
    match cl {
        None => Err(Reply::status(411)),
        Some(v) => match parse_length(v) {
            Some(n) if n > MAX_BODY => Err(Reply::status(413)),
            Some(0) => Ok(Framing::Empty),
            Some(n) => Ok(Framing::Length(n)),
            None => Err(Reply::status(400)),
        },
    }
}

/// `Host: 127.0.0.1:4181`, `localhost:4181`, `[::1]:4181` — loopback, any
/// port. Anything else is a request this relay was not meant to get.
fn host_is_loopback(value: &[u8]) -> bool {
    let Ok(host) = std::str::from_utf8(value) else {
        return false;
    };
    let host = host.trim();
    let name = if host.starts_with('[') {
        match host.find(']') {
            Some(end) => &host[..=end],
            None => return false,
        }
    } else {
        host.rsplit_once(':').map_or(host, |(name, _)| name)
    };
    fleet_proto::net::is_loopback(name)
}

/// Send `request` to the hub's `/hook` and read its answer.
async fn forward(request: &Inbound, upstream: &Dialer) -> Result<Reply, String> {
    let endpoint = upstream.endpoint();
    let mut head = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Length: {}\r\nConnection: close\r\n\
         User-Agent: fleet-agent/{} (hook relay)\r\n",
        endpoint.hook_path(),
        endpoint.authority(),
        request.body.len(),
        env!("CARGO_PKG_VERSION"),
    )
    .into_bytes();
    let mut has_type = false;
    for (name, value) in &request.headers {
        // httparse never yields a CR or LF in a value; checked anyway, since
        // a header smuggled into the hub's request would be the hub's problem.
        if value.iter().any(|&b| b == b'\r' || b == b'\n') {
            return Err(format!("a line break in the {name} header"));
        }
        has_type |= name == "content-type";
        head.extend_from_slice(name.as_bytes());
        head.extend_from_slice(b": ");
        head.extend_from_slice(value);
        head.extend_from_slice(b"\r\n");
    }
    if !has_type {
        head.extend_from_slice(b"content-type: application/json\r\n");
    }
    head.extend_from_slice(b"\r\n");

    let mut io = upstream.connect_io().await.map_err(|e| e.to_string())?;
    io.write_all(&head).await.map_err(|e| e.to_string())?;
    io.write_all(&request.body)
        .await
        .map_err(|e| e.to_string())?;
    io.flush().await.map_err(|e| e.to_string())?;
    read_response(&mut io).await
}

/// Read the hub's answer.
async fn read_response<S: AsyncRead + Unpin>(io: &mut S) -> Result<Reply, String> {
    let mut buf = Vec::with_capacity(1024);
    // False only right after an interim answer, whose successor may already
    // be in `buf`.
    let mut need_more = true;
    loop {
        if need_more && read_more(io, &mut buf).await.map_err(|e| e.to_string())? == 0 {
            return Err("the hub closed before answering".into());
        }
        need_more = true;
        let mut slots = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut resp = httparse::Response::new(&mut slots);
        match resp.parse(&buf) {
            Ok(httparse::Status::Complete(n)) => {
                let status = resp.code.unwrap_or(0);
                let header = |name: &str| {
                    resp.headers
                        .iter()
                        .find(|h| h.name.eq_ignore_ascii_case(name))
                        .map(|h| h.value)
                };
                // An interim 1xx is not the answer; hyper sends none unasked.
                if (100..200).contains(&status) {
                    buf.drain(..n);
                    need_more = buf.is_empty();
                    continue;
                }
                let content_type = header("content-type")
                    .and_then(|v| std::str::from_utf8(v).ok())
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty() && !v.contains(['\r', '\n']));
                let framing = if status == 204 || status == 304 {
                    Framing::Empty
                } else if let Some(te) = header("transfer-encoding") {
                    if trimmed_eq(te, b"chunked") {
                        Framing::Chunked
                    } else {
                        Framing::ToEof
                    }
                } else if let Some(cl) = header("content-length") {
                    match parse_length(cl) {
                        Some(0) => Framing::Empty,
                        Some(n) => Framing::Length(n),
                        None => return Err("a malformed Content-Length".into()),
                    }
                } else {
                    Framing::ToEof
                };
                let body = read_body(io, buf.split_off(n), framing, MAX_REPLY_BODY)
                    .await
                    .map_err(|e| match e {
                        BodyError::TooLarge => "an answer over the relay's limit".to_string(),
                        BodyError::Malformed(why) => why.to_string(),
                        BodyError::Io(e) => e.to_string(),
                    })?;
                return Ok(Reply {
                    status,
                    content_type,
                    body,
                });
            }
            Ok(httparse::Status::Partial) if buf.len() < MAX_HEAD => continue,
            Ok(httparse::Status::Partial) => return Err("an answer head over 16 KiB".into()),
            Err(e) => return Err(format!("not an HTTP answer: {e}")),
        }
    }
}

/// Write `reply` to Claude Code and end the exchange.
async fn write_reply<S: AsyncWrite + Unpin>(client: &mut S, reply: &Reply) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {} {}\r\n", reply.status, reason(reply.status));
    // A 204 carries no body and no length.
    if reply.status != 204 {
        head.push_str(&format!("Content-Length: {}\r\n", reply.body.len()));
        if let Some(ct) = &reply.content_type {
            head.push_str(&format!("Content-Type: {ct}\r\n"));
        }
    }
    head.push_str("Connection: close\r\n\r\n");
    client.write_all(head.as_bytes()).await?;
    if reply.status != 204 {
        client.write_all(&reply.body).await?;
    }
    client.flush().await
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        413 => "Content Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Relayed",
    }
}

/// Append whatever `io` has next to `buf`; `0` at the end of the stream.
async fn read_more<S: AsyncRead + Unpin>(io: &mut S, buf: &mut Vec<u8>) -> std::io::Result<usize> {
    let mut chunk = [0u8; 8192];
    let n = io.read(&mut chunk).await?;
    buf.extend_from_slice(&chunk[..n]);
    Ok(n)
}

/// Read a body framed as `framing`, starting from what is already in `buf`,
/// refusing one over `cap`.
async fn read_body<S: AsyncRead + Unpin>(
    io: &mut S,
    mut buf: Vec<u8>,
    framing: Framing,
    cap: usize,
) -> Result<Vec<u8>, BodyError> {
    match framing {
        Framing::Empty => Ok(Vec::new()),
        Framing::Length(n) => {
            if n > cap {
                return Err(BodyError::TooLarge);
            }
            while buf.len() < n {
                if read_more(io, &mut buf).await.map_err(BodyError::Io)? == 0 {
                    return Err(BodyError::Malformed("the body ended early"));
                }
            }
            buf.truncate(n);
            Ok(buf)
        }
        Framing::ToEof => {
            while read_more(io, &mut buf).await.map_err(BodyError::Io)? > 0 {
                if buf.len() > cap {
                    return Err(BodyError::TooLarge);
                }
            }
            if buf.len() > cap {
                return Err(BodyError::TooLarge);
            }
            Ok(buf)
        }
        Framing::Chunked => {
            let mut body = Vec::new();
            loop {
                let line = take_line(io, &mut buf).await?;
                let size = std::str::from_utf8(&line)
                    .ok()
                    .map(|l| l.split(';').next().unwrap_or("").trim())
                    .and_then(|hex| usize::from_str_radix(hex, 16).ok())
                    .ok_or(BodyError::Malformed("a malformed chunk size"))?;
                if size == 0 {
                    // Trailers, up to the empty line that ends the body.
                    while !take_line(io, &mut buf).await?.is_empty() {}
                    return Ok(body);
                }
                if body.len().saturating_add(size) > cap {
                    return Err(BodyError::TooLarge);
                }
                while buf.len() < size + 2 {
                    if read_more(io, &mut buf).await.map_err(BodyError::Io)? == 0 {
                        return Err(BodyError::Malformed("a chunk ended early"));
                    }
                }
                if &buf[size..size + 2] != b"\r\n" {
                    return Err(BodyError::Malformed("a chunk without its line end"));
                }
                body.extend_from_slice(&buf[..size]);
                buf.drain(..size + 2);
            }
        }
    }
}

/// The next CRLF-terminated line of a chunked body, without its CRLF.
async fn take_line<S: AsyncRead + Unpin>(
    io: &mut S,
    buf: &mut Vec<u8>,
) -> Result<Vec<u8>, BodyError> {
    loop {
        if let Some(at) = buf.windows(2).position(|w| w == b"\r\n") {
            let line = buf[..at].to_vec();
            buf.drain(..at + 2);
            return Ok(line);
        }
        if buf.len() > MAX_HEAD {
            return Err(BodyError::Malformed("a chunk line over 16 KiB"));
        }
        if read_more(io, buf).await.map_err(BodyError::Io)? == 0 {
            return Err(BodyError::Malformed("the body ended inside a chunk line"));
        }
    }
}

fn parse_length(v: &[u8]) -> Option<usize> {
    let s = std::str::from_utf8(v).ok()?.trim();
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

fn trimmed_eq(v: &[u8], want: &[u8]) -> bool {
    std::str::from_utf8(v)
        .map(|s| s.trim().as_bytes().eq_ignore_ascii_case(want))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::Endpoint;
    use tokio::net::TcpStream;

    /// A dialer at a plain loopback "hub" on `port` (`--insecure`).
    fn dialer(port: u16) -> Arc<Dialer> {
        let ep = Endpoint::parse(&format!("http://127.0.0.1:{port}"), true).unwrap();
        Arc::new(Dialer::new(ep, "agent-token".into(), None).unwrap())
    }

    /// A port with nothing listening on it.
    async fn dead_port() -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    }

    /// Start a relay in front of `upstream`; its port.
    async fn relay_to(upstream: Arc<Dialer>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(serve(listener, upstream));
        port
    }

    /// Send `raw` to the relay and read its whole answer.
    async fn ask(port: u16, raw: &[u8]) -> (u16, String, Vec<u8>) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(raw).await.unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).await.unwrap();
        let mut slots = [httparse::EMPTY_HEADER; 16];
        let mut resp = httparse::Response::new(&mut slots);
        let n = match resp.parse(&out).unwrap() {
            httparse::Status::Complete(n) => n,
            httparse::Status::Partial => panic!("a partial answer: {out:?}"),
        };
        let head = String::from_utf8_lossy(&out[..n]).to_string();
        (resp.code.unwrap(), head, out[n..].to_vec())
    }

    /// A fake hub that takes ONE request, hands it to the test, and answers
    /// with `answer`.
    async fn fake_hub(answer: &'static [u8]) -> (u16, tokio::sync::oneshot::Receiver<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut got = Vec::new();
            // The relay sends a Content-Length: read the head, then that much.
            loop {
                read_more(&mut s, &mut got).await.unwrap();
                let mut slots = [httparse::EMPTY_HEADER; 32];
                let mut req = httparse::Request::new(&mut slots);
                if let httparse::Status::Complete(n) = req.parse(&got).unwrap() {
                    let len: usize = req
                        .headers
                        .iter()
                        .find(|h| h.name.eq_ignore_ascii_case("content-length"))
                        .map(|h| std::str::from_utf8(h.value).unwrap().parse().unwrap())
                        .unwrap();
                    while got.len() < n + len {
                        read_more(&mut s, &mut got).await.unwrap();
                    }
                    break;
                }
            }
            s.write_all(answer).await.unwrap();
            s.shutdown().await.unwrap();
            let _ = tx.send(got);
        });
        (port, rx)
    }

    fn hook_request(host: &str, extra: &str, body: &str) -> Vec<u8> {
        format!(
            "POST /hook HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer host-tok\r\n\
             Content-Type: application/json\r\nX-Fleet-Pane: %7\r\nCookie: secret\r\n{extra}\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    #[tokio::test]
    async fn a_hook_reaches_the_hub_with_its_own_bearer_and_the_answer_comes_back() {
        let (hub, seen) = fake_hub(
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 17\r\n\r\n{\"decision\":\"x\"}\n",
        )
        .await;
        let relay = relay_to(dialer(hub)).await;
        let body = r#"{"hook_event_name":"Stop","session_id":"s1"}"#;
        let (status, head, answer) = ask(
            relay,
            &hook_request(&format!("127.0.0.1:{relay}"), "X-Fleet-Sync: 1\r\n", body),
        )
        .await;
        assert_eq!(status, 200);
        assert!(head.contains("Content-Type: application/json"), "{head}");
        assert_eq!(answer, b"{\"decision\":\"x\"}\n");

        let sent = String::from_utf8(seen.await.unwrap()).unwrap();
        assert!(sent.starts_with("POST /hook HTTP/1.1\r\n"), "{sent}");
        assert!(
            sent.contains(&format!("Host: 127.0.0.1:{hub}\r\n")),
            "{sent}"
        );
        assert!(
            sent.contains("authorization: Bearer host-tok\r\n"),
            "{sent}"
        );
        assert!(sent.contains("x-fleet-pane: %7\r\n"), "{sent}");
        assert!(sent.contains("x-fleet-sync: 1\r\n"), "{sent}");
        assert!(sent.ends_with(body), "{sent}");
        // Only the hook contract's headers cross; the agent adds no token.
        assert!(!sent.to_ascii_lowercase().contains("cookie"), "{sent}");
        assert!(!sent.contains("agent-token"), "{sent}");
    }

    #[tokio::test]
    async fn a_no_content_answer_and_a_chunked_one_are_relayed() {
        let (hub, _seen) = fake_hub(b"HTTP/1.1 204 No Content\r\n\r\n").await;
        let relay = relay_to(dialer(hub)).await;
        let (status, head, answer) = ask(relay, &hook_request("localhost:4181", "", "{}")).await;
        assert_eq!(status, 204);
        assert!(!head.contains("Content-Length"), "{head}");
        assert!(answer.is_empty());

        let (hub, _seen) = fake_hub(
            b"HTTP/1.1 401 Unauthorized\r\ntransfer-encoding: chunked\r\n\r\n3\r\nno \r\n5\r\ntoken\r\n0\r\n\r\n",
        )
        .await;
        let relay = relay_to(dialer(hub)).await;
        let (status, _, answer) = ask(relay, &hook_request("[::1]:4181", "", "{}")).await;
        assert_eq!(status, 401);
        assert_eq!(answer, b"no token");
    }

    #[tokio::test]
    async fn a_chunked_hook_body_is_forwarded_with_its_length() {
        let (hub, seen) = fake_hub(b"HTTP/1.1 204 No Content\r\n\r\n").await;
        let relay = relay_to(dialer(hub)).await;
        let raw = b"POST /hook HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer t\r\n\
                    Transfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n";
        let (status, _, _) = ask(relay, raw).await;
        assert_eq!(status, 204);
        let sent = String::from_utf8(seen.await.unwrap()).unwrap();
        assert!(sent.contains("Content-Length: 7\r\n"), "{sent}");
        assert!(
            sent.contains("content-type: application/json\r\n"),
            "{sent}"
        );
        assert!(sent.ends_with("{\"a\":1}"), "{sent}");
    }

    /// curl's `Expect: 100-continue` is answered at once, so a larger
    /// SessionStart or guard body does not wait out curl's one-second pause.
    #[tokio::test]
    async fn an_expect_continue_is_answered_before_the_body_is_sent() {
        let (hub, seen) = fake_hub(b"HTTP/1.1 204 No Content\r\n\r\n").await;
        let relay = relay_to(dialer(hub)).await;
        let mut s = TcpStream::connect(("127.0.0.1", relay)).await.unwrap();
        s.write_all(
            b"POST /hook HTTP/1.1\r\nHost: 127.0.0.1\r\nExpect: 100-continue\r\nContent-Length: 2\r\n\r\n",
        )
        .await
        .unwrap();
        let mut interim = [0u8; 25];
        tokio::time::timeout(Duration::from_millis(500), s.read_exact(&mut interim))
            .await
            .expect("the 100 Continue comes before the body is sent")
            .unwrap();
        assert_eq!(&interim, b"HTTP/1.1 100 Continue\r\n\r\n");
        s.write_all(b"{}").await.unwrap();
        let mut rest = Vec::new();
        s.read_to_end(&mut rest).await.unwrap();
        assert!(rest.starts_with(b"HTTP/1.1 204"), "{rest:?}");
        assert!(String::from_utf8(seen.await.unwrap())
            .unwrap()
            .ends_with("{}"));
    }

    /// Every refusal is answered by the relay itself: the upstream is a dead
    /// port, so a request that slipped through would come back 502 instead.
    #[tokio::test]
    async fn what_is_not_a_hook_never_reaches_the_hub() {
        let relay = relay_to(dialer(dead_port().await)).await;
        let cases: [(&[u8], u16); 7] = [
            (b"GET /hook HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n", 405),
            (
                b"POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2\r\n\r\n{}",
                404,
            ),
            (
                b"POST /hook?x=1 HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2\r\n\r\n{}",
                404,
            ),
            (
                b"POST /hook HTTP/1.1\r\nHost: evil.example\r\nContent-Length: 2\r\n\r\n{}",
                403,
            ),
            (
                b"POST /hook HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://evil.example\r\nContent-Length: 2\r\n\r\n{}",
                403,
            ),
            (b"POST /hook HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n", 411),
            (
                b"POST /hook HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 99999999\r\n\r\n",
                413,
            ),
        ];
        for (raw, want) in cases {
            let (status, _, _) = ask(relay, raw).await;
            assert_eq!(status, want, "{}", String::from_utf8_lossy(raw));
        }
    }

    #[tokio::test]
    async fn a_hub_that_cannot_be_reached_is_a_bad_gateway() {
        let relay = relay_to(dialer(dead_port().await)).await;
        let (status, _, _) = ask(relay, &hook_request("127.0.0.1", "", "{}")).await;
        assert_eq!(status, 502);
    }

    #[test]
    fn the_hook_path_keeps_a_hub_s_prefix() {
        let at = |url: &str| Endpoint::parse(url, false).unwrap();
        assert_eq!(at("https://hub.example").hook_path(), "/hook");
        assert_eq!(at("https://hub.example/fleet").hook_path(), "/fleet/hook");
        assert_eq!(
            at("wss://hub.example/fleet/agent").hook_path(),
            "/fleet/hook"
        );
        assert_eq!(at("https://hub.example:443").authority(), "hub.example:443");
    }

    #[test]
    fn loopback_hosts_only() {
        for ok in [
            "127.0.0.1",
            "127.0.0.1:4181",
            "localhost:4181",
            "[::1]:4181",
            "[::1]",
        ] {
            assert!(host_is_loopback(ok.as_bytes()), "{ok}");
        }
        for bad in [
            "evil.example",
            "10.0.0.1:4181",
            "[::2]:1",
            "",
            "127.0.0.1.evil:80",
        ] {
            assert!(!host_is_loopback(bad.as_bytes()), "{bad}");
        }
    }
}
