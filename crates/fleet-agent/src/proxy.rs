//! Reaching the hub through an HTTP proxy (`CONNECT`), for a host on a
//! network that lets nothing out except through one — a corporate network
//! a laptop is carried into.
//!
//! The proxy comes from the config's `proxy` (`install --proxy`), else from
//! `HTTPS_PROXY` / `https_proxy` / `ALL_PROXY` / `all_proxy`, unless
//! `NO_PROXY` / `no_proxy` names the hub. Only an `http://` proxy is spoken:
//! the tunnel it opens carries the agent's own TLS to the hub end to end, so
//! the proxy sees the hub's name and nothing else. A proxy that inspects TLS
//! (re-signs it with its own CA) needs that CA passed as `--ca-file`.
//!
//! Both the WebSocket and the hook relay's requests go through
//! [`crate::conn::Dialer::connect_io`], so both use the proxy.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// The most a proxy's answer to `CONNECT` may take before its blank line.
const MAX_HEAD: usize = 16 * 1024;

/// An `http://` proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proxy {
    pub host: String,
    pub port: u16,
    /// `Basic <base64(user:pass)>`, from the URL's userinfo.
    authorization: Option<String>,
}

impl Proxy {
    /// `http://[user[:pass]@]host[:port][/]`. The port defaults to 80; any
    /// other scheme is refused, as is a path.
    pub fn parse(url: &str) -> Result<Self, String> {
        let url = url.trim();
        let Some(rest) = url.strip_prefix("http://") else {
            return Err(format!(
                "proxy {url:?}: only an http:// proxy is supported (it tunnels the agent's \
                 own TLS with CONNECT)"
            ));
        };
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        if rest.contains('/') || rest.contains('?') || rest.contains('#') {
            return Err(format!("proxy {url:?}: a proxy URL has no path"));
        }
        let (userinfo, authority) = match rest.rsplit_once('@') {
            Some((u, a)) => (Some(u), a),
            None => (None, rest),
        };
        let (host, port) = split_host_port(authority)
            .ok_or_else(|| format!("proxy {url:?}: no host, or a port that is not a number"))?;
        let authorization = userinfo.map(|u| {
            let (user, pass) = u.split_once(':').unwrap_or((u, ""));
            let plain = format!("{}:{}", percent_decode(user), percent_decode(pass));
            format!("Basic {}", fleet_proto::encode_b64(plain.as_bytes()))
        });
        Ok(Self {
            host,
            port,
            authorization,
        })
    }

    /// The proxy for a hub at `hub_host`: `configured` if set (an empty
    /// string turns the environment off too), else the environment's, unless
    /// its no-proxy list names the hub. `env` looks a variable up.
    pub fn resolve(
        configured: Option<&str>,
        hub_host: &str,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Option<Self>, String> {
        if let Some(url) = configured {
            return if url.trim().is_empty() {
                Ok(None)
            } else {
                Self::parse(url).map(Some)
            };
        }
        let first = |names: &[&str]| {
            names
                .iter()
                .find_map(|n| env(n).filter(|v| !v.trim().is_empty()))
        };
        let Some(url) = first(&["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]) else {
            return Ok(None);
        };
        if let Some(list) = first(&["NO_PROXY", "no_proxy"]) {
            if no_proxy_matches(&list, hub_host) {
                return Ok(None);
            }
        }
        Self::parse(&url).map(Some)
    }

    /// The `CONNECT` request for `host:port`.
    pub fn connect_request(&self, host: &str, port: u16) -> String {
        let target = if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };
        let mut req = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n");
        if let Some(auth) = &self.authorization {
            req.push_str(&format!("Proxy-Authorization: {auth}\r\n"));
        }
        req.push_str("\r\n");
        req
    }

    /// A TCP stream to `host:port` through this proxy, ready for TLS.
    pub async fn tunnel(&self, host: &str, port: u16) -> Result<TcpStream, String> {
        let mut tcp = TcpStream::connect((self.host.as_str(), self.port))
            .await
            .map_err(|e| format!("proxy {}:{}: {e}", self.host, self.port))?;
        let _ = tcp.set_nodelay(true);
        tcp.write_all(self.connect_request(host, port).as_bytes())
            .await
            .map_err(|e| format!("proxy {}:{}: {e}", self.host, self.port))?;
        // Byte by byte up to the blank line: nothing after it is the
        // proxy's, and a buffered read could swallow the start of the TLS
        // stream if the proxy wrote both at once.
        let mut head = Vec::with_capacity(256);
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if head.len() >= MAX_HEAD {
                return Err(format!(
                    "proxy {}:{}: its answer to CONNECT has no end",
                    self.host, self.port
                ));
            }
            let n = tcp
                .read(&mut byte)
                .await
                .map_err(|e| format!("proxy {}:{}: {e}", self.host, self.port))?;
            if n == 0 {
                return Err(format!(
                    "proxy {}:{} closed the connection before answering CONNECT",
                    self.host, self.port
                ));
            }
            head.push(byte[0]);
        }
        match connect_status(&head) {
            Some(200..=299) => Ok(tcp),
            Some(407) => Err(format!(
                "proxy {}:{} wants credentials (407): put them in the proxy URL, \
                 http://user:password@host:port",
                self.host, self.port
            )),
            Some(code) => Err(format!(
                "proxy {}:{} refused CONNECT to {host}:{port}: HTTP {code}",
                self.host, self.port
            )),
            None => Err(format!(
                "proxy {}:{}: its answer to CONNECT is not HTTP",
                self.host, self.port
            )),
        }
    }
}

/// The status code of a proxy's answer head.
fn connect_status(head: &[u8]) -> Option<u16> {
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut resp = httparse::Response::new(&mut headers);
    match resp.parse(head) {
        Ok(httparse::Status::Complete(_)) => resp.code,
        _ => None,
    }
}

/// `host[:port]` or `[v6][:port]`, the port defaulting to 80.
fn split_host_port(authority: &str) -> Option<(String, u16)> {
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (h, after) = v6.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(p) => p.parse().ok()?,
            None if after.is_empty() => 80,
            None => return None,
        };
        (h.to_string(), port)
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse().ok()?),
            None => (authority.to_string(), 80),
        }
    };
    (!host.is_empty()).then_some((host, port))
}

/// `%xx` decoding for a URL's userinfo; anything malformed stays as it was.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether a `NO_PROXY` list names `host`: `*`, the host itself, or a domain
/// suffix (`example.com` and `.example.com` both cover `hub.example.com`).
/// A port on an entry is ignored.
pub fn no_proxy_matches(list: &str, host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    list.split(',').map(str::trim).any(|entry| {
        if entry == "*" {
            return true;
        }
        let entry = entry
            .rsplit_once(':')
            .filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit()))
            .map_or(entry, |(h, _)| h);
        let entry = entry.trim_start_matches('.').to_ascii_lowercase();
        !entry.is_empty() && (host == entry || host.ends_with(&format!(".{entry}")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn a_proxy_url_parses_with_and_without_credentials() {
        let p = Proxy::parse("http://proxy.corp:3128").unwrap();
        assert_eq!((p.host.as_str(), p.port), ("proxy.corp", 3128));
        assert_eq!(
            p.connect_request("fleet.example", 443),
            "CONNECT fleet.example:443 HTTP/1.1\r\nHost: fleet.example:443\r\n\r\n"
        );

        let p = Proxy::parse("http://me%40corp:s3cr%3At@10.0.0.1/").unwrap();
        assert_eq!((p.host.as_str(), p.port), ("10.0.0.1", 80));
        let req = p.connect_request("fleet.example", 443);
        // base64("me@corp:s3cr:t")
        assert!(
            req.contains("Proxy-Authorization: Basic bWVAY29ycDpzM2NyOnQ=\r\n"),
            "{req}"
        );

        let p = Proxy::parse("http://[::1]:8080").unwrap();
        assert_eq!((p.host.as_str(), p.port), ("::1", 8080));
    }

    #[test]
    fn what_is_not_an_http_proxy_is_refused() {
        for bad in [
            "https://proxy:443",
            "socks5://proxy:1080",
            "proxy:3128",
            "http://",
            "http://proxy:port",
            "http://proxy:3128/path",
        ] {
            assert!(Proxy::parse(bad).is_err(), "{bad}");
        }
    }

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn the_config_wins_over_the_environment_and_empty_turns_both_off() {
        let e = env(&[("HTTPS_PROXY", "http://env:1")]);
        let p = Proxy::resolve(Some("http://cfg:2"), "hub.example", &e).unwrap();
        assert_eq!(p.unwrap().host, "cfg");
        assert_eq!(Proxy::resolve(Some(""), "hub.example", &e).unwrap(), None);
        let p = Proxy::resolve(None, "hub.example", &e).unwrap();
        assert_eq!(p.unwrap().host, "env");
        assert_eq!(Proxy::resolve(None, "hub.example", env(&[])).unwrap(), None);
        let p = Proxy::resolve(None, "hub.example", env(&[("all_proxy", "http://all:3")]));
        assert_eq!(p.unwrap().unwrap().host, "all");
    }

    #[test]
    fn no_proxy_keeps_the_hub_direct() {
        let e = env(&[
            ("https_proxy", "http://env:1"),
            ("no_proxy", "localhost, .rlt.sk:443"),
        ]);
        assert_eq!(Proxy::resolve(None, "fleet.rlt.sk", &e).unwrap(), None);
        assert!(Proxy::resolve(None, "hub.example", &e).unwrap().is_some());
        assert!(no_proxy_matches("*", "anything"));
        assert!(no_proxy_matches("rlt.sk", "rlt.sk"));
        assert!(!no_proxy_matches("rlt.sk", "notrlt.sk"));
        assert!(!no_proxy_matches("", "rlt.sk"));
    }

    /// End to end against a CONNECT proxy on loopback: the bytes after the
    /// tunnel is up reach the target untouched, and a refusal is reported.
    #[tokio::test]
    async fn a_tunnel_carries_bytes_and_a_refusal_says_so() {
        use tokio::net::TcpListener;
        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_port = target.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = target.accept().await.unwrap();
            let mut buf = [0u8; 5];
            s.read_exact(&mut buf).await.unwrap();
            s.write_all(&buf).await.unwrap();
        });
        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_port = proxy.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut client, _) = proxy.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut b = [0u8; 1];
                    while !head.ends_with(b"\r\n\r\n") {
                        client.read_exact(&mut b).await.unwrap();
                        head.push(b[0]);
                    }
                    let head = String::from_utf8(head).unwrap();
                    let target = head
                        .strip_prefix("CONNECT ")
                        .and_then(|r| r.split_once(' '))
                        .map(|(t, _)| t.to_string())
                        .unwrap();
                    if target.starts_with("denied") {
                        client
                            .write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n")
                            .await
                            .unwrap();
                        return;
                    }
                    let mut upstream = TcpStream::connect(&target).await.unwrap();
                    client
                        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                        .await
                        .unwrap();
                    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
                });
            }
        });
        let p = Proxy::parse(&format!("http://127.0.0.1:{proxy_port}")).unwrap();
        let mut s = p.tunnel("127.0.0.1", target_port).await.unwrap();
        s.write_all(b"hello").await.unwrap();
        let mut back = [0u8; 5];
        s.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"hello");

        let err = p.tunnel("denied.example", 443).await.unwrap_err();
        assert!(err.contains("HTTP 403"), "{err}");
    }
}
