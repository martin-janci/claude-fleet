//! A loopback TCP listener whose connections are spliced to a [`Dialer`]:
//! how the device mirror reaches a device host's adb server and scrcpy
//! socket (`docs/debug-devices.md`, *Mirror*; spec
//! `docs/superpowers/specs/2026-10-11-scrcpy-device-mirror-design.md`).
//!
//! The target is always the host's own `127.0.0.1`, and only on
//! [`allowed_port`]s; nothing here dials a caller-supplied address.

pub mod ssh;

#[cfg(test)]
pub(crate) mod tests;

use crate::ipc_error::{codes, IpcError};
use std::ops::RangeInclusive;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::task::AbortHandle;

/// The adb server's port on a device host.
pub const ADB_SERVER_PORT: u16 = 5037;
/// scrcpy's forward ports (`--port`'s default range).
pub const SCRCPY_PORTS: RangeInclusive<u16> = 27183..=27199;

/// May the relay reach `port` on a host's loopback?
pub fn allowed_port(port: u16) -> bool {
    port == ADB_SERVER_PORT || SCRCPY_PORTS.contains(&port)
}

/// [`allowed_port`] as a refusal.
pub fn check_port(port: u16) -> Result<(), IpcError> {
    if allowed_port(port) {
        return Ok(());
    }
    Err(IpcError::new(
        codes::E_INVALID,
        format!(
            "port {port} is not relayed; only the adb server ({ADB_SERVER_PORT}) and scrcpy's {}-{} are",
            SCRCPY_PORTS.start(),
            SCRCPY_PORTS.end()
        ),
    ))
}

/// A bidirectional byte stream: the same blanket trait and boxed type the
/// network layer uses.
pub use crate::net::conn::Duplex as Stream;
pub type BoxedStream = crate::net::conn::Stream;

/// Opens one stream to `127.0.0.1:<port>` on `host`.
#[async_trait::async_trait]
pub trait Dialer: Send + Sync {
    async fn dial(&self, host: &str, port: u16) -> Result<BoxedStream, IpcError>;
}

/// A `127.0.0.1:<ephemeral>` listener: every connection to it is spliced to
/// `dialer.dial(host, port)`. Dropping it closes the listener and every
/// connection it carries.
pub struct RelayListener {
    local_port: u16,
    tasks: Arc<Mutex<Vec<AbortHandle>>>,
}

impl RelayListener {
    pub async fn bind(dialer: Arc<dyn Dialer>, host: &str, port: u16) -> Result<Self, IpcError> {
        check_port(port)?;
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let local_port = listener.local_addr()?.port();
        let tasks: Arc<Mutex<Vec<AbortHandle>>> = Arc::default();
        let conns = Arc::clone(&tasks);
        let host = host.to_string();
        let accept = tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let dialer = Arc::clone(&dialer);
                let host = host.clone();
                let conn = tokio::spawn(async move {
                    match dialer.dial(&host, port).await {
                        Ok(mut remote) => {
                            let _ = tokio::io::copy_bidirectional(&mut sock, &mut remote).await;
                        }
                        Err(e) => {
                            tracing::warn!(host = %host, port, error = %e, "relay: dial failed")
                        }
                    }
                });
                let mut g = conns.lock().unwrap_or_else(|p| p.into_inner());
                g.retain(|h| !h.is_finished());
                g.push(conn.abort_handle());
            }
        });
        tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(accept.abort_handle());
        Ok(Self { local_port, tasks })
    }

    pub fn local_port(&self) -> u16 {
        self.local_port
    }
}

impl Drop for RelayListener {
    fn drop(&mut self) {
        for h in self
            .tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .drain(..)
        {
            h.abort();
        }
    }
}
