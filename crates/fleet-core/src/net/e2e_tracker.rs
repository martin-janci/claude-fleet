//! The end-to-end fake tracker override (work graph M10.2).
//!
//! `scripts/hub-e2e.sh` drives a real `fleet-hub` against a tiny loopback
//! HTTP server that answers in Jira Cloud's shapes (`scripts/e2e/fake_jira.py`).
//! A tracker keeps its real site URL (`https://acme.atlassian.net`, which the
//! site checks accept); only the transport changes: every tracker request is
//! sent, in plain HTTP, to the loopback address in [`ENV`], with the site it
//! was meant for in [`SITE_HEADER`] so one fake can play several trackers.
//!
//! **Never in a release build.** The transport is compiled for tests and
//! under the `e2e` cargo feature only, and the process honours [`ENV`] only
//! under the feature, which itself refuses to compile without
//! `debug_assertions` (below). A build without the feature refuses to start
//! a hub that has [`ENV`] set ([`crate::service::trackers::TrackerNet::from_env`]),
//! so an operator can never believe a fake is in use when it is not, or the
//! other way round.

#[cfg(all(feature = "e2e", not(debug_assertions)))]
compile_error!(
    "the `e2e` feature is for debug builds only: it redirects tracker traffic to loopback"
);

use super::https::{
    parse_response, parse_target, render_request, HttpTransport, Request, Response, TransportError,
};

pub use super::E2E_TRACKER_ENV as ENV;
/// The site a request was meant for (`acme.atlassian.net`).
pub const SITE_HEADER: &str = "X-Fleet-E2E-Site";
/// Largest answer read from the fake.
const MAX_BODY: u64 = 16 * 1024 * 1024;

/// Sends every request to one loopback HTTP server.
#[derive(Debug, Clone)]
pub struct LoopbackTransport {
    endpoint: fleet_proto::net::Endpoint,
}

impl LoopbackTransport {
    /// `base` must be `http://` on a loopback address, with nothing after the
    /// authority: this is a test double, never a way to reach a real host.
    pub fn new(base: &str) -> Result<Self, String> {
        let endpoint = fleet_proto::net::Endpoint::parse(base.trim_end_matches('/'))
            .map_err(|e| format!("{ENV}: {e}"))?;
        if endpoint.is_tls() {
            return Err(format!(
                "{ENV} must be plain http:// (it is a loopback fake)"
            ));
        }
        if !endpoint.is_loopback() {
            return Err(format!(
                "{ENV} must name a loopback address, not {}",
                endpoint.host()
            ));
        }
        if !endpoint.path().trim_matches('/').is_empty() {
            return Err(format!("{ENV} takes no path"));
        }
        Ok(LoopbackTransport { endpoint })
    }
}

#[async_trait::async_trait]
impl HttpTransport for LoopbackTransport {
    async fn send(&self, req: Request) -> Result<Response, TransportError> {
        // The original URL still has to be one the adapter could send: parse
        // it the same way the real transport does.
        let original = parse_target(&req.url)?;
        let site = original.endpoint.authority().to_string();
        let url = format!("http://{}{}", self.endpoint.authority(), original.target);
        let req = Request { url, ..req }.header(SITE_HEADER, site);
        let at = parse_target(&req.url)?;
        let bytes = render_request(&req, &at)?;
        let host = at.endpoint.host().to_string();
        let port = at.endpoint.port();
        let exchange = async {
            let stream = super::conn::connect(&at.endpoint, req.timeout)
                .await
                .map_err(TransportError::Connect)?;
            super::conn::speak(stream, &host, port, &bytes, MAX_BODY)
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn only_a_plain_loopback_base_is_accepted() {
        assert!(LoopbackTransport::new("http://127.0.0.1:8080").is_ok());
        assert!(LoopbackTransport::new("http://localhost:8080/").is_ok());
        for bad in [
            "https://127.0.0.1:8080",
            "http://example.com:8080",
            "http://10.0.0.1:8080",
            "http://127.0.0.1:8080/rest",
            "not a url",
        ] {
            assert!(LoopbackTransport::new(bad).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn a_request_keeps_its_path_and_names_its_site() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let mut got = Vec::new();
            loop {
                let n = s.read(&mut buf).await.unwrap();
                got.extend_from_slice(&buf[..n]);
                if n == 0 || got.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let body = br#"{"ok":true}"#;
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            s.write_all(head.as_bytes()).await.unwrap();
            s.write_all(body).await.unwrap();
            String::from_utf8_lossy(&got).into_owned()
        });
        let t = LoopbackTransport::new(&format!("http://127.0.0.1:{port}")).unwrap();
        let resp = t
            .send(Request::get(
                "https://acme.atlassian.net/rest/api/3/myself?expand=groups",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.text(), r#"{"ok":true}"#);
        let seen = server.await.unwrap();
        assert!(
            seen.starts_with("GET /rest/api/3/myself?expand=groups HTTP/1.1\r\n"),
            "{seen}"
        );
        assert!(
            seen.contains("X-Fleet-E2E-Site: acme.atlassian.net\r\n"),
            "{seen}"
        );
    }
}
