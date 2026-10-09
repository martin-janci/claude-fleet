//! The two ways the updater learns what to run (design §7 "Selection"):
//! the hub, with its `updater` token ([`HubHttp`] under
//! `fleet_update::HubUpdateChannel`), or — `--standalone`, a Docker host with
//! no hub above it — the published channel itself ([`GitFetch`] under
//! `fleet_update::GitUpdateChannel`). Both end in `verify_target`.

use std::time::Duration;

use async_trait::async_trait;
use fleet_update::{Fetch, HubTransport, UpdateError};

use crate::http::{Client, Url};

/// `POST <hub>/update/*` with the updater's bearer token.
pub struct HubHttp {
    base: String,
    token: String,
    client: Client,
    client_header: String,
}

/// The `Host:` the hub will accept. A public hub refuses any name outside its
/// allowlist (the DNS-rebinding guard) — and `fleet-hub:4180`, the name it
/// has inside the compose network, is not on it; its public URL's host
/// always is. So: `FLEET_UPDATER_HUB_HOST`, else the host of
/// `FLEET_HUB_PUBLIC_URL` (the compose service reads `fleet-hub.env` too),
/// else the URL's own authority.
pub fn hub_host_header(explicit: Option<&str>, public_url: Option<&str>) -> Option<String> {
    if let Some(h) = explicit.map(str::trim).filter(|h| !h.is_empty()) {
        return Some(h.to_string());
    }
    let u = Url::parse(public_url?.trim()).ok()?;
    Some(u.authority())
}

impl HubHttp {
    pub fn new(base: &str, token: &str, host_header: Option<String>) -> Result<HubHttp, String> {
        let url = Url::parse(base)?;
        if token.trim().is_empty() {
            return Err("no updater token (fleet-hub pair --mode updater)".into());
        }
        Ok(HubHttp {
            base: base.trim_end_matches('/').to_string(),
            token: token.trim().to_string(),
            client: Client::new(url.tls, Duration::from_secs(30))?.with_host_header(host_header),
            client_header: format!(
                "fleet-updater/{} (linux-{})",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::ARCH
            ),
        })
    }
}

#[async_trait]
impl HubTransport for HubHttp {
    async fn post(&self, path: &str, body: Vec<u8>) -> Result<Vec<u8>, UpdateError> {
        let auth = format!("Bearer {}", self.token);
        let r = self
            .client
            .send(
                "POST",
                &format!("{}{path}", self.base),
                &[
                    ("Authorization", &auth),
                    ("Content-Type", "application/json"),
                    ("Accept", "application/json"),
                    ("User-Agent", &self.client_header),
                ],
                Some(&body),
                4 << 20,
            )
            .await
            .map_err(UpdateError::Transport)?;
        if !r.ok() {
            return Err(UpdateError::Http {
                status: r.status,
                body: r.text().chars().take(500).collect(),
            });
        }
        Ok(r.body)
    }
}

/// The hosts a channel document, a manifest or its redirect may live on
/// (as `fleet_core::service::update::fetch::FETCH_HOSTS`).
pub const FETCH_HOSTS: &[&str] = &[
    "raw.githubusercontent.com",
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];
const MAX_REDIRECTS: usize = 3;

/// HTTPS GET to GitHub's hosts (plus a configured mirror), following the
/// redirect a release download answers with. What arrives is untrusted;
/// the signature decides.
pub struct GitFetch {
    client: Client,
    extra_host: Option<String>,
}

impl GitFetch {
    pub fn new(mirror_base: Option<&str>) -> Result<GitFetch, String> {
        Ok(GitFetch {
            client: Client::new(true, Duration::from_secs(60))?,
            extra_host: mirror_base.map(Url::parse).transpose()?.map(|u| u.host),
        })
    }

    fn allowed(&self, url: &str) -> Result<(), String> {
        let u = Url::parse(url)?;
        if !u.tls {
            return Err(format!("{url}: https only"));
        }
        if FETCH_HOSTS.contains(&u.host.as_str())
            || self.extra_host.as_deref() == Some(u.host.as_str())
        {
            Ok(())
        } else {
            Err(format!("{url}: {} is not a release host", u.host))
        }
    }
}

#[async_trait]
impl Fetch for GitFetch {
    async fn get(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, UpdateError> {
        let mut url = url.to_string();
        for _ in 0..=MAX_REDIRECTS {
            self.allowed(&url).map_err(UpdateError::Transport)?;
            let r = self
                .client
                .send(
                    "GET",
                    &url,
                    &[("Accept", "*/*"), ("User-Agent", "fleet-updater")],
                    None,
                    max_bytes + (64 << 10),
                )
                .await
                .map_err(UpdateError::Transport)?;
            match r.status {
                200 => {
                    if r.body.len() as u64 > max_bytes {
                        return Err(UpdateError::TooLarge);
                    }
                    return Ok(r.body);
                }
                301 | 302 | 303 | 307 | 308 => {
                    url = r
                        .header("location")
                        .ok_or_else(|| {
                            UpdateError::Transport(format!("{url}: redirect without Location"))
                        })?
                        .to_string();
                }
                status => {
                    return Err(UpdateError::Http {
                        status,
                        body: r.text().chars().take(300).collect(),
                    })
                }
            }
        }
        Err(UpdateError::Transport(format!("{url}: too many redirects")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_release_hosts_over_https() {
        let f = GitFetch {
            client: Client::new(false, Duration::from_secs(1)).unwrap(),
            extra_host: Some("mirror.example".into()),
        };
        assert!(f
            .allowed("https://raw.githubusercontent.com/a/b/stable.json")
            .is_ok());
        assert!(f.allowed("https://mirror.example/stable.json").is_ok());
        assert!(f.allowed("http://raw.githubusercontent.com/a").is_err());
        assert!(f.allowed("https://evil.example/a").is_err());
    }

    #[test]
    fn a_hub_needs_a_token_and_a_url() {
        assert!(HubHttp::new("http://fleet-hub:4180", "", None).is_err());
        assert!(HubHttp::new("fleet-hub:4180", "t", None).is_err());
        assert!(HubHttp::new("http://fleet-hub:4180/", "t", None).is_ok());
    }

    #[test]
    fn the_host_header_is_one_the_hub_allows() {
        assert_eq!(
            hub_host_header(None, Some("https://fleet.example.com")).as_deref(),
            Some("fleet.example.com")
        );
        assert_eq!(
            hub_host_header(None, Some("https://fleet.example.com:8443/")).as_deref(),
            Some("fleet.example.com:8443")
        );
        assert_eq!(
            hub_host_header(Some("hub.lan"), Some("https://fleet.example.com")).as_deref(),
            Some("hub.lan")
        );
        assert_eq!(hub_host_header(None, None), None);
        assert_eq!(hub_host_header(Some(" "), Some("not a url")), None);
    }
}
