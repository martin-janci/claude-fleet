//! A bad bearer is logged at warn once per [`AUTH_FAIL_INTERVAL`] per address
//! (hub-ops F3: failed bearers on `/mcp` were one warn line each,
//! unthrottled, and the line named no peer); every one is answered 401, so a
//! client never reads a dead token as a busy hub.

use super::*;
use std::net::Ipv4Addr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn serve_test_app() -> std::net::SocketAddr {
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let app = test_app(store, "s3cret", crate::agent::ws::AgentWsState::disabled());
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    addr
}

/// The status line of `POST /mcp` with `bearer`, optionally behind a proxy
/// that appended `forwarded` as the client's address.
async fn status_of(addr: std::net::SocketAddr, bearer: &str, forwarded: Option<&str>) -> String {
    let mut req = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {bearer}\r\n\
         Accept: application/json, text/event-stream\r\nContent-Type: application/json\r\n\
         Content-Length: 2\r\nConnection: close\r\n"
    );
    if let Some(ip) = forwarded {
        req.push_str(&format!("X-Forwarded-For: {ip}\r\n"));
    }
    req.push_str("\r\n{}");
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        s.read_to_end(&mut buf),
    )
    .await;
    String::from_utf8_lossy(&buf)
        .lines()
        .next()
        .unwrap_or("")
        .to_string()
}

#[tokio::test]
async fn a_repeated_bad_bearer_is_still_401() {
    let addr = serve_test_app().await;
    assert!(
        status_of(addr, "wrong", None).await.contains("401"),
        "the first failure is a plain 401"
    );
    assert!(
        status_of(addr, "wrong", None).await.contains("401"),
        "the repeat inside AUTH_FAIL_INTERVAL too: only its log line is throttled"
    );
    assert!(
        status_of(addr, "s3cret", None).await.contains("200"),
        "a valid bearer is never throttled — successes do not touch the bucket"
    );
    assert!(
        status_of(addr, "wrong", Some("203.0.113.9"))
            .await
            .contains("401"),
        "a different client behind the same loopback proxy has its own bucket"
    );
    tokio::time::sleep(AUTH_FAIL_INTERVAL + std::time::Duration::from_millis(50)).await;
    assert!(
        status_of(addr, "wrong", None).await.contains("401"),
        "the bucket refills after the interval"
    );
}

/// The raw response head to `head` + `body` written as-is to `/mcp`.
async fn raw_status(addr: std::net::SocketAddr, head: &str, body: &[u8]) -> String {
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(head.as_bytes()).await.unwrap();
    // The hub may answer and close before the whole body is written.
    let _ = s.write_all(body).await;
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), s.read_to_end(&mut buf)).await;
    String::from_utf8_lossy(&buf)
        .lines()
        .next()
        .unwrap_or("")
        .to_string()
}

/// `/mcp` refuses a body over `MCP_BODY_MAX` with 413, whether it declares
/// its length or streams it chunked, and still serves one under the cap.
#[tokio::test]
async fn an_oversized_mcp_body_is_413() {
    let addr = serve_test_app().await;
    let head = |len: usize| {
        format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer s3cret\r\n\
             Content-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
        )
    };
    let declared = raw_status(addr, &head(MCP_BODY_MAX + 1), b"").await;
    assert!(
        declared.contains("413"),
        "declared length over the cap: {declared}"
    );

    let chunk = vec![b'x'; 1024 * 1024];
    let mut chunked = Vec::new();
    for _ in 0..(MCP_BODY_MAX / chunk.len() + 1) {
        chunked.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
        chunked.extend_from_slice(&chunk);
        chunked.extend_from_slice(b"\r\n");
    }
    chunked.extend_from_slice(b"0\r\n\r\n");
    let streamed = raw_status(
        addr,
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer s3cret\r\n\
         Content-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        &chunked,
    )
    .await;
    assert!(
        streamed.contains("413"),
        "chunked body over the cap: {streamed}"
    );

    let ok = raw_status(addr, &head(2), b"{}").await;
    assert!(
        ok.contains("200"),
        "a body under the cap still reaches /mcp: {ok}"
    );
}
