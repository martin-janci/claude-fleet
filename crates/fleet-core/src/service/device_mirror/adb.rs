//! Just enough of the adb wire protocol to ask a device host's adb server
//! for its version and its forwards without running an adb client, so a
//! mismatched local client is never pointed at it (it would `host:kill`
//! the server).

use crate::ipc_error::{codes, IpcError};
use crate::relay::{Stream, SCRCPY_PORTS};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

/// The longest one host service may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

fn proto_err(what: impl Into<String>) -> IpcError {
    IpcError::new(codes::E_PARSE, format!("adb server: {}", what.into()))
}

async fn read_hex4(s: &mut (impl AsyncRead + Unpin + ?Sized)) -> Result<usize, IpcError> {
    let mut b = [0u8; 4];
    s.read_exact(&mut b).await?;
    let t = std::str::from_utf8(&b).map_err(|_| proto_err("bad length"))?;
    usize::from_str_radix(t, 16).map_err(|_| proto_err(format!("bad length {t:?}")))
}

async fn status(s: &mut (impl Stream + ?Sized)) -> Result<(), IpcError> {
    let mut st = [0u8; 4];
    s.read_exact(&mut st).await?;
    match &st {
        b"OKAY" => Ok(()),
        b"FAIL" => {
            let n = read_hex4(s).await?;
            let mut msg = vec![0u8; n.min(4096)];
            s.read_exact(&mut msg).await?;
            Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!("adb server refused: {}", String::from_utf8_lossy(&msg)),
            ))
        }
        other => Err(proto_err(format!(
            "unexpected status {:?}",
            String::from_utf8_lossy(other)
        ))),
    }
}

async fn send(s: &mut (impl Stream + ?Sized), service: &str) -> Result<(), IpcError> {
    s.write_all(format!("{:04x}{service}", service.len()).as_bytes())
        .await?;
    s.flush().await?;
    status(s).await
}

/// Send one host service and return its length-prefixed payload.
pub async fn request(s: &mut (impl Stream + ?Sized), service: &str) -> Result<Vec<u8>, IpcError> {
    tokio::time::timeout(REQUEST_TIMEOUT, async {
        send(s, service).await?;
        let n = read_hex4(s).await?;
        let mut body = vec![0u8; n];
        s.read_exact(&mut body).await?;
        Ok(body)
    })
    .await
    .map_err(|_| {
        IpcError::new(
            codes::E_TIMEOUT,
            format!("adb server did not answer {service}"),
        )
    })?
}

pub async fn server_version(s: &mut (impl Stream + ?Sized)) -> Result<u32, IpcError> {
    let body = request(s, "host:version").await?;
    let t = std::str::from_utf8(&body).map_err(|_| proto_err("bad version"))?;
    u32::from_str_radix(t.trim(), 16).map_err(|_| proto_err(format!("bad version {t:?}")))
}

pub async fn forwarded_ports(s: &mut (impl Stream + ?Sized)) -> Result<Vec<u16>, IpcError> {
    let body = request(s, "host:list-forward").await?;
    Ok(String::from_utf8_lossy(&body)
        .lines()
        .filter_map(|l| {
            l.split_whitespace()
                .nth(1)?
                .strip_prefix("tcp:")?
                .parse()
                .ok()
        })
        .collect())
}

pub async fn kill_forward(
    s: &mut (impl Stream + ?Sized),
    serial: &str,
    port: u16,
) -> Result<(), IpcError> {
    tokio::time::timeout(
        REQUEST_TIMEOUT,
        send(s, &format!("host-serial:{serial}:killforward:tcp:{port}")),
    )
    .await
    .map_err(|_| IpcError::new(codes::E_TIMEOUT, "adb server did not answer killforward"))?
}

/// `Android Debug Bridge version 1.0.41` → 41.
pub fn parse_local_version(text: &str) -> Option<u32> {
    let line = text
        .lines()
        .find(|l| l.starts_with("Android Debug Bridge version "))?;
    line.rsplit('.').next()?.trim().parse().ok()
}

pub async fn local_version(adb: &Path) -> Result<u32, IpcError> {
    let mut cmd = crate::proc::command(adb);
    cmd.arg("version").kill_on_drop(true);
    let out = tokio::time::timeout(REQUEST_TIMEOUT, cmd.output())
        .await
        .map_err(|_| {
            IpcError::new(
                codes::E_TIMEOUT,
                format!("{} version did not answer", adb.display()),
            )
        })?
        .map_err(|e| {
            IpcError::new(
                codes::E_TOOL_MISSING,
                format!("could not run {}: {e}", adb.display()),
            )
        })?;
    parse_local_version(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| proto_err(format!("{} version printed no version", adb.display())))
}

pub fn check_versions(local: u32, remote: u32) -> Result<(), IpcError> {
    if local == remote {
        return Ok(());
    }
    Err(IpcError::new(
        codes::E_ADB_VERSION_MISMATCH,
        format!(
            "this machine's adb speaks protocol {local}, the device host's adb server {remote}; \
             update platform-tools on the older side (a mismatched adb would restart the host's server)"
        ),
    ))
}

pub fn free_forward_port(used: &[u16]) -> Option<u16> {
    SCRCPY_PORTS.clone().find(|p| !used.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc_error::codes;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// One end of a duplex pipe, after a fake server answered `reply` to
    /// whatever request arrives; returns (client end, the request the server read).
    async fn served(
        reply: impl Into<Vec<u8>>,
    ) -> (tokio::io::DuplexStream, tokio::task::JoinHandle<String>) {
        let reply = reply.into();
        let (client, mut server) = tokio::io::duplex(4096);
        let req = tokio::spawn(async move {
            let mut len = [0u8; 4];
            server.read_exact(&mut len).await.unwrap();
            let n = usize::from_str_radix(std::str::from_utf8(&len).unwrap(), 16).unwrap();
            let mut body = vec![0u8; n];
            server.read_exact(&mut body).await.unwrap();
            server.write_all(&reply).await.unwrap();
            String::from_utf8(body).unwrap()
        });
        (client, req)
    }

    #[tokio::test]
    async fn server_version_reads_the_hex_payload() {
        let (mut c, req) = served(*b"OKAY00040029").await;
        assert_eq!(server_version(&mut c).await.unwrap(), 41);
        assert_eq!(req.await.unwrap(), "host:version");
    }

    #[tokio::test]
    async fn a_fail_reply_is_an_error_with_the_servers_message() {
        let (mut c, _req) = served(*b"FAIL0005nope!").await;
        let e = server_version(&mut c).await.unwrap_err();
        assert!(e.message.contains("nope!"), "{}", e.message);
    }

    #[tokio::test]
    async fn a_closed_stream_is_an_error_not_a_hang() {
        let (mut c, server) = tokio::io::duplex(64);
        drop(server);
        assert!(server_version(&mut c).await.is_err());
    }

    #[tokio::test]
    async fn forwarded_ports_lists_the_tcp_local_ports() {
        let list = "R5CT1 tcp:27183 localabstract:scrcpy_1\nemulator-5554 tcp:27185 tcp:8080\n";
        let (mut c, req) = served(format!("OKAY{:04x}{list}", list.len())).await;
        assert_eq!(forwarded_ports(&mut c).await.unwrap(), vec![27183, 27185]);
        assert_eq!(req.await.unwrap(), "host:list-forward");
    }

    #[tokio::test]
    async fn kill_forward_names_the_serial_and_port() {
        let (mut c, req) = served(*b"OKAYOKAY").await;
        kill_forward(&mut c, "R5CT1", 27183).await.unwrap();
        assert_eq!(
            req.await.unwrap(),
            "host-serial:R5CT1:killforward:tcp:27183"
        );
    }

    #[test]
    fn the_local_version_is_the_last_component_of_adbs_banner() {
        let text =
            "Android Debug Bridge version 1.0.41\nVersion 35.0.2-12147458\nInstalled as /x/adb\n";
        assert_eq!(parse_local_version(text), Some(41));
        assert_eq!(parse_local_version("garbage"), None);
    }

    #[test]
    fn a_version_mismatch_is_refused_naming_both() {
        assert!(check_versions(41, 41).is_ok());
        let e = check_versions(41, 40).unwrap_err();
        assert_eq!(e.code, codes::E_ADB_VERSION_MISMATCH);
        assert!(
            e.message.contains("41") && e.message.contains("40"),
            "{}",
            e.message
        );
    }

    #[test]
    fn the_first_unused_scrcpy_port_is_picked() {
        assert_eq!(free_forward_port(&[]), Some(27183));
        assert_eq!(free_forward_port(&[27183, 27184, 8080]), Some(27185));
        let all: Vec<u16> = crate::relay::SCRCPY_PORTS.collect();
        assert_eq!(free_forward_port(&all), None);
    }
}
