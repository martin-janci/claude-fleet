//! The e2e fake tracker's transport (work graph M10.2). **Test-only**: the
//! module exists only with the `e2e` cargo feature, which no shipped build
//! enables, and [`crate::service::trackers::e2e_loopback_port`] refuses the
//! override in a build without debug assertions even then.
//!
//! [`LoopbackTransport`] is [`super::https::DirectTransport`] with one thing
//! changed: the connection. The request is still an absolute `https://` URL,
//! still checked against the tracker's host policy (`*.atlassian.net` for
//! Jira Cloud) and still rendered with that host in `Host:`; only the bytes
//! go, in plain HTTP, to `127.0.0.1:<port>`, where `scripts/hub-e2e.sh` runs
//! `scripts/e2e-fake-jira.py`. Nothing else is redirected: a Data Center,
//! Asana, Linear or `via_host` / `via_cli` tracker keeps its own transport
//! and its own fences (the loopback / link-local refusal above all).

use super::conn;
use super::https::{
    parse_response, parse_target, render_request, HostPolicy, HttpTransport, Request, Response,
    TransportError, CONNECT_TIMEOUT, DEFAULT_MAX_BODY,
};

/// See the module docs.
pub struct LoopbackTransport {
    allow_host: HostPolicy,
    port: u16,
}

impl LoopbackTransport {
    pub fn new(allow_host: HostPolicy, port: u16) -> Self {
        LoopbackTransport { allow_host, port }
    }
}

#[async_trait::async_trait]
impl HttpTransport for LoopbackTransport {
    async fn send(&self, req: Request) -> Result<Response, TransportError> {
        let at = parse_target(&req.url)?;
        // The same fences as the real transport, before anything is sent.
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
        let fake = fleet_proto::net::Endpoint::parse(&format!("http://127.0.0.1:{}", self.port))
            .map_err(TransportError::Refused)?;
        let exchange = async {
            let stream = conn::connect(&fake, CONNECT_TIMEOUT)
                .await
                .map_err(TransportError::Connect)?;
            conn::speak(stream, "127.0.0.1", self.port, &bytes, DEFAULT_MAX_BODY)
                .await
                .map_err(TransportError::Connect)
        };
        let raw = tokio::time::timeout(req.timeout, exchange)
            .await
            .map_err(|_| TransportError::Timeout)??;
        parse_response(&raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// One canned answer on a loopback port; returns the port and the
    /// request head it read.
    async fn one_shot(answer: &'static str) -> (u16, tokio::task::JoinHandle<String>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let h = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = s.read(&mut buf).await.unwrap();
            s.write_all(answer.as_bytes()).await.unwrap();
            s.shutdown().await.unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        (port, h)
    }

    fn atlassian() -> HostPolicy {
        Arc::new(crate::store::is_allowed_tracker_host)
    }

    #[tokio::test]
    async fn it_sends_an_allowed_https_request_to_the_loopback_port() {
        let (port, seen) =
            one_shot("HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n{\"ok\":true}").await;
        let t = LoopbackTransport::new(atlassian(), port);
        let r = t
            .send(Request::get("https://e2e.atlassian.net/rest/api/3/myself"))
            .await
            .unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.text(), "{\"ok\":true}");
        let head = seen.await.unwrap();
        assert!(
            head.starts_with("GET /rest/api/3/myself HTTP/1.1\r\n"),
            "{head}"
        );
        assert!(head.contains("Host: e2e.atlassian.net\r\n"), "{head}");
    }

    #[tokio::test]
    async fn it_keeps_the_host_fence_and_refuses_plaintext() {
        // Port 9 (discard): nothing may even be attempted.
        let t = LoopbackTransport::new(atlassian(), 9);
        for url in [
            "https://evil.example.com/rest/api/3/myself",
            "https://169.254.169.254/latest/meta-data",
            "http://e2e.atlassian.net/rest/api/3/myself",
        ] {
            let e = t.send(Request::get(url)).await.unwrap_err();
            assert!(matches!(e, TransportError::Refused(_)), "{url}: {e:?}");
        }
    }
}
