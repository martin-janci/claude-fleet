//! The hub's [`Fetch`] for the release channel: HTTPS through
//! [`DirectTransport`], to GitHub's hosts only, following the redirects a
//! release download answers with (to the asset CDN) — and nothing else.
//! What arrives is untrusted either way; the signature decides.

use std::sync::Arc;

use async_trait::async_trait;
use fleet_update::{Fetch, UpdateError};

use crate::net::https::{DirectTransport, HttpTransport, Request, TransportError};

/// The hosts a channel document, a manifest or its redirect may live on.
pub const FETCH_HOSTS: &[&str] = &[
    "raw.githubusercontent.com",
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];
const MAX_REDIRECTS: usize = 3;
/// See [`HttpsFetch::new`]: the e2e fake GitHub's port (`e2e` debug builds only).
#[cfg(all(feature = "e2e", debug_assertions))]
pub const E2E_UPDATE_PORT_ENV: &str = "FLEET_E2E_UPDATE_PORT";

pub struct HttpsFetch {
    transport: Arc<dyn HttpTransport>,
}

impl HttpsFetch {
    /// GitHub's hosts, plus the host of `extra_base` when one is configured
    /// (`FLEET_UPDATE_CHANNEL_URL`, a mirror).
    pub fn new(extra_base: Option<&str>) -> Self {
        let extra = extra_base.and_then(host_of).map(str::to_string);
        let policy: crate::net::https::HostPolicy =
            Arc::new(move |h: &str| FETCH_HOSTS.contains(&h) || extra.as_deref() == Some(h));
        // hub-e2e section U (test-only): the same https:// URLs and host fence,
        // the bytes to a fake GitHub on 127.0.0.1:$FLEET_E2E_UPDATE_PORT. Only
        // an `e2e` debug build reads the variable; no shipped build has it.
        #[cfg(all(feature = "e2e", debug_assertions))]
        if let Some(port) = std::env::var(E2E_UPDATE_PORT_ENV)
            .ok()
            .and_then(|p| p.trim().parse::<u16>().ok())
            .filter(|p| *p != 0)
        {
            return HttpsFetch {
                transport: Arc::new(crate::net::e2e_loopback::LoopbackTransport::new(
                    policy, port,
                )),
            };
        }
        HttpsFetch {
            transport: Arc::new(DirectTransport::new(policy)),
        }
    }

    /// Over any transport (tests).
    pub fn with_transport(transport: Arc<dyn HttpTransport>) -> Self {
        HttpsFetch { transport }
    }
}

fn host_of(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    Some(host.split(':').next().unwrap_or(host)).filter(|h| !h.is_empty())
}

#[async_trait]
impl Fetch for HttpsFetch {
    async fn get(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, UpdateError> {
        let mut url = url.to_string();
        for _ in 0..=MAX_REDIRECTS {
            let req = Request::get(url.clone()).header("Accept", "*/*");
            let resp = self.transport.send(req).await.map_err(|e| match e {
                TransportError::Timeout => UpdateError::Transport(format!("{url}: timed out")),
                other => UpdateError::Transport(format!("{url}: {other}")),
            })?;
            match resp.status {
                200 => {
                    if resp.body.len() as u64 > max_bytes {
                        return Err(UpdateError::TooLarge);
                    }
                    return Ok(resp.body);
                }
                301 | 302 | 303 | 307 | 308 => {
                    let next = resp
                        .headers
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case("location"))
                        .map(|(_, v)| v.clone())
                        .ok_or_else(|| {
                            UpdateError::Protocol(format!("{url}: redirect without Location"))
                        })?;
                    if !next.starts_with("https://") {
                        return Err(UpdateError::Protocol(format!(
                            "{url}: redirect to a non-https URL"
                        )));
                    }
                    url = next;
                }
                status => {
                    return Err(UpdateError::Http {
                        status,
                        body: String::from_utf8_lossy(&resp.body)
                            .chars()
                            .take(200)
                            .collect(),
                    })
                }
            }
        }
        Err(UpdateError::Protocol(format!("{url}: too many redirects")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::https::Response;
    use std::sync::Mutex;

    struct Scripted(Mutex<Vec<(String, Response)>>);

    #[async_trait]
    impl HttpTransport for Scripted {
        async fn send(&self, req: Request) -> Result<Response, TransportError> {
            let mut v = self.0.lock().unwrap();
            let (want, resp) = v.remove(0);
            assert_eq!(req.url, want);
            Ok(resp)
        }
    }

    fn resp(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Response {
        Response {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: body.to_vec(),
        }
    }

    #[tokio::test]
    async fn follows_a_release_redirect() {
        let t = Scripted(Mutex::new(vec![
            (
                "https://github.com/o/r/releases/download/v1/m.json".into(),
                resp(
                    302,
                    &[("Location", "https://objects.githubusercontent.com/x")],
                    b"",
                ),
            ),
            (
                "https://objects.githubusercontent.com/x".into(),
                resp(200, &[], b"{}"),
            ),
        ]));
        let f = HttpsFetch::with_transport(Arc::new(t));
        assert_eq!(
            f.get("https://github.com/o/r/releases/download/v1/m.json", 10)
                .await
                .unwrap(),
            b"{}"
        );
    }

    #[tokio::test]
    async fn refuses_http_errors_plaintext_redirects_and_big_bodies() {
        let f = |v: Vec<(String, Response)>| {
            HttpsFetch::with_transport(Arc::new(Scripted(Mutex::new(v))))
        };
        let u = "https://raw.githubusercontent.com/a";
        assert!(matches!(
            f(vec![(u.into(), resp(404, &[], b"nope"))])
                .get(u, 10)
                .await,
            Err(UpdateError::Http { status: 404, .. })
        ));
        assert!(matches!(
            f(vec![(
                u.into(),
                resp(302, &[("location", "http://evil/")], b"")
            )])
            .get(u, 10)
            .await,
            Err(UpdateError::Protocol(_))
        ));
        assert_eq!(
            f(vec![(u.into(), resp(200, &[], b"0123456789ab"))])
                .get(u, 10)
                .await,
            Err(UpdateError::TooLarge)
        );
    }

    #[test]
    fn host_of_urls() {
        assert_eq!(
            host_of("https://mirror.example:8443/c/"),
            Some("mirror.example")
        );
        assert_eq!(host_of("https://u@h.example/x"), Some("h.example"));
        assert_eq!(host_of("http://h.example/"), None);
    }
}
