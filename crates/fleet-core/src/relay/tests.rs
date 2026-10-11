use super::*;
use crate::ipc_error::codes;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// An echo server on loopback; returns its address.
pub(crate) async fn echo_server() -> SocketAddr {
    let l = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    addr
}

/// Dials one fixed address whatever host/port it is asked for, and records
/// what it was asked.
pub(crate) struct FixedDialer {
    pub addr: SocketAddr,
    pub asked: std::sync::Mutex<Vec<(String, u16)>>,
}

#[async_trait::async_trait]
impl Dialer for FixedDialer {
    async fn dial(&self, host: &str, port: u16) -> Result<BoxedStream, IpcError> {
        self.asked.lock().unwrap().push((host.to_string(), port));
        Ok(Box::new(TcpStream::connect(self.addr).await?))
    }
}

struct FailDialer;

#[async_trait::async_trait]
impl Dialer for FailDialer {
    async fn dial(&self, _: &str, _: u16) -> Result<BoxedStream, IpcError> {
        Err(IpcError::new(codes::E_SSH, "no route"))
    }
}

#[test]
fn only_the_adb_server_and_scrcpy_ports_are_allowed() {
    for p in [5037, 27183, 27190, 27199] {
        assert!(allowed_port(p), "{p}");
    }
    for p in [0, 22, 5036, 5038, 27182, 27200, 65535] {
        assert!(!allowed_port(p), "{p}");
    }
    assert_eq!(check_port(22).unwrap_err().code, codes::E_INVALID);
}

#[tokio::test]
async fn a_connection_to_the_listener_is_spliced_to_the_dialed_stream() {
    let dialer = Arc::new(FixedDialer {
        addr: echo_server().await,
        asked: Default::default(),
    });
    let relay = RelayListener::bind(dialer.clone(), "mefistos", 5037)
        .await
        .unwrap();
    let mut c = TcpStream::connect(("127.0.0.1", relay.local_port()))
        .await
        .unwrap();
    c.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    c.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"ping");
    assert_eq!(
        *dialer.asked.lock().unwrap(),
        vec![("mefistos".to_string(), 5037)]
    );
}

#[tokio::test]
async fn a_port_outside_the_allowlist_is_refused_at_bind() {
    let dialer = Arc::new(FixedDialer {
        addr: echo_server().await,
        asked: Default::default(),
    });
    let err = RelayListener::bind(dialer, "h", 22).await.err().unwrap();
    assert_eq!(err.code, codes::E_INVALID);
}

#[tokio::test]
async fn a_failed_dial_closes_the_local_connection() {
    let relay = RelayListener::bind(Arc::new(FailDialer), "h", 5037)
        .await
        .unwrap();
    let mut c = TcpStream::connect(("127.0.0.1", relay.local_port()))
        .await
        .unwrap();
    let mut buf = [0u8; 1];
    let n = tokio::time::timeout(Duration::from_secs(5), c.read(&mut buf))
        .await
        .unwrap()
        .unwrap_or(0);
    assert_eq!(n, 0);
}

#[tokio::test]
async fn dropping_the_listener_ends_its_connections() {
    let dialer = Arc::new(FixedDialer {
        addr: echo_server().await,
        asked: Default::default(),
    });
    let relay = RelayListener::bind(dialer, "h", 5037).await.unwrap();
    let port = relay.local_port();
    let mut c = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    c.write_all(b"x").await.unwrap();
    let mut b = [0u8; 1];
    c.read_exact(&mut b).await.unwrap();
    drop(relay);
    let n = tokio::time::timeout(Duration::from_secs(5), c.read(&mut b))
        .await
        .unwrap()
        .unwrap_or(0);
    assert_eq!(n, 0, "the spliced connection must close");
    assert!(
        TcpStream::connect(("127.0.0.1", port)).await.is_err(),
        "the listener must be gone"
    );
}

#[cfg(unix)]
mod ssh_dialer {
    use super::super::ssh::SshDialer;
    use super::*;
    use crate::tmux::fake_exec::{write_exec, PROBE_GUARD};

    /// A fake `ssh` that records its argv and echoes stdin to stdout.
    fn fake_ssh(dir: &std::path::Path) -> std::path::PathBuf {
        let log = dir.join("argv");
        write_exec(
            dir,
            "ssh",
            &format!(
                "#!/bin/sh\n{PROBE_GUARD}printf '%s\\n' \"$@\" > {}\nexec cat\n",
                crate::shell::quote(&log.display().to_string())
            ),
        )
    }

    #[tokio::test]
    async fn it_runs_ssh_dash_w_to_the_hosts_loopback_and_splices_stdio() {
        let dir = tempfile::tempdir().unwrap();
        let d = SshDialer::with_binary(fake_ssh(dir.path()));
        let mut s = d.dial("mefistos", 5037).await.unwrap();
        s.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        s.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");
        let argv = std::fs::read_to_string(dir.path().join("argv")).unwrap();
        let argv: Vec<&str> = argv.lines().collect();
        assert_eq!(
            argv[argv.len() - 4..],
            ["-W", "127.0.0.1:5037", "--", "mefistos"]
        );
    }

    #[tokio::test]
    async fn it_refuses_a_port_or_an_alias_that_could_escape() {
        let dir = tempfile::tempdir().unwrap();
        let d = SshDialer::with_binary(fake_ssh(dir.path()));
        assert_eq!(
            d.dial("mefistos", 22).await.err().unwrap().code,
            codes::E_INVALID
        );
        assert_eq!(
            d.dial("-oProxyCommand=touch /tmp/x", 5037)
                .await
                .err()
                .unwrap()
                .code,
            codes::E_INVALID
        );
        assert!(!dir.path().join("argv").exists(), "ssh must not have run");
    }

    #[tokio::test]
    async fn the_production_argv_carries_batch_mode_and_a_connect_timeout() {
        let d = SshDialer::new(crate::ssh::SshClient::new());
        let argv = d.argv("mefistos", 27183);
        assert!(argv.iter().any(|a| a == "BatchMode=yes"), "{argv:?}");
        assert!(
            argv.iter().any(|a| a.starts_with("ConnectTimeout=")),
            "{argv:?}"
        );
        assert_eq!(
            argv[argv.len() - 4..],
            ["-W", "127.0.0.1:27183", "--", "mefistos"]
        );
    }
}
