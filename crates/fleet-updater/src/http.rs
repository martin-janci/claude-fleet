//! A minimal HTTP/1.1 client: one request per connection (`Connection:
//! close`), the whole answer read to EOF, then parsed. Enough for the Docker
//! Engine API over its unix socket and for the hub's two `/update` routes, and
//! small enough to read in one sitting (design §8.1: no `bollard`, no hyper
//! client).

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Send one request over `stream` and read the answer, at most `max_bytes`
/// of it (head included).
pub async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
    max_bytes: u64,
) -> std::io::Result<Response> {
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(b) = body {
        head.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    if let Some(b) = body {
        stream.write_all(b).await?;
    }
    stream.flush().await?;

    let mut raw = Vec::new();
    (&mut stream)
        .take(max_bytes + 1)
        .read_to_end(&mut raw)
        .await?;
    if raw.len() as u64 > max_bytes {
        return Err(std::io::Error::other(format!(
            "response larger than {max_bytes} bytes"
        )));
    }
    parse(&raw).map_err(std::io::Error::other)
}

/// Parse a complete response (the connection closed after it).
pub fn parse(raw: &[u8]) -> Result<Response, String> {
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("no end of headers")?;
    let head = std::str::from_utf8(&raw[..end]).map_err(|_| "headers are not UTF-8")?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().ok_or("empty response")?;
    let mut parts = status_line.splitn(3, ' ');
    let proto = parts.next().unwrap_or_default();
    if !proto.starts_with("HTTP/1.") {
        return Err(format!("not an HTTP/1 response: {status_line:?}"));
    }
    let status: u16 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("bad status line {status_line:?}"))?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    let rest = &raw[end + 4..];
    let mut resp = Response {
        status,
        headers,
        body: Vec::new(),
    };
    resp.body = if resp
        .header("transfer-encoding")
        .is_some_and(|t| t.to_ascii_lowercase().contains("chunked"))
    {
        dechunk(rest)?
    } else if let Some(len) = resp.header("content-length") {
        let len: usize = len.parse().map_err(|_| "bad Content-Length")?;
        if rest.len() < len {
            return Err(format!("body cut short: {} of {len} bytes", rest.len()));
        }
        rest[..len].to_vec()
    } else {
        // Neither: the body runs to EOF (a hijacked Docker exec stream).
        rest.to_vec()
    };
    Ok(resp)
}

fn dechunk(mut rest: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let eol = rest
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or("chunk size line not terminated")?;
        let line = std::str::from_utf8(&rest[..eol]).map_err(|_| "bad chunk size")?;
        let size_hex = line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_hex, 16)
            .map_err(|_| format!("bad chunk size {size_hex:?}"))?;
        rest = &rest[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        if rest.len() < size {
            return Err("chunk cut short".into());
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size..];
        rest = rest.strip_prefix(b"\r\n").ok_or("chunk not terminated")?;
    }
}

/// Docker's multiplexed stdout / stderr stream (a non-TTY exec or `logs`):
/// frames of `[stream, 0, 0, 0, len (u32 big-endian)]` + payload. Returns
/// (stdout, stderr). Bytes that are not framed are taken as stdout.
pub fn demux(mut raw: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    while raw.len() >= 8 && matches!(raw[0], 0..=2) && raw[1..4] == [0, 0, 0] {
        let len = u32::from_be_bytes([raw[4], raw[5], raw[6], raw[7]]) as usize;
        let end = (8 + len).min(raw.len());
        let payload = &raw[8..end];
        if raw[0] == 2 {
            err.extend_from_slice(payload);
        } else {
            out.extend_from_slice(payload);
        }
        raw = &raw[end..];
    }
    out.extend_from_slice(raw);
    (out, err)
}

/// A parsed `http://` or `https://` base URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    pub tls: bool,
    pub host: String,
    pub port: u16,
    /// Always starts with `/`.
    pub path: String,
}

impl Url {
    pub fn parse(url: &str) -> Result<Url, String> {
        let (tls, rest) = if let Some(r) = url.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = url.strip_prefix("http://") {
            (false, r)
        } else {
            return Err(format!("{url}: only http:// and https:// URLs"));
        };
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        if authority.contains('@') {
            return Err(format!("{url}: credentials in a URL are refused"));
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) if !h.is_empty() => (
                h,
                p.parse::<u16>()
                    .map_err(|_| format!("{url}: bad port {p:?}"))?,
            ),
            _ => (authority, if tls { 443 } else { 80 }),
        };
        if host.is_empty() {
            return Err(format!("{url}: no host"));
        }
        Ok(Url {
            tls,
            host: host.to_string(),
            port,
            path: path.to_string(),
        })
    }

    /// The `Host:` header value.
    pub fn authority(&self) -> String {
        let default = if self.tls { 443 } else { 80 };
        if self.port == default {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// HTTP(S) over TCP, with the image's CA bundle for TLS.
#[derive(Clone)]
pub struct Client {
    tls: Option<tokio_rustls::TlsConnector>,
    timeout: Duration,
    /// Sent as `Host:` instead of the URL's authority (the hub's public name
    /// while connecting to it inside the compose network).
    host_header: Option<String>,
}

const CA_BUNDLES: [&str; 4] = [
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/cert.pem",
    "/etc/ssl/ca-bundle.pem",
];

impl Client {
    /// `tls` loads the CA bundle now; without it an `https://` request fails.
    pub fn new(tls: bool, timeout: Duration) -> Result<Client, String> {
        Ok(Client {
            tls: if tls { Some(connector()?) } else { None },
            timeout,
            host_header: None,
        })
    }

    pub fn with_host_header(mut self, host: Option<String>) -> Self {
        self.host_header = host;
        self
    }

    pub async fn send(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
        max_bytes: u64,
    ) -> Result<Response, String> {
        let u = Url::parse(url)?;
        let host = self.host_header.clone().unwrap_or_else(|| u.authority());
        let fut = async {
            let tcp = tokio::net::TcpStream::connect((u.host.as_str(), u.port))
                .await
                .map_err(|e| format!("{}: {e}", u.authority()))?;
            if u.tls {
                let tls = self
                    .tls
                    .as_ref()
                    .ok_or_else(|| format!("{url}: TLS is not set up for this client"))?;
                let name = rustls_pki_types::ServerName::try_from(u.host.clone())
                    .map_err(|e| format!("{}: {e}", u.host))?;
                let stream = tls
                    .connect(name, tcp)
                    .await
                    .map_err(|e| format!("{}: TLS: {e}", u.authority()))?;
                exchange(stream, method, &host, &u.path, headers, body, max_bytes).await
            } else {
                exchange(tcp, method, &host, &u.path, headers, body, max_bytes).await
            }
            .map_err(|e| format!("{url}: {e}"))
        };
        tokio::time::timeout(self.timeout, fut)
            .await
            .map_err(|_| format!("{url}: timed out"))?
    }
}

fn connector() -> Result<tokio_rustls::TlsConnector, String> {
    use rustls_pki_types::pem::PemObject;
    use rustls_pki_types::CertificateDer;
    use tokio_rustls::rustls;

    let bundle = std::env::var_os("SSL_CERT_FILE")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            CA_BUNDLES
                .iter()
                .map(std::path::PathBuf::from)
                .find(|p| p.is_file())
        })
        .ok_or("no CA bundle found (install ca-certificates or set SSL_CERT_FILE)")?;
    let mut roots = rustls::RootCertStore::empty();
    let certs =
        CertificateDer::pem_file_iter(&bundle).map_err(|e| format!("{}: {e}", bundle.display()))?;
    for cert in certs.flatten() {
        let _ = roots.add(cert);
    }
    if roots.is_empty() {
        return Err(format!("{}: no usable certificate in it", bundle.display()));
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(tokio_rustls::TlsConnector::from(Arc::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_content_length_body() {
        let r = parse(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nX: y\r\n\r\nhelloEXTRA").unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body, b"hello");
        assert_eq!(r.header("x"), Some("y"));
    }

    #[test]
    fn parses_a_chunked_body() {
        let r = parse(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3;ext=1\r\n:1}\r\n0\r\n\r\n",
        )
        .unwrap();
        assert_eq!(r.body, b"{\"a\":1}");
    }

    #[test]
    fn a_body_without_a_length_runs_to_eof() {
        let r = parse(b"HTTP/1.1 200 OK\r\nContent-Type: x\r\n\r\nraw stream").unwrap();
        assert_eq!(r.body, b"raw stream");
    }

    #[test]
    fn refuses_garbage() {
        assert!(parse(b"hello").is_err());
        assert!(parse(b"SSH-2.0 x\r\n\r\n").is_err());
        assert!(parse(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nshort").is_err());
        assert!(parse(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n").is_err());
    }

    #[test]
    fn demuxes_docker_frames() {
        let mut raw = vec![1, 0, 0, 0, 0, 0, 0, 3];
        raw.extend_from_slice(b"out");
        raw.extend_from_slice(&[2, 0, 0, 0, 0, 0, 0, 3]);
        raw.extend_from_slice(b"err");
        raw.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 1]);
        raw.extend_from_slice(b"!");
        let (o, e) = demux(&raw);
        assert_eq!(o, b"out!");
        assert_eq!(e, b"err");
        // A TTY stream has no frames.
        assert_eq!(demux(b"{\"ready\":true}").0, b"{\"ready\":true}");
    }

    #[test]
    fn urls() {
        let u = Url::parse("http://fleet-hub:4180").unwrap();
        assert_eq!(
            (u.tls, u.host.as_str(), u.port, u.path.as_str()),
            (false, "fleet-hub", 4180, "/")
        );
        let u = Url::parse("https://raw.githubusercontent.com/a/b/stable.json").unwrap();
        assert_eq!((u.port, u.path.as_str()), (443, "/a/b/stable.json"));
        assert_eq!(u.authority(), "raw.githubusercontent.com");
        assert!(Url::parse("ftp://x").is_err());
        assert!(Url::parse("http://user:pw@x/").is_err());
        assert!(Url::parse("http://x:notaport/").is_err());
    }

    #[tokio::test]
    async fn exchange_writes_one_request_and_reads_the_answer() {
        let (client, mut server) = tokio::io::duplex(4096);
        let srv = tokio::spawn(async move {
            let mut buf = vec![0u8; 1024];
            let n = server.read(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).into_owned();
            server
                .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\nok")
                .await
                .unwrap();
            drop(server);
            req
        });
        let r = exchange(
            client,
            "POST",
            "docker",
            "/containers/create?name=x",
            &[("Content-Type", "application/json")],
            Some(b"{}"),
            1 << 20,
        )
        .await
        .unwrap();
        assert_eq!(r.status, 201);
        assert_eq!(r.body, b"ok");
        let req = srv.await.unwrap();
        assert!(req.starts_with(
            "POST /containers/create?name=x HTTP/1.1\r\nHost: docker\r\nConnection: close\r\n"
        ));
        assert!(req.contains("Content-Length: 2\r\n\r\n{}"));
    }
}
