//! [`super::keep_alive`] against a scripted HTTP/1.1 server on loopback: what
//! is reused, what is not, and the one retry a dead kept connection earns.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// What the scripted server does with the n-th request (0-based, across
/// connections).
#[derive(Clone, Copy)]
enum Answer {
    /// `Content-Length` body, connection left open.
    Sized(&'static str),
    /// Chunked body (split into two chunks), connection left open.
    Chunked(&'static str),
    /// `Connection: close`, then close.
    Closing(&'static str),
    /// Sized body, then close without saying so — the server's own idle
    /// timeout, as a kept connection sees it.
    SizedThenDrop(&'static str),
    /// Read the request, answer nothing, close.
    Silent,
    /// Announce fewer bytes than are sent.
    Overlong(&'static str),
}

struct Server {
    port: u16,
    accepted: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
}

async fn server(script: Vec<Answer>) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(AtomicUsize::new(0));
    let (acc, reqs) = (Arc::clone(&accepted), Arc::clone(&requests));
    let script = Arc::new(script);
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            acc.fetch_add(1, Ordering::SeqCst);
            let (reqs, script) = (Arc::clone(&reqs), Arc::clone(&script));
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    // One request: head, then Content-Length bytes.
                    let split = loop {
                        if let Some(i) = http1::find(&buf, b"\r\n\r\n") {
                            break i;
                        }
                        match sock.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    };
                    let head = String::from_utf8_lossy(&buf[..split]).into_owned();
                    let len: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("Content-Length: "))
                        .map_or(0, |v| v.trim().parse().unwrap());
                    while buf.len() < split + 4 + len {
                        match sock.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    buf.drain(..split + 4 + len);
                    let n = reqs.fetch_add(1, Ordering::SeqCst);
                    let answer = script.get(n).copied().unwrap_or(Answer::Silent);
                    let reply = match answer {
                        Answer::Sized(b) | Answer::SizedThenDrop(b) => {
                            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{b}", b.len())
                        }
                        Answer::Chunked(b) => {
                            let (x, y) = b.split_at(b.len() / 2);
                            format!(
                                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{x}\r\n{:x}\r\n{y}\r\n0\r\n\r\n",
                                x.len(),
                                y.len()
                            )
                        }
                        Answer::Closing(b) => format!(
                            "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{b}",
                            b.len()
                        ),
                        Answer::Silent => return,
                        Answer::Overlong(b) => {
                            format!("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{b}")
                        }
                    };
                    if sock.write_all(reply.as_bytes()).await.is_err() {
                        return;
                    }
                    if matches!(answer, Answer::Closing(_) | Answer::SizedThenDrop(_)) {
                        return;
                    }
                }
            });
        }
    });
    Server {
        port,
        accepted,
        requests,
    }
}

async fn call(port: u16) -> Result<HubResponse, String> {
    TcpTransport
        .post_json(&format!("http://127.0.0.1:{port}/mcp"), "t", "{}".into())
        .await
}

fn key(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

#[tokio::test]
async fn sequential_calls_share_one_connection() {
    let s = server(vec![
        Answer::Sized("one"),
        Answer::Chunked("two two"),
        Answer::Sized("three"),
    ])
    .await;
    assert_eq!(call(s.port).await.unwrap().body, "one");
    assert_eq!(
        call(s.port).await.unwrap().body,
        "two two",
        "a chunked body keeps the connection too"
    );
    assert_eq!(call(s.port).await.unwrap().body, "three");
    assert_eq!(
        s.accepted.load(Ordering::SeqCst),
        1,
        "one connection, three answers"
    );
    assert_eq!(keep_alive::idle(&key(s.port)), 1);
}

#[tokio::test]
async fn a_closing_answer_is_not_kept() {
    let s = server(vec![Answer::Closing("bye"), Answer::Sized("again")]).await;
    assert_eq!(call(s.port).await.unwrap().body, "bye");
    assert_eq!(keep_alive::idle(&key(s.port)), 0);
    assert_eq!(call(s.port).await.unwrap().body, "again");
    assert_eq!(s.accepted.load(Ordering::SeqCst), 2);
}

/// The server closed the kept connection while it sat idle: the next call
/// is sent once more on a fresh connection, and the server saw it once.
#[tokio::test]
async fn a_kept_connection_the_server_closed_costs_one_retry() {
    let s = server(vec![
        Answer::SizedThenDrop("first"),
        Answer::Sized("second"),
    ])
    .await;
    assert_eq!(call(s.port).await.unwrap().body, "first");
    // Let the server's close reach this side.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(call(s.port).await.unwrap().body, "second");
    assert_eq!(s.accepted.load(Ordering::SeqCst), 2);
    assert_eq!(
        s.requests.load(Ordering::SeqCst),
        2,
        "nothing was sent twice"
    );
}

/// A FRESH connection that answers nothing is an error, not a retry: the
/// server may have started on the request.
#[tokio::test]
async fn a_fresh_connection_that_answers_nothing_is_not_retried() {
    let s = server(vec![Answer::Silent]).await;
    let err = call(s.port).await.unwrap_err();
    assert!(
        err.contains("closed the connection before answering"),
        "{err}"
    );
    assert_eq!(s.requests.load(Ordering::SeqCst), 1);
    assert_eq!(s.accepted.load(Ordering::SeqCst), 1);
}

/// More bytes than announced: the answer is cut to its length and the
/// connection, whose framing can no longer be trusted, is not kept.
#[tokio::test]
async fn an_overlong_answer_is_not_kept() {
    let s = server(vec![Answer::Overlong("okEXTRA")]).await;
    assert_eq!(call(s.port).await.unwrap().body, "ok");
    assert_eq!(keep_alive::idle(&key(s.port)), 0);
}

/// Against the real `/mcp` server (axum + rmcp, as the hub and the desktop
/// serve it): its answer is framed so the connection can be kept, and the
/// next call goes out on it.
#[tokio::test]
async fn the_real_mcp_endpoint_answers_on_a_kept_connection() {
    let store = Arc::new(std::sync::Mutex::new(
        crate::store::Store::open_in_memory().unwrap(),
    ));
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let (shutdown, task) = crate::mcp::start_with_handle(
        store,
        Arc::new(crate::ssh::SshClient::new()),
        crate::cancel::CancellationRegistry::new(),
        Arc::new(crate::service::tunnel::TunnelSupervisor::new()),
        crate::mcp::McpGuards::new(Arc::new(|_| {})),
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        port,
        "master-token".into(),
        vec![],
        None,
    )
    .await
    .expect("the MCP server starts");
    let list = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
    let url = format!("http://127.0.0.1:{port}/mcp");
    let first = TcpTransport
        .post_json(&url, "master-token", list.into())
        .await
        .unwrap();
    assert_eq!(first.status, 200, "{}", first.body);
    assert!(first.body.contains("send_prompt"), "{}", first.body);
    assert_eq!(
        keep_alive::idle(&key(port)),
        1,
        "the answer left the connection usable"
    );
    let second = TcpTransport
        .post_json(&url, "master-token", list.into())
        .await
        .unwrap();
    assert_eq!(second.status, 200);
    assert!(second.body.contains("send_prompt"));
    assert_eq!(
        keep_alive::idle(&key(port)),
        1,
        "and the next call went out on it"
    );
    shutdown.cancel();
    let _ = task.await;
}
