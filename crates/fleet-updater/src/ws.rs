//! `pods/exec` over a WebSocket, the one part of the Kubernetes API that is
//! not a request and an answer (`kube::ApiServer::exec`).
//!
//! The API server upgrades `GET …/pods/<pod>/exec?command=…` to a WebSocket
//! speaking `v4.channel.k8s.io`: every binary message starts with a channel
//! byte — 1 stdout, 2 stderr, 3 the final `metav1.Status` (`Success`, or
//! `NonZeroExitCode` with the code in `details.causes`). With no stdin and no
//! TTY the client only reads, answers pings, and closes. That is all of RFC
//! 6455 this needs (no extensions, no fragmentation of what it sends), so it
//! is written out here rather than pulled in as a WebSocket crate.

use base64::Engine as _;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::runtime::ExecOutput;

/// The subprotocol asked for. `v4` has carried the exit status since
/// Kubernetes 1.4; `v5` (1.30+) only adds a stdin-close signal this never
/// needs.
pub const PROTOCOL: &str = "v4.channel.k8s.io";

const HEAD_MAX: usize = 64 << 10;

/// Send the upgrade for `path` over `stream`, then read the command's output
/// to its status, at most `max_bytes` of it.
pub async fn exec<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    max_bytes: u64,
) -> Result<ExecOutput, String> {
    let nonce = nonce();
    let key = base64::engine::general_purpose::STANDARD.encode(&nonce[..16]);
    let mut head = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Protocol: {PROTOCOL}\r\n"
    );
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream
        .write_all(head.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    stream.flush().await.map_err(|e| e.to_string())?;

    // The answer's head, byte by byte: what follows it is already frames.
    let mut raw = Vec::new();
    while !raw.ends_with(b"\r\n\r\n") {
        if raw.len() > HEAD_MAX {
            return Err("the exec answer's headers are too long".into());
        }
        let mut b = [0u8; 1];
        match stream.read(&mut b).await.map_err(|e| e.to_string())? {
            0 => break,
            _ => raw.push(b[0]),
        }
    }
    let status_line =
        String::from_utf8_lossy(raw.split(|&b| b == b'\r').next().unwrap_or(&[])).into_owned();
    let code: u16 = status_line
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| format!("not an HTTP answer to the exec upgrade: {status_line:?}"))?;
    if code != 101 {
        // A refusal is an ordinary answer: read its body to say why. The
        // connection may stay open after it, so read what Content-Length
        // says, or for a moment at most.
        let len = String::from_utf8_lossy(&raw).lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())?
        });
        let mut rest = Vec::new();
        match len {
            Some(n) => {
                rest.resize(n.min(1 << 16), 0);
                let _ = stream.read_exact(&mut rest).await;
            }
            None => {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    (&mut stream).take(1 << 16).read_to_end(&mut rest),
                )
                .await;
            }
        }
        raw.extend_from_slice(&rest);
        let why = crate::http::parse(&raw)
            .ok()
            .map(|r| status_message(&r.body))
            .unwrap_or_default();
        return Err(format!("exec refused: HTTP {code}: {why}"));
    }
    let head = String::from_utf8_lossy(&raw).to_ascii_lowercase();
    if !head.contains(&format!("sec-websocket-protocol: {PROTOCOL}")) {
        return Err(format!(
            "the API server did not agree to {PROTOCOL} (an exec WebSocket needs Kubernetes 1.4+)"
        ));
    }

    let mut out = Output::default();
    let mut message: Vec<u8> = Vec::new();
    let mut total: u64 = 0;
    loop {
        let frame = match read_frame(&mut stream).await {
            Ok(Some(f)) => f,
            // The server may close the TCP connection right after the status.
            Ok(None) => break,
            Err(e) => return Err(format!("reading the exec stream: {e}")),
        };
        total += frame.payload.len() as u64;
        if total > max_bytes {
            return Err(format!("the exec output passed {max_bytes} bytes"));
        }
        match frame.opcode {
            0x0..=0x2 => {
                message.extend_from_slice(&frame.payload);
                if frame.fin {
                    out.take(&message);
                    message.clear();
                }
            }
            0x8 => {
                let _ = write_frame(&mut stream, 0x8, &[], &nonce).await;
                break;
            }
            0x9 => {
                let _ = write_frame(&mut stream, 0xA, &frame.payload, &nonce).await;
            }
            _ => {}
        }
    }
    out.finish()
}

#[derive(Default)]
struct Output {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    status: Vec<u8>,
}

impl Output {
    fn take(&mut self, msg: &[u8]) {
        let Some((&ch, data)) = msg.split_first() else {
            return;
        };
        match ch {
            1 => self.stdout.extend_from_slice(data),
            2 => self.stderr.extend_from_slice(data),
            3 => self.status.extend_from_slice(data),
            _ => {}
        }
    }

    fn finish(self) -> Result<ExecOutput, String> {
        let stdout = String::from_utf8_lossy(&self.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&self.stderr).into_owned();
        if self.status.is_empty() {
            return Err(format!(
                "the exec stream ended without a status (stderr: {})",
                stderr.trim()
            ));
        }
        let exit_code = exit_code(&self.status)?;
        Ok(ExecOutput {
            exit_code,
            stdout,
            stderr,
        })
    }
}

/// The exit code a `v4.channel.k8s.io` status carries; an error for a
/// status that is not about the command's exit (it never ran).
pub fn exit_code(status: &[u8]) -> Result<i64, String> {
    let v: Value = serde_json::from_slice(status).map_err(|e| format!("the exec status: {e}"))?;
    if v["status"] == "Success" {
        return Ok(0);
    }
    if v["reason"] == "NonZeroExitCode" {
        let code = v
            .pointer("/details/causes")
            .and_then(Value::as_array)
            .and_then(|cs| {
                cs.iter()
                    .find(|c| c["reason"] == "ExitCode")
                    .and_then(|c| c["message"].as_str())
                    .and_then(|m| m.trim().parse().ok())
            })
            .unwrap_or(1);
        return Ok(code);
    }
    Err(format!(
        "exec failed: {}",
        v["message"].as_str().unwrap_or("no message")
    ))
}

/// A `metav1.Status`'s message, else the body as text.
pub fn status_message(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v["message"].as_str().map(String::from))
        .unwrap_or_else(|| String::from_utf8_lossy(body).trim().to_string())
}

struct Frame {
    fin: bool,
    opcode: u8,
    payload: Vec<u8>,
}

/// One frame; `None` at a clean end of stream.
async fn read_frame<S: AsyncRead + Unpin>(s: &mut S) -> std::io::Result<Option<Frame>> {
    let mut h = [0u8; 2];
    match s.read_exact(&mut h).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let fin = h[0] & 0x80 != 0;
    let opcode = h[0] & 0x0f;
    let masked = h[1] & 0x80 != 0;
    let len = match h[1] & 0x7f {
        126 => {
            let mut b = [0u8; 2];
            s.read_exact(&mut b).await?;
            u16::from_be_bytes(b) as u64
        }
        127 => {
            let mut b = [0u8; 8];
            s.read_exact(&mut b).await?;
            u64::from_be_bytes(b)
        }
        n => n as u64,
    };
    if len > 64 << 20 {
        return Err(std::io::Error::other(format!("a {len}-byte frame")));
    }
    let mut mask = [0u8; 4];
    if masked {
        s.read_exact(&mut mask).await?;
    }
    let mut payload = vec![0u8; len as usize];
    s.read_exact(&mut payload).await?;
    if masked {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    }
    Ok(Some(Frame {
        fin,
        opcode,
        payload,
    }))
}

/// A client frame: final, masked (RFC 6455 §5.3), payload under 126 bytes
/// (a pong or a close).
async fn write_frame<S: AsyncWrite + Unpin>(
    s: &mut S,
    opcode: u8,
    payload: &[u8],
    nonce: &[u8; 20],
) -> std::io::Result<()> {
    let payload = &payload[..payload.len().min(125)];
    let mask = [nonce[16], nonce[17], nonce[18], nonce[19]];
    let mut f = vec![0x80 | opcode, 0x80 | payload.len() as u8];
    f.extend_from_slice(&mask);
    f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    s.write_all(&f).await?;
    s.flush().await
}

/// Bytes for the key and the mask. Neither is a secret: the key only proves
/// the server speaks WebSocket, the mask only defeats confused proxies.
fn nonce() -> [u8; 20] {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let mut x = (t.as_nanos() as u64)
        ^ ((std::process::id() as u64) << 32)
        ^ N.fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut out = [0u8; 20];
    for chunk in out.chunks_mut(8) {
        // splitmix64
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        chunk.copy_from_slice(&z.to_le_bytes()[..chunk.len()]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server frame (unmasked).
    fn frame(fin: bool, opcode: u8, payload: &[u8]) -> Vec<u8> {
        let mut f = vec![if fin { 0x80 } else { 0 } | opcode];
        match payload.len() {
            n if n < 126 => f.push(n as u8),
            n if n < 65536 => {
                f.push(126);
                f.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                f.push(127);
                f.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        f.extend_from_slice(payload);
        f
    }

    fn chan(ch: u8, data: &[u8]) -> Vec<u8> {
        let mut v = vec![ch];
        v.extend_from_slice(data);
        v
    }

    const UPGRADED: &[u8] = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: x\r\nSec-WebSocket-Protocol: v4.channel.k8s.io\r\n\r\n";

    /// Run `exec` against a server that answers with `answer` and hands back
    /// what the client wrote.
    async fn run(answer: Vec<u8>) -> (Result<ExecOutput, String>, String) {
        let (client, mut server) = tokio::io::duplex(1 << 20);
        let srv = tokio::spawn(async move {
            let mut req = Vec::new();
            let mut b = [0u8; 1];
            while !req.ends_with(b"\r\n\r\n") {
                server.read_exact(&mut b).await.unwrap();
                req.push(b[0]);
            }
            server.write_all(&answer).await.unwrap();
            server.shutdown().await.unwrap();
            // Read what the client sends back (pong, close) until it is done.
            let mut back = Vec::new();
            let _ = server.read_to_end(&mut back).await;
            (String::from_utf8_lossy(&req).into_owned(), back)
        });
        let r = exec(
            client,
            "10.43.0.1",
            "/api/v1/namespaces/ns/pods/p-0/exec?container=c&stdout=true&command=x",
            &[("Authorization", "Bearer tok")],
            1 << 20,
        )
        .await;
        let (req, _back) = srv.await.unwrap();
        (r, req)
    }

    #[tokio::test]
    async fn reads_stdout_stderr_and_a_success_status() {
        let mut a = UPGRADED.to_vec();
        a.extend(frame(true, 2, &chan(1, b"{\"ready\":")));
        // A message in two fragments.
        a.extend(frame(false, 2, &[1]));
        a.extend(frame(true, 0, b"true}"));
        a.extend(frame(true, 9, b"ping"));
        a.extend(frame(true, 2, &chan(2, b"a warning\n")));
        a.extend(frame(
            true,
            2,
            &chan(3, br#"{"metadata":{},"status":"Success"}"#),
        ));
        a.extend(frame(true, 8, &[3, 232]));
        let (r, req) = run(a).await;
        let out = r.unwrap();
        assert_eq!(out.stdout, "{\"ready\":true}");
        assert_eq!(out.stderr, "a warning\n");
        assert_eq!(out.exit_code, 0);
        assert!(req.starts_with("GET /api/v1/namespaces/ns/pods/p-0/exec?"));
        assert!(req.contains("\r\nUpgrade: websocket\r\n"));
        assert!(req.contains("\r\nSec-WebSocket-Protocol: v4.channel.k8s.io\r\n"));
        assert!(req.contains("\r\nAuthorization: Bearer tok\r\n"));
    }

    #[tokio::test]
    async fn a_non_zero_exit_carries_its_code() {
        let mut a = UPGRADED.to_vec();
        a.extend(frame(true, 2, &chan(1, b"{\"ready\":false}")));
        a.extend(frame(
            true,
            2,
            &chan(
                3,
                br#"{"status":"Failure","reason":"NonZeroExitCode","message":"command terminated with non-zero exit code","details":{"causes":[{"reason":"ExitCode","message":"1"}]}}"#,
            ),
        ));
        // Then the connection just ends.
        let out = run(a).await.0.unwrap();
        assert_eq!(out.exit_code, 1);
        assert_eq!(out.stdout, "{\"ready\":false}");
    }

    #[tokio::test]
    async fn a_large_message_is_read_whole() {
        let big = vec![b'x'; 70_000];
        let mut a = UPGRADED.to_vec();
        a.extend(frame(true, 2, &chan(1, &big)));
        a.extend(frame(true, 2, &chan(3, br#"{"status":"Success"}"#)));
        let out = run(a).await.0.unwrap();
        assert_eq!(out.stdout.len(), 70_000);
    }

    #[tokio::test]
    async fn a_refusal_or_a_missing_status_is_an_error() {
        let body = br#"{"kind":"Status","message":"pods \"p-0\" is forbidden: no exec for you"}"#;
        let mut a = format!(
            "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        a.extend_from_slice(body);
        let e = run(a).await.0.unwrap_err();
        assert!(
            e.contains("HTTP 403") && e.contains("no exec for you"),
            "{e}"
        );

        let mut a = UPGRADED.to_vec();
        a.extend(frame(true, 2, &chan(1, b"half")));
        let e = run(a).await.0.unwrap_err();
        assert!(e.contains("without a status"), "{e}");

        let mut a = UPGRADED.to_vec();
        a.extend(frame(
            true,
            2,
            &chan(
                3,
                br#"{"status":"Failure","message":"executable file not found in $PATH"}"#,
            ),
        ));
        let e = run(a).await.0.unwrap_err();
        assert!(e.contains("executable file not found"), "{e}");
    }

    #[tokio::test]
    async fn a_server_without_the_subprotocol_is_refused() {
        let a = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n"
            .to_vec();
        let e = run(a).await.0.unwrap_err();
        assert!(e.contains("v4.channel.k8s.io"), "{e}");
    }

    #[tokio::test]
    async fn client_frames_are_masked() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let n = nonce();
        write_frame(&mut a, 0xA, b"ping", &n).await.unwrap();
        drop(a);
        let mut got = Vec::new();
        b.read_to_end(&mut got).await.unwrap();
        assert_eq!(got[0], 0x8A);
        assert_eq!(got[1], 0x80 | 4);
        let mask = &got[2..6];
        let payload: Vec<u8> = got[6..]
            .iter()
            .enumerate()
            .map(|(i, x)| x ^ mask[i % 4])
            .collect();
        assert_eq!(payload, b"ping");
    }
}
