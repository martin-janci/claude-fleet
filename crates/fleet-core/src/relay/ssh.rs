//! [`Dialer`] over `ssh -W 127.0.0.1:<port> <alias>`: one ssh per stream,
//! multiplexed on the host's ControlMaster (`SshClient::mux_opts`), so a
//! mirror's many short adb connections share one handshake.

use super::{check_port, BoxedStream, Dialer};
use crate::ipc_error::{codes, IpcError};
use crate::ssh::SshClient;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, Join, ReadBuf};
use tokio::process::{Child, ChildStdin, ChildStdout};

/// How long ssh may take to connect a stream.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

type Opts = Box<dyn Fn(&str) -> Vec<String> + Send + Sync>;

pub struct SshDialer {
    binary: PathBuf,
    opts: Opts,
}

impl SshDialer {
    pub fn new(client: SshClient) -> Self {
        let binary = client.ssh_binary().to_path_buf();
        Self {
            binary,
            opts: Box::new(move |host| client.mux_opts(host, CONNECT_TIMEOUT)),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_binary(binary: PathBuf) -> Self {
        Self {
            binary,
            opts: Box::new(|_| Vec::new()),
        }
    }

    pub fn argv(&self, host: &str, port: u16) -> Vec<String> {
        let mut a = (self.opts)(host);
        a.extend([
            "-W".into(),
            format!("127.0.0.1:{port}"),
            "--".into(),
            host.to_string(),
        ]);
        a
    }
}

#[async_trait::async_trait]
impl Dialer for SshDialer {
    async fn dial(&self, host: &str, port: u16) -> Result<BoxedStream, IpcError> {
        check_port(port)?;
        crate::validate::host_alias(host)?;
        let mut child = crate::proc::command(&self.binary)
            .args(self.argv(host, port))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| IpcError::new(codes::E_SSH, format!("could not start ssh: {e}")))?;
        let (Some(out), Some(inp)) = (child.stdout.take(), child.stdin.take()) else {
            return Err(IpcError::new(
                codes::E_INTERNAL,
                "ssh started without pipes",
            ));
        };
        Ok(Box::new(ChildStream {
            io: tokio::io::join(out, inp),
            _child: child,
        }))
    }
}

/// The ssh child's stdio as one stream; dropping it kills the child.
struct ChildStream {
    io: Join<ChildStdout, ChildStdin>,
    _child: Child,
}

impl AsyncRead for ChildStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl AsyncWrite for ChildStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}
