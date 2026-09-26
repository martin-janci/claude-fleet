//! Tests for [`super`] (`http_client`): the transport layer — `Endpoint`
//! parsing, the real TCP/TLS connect, the one-shot HTTP exchange and
//! `split_response` — against real sockets and hand-built response bytes. No
//! `HubBackend`/`Fake` here: that layer's own tests stay with the desktop
//! (`src-tauri/src/backend/tests_remote.rs`), which exercises this module
//! through `HubBackend`'s wiring.
//!
//! Moved here unchanged from `src-tauri/src/backend/tests_remote.rs` when the
//! HTTP client itself moved into `fleet-core` (federation, cycle 3: the hub
//! dials another hub with it).

use super::*;

/// `tokio::test` needs a runtime; these calls never actually await I/O.
fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

/// The prefixes [`is_connect_failure`] matches are not guessed: they are the
/// ones [`connect`] itself produces. Loopback port 1 refuses (or, on a box
/// that filters it, times out) — both arms are `connect …`, so this pins the
/// spelling either way, and it opens no outward socket.
#[tokio::test]
async fn a_real_failed_connect_is_recognised_as_a_connect_failure() {
    let at = Endpoint::parse("http://127.0.0.1:1/mcp").expect("parses");
    let reason = connect(&at).await.err().expect("nothing listens on port 1");
    assert!(
        is_connect_failure(&reason),
        "connect() said {reason:?}, which the breaker would not recognise"
    );
}

// --- the raw HTTP transport --------------------------------------------------

/// A hub address is http or https and nothing else. Anything else must be
/// refused by name rather than producing a confusing connect error.
/// (`normalise_base_url` already rejects these when the setting is read; this
/// is the transport's own guard.)
#[test]
fn the_tcp_transport_refuses_a_scheme_that_is_not_a_hub_address() {
    for url in ["ftp://fleet.example.com/mcp", "file:///etc/passwd"] {
        let e = match block_on(TcpTransport.post_json(url, "t", "{}".into())) {
            Err(e) => e,
            Ok(r) => panic!("{url} should have been refused, got {r:?}"),
        };
        assert!(e.contains("not a hub address"), "for {url}: {e}");
    }
}

/// The platform trust store must actually yield roots on this machine —
/// otherwise every `https://` hub fails with an opaque certificate error and
/// nobody knows why. `TcpTransport` fails closed and says so; this asserts
/// that the happy path really is happy here.
#[test]
fn the_platform_trust_store_yields_roots() {
    let found = rustls_native_certs::load_native_certs();
    assert!(
        !found.certs.is_empty(),
        "no roots loaded; errors: {:?}",
        found.errors
    );
}

/// Proof that the TLS path actually completes a handshake against a real
/// server and reads a real response — the closed-port test above only shows
/// that it fails correctly. Ignored by default because it needs the network;
/// run with `cargo test -p fleet-core --lib -- --ignored tls_really`.
///
/// It is deliberately NOT pointed at a hub: any https server proves the
/// handshake, the platform roots and the read loop. The status will be a 4xx
/// (the host is not an MCP endpoint), which is exactly what we assert — a
/// parsed status means the whole path worked.
#[test]
#[ignore = "needs network"]
fn tls_really_completes_a_handshake_against_a_real_server() {
    let r = block_on(TcpTransport.post_json("https://crates.io/", "t", "{}".into()))
        .expect("a TLS exchange");
    assert!(
        r.status >= 200,
        "expected a parsed HTTP status, got {}",
        r.status
    );
}

/// An https hub that is not listening must come back as an ordinary transport
/// error — which `HubBackend` maps to `E_HUB_UNREACHABLE` — not a panic and
/// not a hang. Port 1 on loopback is closed everywhere.
#[test]
fn an_https_hub_that_is_not_listening_fails_as_a_transport_error() {
    let e = match block_on(TcpTransport.post_json("https://127.0.0.1:1/mcp", "t", "{}".into())) {
        Err(e) => e,
        Ok(r) => panic!("a closed port should have failed, got {r:?}"),
    };
    assert!(e.contains("connect 127.0.0.1:1"), "{e}");
}

#[test]
fn a_raw_http_response_is_split_into_its_status_and_body() {
    let raw = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n\
               event: message\ndata: {\"a\":1}\n\n";
    let r = split_response(raw).expect("a response");
    assert_eq!(r.status, 200);
    assert!(r.body.starts_with("event: message"));

    // The status token, not a substring: a reason phrase carrying " 200"
    // must not read as success.
    let r = split_response("HTTP/1.1 500 Internal Error 200 OK\r\n\r\nboom").unwrap();
    assert_eq!(r.status, 500);

    assert!(split_response("").is_err());
}

// --- a peer that half-closes -------------------------------------------------
//
// rustls 0.23 reports a TCP close with no `close_notify` as
// `UnexpectedEof`, and `speak` used to `?` that — discarding a body that had
// already arrived. Nothing exercised it, because the one live test points at
// crates.io, which does send `close_notify`.

/// Hands back `body` (in whatever chunks the reader's buffer allows), then
/// fails with `kind` instead of reporting a clean end of stream — which is
/// what a peer that drops the connection without `close_notify` looks like.
struct HalfClosing {
    body: Vec<u8>,
    kind: std::io::ErrorKind,
}

impl tokio::io::AsyncRead for HalfClosing {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if !self.body.is_empty() {
            // Never more than the buffer has room for; `put_slice` panics
            // rather than truncating, and `read_to_end` grows its buffer
            // between calls.
            let n = self.body.len().min(buf.remaining());
            let rest = self.body.split_off(n);
            buf.put_slice(&self.body);
            self.body = rest;
            return std::task::Poll::Ready(Ok(()));
        }
        // Deliberately an error rather than `Ok(())` with nothing written:
        // zero bytes IS a clean EOF, which is the case this test is not about.
        std::task::Poll::Ready(Err(std::io::Error::new(self.kind, "peer went away")))
    }
}

impl tokio::io::AsyncWrite for HalfClosing {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::task::Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

fn half_closing(body: &str, kind: std::io::ErrorKind) -> HalfClosing {
    HalfClosing {
        body: body.as_bytes().to_vec(),
        kind,
    }
}

const ONE_RESPONSE: &str = "HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n\
                            event: message\ndata: {\"a\":1}\n\n";

/// A hub or proxy that closes the TCP connection without `close_notify` has
/// still answered. Throwing the answer away turns a working hub into
/// `E_HUB_UNREACHABLE` for no reason, and the plain-HTTP path has no
/// equivalent failure — so this was a regression the TLS work introduced.
#[test]
fn a_peer_that_half_closes_still_yields_the_response_it_already_sent() {
    let raw = block_on(speak(
        half_closing(ONE_RESPONSE, std::io::ErrorKind::UnexpectedEof),
        "hub.example.com",
        443,
        "GET / HTTP/1.1\r\n\r\n",
    ))
    .expect("a complete response must survive a missing close_notify");
    assert_eq!(raw, ONE_RESPONSE.as_bytes());
    // And it still parses, which is the thing the caller actually needs.
    assert_eq!(split_response(&raw).expect("a response").status, 200);
}

/// The other half: an `UnexpectedEof` with nothing read is a real failure and
/// must stay one. Otherwise a hub that never answered would surface as an
/// empty body and a confusing parse error instead of "it did not answer".
#[test]
fn an_unexpected_eof_with_nothing_read_is_still_a_failure() {
    let e = block_on(speak(
        half_closing("", std::io::ErrorKind::UnexpectedEof),
        "hub.example.com",
        443,
        "GET / HTTP/1.1\r\n\r\n",
    ))
    .expect_err("nothing arrived, so this is not a response");
    assert!(e.contains("read from hub.example.com:443"), "{e}");
}

/// And only `UnexpectedEof` is forgiven. A connection reset mid-body means
/// the bytes in hand are a *truncated* response, which must not be parsed as
/// if it were whole.
#[test]
fn a_real_read_error_is_not_swallowed_even_with_bytes_in_hand() {
    let e = block_on(speak(
        half_closing(ONE_RESPONSE, std::io::ErrorKind::ConnectionReset),
        "hub.example.com",
        443,
        "GET / HTTP/1.1\r\n\r\n",
    ))
    .expect_err("a reset is not a clean end of response");
    assert!(e.contains("read from hub.example.com:443"), "{e}");
}

// --- the endpoint ------------------------------------------------------------

/// One parse for every request this app makes, so a fix lands once.
#[test]
fn an_endpoint_splits_a_hub_url_into_what_a_hand_written_request_needs() {
    let at = Endpoint::parse("https://fleet.example.com/mcp").expect("a hub URL");
    assert_eq!(at.host(), "fleet.example.com");
    assert_eq!(at.port(), 443, "https defaults to 443");
    assert!(at.is_tls());
    assert_eq!(
        at.authority(),
        "fleet.example.com",
        "no port in the Host header when it is the scheme's default — the \
         hub's allowlist is matched against exactly this string"
    );
    assert_eq!(at.target(), "/mcp");

    let at = Endpoint::parse("http://hub.example.com:4180/fleet/events").expect("a hub URL");
    assert_eq!(at.port(), 4180);
    assert!(!at.is_tls());
    assert_eq!(
        at.authority(),
        "hub.example.com:4180",
        "a non-default port is"
    );
    assert_eq!(at.target(), "/fleet/events");

    assert!(Endpoint::parse("ftp://hub.example.com").is_err());
    assert!(Endpoint::parse("not a url").is_err());
}

/// The other half of `fleet_proto::net::Endpoint`'s two authority forms.
/// This app has never put a scheme-default port in its `Host` header — the
/// `url` crate it parsed with dropped one — and a hub's `allowed_hosts` may
/// be spelled without it, so it must keep not doing so. (`fleet-agent` keeps
/// the port it was given; see `authority_as_written` there.)
#[test]
fn a_scheme_default_port_stays_out_of_the_host_header() {
    for (url, authority) in [
        ("https://fleet.example.com:443/mcp", "fleet.example.com"),
        ("http://fleet.example.com:80/mcp", "fleet.example.com"),
        ("https://[::1]:443/mcp", "[::1]"),
        // A non-default port is part of the header, as before.
        (
            "https://fleet.example.com:8443/mcp",
            "fleet.example.com:8443",
        ),
    ] {
        let at = Endpoint::parse(url).unwrap_or_else(|e| panic!("{url}: {e}"));
        assert_eq!(at.authority(), authority, "{url}");
    }
}

/// An IPv6-literal hub was simply unreachable: `host_str()` keeps the URL's
/// brackets, and `[::1]` neither resolves nor parses as a certificate name,
/// so `https://[::1]:8787` failed before a single byte went out. The `Host`
/// header is the one place the brackets belong.
#[test]
fn an_ipv6_literal_hub_connects_and_still_sends_a_bracketed_host_header() {
    let at = Endpoint::parse("https://[2001:db8::1]:8787/mcp").expect("an IPv6 hub URL");
    assert_eq!(
        at.host(),
        "2001:db8::1",
        "the connect and SNI name must be unbracketed"
    );
    assert_eq!(at.port(), 8787);
    assert_eq!(
        at.authority(),
        "[2001:db8::1]:8787",
        "the Host header keeps the brackets"
    );
    // And the unbracketed form really is what rustls accepts as a name.
    assert!(
        tokio_rustls::rustls::pki_types::ServerName::try_from(at.host().to_string()).is_ok(),
        "an IP literal is matched against an IP SAN"
    );
    assert!(
        tokio_rustls::rustls::pki_types::ServerName::try_from("[2001:db8::1]".to_string()).is_err(),
        "the bracketed form is what used to be passed, and it is rejected"
    );
    // Loopback too, since that is the tunnelled setup docs/hub.md describes.
    let at = Endpoint::parse("http://[::1]:8787/events").expect("a loopback IPv6 hub URL");
    assert_eq!(at.host(), "::1");
    assert_eq!(at.authority(), "[::1]:8787");
}

#[test]
fn an_endpoint_knows_whether_it_is_loopback() {
    for ok in [
        "http://127.0.0.1:7777",
        "http://localhost",
        "http://[::1]:9",
    ] {
        assert!(Endpoint::parse(ok).unwrap().is_loopback(), "{ok}");
    }
    for far in [
        "http://10.0.0.5",
        "https://hub.example",
        "http://127.0.0.1.example",
    ] {
        assert!(!Endpoint::parse(far).unwrap().is_loopback(), "{far}");
    }
}

// --- chunked framing ---------------------------------------------------------

#[test]
fn a_chunked_body_is_rejoined() {
    let raw = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
               5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
    assert_eq!(split_response(raw).expect("a response").body, "hello world");
}

/// The shortcut this replaces worked only because a chunk-size line is not a
/// `data:` line and rmcp happened to write one frame per body frame. Split a
/// frame across two chunks and the size line lands INSIDE a `data:` line.
#[test]
fn a_chunk_boundary_inside_a_data_line_no_longer_corrupts_the_payload() {
    let payload = r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"[]"}]}}"#;
    let frame = format!("event: message\ndata: {payload}\n\n");
    let cut = frame.len() / 2;
    let (a, b) = frame.split_at(cut);
    let raw = format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
         {:x}\r\n{a}\r\n{:x}\r\n{b}\r\n0\r\n\r\n",
        a.len(),
        b.len()
    );
    let body = split_response(&raw).expect("a response").body;
    assert_eq!(body, frame, "the two chunks are rejoined byte for byte");
    assert_eq!(
        crate::mcp::wire::last_event_payload(&body),
        payload,
        "and the envelope reads back whole"
    );
}

#[test]
fn a_body_that_did_not_declare_chunked_is_left_alone() {
    let raw = "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n5\r\nx";
    assert_eq!(
        split_response(raw).expect("a response").body,
        "5\r\nx",
        "what looks like a chunk header is body when nothing declared chunking"
    );
}

/// SF-6. The whole-body path decoded UTF-8 BEFORE de-chunking: `speak` ran
/// `from_utf8_lossy` over the raw bytes, chunk framing and all, so a
/// character split by a chunk boundary became two replacement characters,
/// the chunk's byte count stopped matching, and the call died blaming the hub
/// ("the body is not the UTF-8 it claimed to be") for a client-side ordering
/// bug. Driven through `speak`, the real caller, because the old test fed
/// `dechunk` a hand-built `&str` that `speak` could never produce.
#[test]
fn a_character_split_across_two_chunks_survives_the_whole_body_path() {
    let text = "event: message\ndata: {\"friendly_name\":\"Zürich\"}\n\n";
    let bytes = text.as_bytes();
    // Cut between the two bytes of "ü".
    let cut = text.find('ü').expect("the fixture has one") + 1;
    let (a, b) = bytes.split_at(cut);
    let mut raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    for chunk in [a, b] {
        raw.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
        raw.extend_from_slice(chunk);
        raw.extend_from_slice(b"\r\n");
    }
    raw.extend_from_slice(b"0\r\n\r\n");

    let got = block_on(speak(
        HalfClosing {
            body: raw,
            kind: std::io::ErrorKind::UnexpectedEof,
        },
        "hub.example.com",
        443,
        "POST /mcp HTTP/1.1\r\n\r\n",
    ))
    .expect("the bytes arrived");
    let r = split_response(&got).expect("a split character is not a malformed body");
    assert_eq!(r.body, text, "de-chunk the bytes, THEN decode them");
}

/// The de-chunking itself (partial-chunk tolerance, a malformed size line, a
/// chunk splitting a character) is `http1`'s and tested there
/// (`tests_http1.rs`); this is `split_response`'s own wiring of it — decode
/// lossily rather than panic on bytes that are not valid UTF-8 at all.
#[test]
fn invalid_utf8_in_a_chunked_response_decodes_lossily_via_split_response() {
    let r = split_response(
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\n\xFF\r\n0\r\n\r\n",
    )
    .expect("a response");
    assert_eq!(r.body, "\u{FFFD}");
}

/// The other prefix [`is_connect_failure`] matches. The refused-port test
/// above pins `connect …`; this pins `TLS handshake with …` against the real
/// [`connect`], the same way: a loopback listener that accepts and hangs up
/// is a peer that was REACHED over TCP but never completed a handshake — the
/// breaker must count it as unreachable (no `/mcp` socket ever came up), and
/// the reason must not come back as the plain `connect ` arm either. Opens no
/// outward socket; the trust store must load for the handshake to start at
/// all, which `the_platform_trust_store_yields_roots` asserts separately.
#[tokio::test]
async fn a_real_failed_tls_handshake_is_recognised_as_a_connect_failure() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback listener");
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        // Accept, then drop: the client's ClientHello meets EOF.
        let (sock, _) = listener.accept().await.expect("the client connects");
        drop(sock);
    });
    let at = Endpoint::parse(&format!("https://127.0.0.1:{port}/mcp")).expect("parses");
    let reason = connect(&at)
        .await
        .err()
        .expect("a peer that hangs up cannot complete a handshake");
    server.await.unwrap();
    assert!(
        reason.starts_with(&format!("TLS handshake with 127.0.0.1:{port} failed: ")),
        "connect() said {reason:?}, which is not the TLS-handshake spelling"
    );
    assert!(
        is_connect_failure(&reason),
        "connect() said {reason:?}, which the breaker would not recognise"
    );
}

// --- the trust store, loaded off the runtime and never cached as a failure ---

// The two trust-store source assertions (`the_trust_store_is_never_loaded_on_a_runtime_worker`,
// `a_failed_trust_store_read_is_not_remembered`) live with the code in
// `crate::net::tls` (work graph M3.0). They inspect `tls.rs` only; the one
// below is the half of the old desktop assertion that looked at the
// TRANSPORT file.

/// Before the move, `the_trust_store_is_never_loaded_on_a_runtime_worker`
/// read `remote.rs` — the transport — and required that the trust store was
/// read in one place and the connector built only behind `spawn_blocking`.
/// The transport now delegates to [`crate::net::tls`] through
/// [`crate::net::conn::connect`], and `tls.rs`'s own test guards that file;
/// what nothing guarded any more is the transport growing a second reader or
/// a second cache of its own — the regression the desktop test existed for,
/// which `tls.rs` cannot see from where it sits. A source assertion, for the
/// same reason as the original: the defect is a *thread*, not observable
/// in-process.
#[test]
fn the_transport_reaches_the_trust_store_only_through_net_tls() {
    for (name, src) in [
        ("mod.rs", include_str!("mod.rs")),
        ("http1.rs", include_str!("http1.rs")),
    ] {
        let code: Vec<&str> = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect();
        for forbidden in [
            "load_native_certs",
            "build_tls_connector",
            "tls_connector(",
            "TlsConnector",
            "RootCertStore",
            "ClientConfig",
            "OnceLock",
            "spawn_blocking",
        ] {
            let hits: Vec<&&str> = code.iter().filter(|l| l.contains(forbidden)).collect();
            assert!(
                hits.is_empty(),
                "http_client/{name} must not touch the trust store or a TLS connector \
                 itself (`{forbidden}`); that is `crate::net::tls`'s, reached through \
                 `crate::net::conn::connect`: {hits:#?}"
            );
        }
    }
    // And the one socket opener really is the shared one.
    let src = include_str!("mod.rs");
    let body = src
        .split("pub async fn connect(at: &Endpoint)")
        .nth(1)
        .expect("`connect` is defined in mod.rs");
    let body = &body[..body.find("\n}\n").expect("`connect` has a body")];
    assert!(
        body.contains("crate::net::conn::connect(&at.at, CONNECT_TIMEOUT)"),
        "`connect` must delegate to `crate::net::conn::connect`, whose TLS arm goes \
         through `net::tls::tls_connector` (blocking pool, no cached failure): {body}"
    );
}
